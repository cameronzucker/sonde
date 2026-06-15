//! `Driver<P, C>` — the real-time bridge from the sans-IO [`Connection`] to a
//! live, threaded `PhyTransport` worker (Codex blockers #3, #5).
//!
//! [`Link`](crate::Link) drives the connection with a caller-supplied logical
//! `now`; perfect for the deterministic gates but wrong for a real radio
//! runtime, where `send_frame` only *queues* to a worker that keys PTT and airs
//! the samples later. `Driver` closes that gap:
//!
//! - **Injected clock** ([`Clock`]) — testable real time. A real loop sleeps
//!   until [`Driver::next_wakeup`], then calls [`Driver::poll`].
//! - **Freeze on *actual* keying** — the link's turn-recovery timer means "how
//!   long I wait for the peer *after I stop keying*". The driver reads the PHY's
//!   [`tx_in_flight`](PhyTransport::tx_in_flight) signal and runs the link on
//!   `logical = real − time-actually-spent-keying`, so the link's clock is
//!   frozen for exactly as long as the worker holds PTT — no airtime estimate.
//!   (An earlier estimate-based version was found unsound by review precisely
//!   because *enqueue* time ≠ *keyed* time; this uses the real signal instead.)
//! - **Half-duplex TX gate** — a new over is not flushed while the worker is
//!   still keying (`tx_in_flight() > 0`).
//! - **Error propagation** — synchronous `send_frame` enqueue errors are
//!   captured ([`Driver::take_errors`]). Failures during the asynchronous
//!   encode/transmit are reflected by the PHY into `channel_quality()` (FER).
//!
//! Per RADIO-1 this keys nothing by itself: it drives whatever `PhyTransport` it
//! is given (an in-memory double in tests, `SondePhy` in production). The PHY's
//! `Radio` decides whether real RF is emitted.

use std::collections::VecDeque;
use std::time::Duration;

use sonde_phy::error::PhyError;
use sonde_phy::phy_api::PhyTransport;

use crate::clock::Clock;
use crate::conn::{ConnState, Connection, HostEvent};
use crate::frame::LinkFrame;
use crate::host::HostCommand;

/// How often to re-poll while the worker is keying, to detect TX completion.
const TX_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Part-97 §97.119 periodic-ID cadence, measured in **real** wall-clock time. A
/// station must identify at least every 10 minutes during a communication; 9 min
/// leaves margin. This lives in the Driver, not the sans-IO `Connection`, because
/// the connection's logical clock freezes during keying — a logical-time cadence
/// would drift past 10 real minutes by exactly the accumulated keyed airtime (see
/// `docs/superpowers/specs/2026-06-15-frame-conn-id-addressing-design.md` §2.4).
const ID_INTERVAL: Duration = Duration::from_secs(9 * 60);

/// Real-time driver wrapping a [`Connection`] over a `P: PhyTransport`, clocked
/// by a `C: Clock`.
pub struct Driver<P: PhyTransport, C: Clock> {
    conn: Connection,
    phy: P,
    clock: C,
    /// `true` while an over we flushed is still being keyed by the worker.
    tx_active: bool,
    /// Real-time instant at which the current keyed over began.
    tx_start: Duration,
    /// Total real time the worker has spent keying our overs — subtracted from
    /// real time to give the link's frozen-during-TX logical clock.
    tx_airtime: Duration,
    /// Real (wall-clock) time the last ID-bearing frame was transmitted, for the
    /// Part-97 ≤10-minute cadence. `None` until the first transmission.
    last_id_real: Option<Duration>,
    errors: VecDeque<PhyError>,
}

impl<P: PhyTransport, C: Clock> Driver<P, C> {
    /// Wrap a connection + transport + clock. Frames are transmitted at the
    /// connection's *current* mode ([`Connection::current_hint`]), so a
    /// mid-session mode change takes effect on the wire automatically.
    pub fn new(phy: P, conn: Connection, clock: C) -> Self {
        Self {
            conn,
            phy,
            clock,
            tx_active: false,
            tx_start: Duration::ZERO,
            tx_airtime: Duration::ZERO,
            last_id_real: None,
            errors: VecDeque::new(),
        }
    }

    /// The link's logical time: real time minus time spent keying (including the
    /// in-progress over, so the clock is frozen for the whole keyed interval).
    fn logical(&self) -> Duration {
        let pending = if self.tx_active {
            self.clock.now().saturating_sub(self.tx_start)
        } else {
            Duration::ZERO
        };
        self.clock.now().saturating_sub(self.tx_airtime + pending)
    }

    /// Begin connecting (initiator).
    pub fn connect(&mut self) {
        let now = self.logical();
        self.conn.connect(now);
    }

    /// Enqueue a host message for reliable, in-order delivery.
    pub fn send(&mut self, data: Vec<u8>) {
        self.conn.send(data);
    }

    /// Begin a clean teardown.
    pub fn disconnect(&mut self) {
        let now = self.logical();
        self.conn.disconnect(now);
    }

    /// Apply a host/TNC command.
    pub fn command(&mut self, cmd: HostCommand) {
        match cmd {
            HostCommand::Connect => self.connect(),
            HostCommand::Send(data) => self.send(data),
            HostCommand::Disconnect => self.disconnect(),
        }
    }

    /// Current connection state.
    pub fn state(&self) -> ConnState {
        self.conn.state()
    }

    /// Current link adaptation rung (introspection; rises = more robust). Reflects
    /// mode adaptation driven by the PHY's `channel_quality()` measurements.
    pub fn current_rung(&self) -> u8 {
        self.conn.current_rung()
    }

    /// Drain any synchronous `send_frame` errors observed since the last call.
    pub fn take_errors(&mut self) -> Vec<PhyError> {
        self.errors.drain(..).collect()
    }

    /// Whether the Part-97 periodic-ID cadence is due in real time: never sent, or
    /// `ID_INTERVAL` of real wall-clock has elapsed since the last ID-bearing TX.
    fn id_due(&self, real: Duration) -> bool {
        match self.last_id_real {
            None => true,
            Some(t) => real.saturating_sub(t) >= ID_INTERVAL,
        }
    }

    /// Real-time instant at which the driver next wants to be polled. While the
    /// worker is keying, poll again soon to detect completion; otherwise map the
    /// link's logical deadline back to real time.
    pub fn next_wakeup(&self) -> Option<Duration> {
        if self.tx_active {
            return Some(self.clock.now() + TX_POLL_INTERVAL);
        }
        self.conn.next_timeout().map(|d| d + self.tx_airtime)
    }

    /// One driver iteration at the current clock time: settle TX completion,
    /// ingest inbound frames, fire timers, flush the next over (if the worker is
    /// free), and return host events.
    pub fn poll(&mut self) -> Vec<HostEvent> {
        let real = self.clock.now();

        // Settle TX completion: when the worker has released PTT, commit the
        // real time it spent keying into the frozen-clock accumulator.
        if self.tx_active && self.phy.tx_in_flight() == 0 {
            self.tx_airtime += real.saturating_sub(self.tx_start);
            self.tx_active = false;
        }
        let logical = self.logical();

        // Ingest: decode inbound frames into a batch first (a failed decode is
        // dropped). Batching lets us feed the channel measurement BEFORE the SM
        // reacts to the over, so `apply_peer_feedback`'s worse-direction-wins sees
        // the fresh reading.
        let mut inbound = Vec::new();
        while let Some(rx) = self.phy.poll_rx() {
            if let Ok(frame) = LinkFrame::decode(rx.payload()) {
                inbound.push(frame);
            }
        }
        // Feed the per-over channel measurement into mode adaptation — only when we
        // actually decoded something (so an idle/stale snapshot is never re-applied,
        // which would corrupt the SNR EWMA / FER cadence). A total-loss degradation
        // that yields no decode is handled by the sender's self-downshift + P1, not
        // this measurement path.
        if !inbound.is_empty() {
            let q = self.phy.channel_quality();
            self.conn.observe_quality(
                q.aggregate_snr_db(),
                q.frame_error_rate(),
                q.recent_frames_total(),
            );
        }
        for frame in inbound {
            self.conn.handle_frame(frame, logical);
        }
        // Timers at logical time.
        self.conn.handle_timeout(logical);
        // Flush the next over only when the worker is free (half-duplex).
        if !self.tx_active && self.phy.tx_in_flight() == 0 {
            let mut frames = 0u32;
            let mut sent_id_bearing = false;
            let mut first = true;
            let hint = self.conn.current_hint();
            while let Some(frame) = self.conn.poll_transmit(logical) {
                // On the first frame of this over, fold in a Part-97 periodic ID
                // (real-time cadence) unless the over already opens with an
                // ID-bearing frame (CONN/CONN_ACK/DISC/etc. — that satisfies the
                // cadence; do not double-ID, per the Codex review). Only valid once
                // `Connected`, where the peer (and so the ID block) is known.
                if first {
                    first = false;
                    if !frame.frame_type.is_id_bearing()
                        && self.conn.state() == ConnState::Connected
                        && self.id_due(real)
                    {
                        if let Ok(bytes) = self.conn.make_id_frame().encode() {
                            match self.phy.send_frame(&bytes, hint) {
                                Ok(_) => {
                                    frames += 1;
                                    sent_id_bearing = true;
                                }
                                Err(e) => self.errors.push_back(e),
                            }
                        }
                    }
                }
                let id_bearing = frame.frame_type.is_id_bearing();
                if let Ok(bytes) = frame.encode() {
                    match self.phy.send_frame(&bytes, hint) {
                        Ok(_) => {
                            frames += 1;
                            // Only a SUCCESSFUL send advances the ID cadence — a
                            // failed ID-bearing send must not let a later TX skip a
                            // required station ID (sonde-i3h).
                            sent_id_bearing |= id_bearing;
                        }
                        Err(e) => self.errors.push_back(e),
                    }
                }
            }
            // Any ID-bearing transmission (the periodic ID OR a start/end
            // CONN/CONN_ACK/DISC in this over) resets the real-time cadence.
            if sent_id_bearing {
                self.last_id_real = Some(real);
            }
            if frames > 0 {
                self.tx_active = true;
                self.tx_start = real;
            }
        }
        // Host events.
        let mut events = Vec::new();
        while let Some(e) = self.conn.poll_event() {
            events.push(e);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Callsign, FrameType, LinkFrame};
    use crate::profile::ModeProfile;
    use sonde_phy::modes::{ModeHint, ModeTable};
    use sonde_phy::phy_api::{ChannelQualityReport, RxFrame, TxToken};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn call(s: &str) -> Callsign {
        Callsign::new(s).unwrap()
    }

    fn profile() -> ModeProfile {
        ModeProfile::new(Duration::from_millis(10), 4)
    }

    /// A manually-advanced clock shared between drivers and PHY doubles.
    #[derive(Clone)]
    struct ManualClock(Rc<Cell<Duration>>);
    impl ManualClock {
        fn new() -> Self {
            Self(Rc::new(Cell::new(Duration::ZERO)))
        }
        fn advance(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }
    impl Clock for ManualClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
    }

    /// A lossless in-memory half-duplex wire (immediate delivery; tx completes
    /// synchronously, so `tx_in_flight` defaults to 0).
    type Queues = Rc<RefCell<[VecDeque<Vec<u8>>; 2]>>;
    struct WireEnd {
        q: Queues,
        side: usize,
    }
    impl PhyTransport for WireEnd {
        fn send_frame(&mut self, payload: &[u8], _hint: ModeHint) -> Result<TxToken, PhyError> {
            self.q.borrow_mut()[1 - self.side].push_back(payload.to_vec());
            Ok(TxToken(0))
        }
        fn poll_rx(&mut self) -> Option<RxFrame> {
            let bytes = self.q.borrow_mut()[self.side].pop_front()?;
            let mode = ModeTable::default().resolve(ModeHint::MainAuto, None);
            Some(RxFrame::new(bytes, mode, None, 10.0, true))
        }
        fn channel_quality(&self) -> ChannelQualityReport {
            ChannelQualityReport::empty()
        }
    }
    fn wire() -> (WireEnd, WireEnd) {
        let q: Queues = Rc::new(RefCell::new([VecDeque::new(), VecDeque::new()]));
        (
            WireEnd {
                q: Rc::clone(&q),
                side: 0,
            },
            WireEnd { q, side: 1 },
        )
    }

    /// A wire end that also reports a fixed `channel_quality()` — to prove the
    /// Driver feeds the PHY measurement into mode adaptation end-to-end.
    struct MeasuringWire {
        q: Queues,
        side: usize,
        snr_db: f32,
        frames_total: u32,
        frames_failed: u32,
    }
    impl PhyTransport for MeasuringWire {
        fn send_frame(&mut self, payload: &[u8], _hint: ModeHint) -> Result<TxToken, PhyError> {
            self.q.borrow_mut()[1 - self.side].push_back(payload.to_vec());
            Ok(TxToken(0))
        }
        fn poll_rx(&mut self) -> Option<RxFrame> {
            let bytes = self.q.borrow_mut()[self.side].pop_front()?;
            let mode = ModeTable::default().resolve(ModeHint::MainAuto, None);
            Some(RxFrame::new(bytes, mode, None, 10.0, true))
        }
        fn channel_quality(&self) -> ChannelQualityReport {
            ChannelQualityReport::from_parts(
                Vec::new(),
                self.snr_db,
                self.frames_total,
                self.frames_failed,
                None,
            )
        }
    }

    /// A transport whose `send_frame` always fails (to exercise error capture).
    struct ErrPhy;
    impl PhyTransport for ErrPhy {
        fn send_frame(&mut self, _payload: &[u8], _hint: ModeHint) -> Result<TxToken, PhyError> {
            Err(PhyError::AudioIo("boom".into()))
        }
        fn poll_rx(&mut self) -> Option<RxFrame> {
            None
        }
        fn channel_quality(&self) -> ChannelQualityReport {
            ChannelQualityReport::empty()
        }
    }

    /// A transport that reports `tx_in_flight() == 1` for `key_duration` after
    /// each `send_frame` — models a worker that holds PTT for a real interval.
    struct KeyedPhy {
        clk: ManualClock,
        key_duration: Duration,
        busy_until: Cell<Option<Duration>>,
    }
    impl PhyTransport for KeyedPhy {
        fn send_frame(&mut self, _payload: &[u8], _hint: ModeHint) -> Result<TxToken, PhyError> {
            self.busy_until
                .set(Some(self.clk.now() + self.key_duration));
            Ok(TxToken(0))
        }
        fn poll_rx(&mut self) -> Option<RxFrame> {
            None
        }
        fn channel_quality(&self) -> ChannelQualityReport {
            ChannelQualityReport::empty()
        }
        fn tx_in_flight(&self) -> usize {
            match self.busy_until.get() {
                Some(t) if self.clk.now() < t => 1,
                _ => 0,
            }
        }
    }

    /// Like [`WireEnd`] but also records every frame this end transmits, so a test
    /// can inspect the on-air stream (e.g. for periodic-ID frames).
    struct RecordingWire {
        q: Queues,
        side: usize,
        sent: Rc<RefCell<Vec<Vec<u8>>>>,
    }
    impl PhyTransport for RecordingWire {
        fn send_frame(&mut self, payload: &[u8], _hint: ModeHint) -> Result<TxToken, PhyError> {
            self.sent.borrow_mut().push(payload.to_vec());
            self.q.borrow_mut()[1 - self.side].push_back(payload.to_vec());
            Ok(TxToken(0))
        }
        fn poll_rx(&mut self) -> Option<RxFrame> {
            let bytes = self.q.borrow_mut()[self.side].pop_front()?;
            let mode = ModeTable::default().resolve(ModeHint::MainAuto, None);
            Some(RxFrame::new(bytes, mode, None, 10.0, true))
        }
        fn channel_quality(&self) -> ChannelQualityReport {
            ChannelQualityReport::empty()
        }
    }

    /// A recording wire pair; returns (endA, endB, A's sent log, B's sent log).
    #[allow(clippy::type_complexity)]
    fn recording_wire() -> (
        RecordingWire,
        RecordingWire,
        Rc<RefCell<Vec<Vec<u8>>>>,
        Rc<RefCell<Vec<Vec<u8>>>>,
    ) {
        let q: Queues = Rc::new(RefCell::new([VecDeque::new(), VecDeque::new()]));
        let a_sent = Rc::new(RefCell::new(Vec::new()));
        let b_sent = Rc::new(RefCell::new(Vec::new()));
        (
            RecordingWire {
                q: Rc::clone(&q),
                side: 0,
                sent: Rc::clone(&a_sent),
            },
            RecordingWire {
                q,
                side: 1,
                sent: Rc::clone(&b_sent),
            },
            a_sent,
            b_sent,
        )
    }

    /// Count `ID`-type frames in a recorded transmit log.
    fn id_frame_count(sent: &Rc<RefCell<Vec<Vec<u8>>>>) -> usize {
        sent.borrow()
            .iter()
            .filter_map(|b| LinkFrame::decode(b).ok())
            .filter(|f| f.frame_type == FrameType::Id)
            .count()
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

    #[test]
    fn driver_delivers_a_message_over_a_real_phytransport_in_real_time() {
        let clk = ManualClock::new();
        let (ea, eb) = wire();
        let mut a = Driver::new(
            ea,
            Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8),
            clk.clone(),
        );
        let mut b = Driver::new(
            eb,
            Connection::acceptor(call("W2XYZ"), profile(), 8),
            clk.clone(),
        );
        a.connect();
        let mut b_ev = Vec::new();
        a.send(b"driven".to_vec());
        for _ in 0..200 {
            a.poll();
            b_ev.extend(b.poll());
            clk.advance(Duration::from_millis(5));
        }
        assert_eq!(messages(&b_ev), vec![b"driven".to_vec()]);
    }

    #[test]
    fn a_failed_id_bearing_send_does_not_advance_the_id_cadence() {
        // sonde-i3h: if an ID-bearing frame's send_frame fails, the Part-97
        // periodic-ID cadence must NOT advance — else a later SUCCESSFUL TX could
        // skip a required station ID (the driver wrongly believing one aired).
        let clk = ManualClock::new();
        let mut a = Driver::new(
            ErrPhy,
            Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8),
            clk.clone(),
        );
        a.connect();
        a.poll(); // flushes the CONN (ID-bearing); ErrPhy fails every send
        assert!(
            !a.take_errors().is_empty(),
            "the failed CONN send is surfaced"
        );
        assert!(
            a.last_id_real.is_none(),
            "a failed ID-bearing send must not advance the ID cadence"
        );
    }

    #[test]
    fn driver_adapts_the_rung_from_the_phys_channel_quality() {
        // End-to-end glue: B's PHY reports a poor channel; B folds it into adaptation
        // and feeds back a robust rung, and the floor-holding sender A obeys (its
        // rung climbs toward BASE). Proves the Driver → channel_quality →
        // observe_quality → adapt_rung → apply path is wired, not just the in-memory
        // logic. (RADIO-1: in-memory doubles, nothing keyed.)
        let clk = ManualClock::new();
        let q: Queues = Rc::new(RefCell::new([VecDeque::new(), VecDeque::new()]));
        let ea = WireEnd {
            q: Rc::clone(&q),
            side: 0,
        };
        let eb = MeasuringWire {
            q,
            side: 1,
            snr_db: -15.0, // a collapsed channel on A's overs
            frames_total: 10,
            frames_failed: 0,
        };
        let mut a = Driver::new(
            ea,
            Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8),
            clk.clone(),
        );
        let mut b = Driver::new(
            eb,
            Connection::acceptor(call("W2XYZ"), profile(), 8),
            clk.clone(),
        );
        a.connect();
        let start = a.current_rung();
        a.send(b"adapt".to_vec());
        let mut b_ev = Vec::new();
        for _ in 0..80 {
            a.poll();
            b_ev.extend(b.poll());
            clk.advance(Duration::from_millis(20));
        }
        assert!(
            a.current_rung() > start,
            "A downshifts from B's poor-channel feedback (end-to-end through the Driver)"
        );
        assert!(
            messages(&b_ev).contains(&b"adapt".to_vec()),
            "and the message still delivers"
        );
    }

    #[test]
    fn send_frame_errors_are_captured_not_dropped() {
        let clk = ManualClock::new();
        let mut a = Driver::new(
            ErrPhy,
            Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8),
            clk.clone(),
        );
        a.connect();
        a.poll();
        assert!(
            !a.take_errors().is_empty(),
            "the failed send must be surfaced"
        );
        assert!(a.take_errors().is_empty(), "errors drain on take");
    }

    #[test]
    fn logical_clock_freezes_for_the_real_keying_interval() {
        // The PHY keys for far longer than the connect-retry budget. Because the
        // link's clock is frozen on ACTUAL keying (tx_in_flight), the connect
        // timer must NOT fire during keying — the attempt stays Connecting
        // rather than exhausting retries. (Without the freeze, real time would
        // blow through MAX_CONN_RETRIES * turn_recovery and close the link.)
        let clk = ManualClock::new();
        let phy = KeyedPhy {
            clk: clk.clone(),
            key_duration: Duration::from_secs(10),
            busy_until: Cell::new(None),
        };
        let mut a = Driver::new(
            phy,
            Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8),
            clk.clone(),
        );
        a.connect(); // turn-recovery is 20 ms logical
        for _ in 0..50 {
            a.poll();
            clk.advance(Duration::from_millis(100)); // 5 s real, all during keying
        }
        assert_eq!(
            a.state(),
            ConnState::Connecting,
            "frozen logical clock ⇒ no spurious retry/timeout while keying"
        );
    }

    #[test]
    fn periodic_id_is_emitted_after_the_real_interval_and_not_before() {
        // Part-97 ≤10-min cadence in REAL time. A connects (the CONN is the start
        // ID — no separate ID frame yet), then after ID_INTERVAL of real time A
        // must fold a periodic ID into its next over.
        let clk = ManualClock::new();
        let (ea, eb, a_sent, _b_sent) = recording_wire();
        let mut a = Driver::new(
            ea,
            Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8),
            clk.clone(),
        );
        let mut b = Driver::new(
            eb,
            Connection::acceptor(call("W2XYZ"), profile(), 8),
            clk.clone(),
        );
        a.connect();
        // Settle the handshake + an initial exchange, well under ID_INTERVAL.
        a.send(b"hi".to_vec());
        for _ in 0..40 {
            a.poll();
            b.poll();
            clk.advance(Duration::from_millis(50)); // 2 s total real ≪ 9 min
        }
        assert_eq!(a.state(), ConnState::Connected);
        assert_eq!(
            id_frame_count(&a_sent),
            0,
            "no periodic ID before the interval (CONN already identified the start)"
        );

        // Jump real time past the cadence, then give A something to transmit.
        clk.advance(ID_INTERVAL + Duration::from_secs(1));
        a.send(b"after".to_vec());
        let mut b_ev = Vec::new();
        for _ in 0..60 {
            a.poll();
            b_ev.extend(b.poll());
            clk.advance(Duration::from_millis(50));
        }
        assert!(
            id_frame_count(&a_sent) >= 1,
            "A folds a periodic ID into its over once ID_INTERVAL of real time has passed"
        );
        // The whole message still delivers — the ID frame rides along in the over,
        // it does not disrupt delivery.
        assert!(messages(&b_ev).contains(&b"after".to_vec()));
    }

    #[test]
    fn next_wakeup_polls_soon_while_keying() {
        let clk = ManualClock::new();
        let phy = KeyedPhy {
            clk: clk.clone(),
            key_duration: Duration::from_secs(5),
            busy_until: Cell::new(None),
        };
        let mut a = Driver::new(
            phy,
            Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8),
            clk.clone(),
        );
        a.connect();
        a.poll(); // flush CONN → worker is now keying
        assert_eq!(
            a.next_wakeup(),
            Some(clk.now() + TX_POLL_INTERVAL),
            "while keying, poll again soon to detect completion"
        );
    }
}
