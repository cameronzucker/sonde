//! Sans-IO connection state machine (design §4) — half-duplex, turn-taking.
//!
//! `Connection` does **no I/O and reads no wall-clock**. It is driven purely by
//! method calls with logical time injected (`now: Duration`):
//! - host intent: [`Connection::connect`], [`Connection::send`],
//!   [`Connection::disconnect`];
//! - inbound frames: [`Connection::handle_frame`];
//! - the clock: [`Connection::handle_timeout`];
//! - outputs: [`Connection::poll_transmit`] (next frame to put on the wire) and
//!   [`Connection::poll_event`] (host-facing events).
//!
//! This keeps the floor/turn timers, ARQ, and retransmission fully reproducible
//! from a seed, and lets the same core drive both the in-memory lossy medium
//! (gates G1–G4) and a real `PhyTransport` (G5) through a thin `Link<P>` adapter.
//!
//! # Floor model (design §3.5)
//!
//! The `PhyTransport` seam has no carrier-sense, so the floor is passed in-band
//! via `END_OF_OVER`. The connection initiator owns the first floor. On
//! receiving the end of a peer's over, a station takes the floor **only if it
//! has unacked data or owes an ack for DATA just received** (the quiescence
//! rule); otherwise the floor goes **`Idle`** (free) and *either* station may
//! later acquire it when it next has something to send — the model is symmetric,
//! so neither station hogs an idle floor and an acceptor can originate data.
//! A turn-recovery timer re-takes the floor if a *data*-sender's reply is lost;
//! sustained silence past `DEAD_OVERS_TOLERATED` is reported as `PeerLost`
//! (honest failure, never a hang or silent corruption). Closing the connection
//! resets all ARQ/reassembly state so a same-object reconnect starts clean.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use crate::arq::{Reassembler, RecvBuffer, SendWindow, FIRST_SEQ};
use crate::frame::{Callsign, FrameType, LinkFrame, StationId};
use crate::mac::{ArqStrategy, Ladder};
use crate::profile::ModeProfile;
use sonde_phy::modes::ModeHint;

/// CONN retransmissions before a connect attempt fails.
const MAX_CONN_RETRIES: u32 = 5;
/// DISC retransmissions before teardown completes best-effort.
const MAX_DISC_RETRIES: u32 = 3;
/// Consecutive silent overs tolerated before the link is declared dead.
const DEAD_OVERS_TOLERATED: u32 = 6;
/// Consecutive silent overs at a non-base mode before falling to the robust
/// BASE mode (design P1: converge to the floor under degradation rather than
/// dying). Must be `< DEAD_OVERS_TOLERATED` so BASE gets a fresh death budget.
const DOWNSHIFT_TO_BASE_OVERS: u32 = 3;

/// EWMA weight for a *rising* SNR observation (symmetric-SNR adaptation design F3):
/// a falling SNR is applied immediately (fast downshift), a rising SNR is smoothed
/// at this weight (≈1–2 good overs to climb a rung — fast, not a permit crawl).
const SNR_RISE_ALPHA: f32 = 0.5;

/// Connection lifecycle state (design §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// No session.
    Closed,
    /// CONN sent, awaiting CONN_ACK.
    Connecting,
    /// Session up.
    Connected,
    /// DISC sent, awaiting DISC_ACK.
    Disconnecting,
}

/// Half-duplex floor sub-state within `Connected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Floor {
    /// This station holds the floor (may key up).
    Sending,
    /// The peer holds the floor (this station listens for its over).
    Listening,
    /// Nobody holds the floor — the link is quiescent. Either station may
    /// acquire it by transmitting when it has something to say.
    Idle,
}

/// Events surfaced to the host (subsystem #8 will map these to its API).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// The session is established.
    Connected,
    /// A whole, in-order host message was reassembled and delivered.
    DataReceived(Vec<u8>),
    /// The session was torn down cleanly.
    Disconnected,
    /// The link died (sustained silence) — honest failure.
    PeerLost,
    /// A connect attempt exhausted its retries.
    ConnectFailed,
}

/// A half-duplex, connected-mode link connection to a single peer.
pub struct Connection {
    local: Callsign,
    /// The peer station. An **initiator** is *configured* with whom it calls
    /// (`Some`, `remote_is_learned == false`). A fresh **acceptor** is a listener
    /// (`None`, `remote_is_learned == true`) that *learns* the peer from the
    /// `SRC` of the inbound `CONN`. A learned peer is cleared on session reset so
    /// the listener can accept a different station next time; a configured peer is
    /// preserved (Codex review of sonde-sbt).
    remote: Option<Callsign>,
    /// Whether `remote` is learned-from-CONN (acceptor) vs configured (initiator).
    remote_is_learned: bool,
    /// Active airtime-aware profile for `current_rung` — derives all link timers.
    /// Swapped on a rung change ([`Connection::apply_rung`]) to the target rung's
    /// profile, so per-mode timers track the active mode (gap inventory B3).
    profile: ModeProfile,
    /// The adaptation ladder this connection runs over. Built from the PHY's
    /// published modes (sonde-3tm registry handshake) via `*_with_ladder`, or the
    /// built-in uniform-profile mirror for the profile-injecting constructors.
    ladder: Ladder,
    conn_id: u32,

    state: ConnState,
    floor: Floor,

    send: SendWindow,
    recv: RecvBuffer,
    reasm: Reassembler,
    msg_end_seqs: HashSet<u32>,

    outbox: VecDeque<LinkFrame>,
    events: VecDeque<HostEvent>,

    deadline: Option<Duration>,
    awaiting_reply: bool,
    owe_ack: bool,
    /// Whether to emit the SACK bitmap. Selective repeat sets it; the floor
    /// `WholeMessage` strategy clears it (cumulative-only, the "no NACK" model).
    sack_enabled: bool,
    /// Current link mode / ladder-rung id (stamped on every outgoing frame; the
    /// ARQ window + strategy follow it). Starts at `DEFAULT_RUNG`.
    current_rung: u8,
    /// Latest raw per-over SNR (dB) measured on the peer's transmissions, `NaN`
    /// until a measurement arrives. Drives **downshift** (no smoothing lag).
    snr_raw: f32,
    /// Asymmetrically-smoothed SNR (dB): falling applied immediately, rising via
    /// [`SNR_RISE_ALPHA`] EWMA. Drives **upshift** (needs sustained good SNR).
    snr_smoothed: f32,
    /// Recent frame-error rate observed on the peer's transmissions, with its
    /// sample count — the FER confidence gate for adaptation. Reset on rung change
    /// (FER is mode-conditioned).
    fer: f32,
    fer_samples: u32,
    /// Window the connection was constructed with — restored on session reset.
    initial_window: u32,
    silent_overs: u32,
    conn_retries: u32,
    disc_retries: u32,
}

impl Connection {
    /// Construct the connection initiator (owns the first floor after CONN_ACK).
    /// The initiator is *configured* with the peer it calls. Runs over the built-in
    /// ladder with the supplied profile on every rung (back-compat); use
    /// [`Connection::initiator_with_ladder`] to run over a PHY-published ladder.
    pub fn initiator(
        local: Callsign,
        remote: Callsign,
        conn_id: u32,
        profile: ModeProfile,
        window: u32,
    ) -> Self {
        Self::new(local, Some(remote), false, conn_id, profile, window)
    }

    /// Construct a connection acceptor — a listener that *learns* the calling
    /// station from the inbound `CONN` (no pre-configured peer) and adopts the
    /// peer's `CONN_ID`. Per the conn_id-addressing change, the data plane carries
    /// no callsigns, so the peer identity comes from the `CONN`'s station-ID block.
    pub fn acceptor(local: Callsign, profile: ModeProfile, window: u32) -> Self {
        Self::new(local, None, true, 0, profile, window)
    }

    /// Initiator over an explicit adaptation [`Ladder`] (the sonde-3tm registry
    /// handshake: a ladder built from the modes the PHY publishes). The active
    /// profile + timers are derived from the ladder's default rung and swap per-mode
    /// on adaptation (gap inventory B3).
    pub fn initiator_with_ladder(
        local: Callsign,
        remote: Callsign,
        conn_id: u32,
        ladder: Ladder,
        window: u32,
    ) -> Self {
        Self::with_ladder(local, Some(remote), false, conn_id, ladder, window)
    }

    /// Acceptor over an explicit adaptation [`Ladder`] (see
    /// [`Connection::initiator_with_ladder`]).
    pub fn acceptor_with_ladder(local: Callsign, ladder: Ladder, window: u32) -> Self {
        Self::with_ladder(local, None, true, 0, ladder, window)
    }

    fn new(
        local: Callsign,
        remote: Option<Callsign>,
        remote_is_learned: bool,
        conn_id: u32,
        profile: ModeProfile,
        window: u32,
    ) -> Self {
        Self::with_ladder(
            local,
            remote,
            remote_is_learned,
            conn_id,
            Ladder::standard_with_uniform_profile(profile),
            window,
        )
    }

    fn with_ladder(
        local: Callsign,
        remote: Option<Callsign>,
        remote_is_learned: bool,
        conn_id: u32,
        ladder: Ladder,
        window: u32,
    ) -> Self {
        let current_rung = ladder.default_rung();
        let profile = ladder.rung(current_rung).profile;
        let mut c = Self {
            local,
            remote,
            remote_is_learned,
            profile,
            ladder,
            conn_id,
            state: ConnState::Closed,
            floor: Floor::Listening,
            send: SendWindow::new(window),
            recv: RecvBuffer::new(window),
            reasm: Reassembler::new(),
            msg_end_seqs: HashSet::new(),
            outbox: VecDeque::new(),
            events: VecDeque::new(),
            deadline: None,
            awaiting_reply: false,
            owe_ack: false,
            sack_enabled: true,
            current_rung,
            snr_raw: f32::NAN,
            snr_smoothed: f32::NAN,
            fer: 0.0,
            fer_samples: 0,
            initial_window: window,
            silent_overs: 0,
            conn_retries: 0,
            disc_retries: 0,
        };
        // The ARQ follows the registered default mode (today the wideband floor =
        // WholeMessage/window-1); `with_strategy` can override before `connect`.
        c.configure_arq_for_rung(c.current_rung);
        c
    }

    /// Select the ARQ strategy (builder; apply before `connect`). The floor
    /// `WholeMessage` strategy is the degenerate stop-and-wait: window 1 and no
    /// SACK (the canonical floor "no NACK" model, design §5). Selective repeat
    /// keeps the constructed window and SACK.
    pub fn with_strategy(mut self, strategy: ArqStrategy) -> Self {
        match strategy {
            ArqStrategy::SelectiveRepeat => {
                self.sack_enabled = true;
                // Restore the constructed window (the default mode may be the
                // window-1 floor, which `new` configured).
                self.send = SendWindow::new(self.initial_window);
                self.recv = RecvBuffer::new(self.initial_window);
            }
            ArqStrategy::WholeMessage => {
                self.sack_enabled = false;
                self.send = SendWindow::new(1);
                self.recv = RecvBuffer::new(1);
            }
        }
        self
    }

    /// Current lifecycle state.
    pub fn state(&self) -> ConnState {
        self.state
    }

    /// The mode hint the driver should transmit under right now (the current
    /// ladder rung). The driver reads this per send so a mid-session mode change
    /// takes effect on the wire.
    pub fn current_hint(&self) -> ModeHint {
        self.ladder.rung(self.current_rung).mode
    }

    /// Current ladder-rung id (for tests / introspection).
    pub fn current_rung(&self) -> u8 {
        self.current_rung
    }

    /// Build a periodic station-`ID` frame (Part-97 §97.119) stamped with the
    /// current `conn_id` and mode. Used by the real-time [`Driver`](crate::Driver)
    /// to satisfy the ≤10-minute ID cadence in **real** time (the sans-IO
    /// connection runs on a logical clock that freezes during keying, so the
    /// cadence itself cannot live here — only the frame primitive does). The peer
    /// is always known mid-session, so this is only valid while `Connected`.
    pub fn make_id_frame(&self) -> LinkFrame {
        LinkFrame::id_frame(self.station_id(), self.conn_id)
            .with_mode(self.current_rung)
            .with_rx_rung(self.recommended_rung())
    }

    /// Begin connecting (initiator). No-op unless `Closed`.
    pub fn connect(&mut self, now: Duration) {
        if self.state != ConnState::Closed {
            return;
        }
        self.state = ConnState::Connecting;
        self.conn_retries = 0;
        let conn = self.make_control(FrameType::Conn, FIRST_SEQ);
        self.outbox.push_back(conn);
        self.deadline = Some(now + self.profile.turn_recovery_timeout());
    }

    /// Enqueue a host message for reliable, in-order delivery. Fragmented across
    /// DATA frames per the mode's per-over MTU; reassembled at the peer.
    pub fn send(&mut self, data: Vec<u8>) {
        let frag = self.profile.per_over_mtu().max(1);
        let mut last = FIRST_SEQ;
        if data.is_empty() {
            last = self.send.enqueue(Vec::new());
        } else {
            for chunk in data.chunks(frag) {
                last = self.send.enqueue(chunk.to_vec());
            }
        }
        self.msg_end_seqs.insert(last);
    }

    /// Begin a clean teardown.
    pub fn disconnect(&mut self, now: Duration) {
        if self.state != ConnState::Connected {
            return;
        }
        self.state = ConnState::Disconnecting;
        self.disc_retries = 0;
        let disc = self.make_control(FrameType::Disc, 0);
        self.outbox.push_back(disc);
        self.deadline = Some(now + self.profile.turn_recovery_timeout());
    }

    /// Feed one decoded inbound frame.
    ///
    /// Addressing is by **`conn_id`**, not by per-frame callsign (the data plane
    /// no longer carries callsigns). The data plane (`DATA`/`ACK`/`KEEPALIVE`) is
    /// demuxed by `session_ok` (Connected + matching `conn_id`). ID-bearing frames
    /// (`CONN`/`CONN_ACK`/`DISC`/`DISC_ACK`/`ID`) validate the station-ID block
    /// inside their handlers (peer bootstrap + defense-in-depth).
    pub fn handle_frame(&mut self, frame: LinkFrame, now: Duration) {
        match frame.frame_type {
            FrameType::Conn => self.on_conn(&frame, now),
            FrameType::ConnAck => self.on_conn_ack(&frame, now),
            FrameType::Disc => self.on_disc(&frame),
            FrameType::DiscAck => self.on_disc_ack(&frame),
            FrameType::Id => self.on_id(&frame, now),
            FrameType::Data if self.session_ok(&frame) => self.on_data(frame, now),
            FrameType::Ack if self.session_ok(&frame) => self.on_ack(&frame, now),
            FrameType::Keepalive if self.session_ok(&frame) => self.on_keepalive(&frame, now),
            _ => {}
        }
    }

    /// Advance timers to `now`.
    pub fn handle_timeout(&mut self, now: Duration) {
        let due = matches!(self.deadline, Some(d) if now >= d);
        match self.state {
            ConnState::Connecting if due => {
                self.conn_retries += 1;
                if self.conn_retries > MAX_CONN_RETRIES {
                    self.close(Some(HostEvent::ConnectFailed));
                } else {
                    let conn = self.make_control(FrameType::Conn, FIRST_SEQ);
                    self.outbox.push_back(conn);
                    self.deadline = Some(now + self.profile.turn_recovery_timeout());
                }
            }
            ConnState::Disconnecting if due => {
                self.disc_retries += 1;
                if self.disc_retries > MAX_DISC_RETRIES {
                    self.close(Some(HostEvent::Disconnected));
                } else {
                    let disc = self.make_control(FrameType::Disc, 0);
                    self.outbox.push_back(disc);
                    self.deadline = Some(now + self.profile.turn_recovery_timeout());
                }
            }
            ConnState::Connected if due => match self.floor {
                Floor::Listening if self.awaiting_reply => {
                    // We sent data and expected a reply that did not arrive. A total
                    // miss yields NO measurement (we decoded nothing), so the
                    // measurement loop can't react — the floor holder downshifts on
                    // its own (the Idle-floor blind spot), and sustained misses still
                    // cascade to the P1 BASE-fallback below.
                    self.silent_overs += 1;
                    let base = self.ladder.base_rung();
                    if self.current_rung != base && self.silent_overs >= DOWNSHIFT_TO_BASE_OVERS {
                        // We cannot get through at this mode — fall to the robust
                        // BASE mode (design P1) and keep trying there with a fresh
                        // death budget. Both ends do this symmetrically (and the
                        // peer also follows our mode-id), so they converge to BASE
                        // instead of dying. BASE is the most-robust *available* rung.
                        // Re-take the floor to retransmit.
                        self.apply_rung(base, now);
                        self.silent_overs = 0;
                        self.floor = Floor::Sending;
                        self.deadline = None;
                    } else if self.silent_overs > DEAD_OVERS_TOLERATED {
                        self.close(Some(HostEvent::PeerLost));
                    } else {
                        // Graceful self-downshift one step on a missed reply (§2.4),
                        // independent of any feedback; sustained misses still cascade
                        // to the P1 BASE-fallback above. Then re-take the floor to
                        // retransmit the unacked window. apply_rung clamps to the next
                        // *available* rung (a no-op once we are at BASE).
                        if self.current_rung != base {
                            self.apply_rung(self.current_rung + 1, now);
                        }
                        self.floor = Floor::Sending;
                        self.deadline = None;
                    }
                }
                Floor::Listening => {
                    // We passed the floor with nothing pending and the peer
                    // declined (or said nothing) — the link is quiescent.
                    self.floor = Floor::Idle;
                    self.deadline = None;
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Next frame to transmit, if any (drains the current over frame by frame).
    pub fn poll_transmit(&mut self, now: Duration) -> Option<LinkFrame> {
        if self.state == ConnState::Connected && self.outbox.is_empty() {
            // Acquire the free floor if we have something to say.
            if self.floor == Floor::Idle && self.want_floor() {
                self.floor = Floor::Sending;
            }
            // While we hold the floor, build this over (data, ack, or — with
            // nothing pending — a keepalive that passes the floor back).
            if self.floor == Floor::Sending {
                self.build_over();
            }
        }
        let f = self.outbox.pop_front();
        if let Some(ref frame) = f {
            if self.state == ConnState::Connected && frame.is_end_of_over() {
                // Our over is done. If we sent data we await an ack (Listening +
                // turn-recovery); if we just relinquished (ack/keepalive, nothing
                // pending) the floor is free (Idle) so either side can acquire it.
                self.awaiting_reply = self.send.has_unacked();
                if self.awaiting_reply {
                    self.floor = Floor::Listening;
                    self.deadline =
                        Some(now + self.profile.turn_recovery_timeout() + self.backoff_jitter());
                } else {
                    self.floor = Floor::Idle;
                    self.deadline = None;
                }
            }
        }
        f
    }

    /// Next host-facing event, if any.
    pub fn poll_event(&mut self) -> Option<HostEvent> {
        self.events.pop_front()
    }

    /// When the next timer is due (for the driver to schedule a wake-up).
    pub fn next_timeout(&self) -> Option<Duration> {
        self.deadline
    }

    // ---- internals ------------------------------------------------------

    /// Whether we have a reason to hold the floor: unacked data to (re)send, or
    /// an ack we owe for DATA just received.
    fn want_floor(&self) -> bool {
        self.send.has_unacked() || self.owe_ack
    }

    /// Switch to ladder rung `id`: restamp the mode, resize the ARQ window in
    /// place (preserving the seq stream + in-flight), flip SACK on/off for the
    /// rung's strategy, and **swap the airtime-aware [`ModeProfile`] to the target
    /// rung** so per-mode timers (turn-recovery/over/keepalive) track the active
    /// mode (gap inventory B3). For a uniform-profile ladder (the profile-injecting
    /// constructors) every rung shares one profile, so the swap is a no-op.
    ///
    /// `id` is **clamped to an available rung** ([`Ladder::clamp_available`]) — a
    /// fabricated/unregistered rung is never selected or transmitted (C7), and an
    /// inbound id this build lacks is absorbed toward the nearest more-robust real
    /// rung. (Same-build links share one ladder; cross-build capability negotiation
    /// is out of scope — see the registry-handshake design.)
    fn apply_rung(&mut self, id: u8, now: Duration) {
        let id = self.ladder.clamp_available(id);
        if id == self.current_rung {
            return;
        }
        let old_family = self.ladder.family_of(self.current_rung);
        self.current_rung = id;
        self.configure_arq_for_rung(id);
        // Swap the airtime-aware profile to the target rung (B3): every link timer
        // derives from it, so the mode's real over-airtime now governs the clocks.
        self.profile = self.ladder.rung(id).profile;
        // Re-arm any outstanding turn-recovery deadline from the NEW profile: a
        // downshift to a slower mode lengthened the over-airtime, so a deadline
        // armed under the old (faster) profile would fire a premature retransmit
        // before the slower over could even complete (Codex review). Recompute from
        // `now`; the timeout/floor handlers that null the deadline still override.
        if self.deadline.is_some() {
            self.deadline = Some(now + self.profile.turn_recovery_timeout());
        }
        // FER is mode-conditioned ⇒ always reset on a rung change (design F6).
        self.fer = 0.0;
        self.fer_samples = 0;
        // The reported usable SNR is only valid within a waveform family ⇒ reset
        // the estimate when the family changes (design F6); keep it within a family
        // (physical SNR is rung-independent).
        if self.ladder.family_of(id) != old_family {
            self.snr_raw = f32::NAN;
            self.snr_smoothed = f32::NAN;
        }
    }

    /// Configure the ARQ (window + SACK/strategy) to match a ladder rung. The
    /// registered default mode is the wideband floor (`WholeMessage`, window 1, no
    /// SACK), so the link runs stop-and-wait until a `SelectiveRepeat` mode (the
    /// OFDM family) registers a real waveform.
    fn configure_arq_for_rung(&mut self, id: u8) {
        let r = self.ladder.rung(id);
        self.send.reconfigure(r.window.window);
        self.recv.reconfigure(r.window.window);
        self.sack_enabled = matches!(r.strategy, ArqStrategy::SelectiveRepeat);
    }

    /// Follow a peer frame's advertised mode (the mode-id confirmation path): if
    /// the peer is transmitting at a different rung, adopt it so our replies go
    /// out at the same mode and the two ends converge. Used only by the *follower*
    /// (the listener), never the floor-holding decider (see the role-gate in the
    /// inbound handlers).
    fn follow_mode(&mut self, peer_mode: u8, now: Duration) {
        if peer_mode != self.current_rung {
            self.apply_rung(peer_mode, now);
        }
    }

    /// Feed a per-over channel measurement (the receiver's view of the peer's
    /// transmissions). Called by the real-time `Driver`/`Link` from the PHY's
    /// `channel_quality()`, and by the gates with synthetic values. Falling SNR is
    /// applied immediately (fast downshift); rising SNR is EWMA-smoothed (design
    /// F3). A non-finite SNR (no measurement) leaves the estimate unchanged.
    pub fn observe_quality(&mut self, snr_db: f32, fer: f32, fer_samples: u32) {
        self.fer = fer;
        self.fer_samples = fer_samples;
        if snr_db.is_finite() {
            self.snr_raw = snr_db;
            self.snr_smoothed = if !self.snr_smoothed.is_finite() || snr_db < self.snr_smoothed {
                snr_db // first reading, or falling ⇒ apply at once
            } else {
                SNR_RISE_ALPHA * snr_db + (1.0 - SNR_RISE_ALPHA) * self.snr_smoothed
            };
        }
    }

    /// The ladder rung this station recommends the PEER use (advertised as
    /// `rx_rung`), from its smoothed channel measurement via the symmetric,
    /// FER-gated [`Ladder::adapt_rung`]. No measurement ⇒ `current_rung` (no change).
    fn recommended_rung(&self) -> u8 {
        self.ladder.adapt_rung(
            self.current_rung,
            self.snr_raw,
            self.snr_smoothed,
            self.fer,
            self.fer_samples,
        )
    }

    /// Act on the peer's `rx_rung` feedback while we are the floor-holding decider.
    /// The peer's recommendation is authoritative for *our* TX rung (it observed
    /// our path), but the **worse direction wins** (design F5): take the more
    /// robust of the peer's feedback and our own reverse-path recommendation, so a
    /// good forward path cannot pull the shared rung up and break a bad reverse
    /// path. Applied authoritatively in both directions — no probe, no streak.
    fn apply_peer_feedback(&mut self, peer_rx_rung: u8, now: Duration) {
        // More robust (higher id) of the peer's feedback and our own reverse-path
        // recommendation. `apply_rung` clamps the result to an available rung, so an
        // inbound rung this build lacks is absorbed.
        let target = peer_rx_rung.max(self.recommended_rung());
        self.apply_rung(target, now);
    }

    /// Transition to `Closed`, clearing all per-session ARQ/reassembly state so a
    /// same-object reconnect starts clean (Codex blocker #1). Optionally surface
    /// a host event. Does not touch the outbox (a queued DISC_ACK still flushes).
    fn close(&mut self, event: Option<HostEvent>) {
        self.state = ConnState::Closed;
        self.reset_session();
        if let Some(e) = event {
            self.events.push_back(e);
        }
    }

    /// Reset per-session state to a fresh start. Keeps `local`/profile/conn_id and
    /// a *configured* peer (initiator), but clears a *learned* peer (acceptor) so a
    /// listener can accept a different station on its next session (Codex review).
    fn reset_session(&mut self) {
        if self.remote_is_learned {
            self.remote = None;
        }
        self.send.reset();
        self.recv.reset();
        self.reasm.reset();
        // Restore the default (most-robust available) registered mode and its ARQ
        // config so a same-object reconnect starts fresh at a real rung.
        self.current_rung = self.ladder.default_rung();
        self.configure_arq_for_rung(self.current_rung);
        self.snr_raw = f32::NAN;
        self.snr_smoothed = f32::NAN;
        self.fer = 0.0;
        self.fer_samples = 0;
        self.msg_end_seqs.clear();
        self.floor = Floor::Listening;
        self.awaiting_reply = false;
        self.owe_ack = false;
        self.silent_overs = 0;
        self.conn_retries = 0;
        self.disc_retries = 0;
        self.deadline = None;
    }

    /// Deterministic, bounded jitter added to the turn-recovery deadline so two
    /// stations that lost the floor at the same instant do not phase-lock into
    /// repeated re-take collisions (design §3.5). The callsign tie-break decides
    /// the eventual winner; the jitter (derived from our callsign + the silent-
    /// over count, so it is reproducible and varies per retry) just spreads them
    /// in time. Bounded to half a turn-recovery so it never starves death
    /// detection.
    fn backoff_jitter(&self) -> Duration {
        let mut h = 0xcbf2_9ce4_8422_2325u64; // FNV-1a
        for b in self.local.as_str().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        h = h
            .wrapping_add(self.silent_overs as u64)
            .wrapping_mul(0x100_0000_01b3);
        let span_ms = (self.profile.turn_recovery_timeout().as_millis() / 2).max(1) as u64;
        Duration::from_millis(h % span_ms)
    }

    fn session_ok(&self, frame: &LinkFrame) -> bool {
        self.state == ConnState::Connected && frame.conn_id == self.conn_id
    }

    /// The station-ID block (`SRC` = us, `DST` = peer) for an ID-bearing frame.
    /// Only called once the peer is known (initiator: always; acceptor: after the
    /// CONN that taught it `remote`).
    fn station_id(&self) -> StationId {
        StationId::new(
            self.local.clone(),
            self.remote
                .clone()
                .expect("id-bearing frame requires a known peer"),
        )
    }

    /// Build a control frame, ID-bearing or not per its type. `CONN`/`CONN_ACK`/
    /// `DISC`/`DISC_ACK` carry the station-ID block (Part-97 start/end ID);
    /// `KEEPALIVE`/`NAK` carry no callsigns.
    fn make_control(&self, frame_type: FrameType, seq: u32) -> LinkFrame {
        let f = if frame_type.is_id_bearing() {
            LinkFrame::id_control(frame_type, self.station_id(), self.conn_id, seq)
        } else {
            LinkFrame::control(frame_type, self.conn_id, seq)
        };
        f.with_mode(self.current_rung)
            .with_rx_rung(self.recommended_rung())
    }

    fn make_data(&self, seq: u32, payload: Vec<u8>) -> LinkFrame {
        let mut f = LinkFrame::data(self.conn_id, seq, payload);
        f.ack_through = self.recv.ack_through();
        f.sack = self.sack_to_send();
        f.mode = self.current_rung;
        if self.msg_end_seqs.contains(&seq) {
            f = f.end_of_msg();
        }
        f.with_rx_rung(self.recommended_rung())
    }

    fn make_ack(&self) -> LinkFrame {
        LinkFrame::ack(self.conn_id, self.recv.ack_through(), self.sack_to_send())
            .with_mode(self.current_rung)
            .with_rx_rung(self.recommended_rung())
    }

    /// The SACK bitmap to advertise: the live receive-buffer bitmap under
    /// selective repeat, or `0` under the floor `WholeMessage` strategy.
    fn sack_to_send(&self) -> u32 {
        if self.sack_enabled {
            self.recv.sack()
        } else {
            0
        }
    }

    /// Build this over's frames into the outbox: up to `W` unacked DATA frames
    /// (gaps + fresh), or a bare ACK if there is no data. The last frame carries
    /// `END_OF_OVER` (the floor token).
    fn build_over(&mut self) {
        let frames = self.send.over_frames();
        let mut built: Vec<LinkFrame> = frames
            .into_iter()
            .map(|(seq, p)| self.make_data(seq, p))
            .collect();
        if built.is_empty() {
            // Nothing to send: an ACK if we owe one for received DATA, else a
            // bare KEEPALIVE — either way the over ends and the floor passes, so
            // an idle holder relinquishes instead of starving the peer.
            if self.owe_ack {
                built.push(self.make_ack());
            } else {
                built.push(self.make_control(FrameType::Keepalive, 0));
            }
        }
        let last = built.pop().expect("over has >= 1 frame").end_of_over();
        built.push(last);
        self.owe_ack = false;
        self.outbox.extend(built);
    }

    fn accept_connection_as_acceptor(&mut self, conn_id: u32, seq: u32, now: Duration) {
        self.conn_id = conn_id;
        self.state = ConnState::Connected;
        // The initiator owns the first floor; listen for it, but fall back to
        // the free `Idle` floor if it never speaks (so we can originate).
        self.floor = Floor::Listening;
        self.awaiting_reply = false;
        self.silent_overs = 0;
        self.deadline = Some(now + self.profile.turn_recovery_timeout() + self.backoff_jitter());
        self.events.push_back(HostEvent::Connected);
        let ack = self.make_control(FrameType::ConnAck, seq);
        self.outbox.push_back(ack);
    }

    fn on_conn(&mut self, frame: &LinkFrame, now: Duration) {
        // A CONN must carry a station-ID block and be addressed to us.
        let peer = match &frame.id {
            Some(id) if id.dst == self.local => id.src.clone(),
            _ => return,
        };
        // The reserved (zero) conn_id is never a live session — reject (sonde-44n).
        if frame.conn_id == crate::frame::CONN_ID_RESERVED {
            return;
        }
        match self.state {
            ConnState::Closed => {
                if self.remote_is_learned {
                    // Listener: learn the calling station from CONN.src.
                    self.remote = Some(peer);
                } else if self.remote.as_ref() != Some(&peer) {
                    // Configured initiator idle in Closed: accept only our peer.
                    return;
                }
                self.accept_connection_as_acceptor(frame.conn_id, frame.seq, now);
            }
            ConnState::Connected => {
                // Only a CONN from our current peer is relevant while connected (a
                // third station's CONN is ignored — we hold the single half-duplex
                // session).
                if self.remote.as_ref() == Some(&peer) {
                    if frame.conn_id == self.conn_id {
                        // Idempotent CONN replay for THIS session: the peer didn't
                        // hear our CONN_ACK — resend it, without resetting.
                        let ack = self.make_control(FrameType::ConnAck, frame.seq);
                        self.outbox.push_back(ack);
                    } else {
                        // Fresh CONN from our peer with a NEW conn_id: the peer
                        // rebooted or lost our DISC (half-open). Accept it as a
                        // reconnect — flush the stale session and re-accept — rather
                        // than dropping it and wedging forever (there is no idle
                        // keepalive yet) (sonde-ajn / B6). `reset_session` clears the
                        // old ARQ/reassembly + (for a learner) the learned peer, so
                        // re-establish `remote` before accepting.
                        self.reset_session();
                        self.remote = Some(peer);
                        self.accept_connection_as_acceptor(frame.conn_id, frame.seq, now);
                    }
                }
            }
            ConnState::Connecting => {
                // CONN/CONN collision. We are an initiator with a configured peer;
                // the colliding CONN must be from that peer, then tie-break by
                // callsign (higher keeps the initiator role; lower becomes the
                // acceptor — design §4). Learning never happens here, so it cannot
                // corrupt the tie-break (Codex review).
                if self.remote.as_ref() != Some(&peer) {
                    return;
                }
                if self.local.as_str() <= peer.as_str() {
                    self.accept_connection_as_acceptor(frame.conn_id, frame.seq, now);
                }
            }
            ConnState::Disconnecting => {}
        }
    }

    fn on_conn_ack(&mut self, frame: &LinkFrame, now: Duration) {
        if self.state == ConnState::Connecting && frame.conn_id == self.conn_id {
            // Defense-in-depth: the acceptor's ID block must name our peer (SRC)
            // and us (DST). conn_id is the primary check.
            match &frame.id {
                Some(id) if id.dst == self.local && self.remote.as_ref() == Some(&id.src) => {}
                _ => return,
            }
            self.state = ConnState::Connected;
            self.floor = Floor::Sending; // initiator owns the first floor
            self.silent_overs = 0;
            self.deadline = None;
            self.events.push_back(HostEvent::Connected);
            // We are now the floor-holding decider for the first over: honor the
            // acceptor's initial rung feedback so the first over starts at the right
            // rung instead of blindly at DEFAULT_RUNG (worse-direction-wins).
            self.apply_peer_feedback(frame.rx_rung(), now);
        }
    }

    fn on_disc(&mut self, frame: &LinkFrame) {
        if (self.state == ConnState::Connected || self.state == ConnState::Disconnecting)
            && frame.conn_id == self.conn_id
            && self.id_addressed_to_us(frame)
        {
            let ack = self.make_control(FrameType::DiscAck, 0);
            self.outbox.push_back(ack);
            self.close(Some(HostEvent::Disconnected));
        }
    }

    fn on_disc_ack(&mut self, frame: &LinkFrame) {
        if self.state == ConnState::Disconnecting
            && frame.conn_id == self.conn_id
            && self.id_addressed_to_us(frame)
        {
            self.close(Some(HostEvent::Disconnected));
        }
    }

    /// A received periodic `ID` (Part-97) on this session: it proves the peer is
    /// alive (like a keepalive) and confirms the peer's current mode. No host
    /// event; passes the floor only if it carries the over-end token.
    fn on_id(&mut self, frame: &LinkFrame, now: Duration) {
        if !self.session_ok(frame) || !self.id_addressed_to_us(frame) {
            return;
        }
        self.silent_overs = 0;
        self.adapt_on_inbound(frame, now);
        if frame.is_end_of_over() {
            self.on_over_end();
        }
    }

    /// Apply link-adaptation on an inbound frame, role-gated by floor position.
    /// If we are the floor-holding **decider** (we sent an over and are awaiting its
    /// reply), act on the peer's `rx_rung` feedback (authoritative both directions,
    /// worse-direction-wins). Otherwise we are the **follower** (the peer holds the
    /// floor): adopt its announced `MODE` so our replies match and both ends stay on
    /// one rung. The up/down asymmetry now lives in the measurement (raw vs smoothed
    /// SNR + FER gate in [`Ladder::adapt_rung`]), not in any per-frame counter.
    fn adapt_on_inbound(&mut self, frame: &LinkFrame, now: Duration) {
        if self.awaiting_reply {
            self.apply_peer_feedback(frame.rx_rung(), now);
        } else {
            self.follow_mode(frame.mode, now);
        }
    }

    /// Defense-in-depth check for an ID-bearing frame: its `DST` names us and (once
    /// the peer is known) its `SRC` names our peer. `conn_id` remains the primary
    /// demux; this just rejects an obviously cross-addressed ID-bearing frame.
    fn id_addressed_to_us(&self, frame: &LinkFrame) -> bool {
        match &frame.id {
            // MSRV 1.75: `map_or(true, ...)` rather than `Option::is_none_or` (1.82).
            Some(id) => id.dst == self.local && self.remote.as_ref().map_or(true, |r| *r == id.src),
            None => false,
        }
    }

    fn on_data(&mut self, frame: LinkFrame, now: Duration) {
        self.silent_overs = 0;
        self.adapt_on_inbound(&frame, now);
        self.send.on_ack(frame.ack_through, frame.sack); // piggybacked ack
        let end_of_over = frame.is_end_of_over();
        let end_of_msg = frame.is_end_of_msg();
        let delivered = self.recv.accept(frame.seq, frame.payload, end_of_msg);
        for d in delivered {
            if let Some(msg) = self.reasm.push(d) {
                self.events.push_back(HostEvent::DataReceived(msg));
            }
        }
        self.owe_ack = true;
        if end_of_over {
            self.on_over_end();
        }
    }

    fn on_ack(&mut self, frame: &LinkFrame, now: Duration) {
        self.silent_overs = 0;
        self.adapt_on_inbound(frame, now);
        self.send.on_ack(frame.ack_through, frame.sack);
        if frame.is_end_of_over() {
            self.on_over_end();
        }
    }

    fn on_keepalive(&mut self, frame: &LinkFrame, now: Duration) {
        self.silent_overs = 0;
        self.adapt_on_inbound(frame, now);
        // A keepalive may carry the floor token (an idle holder relinquishing).
        if frame.is_end_of_over() {
            self.on_over_end();
        }
    }

    /// The peer ended its over (passed the floor). Take the floor only if we
    /// have unacked data or owe an ack (the quiescence rule); otherwise the link
    /// goes `Idle` (free) — either station can then acquire it when it next has
    /// something to say. This is symmetric: neither station hogs an idle floor.
    fn on_over_end(&mut self) {
        self.silent_overs = 0;
        if self.want_floor() {
            self.floor = Floor::Sending;
        } else {
            self.floor = Floor::Idle;
            self.awaiting_reply = false;
        }
        self.deadline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac;

    fn call(s: &str) -> Callsign {
        Callsign::new(s).unwrap()
    }

    /// A CONN from `K1ABC` to `W2XYZ` (the standard peer pair in these tests).
    fn conn_from_k1abc(conn_id: u32, seq: u32) -> LinkFrame {
        LinkFrame::id_control(
            FrameType::Conn,
            StationId::new(call("K1ABC"), call("W2XYZ")),
            conn_id,
            seq,
        )
    }

    fn profile() -> ModeProfile {
        // Small per-over MTU so multi-frame messages are easy to force.
        ModeProfile::new(Duration::from_millis(10), 4)
    }

    fn messages(events: &[HostEvent]) -> Vec<Vec<u8>> {
        events
            .iter()
            .filter_map(|e| match e {
                HostEvent::DataReceived(d) => Some(d.clone()),
                _ => None,
            })
            .collect()
    }

    // ---- registry handshake B3: per-mode ModeProfile swap on a rung change -------

    #[test]
    fn apply_rung_swaps_the_mode_profile_to_the_target_rung() {
        // A multi-available-rung ladder (the synthetic algorithm fixture) carries the
        // standard distinct per-rung profiles. Switching rungs must swap the active
        // profile so per-mode timers (turn-recovery/over/keepalive) track the mode
        // (gap inventory B3 — previously apply_rung resized ARQ but never the profile).
        let ladder = crate::mac::test_support::all_available_ladder();
        let mut c =
            Connection::initiator_with_ladder(call("K1ABC"), call("W2XYZ"), 0x1234, ladder, 8);
        // Construction adopts the default (most-robust available = deepest) rung.
        let before = c.profile.over_airtime();
        c.apply_rung(0, Duration::ZERO); // climb to the fastest rung
        let after = c.profile.over_airtime();
        assert_ne!(
            before, after,
            "apply_rung must swap the ModeProfile to the target rung"
        );
        assert_eq!(
            after,
            Duration::from_millis(300),
            "fast rung's over-airtime is now active"
        );
        assert_eq!(
            c.profile.per_over_mtu(),
            1024,
            "fast rung's per-frame capacity is now active"
        );
    }

    #[test]
    fn uniform_profile_ladder_keeps_the_injected_profile_across_rung_changes() {
        // The back-compat path: a profile-injecting constructor builds a uniform
        // ladder, so a rung change is a profile no-op (preserves injected timers/MTU).
        let mut c = Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8);
        let before = c.profile.clone();
        // Force a rung change that clamp keeps real; uniform profile is unchanged.
        c.apply_rung(0, Duration::ZERO);
        assert_eq!(c.profile, before, "uniform-profile ladder: swap is a no-op");
        assert_eq!(c.profile, profile());
    }

    #[test]
    fn apply_rung_rearms_an_outstanding_deadline_for_a_slower_profile() {
        // A turn-recovery deadline armed under a fast rung must be re-armed when we
        // downshift to a slower mode mid-wait — otherwise it fires a premature
        // retransmit before the slower over could even complete (Codex review).
        let ladder = crate::mac::test_support::all_available_ladder();
        let mut c =
            Connection::initiator_with_ladder(call("K1ABC"), call("W2XYZ"), 0x1234, ladder, 8);
        c.apply_rung(0, Duration::ZERO); // fast rung (300 ms over-airtime)
        let now = Duration::from_secs(1);
        // Arm a turn-recovery deadline under the fast profile (as poll_transmit would).
        c.deadline = Some(now + c.profile.turn_recovery_timeout());
        let fast_deadline = c.deadline.unwrap();
        // Downshift to the slowest rung at the same instant.
        c.apply_rung(4, now);
        let slow_deadline = c
            .deadline
            .expect("the deadline stays armed across a downshift");
        assert!(
            slow_deadline > fast_deadline,
            "a downshift to a slower mode must push the deadline out, not fire early"
        );
        assert_eq!(
            slow_deadline,
            now + c.profile.turn_recovery_timeout(),
            "re-armed from the new (slower) profile"
        );
    }

    #[test]
    fn apply_rung_leaves_an_idle_link_without_a_deadline() {
        // No outstanding deadline (Idle floor) ⇒ a rung change must not fabricate one.
        let ladder = crate::mac::test_support::all_available_ladder();
        let mut c =
            Connection::initiator_with_ladder(call("K1ABC"), call("W2XYZ"), 0x1234, ladder, 8);
        assert!(c.deadline.is_none());
        c.apply_rung(0, Duration::from_secs(1));
        assert!(c.deadline.is_none(), "no deadline to re-arm ⇒ stays None");
    }

    const TICK: Duration = Duration::from_millis(10);

    /// A perfect (lossless) in-memory pipe between two connections, advancing a
    /// logical clock. Returns once no frames moved in a full round (settled).
    struct Pair {
        a: Connection,
        b: Connection,
        now: Duration,
        a_events: Vec<HostEvent>,
        b_events: Vec<HostEvent>,
    }

    impl Pair {
        fn new() -> Self {
            let a = Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8);
            let b = Connection::acceptor(call("W2XYZ"), profile(), 8);
            Self {
                a,
                b,
                now: Duration::ZERO,
                a_events: Vec::new(),
                b_events: Vec::new(),
            }
        }

        fn drain_events(&mut self) {
            while let Some(e) = self.a.poll_event() {
                self.a_events.push(e);
            }
            while let Some(e) = self.b.poll_event() {
                self.b_events.push(e);
            }
        }

        /// One round: a transmits its over to b, then b to a, then timers fire.
        fn step(&mut self) -> bool {
            self.step_drop(None)
        }

        /// Like `step`, but drop the frame at `drop_a_idx` in A's over (to
        /// deterministically model a single in-transit loss A→B).
        fn step_drop(&mut self, drop_a_idx: Option<usize>) -> bool {
            let mut moved = false;
            let mut i = 0;
            while let Some(f) = self.a.poll_transmit(self.now) {
                moved = true;
                if Some(i) != drop_a_idx {
                    self.b.handle_frame(f, self.now);
                }
                i += 1;
            }
            while let Some(f) = self.b.poll_transmit(self.now) {
                moved = true;
                self.a.handle_frame(f, self.now);
            }
            self.now += TICK;
            self.a.handle_timeout(self.now);
            self.b.handle_timeout(self.now);
            self.drain_events();
            moved
        }

        fn b_messages(&self) -> Vec<Vec<u8>> {
            messages(&self.b_events)
        }

        fn a_messages(&self) -> Vec<Vec<u8>> {
            messages(&self.a_events)
        }

        fn run(&mut self, max_steps: usize) {
            for _ in 0..max_steps {
                if !self.step() {
                    return;
                }
            }
        }
    }

    #[test]
    fn handshake_connects_both_sides_initiator_holds_first_floor() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        assert_eq!(p.a.state(), ConnState::Connected);
        assert_eq!(p.b.state(), ConnState::Connected);
        assert!(p.a_events.contains(&HostEvent::Connected));
        assert!(p.b_events.contains(&HostEvent::Connected));
    }

    #[test]
    fn single_small_message_is_delivered_byte_exact() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"hi".to_vec());
        p.run(40);
        assert_eq!(
            p.b_events
                .iter()
                .filter_map(|e| match e {
                    HostEvent::DataReceived(d) => Some(d.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![b"hi".to_vec()]
        );
    }

    #[test]
    fn multi_frame_message_reassembles_in_order() {
        // per_over_mtu = 4 ⇒ this 10-byte message fragments across 3 frames.
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"0123456789".to_vec());
        p.run(60);
        let got: Vec<Vec<u8>> = p
            .b_events
            .iter()
            .filter_map(|e| match e {
                HostEvent::DataReceived(d) => Some(d.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(got, vec![b"0123456789".to_vec()]);
    }

    #[test]
    fn multiple_messages_arrive_in_order_without_duplication() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"first".to_vec());
        p.a.send(b"second".to_vec());
        p.a.send(b"third".to_vec());
        p.run(100);
        let got: Vec<Vec<u8>> = p
            .b_events
            .iter()
            .filter_map(|e| match e {
                HostEvent::DataReceived(d) => Some(d.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]
        );
    }

    #[test]
    fn clean_teardown_disconnects_both_sides() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.disconnect(p.now);
        p.run(20);
        assert_eq!(p.a.state(), ConnState::Closed);
        assert_eq!(p.b.state(), ConnState::Closed);
        assert!(p.a_events.contains(&HostEvent::Disconnected));
        assert!(p.b_events.contains(&HostEvent::Disconnected));
    }

    #[test]
    fn exchange_settles_after_delivery_no_duplicate_data() {
        // The quiescence rule must stop data chatter: the message is delivered
        // exactly once even though the idle link keeps a keepalive heartbeat.
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"x".to_vec());
        p.run(200);
        let count = p
            .b_events
            .iter()
            .filter(|e| matches!(e, HostEvent::DataReceived(_)))
            .count();
        assert_eq!(count, 1, "message delivered exactly once, no duplicates");
        assert_eq!(p.a.state(), ConnState::Connected);
        assert_eq!(p.b.state(), ConnState::Connected);
    }

    // ---- SM hardening (design §4 watched failure modes) -----------------

    fn init() -> Connection {
        Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8)
    }
    fn acc() -> Connection {
        Connection::acceptor(call("W2XYZ"), profile(), 8)
    }

    #[test]
    fn handshake_recovers_from_a_lost_conn() {
        let mut a = init();
        let mut b = acc();
        a.connect(Duration::ZERO);
        let lost = a.poll_transmit(Duration::ZERO).unwrap();
        assert_eq!(lost.frame_type, FrameType::Conn); // dropped in transit

        let t = a.profile.turn_recovery_timeout();
        a.handle_timeout(t); // connect-retry fires
        let conn = a.poll_transmit(t).unwrap();
        assert_eq!(conn.frame_type, FrameType::Conn);
        b.handle_frame(conn, t);
        let ack = b.poll_transmit(t).unwrap();
        a.handle_frame(ack, t);

        assert_eq!(a.state(), ConnState::Connected);
        assert_eq!(b.state(), ConnState::Connected);
    }

    #[test]
    fn handshake_recovers_from_a_lost_conn_ack() {
        let mut a = init();
        let mut b = acc();
        a.connect(Duration::ZERO);
        let conn = a.poll_transmit(Duration::ZERO).unwrap();
        b.handle_frame(conn, Duration::ZERO);
        let lost_ack = b.poll_transmit(Duration::ZERO).unwrap();
        assert_eq!(lost_ack.frame_type, FrameType::ConnAck); // dropped

        let t = a.profile.turn_recovery_timeout();
        a.handle_timeout(t); // A retransmits CONN
        let conn2 = a.poll_transmit(t).unwrap();
        b.handle_frame(conn2, t); // B resends CONN_ACK idempotently
        let ack = b.poll_transmit(t).unwrap();
        assert_eq!(ack.frame_type, FrameType::ConnAck);
        a.handle_frame(ack, t);

        assert_eq!(a.state(), ConnState::Connected);
        assert_eq!(b.state(), ConnState::Connected);
    }

    #[test]
    fn frame_with_wrong_conn_id_is_rejected_half_open() {
        let mut b = acc();
        b.handle_frame(conn_from_k1abc(0x1234, FIRST_SEQ), Duration::ZERO);
        assert_eq!(b.state(), ConnState::Connected);
        while b.poll_event().is_some() {} // drain the Connected event
                                          // A DATA frame stamped with a *different* session id must be ignored.
        let stale = LinkFrame::data(0x9999, 1, b"x".to_vec())
            .end_of_msg()
            .end_of_over();
        b.handle_frame(stale, Duration::ZERO);
        assert!(
            b.poll_event().is_none(),
            "stale-session DATA must not deliver"
        );
    }

    #[test]
    fn conn_addressed_to_a_different_station_is_ignored() {
        // With per-frame callsigns gone, a CONN is addressed by its station-ID
        // block. A listener must ignore a CONN whose DST is not itself (it is a
        // call to some other station that happens to be audible).
        let mut b = acc(); // local = W2XYZ
        let not_for_us = LinkFrame::id_control(
            FrameType::Conn,
            StationId::new(call("K1ABC"), call("N0BODY")),
            0x1234,
            FIRST_SEQ,
        );
        b.handle_frame(not_for_us, Duration::ZERO);
        assert_eq!(
            b.state(),
            ConnState::Closed,
            "a CONN for another station must not open a session"
        );
        assert!(b.poll_event().is_none());
    }

    #[test]
    fn acceptor_learns_the_caller_and_ids_back_to_it() {
        // The listener has no pre-configured peer; it learns K1ABC from the CONN
        // and its CONN_ACK must identify back to that learned peer.
        let mut b = acc(); // local = W2XYZ, remote unknown
        b.handle_frame(conn_from_k1abc(0x1234, FIRST_SEQ), Duration::ZERO);
        let ack = b.poll_transmit(Duration::ZERO).expect("CONN_ACK queued");
        assert_eq!(ack.frame_type, FrameType::ConnAck);
        let id = ack.id.expect("CONN_ACK is ID-bearing");
        assert_eq!(id.src.as_str(), "W2XYZ", "we identify as ourselves");
        assert_eq!(id.dst.as_str(), "K1ABC", "addressed to the learned caller");
    }

    #[test]
    fn connected_peer_reconnect_with_new_conn_id_is_accepted() {
        // sonde-ajn (B6): peer reboots / lost our DISC and sends a fresh CONN with a
        // NEW conn_id while we still think we're Connected. Accept it as a reconnect
        // (flush old session, adopt the new conn_id, CONN_ACK) — never wedge forever.
        let mut b = acc();
        b.handle_frame(conn_from_k1abc(0x1111, FIRST_SEQ), Duration::ZERO);
        assert_eq!(b.state, ConnState::Connected);
        let _ = b.poll_transmit(Duration::ZERO); // drain the first CONN_ACK
                                                 // Peer reboots: fresh CONN, different conn_id, same station.
        b.handle_frame(conn_from_k1abc(0x2222, FIRST_SEQ), Duration::ZERO);
        assert_eq!(
            b.state,
            ConnState::Connected,
            "still connected after reconnect"
        );
        assert_eq!(b.conn_id, 0x2222, "adopted the reconnect's new conn_id");
        let ack = b
            .poll_transmit(Duration::ZERO)
            .expect("CONN_ACK for the reconnect");
        assert_eq!(ack.frame_type, FrameType::ConnAck);
        assert_eq!(ack.conn_id, 0x2222, "CONN_ACK carries the new conn_id");
    }

    #[test]
    fn connected_replay_of_same_conn_id_just_resends_conn_ack() {
        // The idempotent path must still hold: a CONN replay with the SAME conn_id
        // (peer didn't hear our CONN_ACK) resends CONN_ACK without resetting.
        let mut b = acc();
        b.handle_frame(conn_from_k1abc(0x1111, FIRST_SEQ), Duration::ZERO);
        let _ = b.poll_transmit(Duration::ZERO);
        b.handle_frame(conn_from_k1abc(0x1111, FIRST_SEQ), Duration::ZERO);
        assert_eq!(b.conn_id, 0x1111, "conn_id unchanged on idempotent replay");
        let ack = b.poll_transmit(Duration::ZERO).expect("CONN_ACK resent");
        assert_eq!(ack.frame_type, FrameType::ConnAck);
    }

    #[test]
    fn conn_bearing_the_reserved_zero_conn_id_is_rejected() {
        // sonde-44n: conn_id 0 is reserved; a CONN proposing it is invalid and must
        // not establish a session (an uninitialized/garbage frame or a peer that
        // failed to pick a random nonzero id).
        let mut b = acc();
        b.handle_frame(
            conn_from_k1abc(crate::frame::CONN_ID_RESERVED, FIRST_SEQ),
            Duration::ZERO,
        );
        assert_eq!(
            b.state,
            ConnState::Closed,
            "reserved conn_id must not connect"
        );
        assert!(
            b.poll_transmit(Duration::ZERO).is_none(),
            "no CONN_ACK for conn_id 0"
        );
    }

    #[test]
    fn a_listener_clears_a_learned_peer_on_close_and_accepts_a_new_one() {
        let mut b = acc(); // learner
        b.handle_frame(conn_from_k1abc(0x1234, FIRST_SEQ), Duration::ZERO);
        while b.poll_event().is_some() {}
        // Peer disconnects → close → learned remote must clear.
        b.handle_frame(
            LinkFrame::id_control(
                FrameType::Disc,
                StationId::new(call("K1ABC"), call("W2XYZ")),
                0x1234,
                0,
            ),
            Duration::ZERO,
        );
        assert_eq!(b.state(), ConnState::Closed);
        while b.poll_event().is_some() {}
        while b.poll_transmit(Duration::ZERO).is_some() {} // flush the DISC_ACK
                                                           // A *different* station now calls; the listener must accept it.
        let other = LinkFrame::id_control(
            FrameType::Conn,
            StationId::new(call("N0CALL"), call("W2XYZ")),
            0x55AA,
            FIRST_SEQ,
        );
        b.handle_frame(other, Duration::ZERO);
        assert_eq!(b.state(), ConnState::Connected);
        let ack = b.poll_transmit(Duration::ZERO).expect("CONN_ACK queued");
        assert_eq!(
            ack.id.unwrap().dst.as_str(),
            "N0CALL",
            "bound to the new peer"
        );
    }

    #[test]
    fn lost_data_frame_is_recovered_by_selective_repeat() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"ABCDEFGHIJ".to_vec()); // 3 fragments (mtu=4)
        p.step_drop(Some(1)); // lose the middle fragment on the first over
        p.run(40);
        assert_eq!(p.b_messages(), vec![b"ABCDEFGHIJ".to_vec()]);
    }

    #[test]
    fn duplicate_data_is_not_delivered_twice() {
        let mut b = acc();
        b.handle_frame(conn_from_k1abc(0x1234, FIRST_SEQ), Duration::ZERO);
        while b.poll_event().is_some() {}
        let data = LinkFrame::data(0x1234, 1, b"hi".to_vec())
            .end_of_msg()
            .end_of_over();
        b.handle_frame(data.clone(), Duration::ZERO);
        b.handle_frame(data, Duration::ZERO); // a retransmit the receiver already has
        let n = std::iter::from_fn(|| b.poll_event())
            .filter(|e| matches!(e, HostEvent::DataReceived(_)))
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn simultaneous_connect_resolves_by_callsign_tiebreak() {
        // Both stations initiate at once (CONN/CONN collision).
        let mut a = Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1111, profile(), 8);
        let mut b = Connection::initiator(call("W2XYZ"), call("K1ABC"), 0x2222, profile(), 8);
        a.connect(Duration::ZERO);
        b.connect(Duration::ZERO);
        // Exchange a few rounds in both directions.
        for _ in 0..6 {
            while let Some(f) = a.poll_transmit(Duration::ZERO) {
                b.handle_frame(f, Duration::ZERO);
            }
            while let Some(f) = b.poll_transmit(Duration::ZERO) {
                a.handle_frame(f, Duration::ZERO);
            }
        }
        assert_eq!(a.state(), ConnState::Connected);
        assert_eq!(b.state(), ConnState::Connected);
        // Exactly one took the acceptor role ⇒ both agree on a single conn_id.
        assert_eq!(a.conn_id, b.conn_id);
    }

    #[test]
    fn sustained_silence_yields_peer_lost_not_a_hang() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"x".to_vec());
        // Every A→B frame is lost forever; A must eventually give up explicitly.
        for _ in 0..60 {
            p.step_drop(Some(0));
            if p.a.state() == ConnState::Closed {
                break;
            }
        }
        assert_eq!(p.a.state(), ConnState::Closed);
        assert!(p.a_events.contains(&HostEvent::PeerLost));
        assert!(
            p.b_messages().is_empty(),
            "never delivered ⇒ no silent corruption"
        );
    }

    #[test]
    fn backoff_jitter_is_deterministic_bounded_and_station_specific() {
        let a = init();
        let b = acc();
        // Deterministic: same inputs ⇒ same jitter.
        assert_eq!(a.backoff_jitter(), a.backoff_jitter());
        // Station-specific: the two callsigns spread, avoiding phase-lock.
        assert_ne!(a.backoff_jitter(), b.backoff_jitter());
        // Bounded to < half a turn-recovery so death detection is never starved.
        assert!(a.backoff_jitter() < a.profile.turn_recovery_timeout());
    }

    #[test]
    fn sack_disabled_suppresses_the_bitmap_even_with_a_buffered_gap() {
        // Selective-repeat receiver with a real out-of-order gap buffered. The
        // default mode today is the WholeMessage floor, so request SelectiveRepeat
        // explicitly to exercise the SACK path (dormant until an OFDM mode lands).
        let mut b = acc().with_strategy(ArqStrategy::SelectiveRepeat);
        b.handle_frame(conn_from_k1abc(0x1234, FIRST_SEQ), Duration::ZERO);
        // seq 2 arrives before seq 1 ⇒ a gap is buffered ⇒ recv.sack() != 0.
        b.handle_frame(LinkFrame::data(0x1234, 2, b"x".to_vec()), Duration::ZERO);
        assert_ne!(b.recv.sack(), 0, "precondition: a gap is buffered");
        assert_ne!(b.make_ack().sack, 0, "selective repeat advertises the gap");

        // The floor WholeMessage strategy must suppress the bitmap (no NACK).
        b.sack_enabled = false;
        assert_eq!(b.make_ack().sack, 0, "floor advertises no SACK");
        assert_eq!(b.make_data(9, b"y".to_vec()).sack, 0);
    }

    #[test]
    fn floor_whole_message_delivers_multi_fragment_in_order_with_no_sack() {
        // Stop-and-wait (window 1, no SACK) over a perfect pipe still delivers a
        // multi-fragment message byte-exact and in order.
        let mut a = Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8)
            .with_strategy(ArqStrategy::WholeMessage);
        let mut b = Connection::acceptor(call("W2XYZ"), profile(), 8)
            .with_strategy(ArqStrategy::WholeMessage);
        a.connect(Duration::ZERO);
        a.send(b"floor-msg!".to_vec()); // 10 bytes, mtu 4 ⇒ 3 fragments
        let mut now = Duration::ZERO;
        let mut got = Vec::new();
        for _ in 0..80 {
            while let Some(f) = a.poll_transmit(now) {
                b.handle_frame(f, now);
            }
            while let Some(f) = b.poll_transmit(now) {
                assert_eq!(f.sack, 0, "floor peer never advertises a SACK");
                a.handle_frame(f, now);
            }
            now += TICK;
            a.handle_timeout(now);
            b.handle_timeout(now);
            while let Some(e) = b.poll_event() {
                if let HostEvent::DataReceived(d) = e {
                    got.push(d);
                }
            }
        }
        assert_eq!(got, vec![b"floor-msg!".to_vec()]);
    }

    // ---- bidirectional floor + session reset (Codex blockers #1, #2) -----

    #[test]
    fn acceptor_can_originate_data_to_the_initiator() {
        // The acceptor, idle and listening, must be able to acquire the floor and
        // send its own host data — not wait forever on an idle initiator.
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20); // settle to idle
        p.b.send(b"from-acceptor".to_vec());
        p.run(80);
        assert_eq!(p.a_messages(), vec![b"from-acceptor".to_vec()]);
    }

    #[test]
    fn both_directions_deliver_in_one_session() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"a->b".to_vec());
        p.b.send(b"b->a".to_vec());
        p.run(120);
        assert_eq!(p.b_messages(), vec![b"a->b".to_vec()]);
        assert_eq!(p.a_messages(), vec![b"b->a".to_vec()]);
    }

    #[test]
    fn disconnect_resets_arq_and_reassembly_so_a_reconnect_is_clean() {
        let mut b = acc();
        b.handle_frame(conn_from_k1abc(0x1234, FIRST_SEQ), Duration::ZERO);
        while b.poll_event().is_some() {}
        // A partial message: one fragment, NOT end-of-msg ⇒ reassembler holds it
        // and the receive high-water advances.
        b.handle_frame(
            LinkFrame::data(0x1234, 1, b"STALE".to_vec()),
            Duration::ZERO,
        );
        assert_eq!(b.recv.ack_through(), 1, "precondition: recv advanced");

        // Peer disconnects.
        b.handle_frame(
            LinkFrame::id_control(
                FrameType::Disc,
                StationId::new(call("K1ABC"), call("W2XYZ")),
                0x1234,
                0,
            ),
            Duration::ZERO,
        );
        assert_eq!(b.state(), ConnState::Closed);
        assert_eq!(b.recv.ack_through(), 0, "recv buffer reset on close");

        // Reconnect (new session id) and send a clean, complete single-fragment
        // message. It must deliver exactly itself — the stale "STALE" fragment
        // must NOT bleed into it.
        while b.poll_event().is_some() {}
        b.handle_frame(conn_from_k1abc(0xBEEF, FIRST_SEQ), Duration::ZERO);
        while b.poll_event().is_some() {}
        b.handle_frame(
            LinkFrame::data(0xBEEF, 1, b"CLEAN".to_vec())
                .end_of_msg()
                .end_of_over(),
            Duration::ZERO,
        );
        let got: Vec<Vec<u8>> = std::iter::from_fn(|| b.poll_event())
            .filter_map(|e| match e {
                HostEvent::DataReceived(d) => Some(d),
                _ => None,
            })
            .collect();
        assert_eq!(
            got,
            vec![b"CLEAN".to_vec()],
            "no stale reassembly bleed-through"
        );
    }

    // ---- measurement-based symmetric adaptation (sonde-qnq) ---------------------

    #[test]
    fn observe_quality_falls_immediately_and_rises_smoothly() {
        let mut c = init();
        c.observe_quality(20.0, 0.0, 10);
        assert_eq!(c.snr_smoothed, 20.0, "first reading seeds the estimate");
        c.observe_quality(0.0, 0.0, 10);
        assert_eq!(c.snr_smoothed, 0.0, "a falling SNR is applied immediately");
        c.observe_quality(10.0, 0.0, 10);
        assert_eq!(
            c.snr_smoothed, 5.0,
            "a rising SNR is EWMA-smoothed (0.5·10+0.5·0)"
        );
        assert_eq!(
            c.snr_raw, 10.0,
            "raw tracks the latest reading (for downshift)"
        );
    }

    // With only ONE registered mode today (the wideband floor, C7), the link holds
    // that single real rung and never selects a fabricated mode. The rich multi-rung
    // adaptation ALGORITHM is unit-tested in mac (`adapt_rung_with`, synthetic
    // all-available ladder); these tests pin the link's honest single-mode behavior.

    #[test]
    fn the_single_registered_mode_is_held_for_any_measurement() {
        // No fabricated OFDM mode is ever recommended/selected; the link recommends
        // the one real mode (the floor = base_rung) regardless of the channel.
        let mut c = init();
        let base = mac::base_rung();
        assert_eq!(
            c.current_rung(),
            base,
            "starts at the only real mode, not OFDM"
        );
        assert_eq!(c.recommended_rung(), base, "no measurement ⇒ the real mode");
        c.observe_quality(-15.0, 0.0, 10);
        assert_eq!(
            c.recommended_rung(),
            base,
            "a collapse can't go below the only mode"
        );
        c.observe_quality(25.0, 0.0, 10);
        assert_eq!(
            c.recommended_rung(),
            base,
            "no faster mode is registered to climb to"
        );
    }

    #[test]
    fn peer_feedback_never_selects_an_unavailable_mode() {
        // Even if a (skewed) peer commands a different rung, the clamp keeps us on
        // the one real mode — nothing fabricated is ever applied.
        let mut c = init();
        let base = mac::base_rung();
        c.apply_peer_feedback(0, Duration::ZERO); // "go to the fastest OFDM rung" — unavailable
        assert_eq!(c.current_rung(), base);
        c.apply_peer_feedback(mac::NUM_RUNGS - 1, Duration::ZERO); // "deep-floor" — unavailable
        assert_eq!(c.current_rung(), base);
    }

    #[test]
    fn link_holds_the_floor_under_bad_feedback_and_still_delivers() {
        // The receiver reports a poor channel; with one registered mode the sender
        // cannot downshift further (the floor IS the base) — it holds and delivers.
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        let base = mac::base_rung();
        p.a.send(b"hello".to_vec());
        p.b.observe_quality(-15.0, 0.0, 10);
        p.run(60);
        assert_eq!(p.a.current_rung(), base, "holds the one real mode");
        assert_eq!(p.b_messages(), vec![b"hello".to_vec()], "still delivers");
    }

    #[test]
    fn a_recoverable_fade_still_delivers_at_the_single_mode() {
        // Drop a couple of overs (a transient fade), then let the channel clear: the
        // message still delivers, and the link never leaves the one real mode.
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        let base = mac::base_rung();
        p.a.send(b"x".to_vec());
        // Drive steps explicitly (so the turn-recovery timer fires through the
        // transient drops); `run` would early-exit on the first no-movement step.
        for i in 0..200 {
            if i < 2 {
                p.step_drop(Some(0)); // a transient fade: two lost overs
            } else {
                p.step();
            }
        }
        assert_eq!(p.a.current_rung(), base, "never selects a fabricated mode");
        assert_eq!(
            p.b_messages(),
            vec![b"x".to_vec()],
            "delivers after the fade"
        );
    }

    #[test]
    fn multi_fragment_delivery_is_byte_exact_at_the_single_mode() {
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"0123456789".to_vec()); // 3 fragments at the test mtu
        p.run(200);
        assert_eq!(p.b_messages(), vec![b"0123456789".to_vec()]);
    }
}
