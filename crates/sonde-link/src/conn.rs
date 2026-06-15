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
use crate::mac::{self, ArqStrategy, BASE_RUNG, DEFAULT_RUNG};
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

/// Recent per-over decode outcomes the link tracks to drive the graceful downshift
/// control loop (Fork B). The window is its OWN clean/missed outcomes, not the
/// PHY's cumulative counters (the sans-IO connection never sees a failed decode).
const QUALITY_WINDOW: usize = 4;
/// Consecutive faster-permitting feedbacks (on active DATA/ACK overs) required
/// before the floor holder probes one rung up. Upshift is non-catastrophic, so it
/// is hysteretic — a brief good patch never speeds the mode up (no thrash).
const UPSHIFT_HYSTERESIS: u8 = 3;

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
    profile: ModeProfile,
    conn_id: u16,

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
    /// Recent per-over decode outcomes (`true` = decoded the peer's over to its
    /// end, `false` = missed/turn-recovery). Drives `recommended_rung`. Cleared on
    /// every rung change so stale evidence cannot justify an immediate re-climb.
    quality: VecDeque<bool>,
    /// Consecutive faster-permitting feedbacks seen while the floor holder; gates
    /// the hysteretic upshift probe.
    upshift_streak: u8,
    /// Window the connection was constructed with — restored on session reset.
    initial_window: u32,
    silent_overs: u32,
    conn_retries: u32,
    disc_retries: u32,
}

impl Connection {
    /// Construct the connection initiator (owns the first floor after CONN_ACK).
    /// The initiator is *configured* with the peer it calls.
    pub fn initiator(
        local: Callsign,
        remote: Callsign,
        conn_id: u16,
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

    fn new(
        local: Callsign,
        remote: Option<Callsign>,
        remote_is_learned: bool,
        conn_id: u16,
        profile: ModeProfile,
        window: u32,
    ) -> Self {
        Self {
            local,
            remote,
            remote_is_learned,
            profile,
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
            current_rung: DEFAULT_RUNG,
            quality: VecDeque::new(),
            upshift_streak: 0,
            initial_window: window,
            silent_overs: 0,
            conn_retries: 0,
            disc_retries: 0,
        }
    }

    /// Select the ARQ strategy (builder; apply before `connect`). The floor
    /// `WholeMessage` strategy is the degenerate stop-and-wait: window 1 and no
    /// SACK (the canonical floor "no NACK" model, design §5). Selective repeat
    /// keeps the constructed window and SACK.
    pub fn with_strategy(mut self, strategy: ArqStrategy) -> Self {
        match strategy {
            ArqStrategy::SelectiveRepeat => self.sack_enabled = true,
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
        mac::rung(self.current_rung).mode
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
            FrameType::ConnAck => self.on_conn_ack(&frame),
            FrameType::Disc => self.on_disc(&frame),
            FrameType::DiscAck => self.on_disc_ack(&frame),
            FrameType::Id => self.on_id(&frame),
            FrameType::Data if self.session_ok(&frame) => self.on_data(frame),
            FrameType::Ack if self.session_ok(&frame) => self.on_ack(&frame),
            FrameType::Keepalive if self.session_ok(&frame) => self.on_keepalive(&frame),
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
                    // We sent data and expected a reply that did not arrive.
                    self.silent_overs += 1;
                    // A missed reply is a miss in our recent-quality window (we
                    // failed to decode the peer's over). It also closes the Idle-
                    // floor blind spot: data lost with no decodable reply yields no
                    // feedback, so the floor holder must downshift on its own.
                    self.record_outcome(false);
                    if self.current_rung != BASE_RUNG
                        && self.silent_overs >= DOWNSHIFT_TO_BASE_OVERS
                    {
                        // We cannot get through at this mode — fall to the robust
                        // BASE mode (design P1) and keep trying there with a fresh
                        // death budget. Both ends do this symmetrically (and the
                        // peer also follows our mode-id), so they converge to BASE
                        // instead of dying. Re-take the floor to retransmit.
                        self.apply_rung(BASE_RUNG);
                        self.silent_overs = 0;
                        self.floor = Floor::Sending;
                        self.deadline = None;
                    } else if self.silent_overs > DEAD_OVERS_TOLERATED {
                        self.close(Some(HostEvent::PeerLost));
                    } else {
                        // Graceful self-downshift one step on a missed reply (§2.4),
                        // independent of any feedback; sustained misses still cascade
                        // to the P1 BASE-fallback above. Then re-take the floor to
                        // retransmit the unacked window.
                        if self.current_rung != BASE_RUNG {
                            self.apply_rung(self.current_rung + 1);
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
    /// place (preserving the seq stream + in-flight), and flip SACK on/off for
    /// the rung's strategy. The airtime-aware timer *profile* is intentionally
    /// NOT swapped here — real per-mode profiles come from the PHY (design §6,
    /// not yet available); the constructed profile is kept until then.
    fn apply_rung(&mut self, id: u8) {
        if id == self.current_rung {
            return;
        }
        self.current_rung = id;
        let r = mac::rung(id);
        self.send.reconfigure(r.window.window);
        self.recv.reconfigure(r.window.window);
        self.sack_enabled = matches!(r.strategy, ArqStrategy::SelectiveRepeat);
        // Any rung change invalidates the recent-quality evidence and the upshift
        // streak: stale clean samples from a faster rung must not justify an
        // immediate re-climb (anti-sawtooth), and a fall to BASE then requires a
        // fresh full-clean dwell before probing up (the BASE cooldown).
        self.note_rung_change();
    }

    /// Clear the recent-quality window + upshift streak (on every rung change).
    fn note_rung_change(&mut self) {
        self.quality.clear();
        self.upshift_streak = 0;
    }

    /// Follow a peer frame's advertised mode (the mode-id confirmation path): if
    /// the peer is transmitting at a different rung, adopt it so our replies go
    /// out at the same mode and the two ends converge. Used only by the *follower*
    /// (the listener), never the floor-holding decider (see the role-gate in the
    /// inbound handlers).
    fn follow_mode(&mut self, peer_mode: u8) {
        if peer_mode != self.current_rung {
            self.apply_rung(peer_mode);
        }
    }

    /// Record a decoded peer over-end (`clean`) or a missed reply (`!clean`) into
    /// the bounded recent-quality window.
    fn record_outcome(&mut self, clean: bool) {
        if self.quality.len() == QUALITY_WINDOW {
            self.quality.pop_front();
        }
        self.quality.push_back(clean);
    }

    /// The ladder rung this station recommends the PEER use, from its recent-
    /// quality window (advertised as `rx_rung` on outgoing frames). A partial or
    /// empty window ⇒ no opinion (`current_rung`, the bootstrapping/cooldown case).
    /// A full clean window ⇒ permit one faster (probe). Any recent miss ⇒ a more-
    /// robust rung scaled by miss count (toward `BASE_RUNG`).
    fn recommended_rung(&self) -> u8 {
        if self.quality.len() < QUALITY_WINDOW {
            return self.current_rung;
        }
        let misses = self.quality.iter().filter(|&&clean| !clean).count() as u8;
        if misses == 0 {
            self.current_rung.saturating_sub(1)
        } else {
            self.current_rung.saturating_add(misses).min(BASE_RUNG)
        }
    }

    /// Act on the peer's `rx_rung` feedback while we are the floor-holding decider
    /// (architecture C). Downshift is receiver-AUTHORITATIVE and immediate; upshift
    /// is sender-gated (hysteretic, one rung, only on `counts_for_upshift` active
    /// DATA/ACK overs — keepalive/ID give liveness but cannot fake upshift
    /// confidence).
    fn apply_peer_feedback(&mut self, peer_rx_rung: u8, counts_for_upshift: bool) {
        let target = peer_rx_rung.min(BASE_RUNG);
        if target > self.current_rung {
            // Receiver says it cannot decode us well — obey immediately.
            self.apply_rung(target); // clears the streak + window
        } else if counts_for_upshift && target < self.current_rung {
            // Peer permits faster — probe one rung up only after sustained permits.
            self.upshift_streak = self.upshift_streak.saturating_add(1);
            if self.upshift_streak >= UPSHIFT_HYSTERESIS {
                self.apply_rung(self.current_rung - 1); // clears the streak + window
            }
        } else if counts_for_upshift {
            // Peer recommends our current rung (not faster) — not sustained-faster.
            self.upshift_streak = 0;
        }
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
        // Restore the default mode + window so a same-object reconnect starts
        // fresh at the top of the ladder, not stuck at a degraded BASE.
        self.current_rung = DEFAULT_RUNG;
        self.send.reconfigure(self.initial_window);
        self.recv.reconfigure(self.initial_window);
        self.sack_enabled = true;
        self.quality.clear();
        self.upshift_streak = 0;
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

    fn accept_connection_as_acceptor(&mut self, conn_id: u16, seq: u32, now: Duration) {
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
                // Idempotent CONN replay for THIS session, from our peer: peer
                // didn't hear our CONN_ACK — resend it. A different conn_id is a
                // stale/half-open peer (R6 follow-up): drop, do not regress.
                if frame.conn_id == self.conn_id && self.remote.as_ref() == Some(&peer) {
                    let ack = self.make_control(FrameType::ConnAck, frame.seq);
                    self.outbox.push_back(ack);
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

    fn on_conn_ack(&mut self, frame: &LinkFrame) {
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
    fn on_id(&mut self, frame: &LinkFrame) {
        if !self.session_ok(frame) || !self.id_addressed_to_us(frame) {
            return;
        }
        self.silent_overs = 0;
        // ID gives liveness; a downshift command on it is still honored, but it
        // never advances the upshift streak (not an active DATA/ACK over).
        self.adapt_on_inbound(frame, false);
        if frame.is_end_of_over() {
            self.on_over_end();
        }
    }

    /// Apply link-adaptation on an inbound frame, role-gated by floor position
    /// (architecture C). If we are the floor-holding **decider** (we sent an over
    /// and are awaiting its reply), act on the peer's `rx_rung` feedback. Otherwise
    /// we are the **follower** (the peer holds the floor): adopt its announced
    /// `MODE` so our replies match. `counts_for_upshift` is set only for active
    /// DATA/ACK overs.
    fn adapt_on_inbound(&mut self, frame: &LinkFrame, counts_for_upshift: bool) {
        if self.awaiting_reply {
            self.apply_peer_feedback(frame.rx_rung(), counts_for_upshift);
        } else {
            self.follow_mode(frame.mode);
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

    fn on_data(&mut self, frame: LinkFrame) {
        self.silent_overs = 0;
        self.adapt_on_inbound(&frame, true);
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

    fn on_ack(&mut self, frame: &LinkFrame) {
        self.silent_overs = 0;
        self.adapt_on_inbound(frame, true);
        self.send.on_ack(frame.ack_through, frame.sack);
        if frame.is_end_of_over() {
            self.on_over_end();
        }
    }

    fn on_keepalive(&mut self, frame: &LinkFrame) {
        self.silent_overs = 0;
        // A keepalive gives liveness + an honored downshift command, but never
        // advances the upshift streak (not an active DATA/ACK over).
        self.adapt_on_inbound(frame, false);
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
        // We decoded the peer's over to its end — a clean outcome for the recent-
        // quality window (drives the rung we recommend the peer).
        self.record_outcome(true);
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

    fn call(s: &str) -> Callsign {
        Callsign::new(s).unwrap()
    }

    /// A CONN from `K1ABC` to `W2XYZ` (the standard peer pair in these tests).
    fn conn_from_k1abc(conn_id: u16, seq: u32) -> LinkFrame {
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
        // Selective-repeat receiver with a real out-of-order gap buffered.
        let mut b = acc();
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

    // ---- mid-session downshift control loop (sonde-ruu, Fork B, architecture C) --

    #[test]
    fn recommended_rung_has_no_opinion_until_the_window_is_full() {
        let mut c = init(); // current_rung = DEFAULT_RUNG
        c.record_outcome(true);
        c.record_outcome(true);
        assert_eq!(
            c.recommended_rung(),
            DEFAULT_RUNG,
            "a partial window must not move the rung (bootstrapping)"
        );
    }

    #[test]
    fn recommended_rung_permits_faster_only_on_a_full_clean_window() {
        let mut c = init();
        for _ in 0..QUALITY_WINDOW {
            c.record_outcome(true);
        }
        assert_eq!(
            c.recommended_rung(),
            DEFAULT_RUNG - 1,
            "a full clean window permits one faster"
        );
    }

    #[test]
    fn recommended_rung_recommends_more_robust_scaled_by_misses() {
        let mut c = init();
        for _ in 0..QUALITY_WINDOW {
            c.record_outcome(false); // all misses
        }
        assert_eq!(
            c.recommended_rung(),
            BASE_RUNG,
            "a full window of misses recommends the robust floor"
        );
    }

    #[test]
    fn downshift_is_immediate_and_receiver_authoritative() {
        let mut c = init(); // rung 1
        c.apply_peer_feedback(BASE_RUNG, true);
        assert_eq!(
            c.current_rung(),
            BASE_RUNG,
            "a more-robust command is obeyed at once"
        );
    }

    #[test]
    fn upshift_requires_sustained_hysteresis_then_steps_one_rung() {
        let mut c = init();
        c.apply_rung(2); // sit mid-ladder
        for _ in 0..UPSHIFT_HYSTERESIS - 1 {
            c.apply_peer_feedback(0, true); // peer permits faster
            assert_eq!(
                c.current_rung(),
                2,
                "no upshift before the hysteresis count"
            );
        }
        c.apply_peer_feedback(0, true);
        assert_eq!(c.current_rung(), 1, "exactly one rung up after hysteresis");
    }

    #[test]
    fn keepalive_feedback_never_advances_an_upshift() {
        let mut c = init();
        c.apply_rung(2);
        for _ in 0..UPSHIFT_HYSTERESIS + 2 {
            c.apply_peer_feedback(0, false); // not an active DATA/ACK over
        }
        assert_eq!(
            c.current_rung(),
            2,
            "liveness feedback must not fake upshift confidence"
        );
    }

    #[test]
    fn a_rung_change_clears_stale_clean_evidence_no_sawtooth() {
        let mut c = init();
        for _ in 0..QUALITY_WINDOW {
            c.record_outcome(true); // would otherwise permit faster
        }
        c.apply_rung(3); // any rung change clears the window + streak
        assert_eq!(
            c.recommended_rung(),
            3,
            "post-change the window is empty ⇒ no opinion (anti-sawtooth / BASE cooldown)"
        );
    }

    #[test]
    fn gate_a_sustained_fade_downshifts_then_clears_and_delivers() {
        // A fade (lost overs) must downshift the rung WITHOUT dropping the link,
        // and once it clears the message still delivers byte-exact.
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        let start = p.a.current_rung();
        p.a.send(b"x".to_vec()); // single-frame message ⇒ a clean whole-over drop
        let mut steps = 0;
        while p.a.current_rung() == start && steps < 30 {
            p.step_drop(Some(0)); // drop A's whole over
            steps += 1;
        }
        assert!(
            p.a.current_rung() > start,
            "a sustained fade must downshift the rung"
        );
        assert_ne!(
            p.a.state(),
            ConnState::Closed,
            "graceful downshift, not a link drop"
        );
        p.run(160); // channel clears
        assert_eq!(
            p.b_messages(),
            vec![b"x".to_vec()],
            "delivers after the fade"
        );
    }

    #[test]
    fn gate_d_delivery_is_byte_exact_across_a_rung_change() {
        // A mid-session rung change (window + strategy reconfigured in place) must
        // preserve the continuous ARQ seq stream — the whole message still arrives.
        let mut p = Pair::new();
        p.a.connect(Duration::ZERO);
        p.run(20);
        p.a.send(b"0123456789".to_vec()); // 3 fragments at mtu 4
        p.a.apply_rung(BASE_RUNG); // force a downshift to window-1 WholeMessage
        p.run(200);
        assert_eq!(p.b_messages(), vec![b"0123456789".to_vec()]);
    }
}
