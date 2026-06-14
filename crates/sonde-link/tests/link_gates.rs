//! Frame-level Gilbert-Elliott lossy half-duplex medium + acceptance gates
//! G1–G5 (design §8).
//!
//! The medium is a *neutral* shared channel between two `PhyTransport`
//! endpoints — it does NOT enforce turn-taking. Both endpoints may transmit at
//! any time; two opposing transmissions that overlap in time **collide** and
//! both are destroyed (a station is deaf while keyed). Correct floor discipline
//! in the connection SM is what keeps that from happening; the harness can also
//! force a collision to test recovery.
//!
//! All randomness is a seeded xorshift PRNG (no `rand` dependency), so every
//! loss / corruption / collision decision is reproducible from the seed.
//!
//! Results from these gates are "link-correct over channel model {params}",
//! never "HF-viable" — over-the-real-PHY viability is gated on the PHY physics
//! gates, owned elsewhere (RADIO-1: nothing here keys a radio).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use sonde_phy::error::PhyError;
use sonde_phy::modes::{ModeHint, ModeTable};
use sonde_phy::phy_api::{ChannelQualityReport, PhyTransport, RxFrame, TxToken};

use sonde_link::{
    ArqStrategy, Callsign, ConnState, Connection, HostCommand, HostEvent, Link, ModeProfile,
    BASE_RUNG,
};

// ---------------------------------------------------------------------------
// Deterministic PRNG (xorshift64*) — reproducible from a seed, no rand dep.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Avoid the all-zero state, which xorshift cannot escape.
        Self(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform Bernoulli trial: `true` with probability `p`.
    fn chance(&mut self, p: f64) -> bool {
        if p <= 0.0 {
            return false;
        }
        if p >= 1.0 {
            return true;
        }
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        unit < p
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Gilbert-Elliott two-state burst-loss model (per direction).
// ---------------------------------------------------------------------------

struct GilbertElliott {
    bad: bool,
    p_g2b: f64,
    p_b2g: f64,
    loss_good: f64,
    loss_bad: f64,
}

impl GilbertElliott {
    /// Advance the chain one frame and report whether this frame is dropped.
    fn drop_next(&mut self, rng: &mut Rng) -> bool {
        if self.bad {
            if rng.chance(self.p_b2g) {
                self.bad = false;
            }
        } else if rng.chance(self.p_g2b) {
            self.bad = true;
        }
        let loss = if self.bad {
            self.loss_bad
        } else {
            self.loss_good
        };
        rng.chance(loss)
    }
}

// ---------------------------------------------------------------------------
// Channel parameters.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ChannelParams {
    seed: u64,
    p_g2b: f64,
    p_b2g: f64,
    loss_good: f64,
    loss_bad: f64,
    corruption_prob: f64,
    rtt: Duration,
}

impl ChannelParams {
    /// A perfectly clean medium (no loss, no corruption) with a one-tick RTT.
    fn clean(rtt: Duration) -> Self {
        Self {
            seed: 1,
            p_g2b: 0.0,
            p_b2g: 1.0,
            loss_good: 0.0,
            loss_bad: 0.0,
            corruption_prob: 0.0,
            rtt,
        }
    }
}

#[derive(Default, Clone, Copy, Debug)]
struct Stats {
    sent: u64,
    lost: u64,
    corrupted: u64,
    collisions: u64,
    delivered: u64,
}

struct InFlight {
    to: usize,
    dir: usize,
    bytes: Vec<u8>,
    deliver_at: Duration,
}

struct Medium {
    now: Duration,
    params: ChannelParams,
    rng: Rng,
    ge: [GilbertElliott; 2],
    inflight: Vec<InFlight>,
    ready: [VecDeque<Vec<u8>>; 2],
    /// When > 0, the next `from`-keyed send is forced to collide regardless of
    /// timing (lets a gate force the collision case deterministically).
    force_collision: bool,
    /// Deep-fade window `[start, end)` during which every frame is dropped
    /// (models a burst the channel cannot carry at all).
    blackout: Option<(Duration, Duration)>,
    /// When set, only frames stamped with the BASE link mode (header MODE byte
    /// == `BASE_RUNG`) get through — models "the fast modes no longer decode,
    /// only the robust floor does", to exercise the link's BASE-fallback (P1).
    base_mode_only: bool,
    stats: Stats,
}

impl Medium {
    fn new(params: ChannelParams) -> Self {
        let ge = || GilbertElliott {
            bad: false,
            p_g2b: params.p_g2b,
            p_b2g: params.p_b2g,
            loss_good: params.loss_good,
            loss_bad: params.loss_bad,
        };
        Self {
            now: Duration::ZERO,
            rng: Rng::new(params.seed),
            ge: [ge(), ge()],
            inflight: Vec::new(),
            ready: [VecDeque::new(), VecDeque::new()],
            force_collision: false,
            blackout: None,
            base_mode_only: false,
            params,
            stats: Stats::default(),
        }
    }

    fn set_now(&mut self, t: Duration) {
        self.now = t;
        let mut i = 0;
        while i < self.inflight.len() {
            if self.inflight[i].deliver_at <= t {
                let f = self.inflight.remove(i);
                self.stats.delivered += 1;
                self.ready[f.to].push_back(f.bytes);
            } else {
                i += 1;
            }
        }
    }

    fn send(&mut self, from: usize, bytes: Vec<u8>) {
        self.stats.sent += 1;
        let dir = from;

        // Deep-fade blackout: the channel carries nothing during the window.
        if let Some((start, end)) = self.blackout {
            if self.now >= start && self.now < end {
                self.stats.lost += 1;
                return;
            }
        }

        // Mode-selective channel: only BASE-mode frames decode (MODE byte at
        // header offset 39). Anything at a faster mode is lost.
        if self.base_mode_only && bytes.len() > 39 && bytes[39] != BASE_RUNG {
            self.stats.lost += 1;
            return;
        }

        // Gilbert-Elliott burst loss (advances the chain every transmitted frame).
        if self.ge[dir].drop_next(&mut self.rng) {
            self.stats.lost += 1;
            return;
        }

        // Half-duplex collision: an opposing transmission overlapping in time
        // destroys both. (rtt-wide intervals; all in-flight share the rtt.)
        let deliver_at = self.now + self.params.rtt;
        let opposing: Vec<usize> = self
            .inflight
            .iter()
            .enumerate()
            .filter(|(_, f)| f.dir != dir && self.now < f.deliver_at)
            .map(|(idx, _)| idx)
            .collect();
        if self.force_collision || !opposing.is_empty() {
            self.force_collision = false;
            for idx in opposing.into_iter().rev() {
                self.inflight.remove(idx);
                self.stats.collisions += 1;
            }
            self.stats.collisions += 1; // this frame is destroyed too
            return;
        }

        // Byte corruption: a flipped bit the link's CRC must catch on decode.
        let mut bytes = bytes;
        if !bytes.is_empty() && self.rng.chance(self.params.corruption_prob) {
            let i = self.rng.below(bytes.len());
            let bit = self.rng.below(8);
            bytes[i] ^= 1 << bit;
            self.stats.corrupted += 1;
        }

        self.inflight.push(InFlight {
            to: 1 - from,
            dir,
            bytes,
            deliver_at,
        });
    }

    fn take_ready(&mut self, side: usize) -> Option<Vec<u8>> {
        self.ready[side].pop_front()
    }
}

/// A shared lossy half-duplex channel with two `PhyTransport` endpoints.
struct Channel {
    medium: Rc<RefCell<Medium>>,
}

impl Channel {
    fn new(params: ChannelParams) -> (Self, LossyEndpoint, LossyEndpoint) {
        let medium = Rc::new(RefCell::new(Medium::new(params)));
        let a = LossyEndpoint {
            medium: Rc::clone(&medium),
            side: 0,
        };
        let b = LossyEndpoint {
            medium: Rc::clone(&medium),
            side: 1,
        };
        (Self { medium }, a, b)
    }

    fn set_now(&self, t: Duration) {
        self.medium.borrow_mut().set_now(t);
    }

    fn stats(&self) -> Stats {
        self.medium.borrow().stats
    }

    fn force_collision(&self) {
        self.medium.borrow_mut().force_collision = true;
    }

    /// Drop every frame in `[start, end)` (a deep-fade burst).
    fn blackout(&self, start: Duration, end: Duration) {
        self.medium.borrow_mut().blackout = Some((start, end));
    }

    /// Only BASE-mode frames decode from now on (the fast modes "stop decoding").
    fn set_base_mode_only(&self, on: bool) {
        self.medium.borrow_mut().base_mode_only = on;
    }
}

struct LossyEndpoint {
    medium: Rc<RefCell<Medium>>,
    side: usize,
}

impl PhyTransport for LossyEndpoint {
    fn send_frame(&mut self, payload: &[u8], _hint: ModeHint) -> Result<TxToken, PhyError> {
        self.medium.borrow_mut().send(self.side, payload.to_vec());
        Ok(TxToken(0))
    }

    fn poll_rx(&mut self) -> Option<RxFrame> {
        let bytes = self.medium.borrow_mut().take_ready(self.side)?;
        let mode = ModeTable::default().resolve(ModeHint::MainAuto, None);
        Some(RxFrame::new(bytes, mode, None, 10.0, true))
    }

    fn channel_quality(&self) -> ChannelQualityReport {
        ChannelQualityReport::empty()
    }
}

// ===========================================================================
// Harness self-tests — prove the medium behaves before any link rides on it.
// ===========================================================================

const TICK: Duration = Duration::from_millis(10);

#[test]
fn clean_channel_delivers_every_frame_after_one_rtt() {
    let (chan, mut a, _b) = Channel::new(ChannelParams::clean(TICK));
    a.send_frame(b"hello", ModeHint::MainAuto).unwrap();
    // Not yet: still in flight.
    chan.set_now(Duration::ZERO);
    // After one RTT it is deliverable to side B (index 1).
    chan.set_now(TICK);
    let got = chan.medium.borrow_mut().take_ready(1);
    assert_eq!(got.as_deref(), Some(&b"hello"[..]));
}

#[test]
fn medium_is_deterministic_for_a_fixed_seed() {
    let params = ChannelParams {
        seed: 42,
        p_g2b: 0.3,
        p_b2g: 0.3,
        loss_good: 0.05,
        loss_bad: 0.9,
        corruption_prob: 0.1,
        rtt: TICK,
    };
    let run = || {
        let mut m = Medium::new(params.clone());
        for i in 0..200u32 {
            m.send(0, i.to_be_bytes().to_vec());
        }
        m.set_now(TICK);
        (m.stats.lost, m.stats.corrupted, m.stats.delivered)
    };
    assert_eq!(
        run(),
        run(),
        "same seed ⇒ identical loss/corruption pattern"
    );
}

#[test]
fn bad_state_produces_a_consecutive_loss_burst() {
    // Sticky bad state, certain loss while bad ⇒ a run of consecutive drops.
    let params = ChannelParams {
        seed: 7,
        p_g2b: 1.0, // jump to bad immediately
        p_b2g: 0.0, // never recover
        loss_good: 0.0,
        loss_bad: 1.0,
        corruption_prob: 0.0,
        rtt: TICK,
    };
    let mut m = Medium::new(params);
    for i in 0..20u32 {
        m.send(0, i.to_be_bytes().to_vec());
    }
    m.set_now(TICK);
    assert_eq!(
        m.stats.delivered, 0,
        "a deep-fade burst drops the whole run"
    );
    assert_eq!(m.stats.lost, 20);
}

#[test]
fn corruption_mutates_delivered_bytes_so_crc_can_catch_it() {
    let params = ChannelParams {
        seed: 3,
        p_g2b: 0.0,
        p_b2g: 1.0,
        loss_good: 0.0,
        loss_bad: 0.0,
        corruption_prob: 1.0, // every frame corrupted
        rtt: TICK,
    };
    let (chan, mut a, _b) = Channel::new(params);
    a.send_frame(b"abcdefgh", ModeHint::MainAuto).unwrap();
    chan.set_now(TICK);
    let got = chan.medium.borrow_mut().take_ready(1).unwrap();
    assert_ne!(
        got, b"abcdefgh",
        "a bit was flipped — link CRC must reject it"
    );
    assert_eq!(chan.stats().corrupted, 1);
}

#[test]
fn overlapping_opposing_transmissions_collide_and_both_are_lost() {
    let (chan, mut a, mut b) = Channel::new(ChannelParams::clean(TICK));
    // Both key up at the same instant in opposite directions.
    a.send_frame(b"from-a", ModeHint::MainAuto).unwrap();
    b.send_frame(b"from-b", ModeHint::MainAuto).unwrap();
    chan.set_now(TICK);
    assert!(chan.medium.borrow_mut().take_ready(0).is_none());
    assert!(chan.medium.borrow_mut().take_ready(1).is_none());
    assert!(chan.stats().collisions >= 2);
}

#[test]
fn same_direction_frames_in_one_over_do_not_collide() {
    let (chan, mut a, _b) = Channel::new(ChannelParams::clean(TICK));
    a.send_frame(b"f1", ModeHint::MainAuto).unwrap();
    a.send_frame(b"f2", ModeHint::MainAuto).unwrap();
    a.send_frame(b"f3", ModeHint::MainAuto).unwrap();
    chan.set_now(TICK);
    assert_eq!(chan.stats().collisions, 0);
    assert_eq!(chan.stats().delivered, 3);
}

#[test]
fn forced_collision_destroys_the_next_frame() {
    let (chan, mut a, _b) = Channel::new(ChannelParams::clean(TICK));
    chan.force_collision();
    a.send_frame(b"doomed", ModeHint::MainAuto).unwrap();
    chan.set_now(TICK);
    assert_eq!(chan.medium.borrow_mut().take_ready(1), None);
    assert!(chan.stats().collisions >= 1);
}

// ===========================================================================
// Acceptance gates G1–G5 (design §8). Two `Link`s over one lossy `Channel`,
// driven by a logical clock. Results are "link-correct over channel model
// {params}", never "HF-viable".
// ===========================================================================

fn gate_profile() -> ModeProfile {
    // 10 ms over airtime ⇒ turn-recovery 20 ms, keepalive 80 ms; per-over MTU 8
    // bytes forces multi-fragment messages so ordering/reassembly are exercised.
    ModeProfile::new(Duration::from_millis(10), 8)
}

fn callsign(s: &str) -> Callsign {
    Callsign::new(s).unwrap()
}

fn delivered_messages(events: &[HostEvent]) -> Vec<Vec<u8>> {
    events
        .iter()
        .filter_map(|e| match e {
            HostEvent::DataReceived(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

/// Two `Link`s sharing one lossy half-duplex `Channel`, advanced by a logical
/// clock. Endpoint A is the initiator (K1ABC), B the acceptor (W2XYZ).
struct LinkPair {
    chan: Channel,
    a: Link<LossyEndpoint>,
    b: Link<LossyEndpoint>,
    now: Duration,
    a_events: Vec<HostEvent>,
    b_events: Vec<HostEvent>,
}

impl LinkPair {
    fn new(params: ChannelParams) -> Self {
        Self::with_strategy(params, ArqStrategy::SelectiveRepeat)
    }

    fn with_strategy(params: ChannelParams, strategy: ArqStrategy) -> Self {
        let (chan, ea, eb) = Channel::new(params);
        let a = Link::new(
            ea,
            Connection::initiator(
                callsign("K1ABC"),
                callsign("W2XYZ"),
                0x1234,
                gate_profile(),
                8,
            )
            .with_strategy(strategy),
            ModeHint::MainAuto,
        );
        let b = Link::new(
            eb,
            Connection::acceptor(callsign("W2XYZ"), callsign("K1ABC"), gate_profile(), 8)
                .with_strategy(strategy),
            ModeHint::MainAuto,
        );
        Self {
            chan,
            a,
            b,
            now: Duration::ZERO,
            a_events: Vec::new(),
            b_events: Vec::new(),
        }
    }

    fn step(&mut self) {
        self.chan.set_now(self.now);
        let ea = self.a.poll(self.now);
        self.a_events.extend(ea);
        let eb = self.b.poll(self.now);
        self.b_events.extend(eb);
        self.now += TICK;
    }

    /// Step until `pred` holds or the step budget is exhausted; returns whether
    /// `pred` ended up true.
    fn run_until(&mut self, max_steps: usize, pred: impl Fn(&LinkPair) -> bool) -> bool {
        for _ in 0..max_steps {
            if pred(self) {
                return true;
            }
            self.step();
        }
        pred(self)
    }

    fn both_connected(&self) -> bool {
        self.a.state() == ConnState::Connected && self.b.state() == ConnState::Connected
    }

    fn b_messages(&self) -> Vec<Vec<u8>> {
        delivered_messages(&self.b_events)
    }

    fn a_messages(&self) -> Vec<Vec<u8>> {
        delivered_messages(&self.a_events)
    }
}

#[test]
fn g1_reliable_in_order_delivery_under_burst_loss_and_corruption() {
    // Bursty Gilbert-Elliott loss + byte corruption (CRC-caught ⇒ dropped).
    let params = ChannelParams {
        seed: 0xC0FFEE,
        p_g2b: 0.15,
        p_b2g: 0.4,
        loss_good: 0.03,
        loss_bad: 0.6,
        corruption_prob: 0.04,
        rtt: TICK,
    };
    let mut p = LinkPair::new(params);
    p.a.connect(Duration::ZERO);
    assert!(
        p.run_until(400, |p| p.both_connected()),
        "handshake must complete"
    );

    let msg: Vec<u8> = (0u8..40).collect(); // 40 bytes ⇒ 5 fragments at mtu 8
    p.a.send(msg.clone());
    assert!(
        p.run_until(4000, |p| !p.b_messages().is_empty()),
        "message must eventually be delivered"
    );

    // Byte-exact, in order, exactly once — or the link would have reported
    // failure; never silent corruption.
    assert_eq!(p.b_messages(), vec![msg]);
    assert_eq!(
        p.a.state(),
        ConnState::Connected,
        "link survived the channel"
    );
}

#[test]
fn g2_connect_and_teardown_under_control_frame_loss() {
    let params = ChannelParams {
        seed: 0x5EED,
        p_g2b: 0.2,
        p_b2g: 0.4,
        loss_good: 0.1,
        loss_bad: 0.5,
        corruption_prob: 0.0,
        rtt: TICK,
    };
    let mut p = LinkPair::new(params);
    p.a.connect(Duration::ZERO);
    assert!(
        p.run_until(600, |p| p.both_connected()),
        "handshake retransmits through control-frame loss"
    );

    p.a.disconnect(p.now);
    assert!(
        p.run_until(600, |p| p.a.state() == ConnState::Closed
            && p.b.state() == ConnState::Closed),
        "teardown completes through control-frame loss"
    );
    assert!(p.a_events.contains(&HostEvent::Disconnected));
}

#[test]
fn g3_burst_recovery_after_a_deep_fade() {
    // Clean channel except for a deep-fade burst shorter than the death window.
    let mut p = LinkPair::new(ChannelParams::clean(TICK));
    p.a.connect(Duration::ZERO);
    assert!(p.run_until(40, |p| p.both_connected()));

    let msg: Vec<u8> = (0u8..24).collect();
    p.a.send(msg.clone());
    // Black out ~4 consecutive overs (turn-recovery is 2 ticks; death is 6
    // silent overs ⇒ ~12 ticks). The link must ride through and recover.
    let start = p.now;
    p.chan.blackout(start, start + TICK * 8);

    assert!(
        p.run_until(2000, |p| !p.b_messages().is_empty()),
        "selective-repeat recovers once the channel reopens"
    );
    assert_eq!(p.b_messages(), vec![msg]);
    assert_eq!(p.a.state(), ConnState::Connected, "rode through the burst");
}

#[test]
fn g4_honest_failure_on_sustained_loss() {
    let mut p = LinkPair::new(ChannelParams::clean(TICK));
    p.a.connect(Duration::ZERO);
    assert!(p.run_until(40, |p| p.both_connected()));

    let msg: Vec<u8> = (0u8..24).collect();
    p.a.send(msg);
    // Total, permanent loss from here on.
    let start = p.now;
    p.chan.blackout(start, Duration::from_secs(3600));

    assert!(
        p.run_until(2000, |p| p.a.state() == ConnState::Closed),
        "link must declare death, not hang"
    );
    assert!(
        p.a_events.contains(&HostEvent::PeerLost),
        "failure is explicit PeerLost"
    );
    assert!(
        p.b_messages().is_empty(),
        "nothing delivered ⇒ no partial/silent delivery"
    );
}

#[test]
fn g6_floor_whole_message_delivers_under_burst_loss() {
    // The degenerate floor strategy (stop-and-wait, no SACK) must still deliver
    // byte-exact in order over a bursty channel — the FT8-class "no NACK" model.
    let params = ChannelParams {
        seed: 0xF100,
        p_g2b: 0.15,
        p_b2g: 0.45,
        loss_good: 0.03,
        loss_bad: 0.55,
        corruption_prob: 0.03,
        rtt: TICK,
    };
    let mut p = LinkPair::with_strategy(params, ArqStrategy::WholeMessage);
    p.a.connect(Duration::ZERO);
    assert!(
        p.run_until(600, |p| p.both_connected()),
        "floor handshake completes"
    );

    let msg: Vec<u8> = (0u8..24).collect(); // multi-fragment, stop-and-wait
    p.a.send(msg.clone());
    assert!(
        p.run_until(8000, |p| !p.b_messages().is_empty()),
        "floor whole-message recovers loss by resend-until-acked"
    );
    assert_eq!(p.b_messages(), vec![msg]);
    assert_eq!(p.a.state(), ConnState::Connected);
}

#[test]
fn host_command_surface_drives_the_full_lifecycle() {
    // The #8 TNC contract: connect / send / disconnect as commands, with
    // delivery + lifecycle observed through the HostEvent stream.
    let params = ChannelParams {
        seed: 0x4057,
        p_g2b: 0.1,
        p_b2g: 0.5,
        loss_good: 0.05,
        loss_bad: 0.4,
        corruption_prob: 0.0,
        rtt: TICK,
    };
    let mut p = LinkPair::new(params);
    p.a.command(HostCommand::Connect, p.now);
    assert!(p.run_until(600, |p| p.both_connected()));

    let msg = b"via-host-command".to_vec();
    p.a.command(HostCommand::Send(msg.clone()), p.now);
    assert!(p.run_until(4000, |p| !p.b_messages().is_empty()));
    assert_eq!(p.b_messages(), vec![msg]);

    p.a.command(HostCommand::Disconnect, p.now);
    assert!(p.run_until(600, |p| p.a.state() == ConnState::Closed
        && p.b.state() == ConnState::Closed));
    assert!(p.b_events.contains(&HostEvent::Disconnected));
}

#[test]
fn g7_bidirectional_delivery_under_burst_loss() {
    // Both stations originate host data over one lossy half-duplex channel; both
    // messages must be delivered byte-exact (the floor must be shareable, not
    // initiator-hogged). Codex blocker #2.
    let params = ChannelParams {
        seed: 0xB1D1,
        p_g2b: 0.12,
        p_b2g: 0.45,
        loss_good: 0.03,
        loss_bad: 0.5,
        corruption_prob: 0.02,
        rtt: TICK,
    };
    let mut p = LinkPair::new(params);
    p.a.connect(Duration::ZERO);
    assert!(p.run_until(400, |p| p.both_connected()));

    let to_b: Vec<u8> = (0u8..24).collect();
    let to_a: Vec<u8> = (100u8..130).collect();
    p.a.send(to_b.clone());
    p.b.send(to_a.clone());
    assert!(
        p.run_until(8000, |p| !p.a_messages().is_empty()
            && !p.b_messages().is_empty()),
        "both directions must deliver"
    );
    assert_eq!(p.b_messages(), vec![to_b]);
    assert_eq!(p.a_messages(), vec![to_a]);
}

#[test]
fn g8_acceptor_originated_data_under_burst_loss() {
    // The acceptor (never the initiator) is the only one with data. It must
    // acquire the idle floor and deliver — the initiator must not starve it.
    let params = ChannelParams {
        seed: 0xACC0,
        p_g2b: 0.12,
        p_b2g: 0.45,
        loss_good: 0.03,
        loss_bad: 0.5,
        corruption_prob: 0.0,
        rtt: TICK,
    };
    let mut p = LinkPair::new(params);
    p.a.connect(Duration::ZERO);
    assert!(p.run_until(400, |p| p.both_connected()));

    let msg: Vec<u8> = (0u8..20).collect();
    p.b.send(msg.clone());
    assert!(p.run_until(8000, |p| !p.a_messages().is_empty()));
    assert_eq!(p.a_messages(), vec![msg]);
}

#[test]
fn g9_link_converges_to_base_mode_under_degradation_and_delivers() {
    // Connect cleanly, then the fast modes "stop decoding" (only BASE-mode frames
    // get through). The link must FALL TO THE BASE mode (design P1) and still
    // deliver — converging to the robust floor instead of declaring PeerLost.
    let mut p = LinkPair::new(ChannelParams::clean(TICK));
    p.a.connect(Duration::ZERO);
    assert!(p.run_until(40, |p| p.both_connected()));

    // The channel degrades: from now on only BASE-mode frames decode.
    p.chan.set_base_mode_only(true);
    let msg: Vec<u8> = (0u8..20).collect();
    p.a.send(msg.clone());

    assert!(
        p.run_until(4000, |p| !p.b_messages().is_empty()),
        "link must fall to BASE and deliver, not die"
    );
    assert_eq!(p.b_messages(), vec![msg]);
    assert_eq!(
        p.a.state(),
        ConnState::Connected,
        "survived by converging to BASE"
    );
    assert_eq!(p.b.state(), ConnState::Connected);
}
