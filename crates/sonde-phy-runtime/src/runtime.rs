//! [`SondePhy`]: the production `PhyTransport` runtime.
//!
//! The public `SondePhy` is a thin handle. A worker thread owns the
//! [`Waveform`] + [`Radio`] and runs a half-duplex pump:
//!
//! 1. Drain any queued TX frames — for each, encode via the waveform and hand
//!    the samples to `Radio::transmit` (which keys PTT).
//! 2. Otherwise capture one RX window via `Radio::receive` and try
//!    `Waveform::decode_scan`; on success, push an `RxFrame` to the RX queue
//!    and bump the channel-quality counters.
//!
//! TX is prioritised over RX so a queued frame never waits behind a long
//! capture — half-duplex means we cannot do both at once anyway.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use sonde_phy::error::PhyError;
use sonde_phy::modes::{ModeFamily, ModeHint, ModeTable};
use sonde_phy::phy_api::{ChannelQualityReport, PhyTransport, RxFrame, TxToken};

use crate::radio::Radio;
use crate::waveform::{DecodeScan, Waveform};

/// How many samples the worker captures per RX window when idle. ~0.25 s at
/// 48 kHz — long enough to contain a short floor frame, short enough to stay
/// responsive to queued TX.
const RX_WINDOW_SAMPLES: usize = 12_000;

/// A TX request handed to the worker.
struct TxJob {
    payload: Vec<u8>,
    hint: ModeHint,
}

/// Number of recent RECEIVED overs the channel-quality report is computed over.
/// Bounded + recent so the report reflects a fade within an over or two — not a
/// lifetime average (Codex review C4 / design §4). 8 also clears the link's
/// `FER_MIN_SAMPLES` (4) credibility bar quickly.
const FER_WINDOW: usize = 8;
/// After this many consecutive `NoSignal` RX windows with no received over, the
/// recent-quality window is cleared so a clean pre-fade reading cannot survive a
/// fade — the report goes back to "no measurement" (NaN). Aging, not lying
/// (Codex review C4). ~RX_WINDOW_SAMPLES each, so this is a few seconds of dead air.
const STALE_AFTER_NOSIGNAL_WINDOWS: u32 = 12;

/// One received over's outcome, retained in the recent-quality window.
#[derive(Clone, Copy)]
struct OverOutcome {
    /// `true` if the over was detected but failed to decode (a real frame error).
    failed: bool,
    /// Channel SNR (dB, 2500 Hz reference) measured from the over, if any.
    snr_2500_db: Option<f32>,
}

/// Shared channel-quality state, updated by the worker, read by
/// `channel_quality()`. A bounded ring of recent RECEIVED overs (newest last)
/// plus a staleness counter; see [`FER_WINDOW`] / [`STALE_AFTER_NOSIGNAL_WINDOWS`].
#[derive(Default)]
struct QualitySnapshot {
    recent: VecDeque<OverOutcome>,
    /// Consecutive `NoSignal` RX windows since the last received over (staleness).
    windows_since_rx: u32,
}

impl QualitySnapshot {
    /// Record a received over (clean or detected-but-failed). Resets staleness.
    fn record_over(&mut self, failed: bool, snr_2500_db: Option<f32>) {
        self.windows_since_rx = 0;
        self.recent.push_back(OverOutcome {
            failed,
            snr_2500_db,
        });
        while self.recent.len() > FER_WINDOW {
            self.recent.pop_front();
        }
    }

    /// Record an RX window that held only noise. Ages the report; once stale,
    /// clears the window so stale SNR/FER do not survive a fade.
    fn record_no_signal(&mut self) {
        self.windows_since_rx = self.windows_since_rx.saturating_add(1);
        if self.windows_since_rx >= STALE_AFTER_NOSIGNAL_WINDOWS {
            self.recent.clear();
        }
    }

    /// Build the link-facing report: raw latest-over SNR (the link owns the EWMA),
    /// windowed FER, and the received-over count. Empty window ⇒ no measurement.
    fn report(&self) -> ChannelQualityReport {
        if self.recent.is_empty() {
            return ChannelQualityReport::empty();
        }
        // Raw per-over SNR = the most recent over that carried a measurement (a
        // failed over with no body carries none). No PHY-side smoothing (C4).
        let snr = self
            .recent
            .iter()
            .rev()
            .find_map(|o| o.snr_2500_db)
            .unwrap_or(f32::NAN);
        let failed = self.recent.iter().filter(|o| o.failed).count() as u32;
        ChannelQualityReport::from_parts(Vec::new(), snr, self.recent.len() as u32, failed, None)
    }
}

/// Production `PhyTransport` runtime. See crate docs.
pub struct SondePhy {
    tx_jobs: Sender<TxJob>,
    rx_frames: Receiver<RxFrame>,
    quality: Arc<Mutex<QualitySnapshot>>,
    shutdown: Arc<Mutex<bool>>,
    worker: Option<JoinHandle<()>>,
    next_token: u64,
    /// Frames queued for TX or actively being keyed. `send_frame` bumps this on
    /// enqueue; the worker drops it when `Radio::transmit` returns (PTT
    /// released). Read by [`PhyTransport::tx_in_flight`]. Shared with the worker.
    in_flight: Arc<AtomicUsize>,
}

impl SondePhy {
    /// Spawn the runtime over the given waveform + radio. The worker thread
    /// starts immediately and begins capturing RX windows.
    pub fn new<W, R>(waveform: W, radio: R) -> Self
    where
        W: Waveform + 'static,
        R: Radio + 'static,
    {
        Self::with_waveforms(vec![Box::new(waveform)], radio)
    }

    /// Spawn the runtime over a REGISTRY of waveforms and a radio. The RX pump
    /// auto-detects the received mode by running each registered waveform's
    /// `decode_scan` on the window (the first to self-sync + decode wins); TX
    /// picks the waveform whose family matches the requested mode. One waveform is
    /// the common case ([`Self::new`]); more than one enables mid-session mode
    /// adaptation across families without the receiver going deaf on a switch
    /// (design 2026-06-15-phy-mode-adaptation-quality §3). Registry order is the
    /// RX try-order (and the tie-break when two families both decode).
    pub fn with_waveforms<R>(waveforms: Vec<Box<dyn Waveform>>, radio: R) -> Self
    where
        R: Radio + 'static,
    {
        let (tx_jobs, job_rx) = mpsc::channel::<TxJob>();
        let (frame_tx, rx_frames) = mpsc::channel::<RxFrame>();
        let quality = Arc::new(Mutex::new(QualitySnapshot::default()));
        let shutdown = Arc::new(Mutex::new(false));
        let in_flight = Arc::new(AtomicUsize::new(0));

        let worker_quality = Arc::clone(&quality);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_in_flight = Arc::clone(&in_flight);
        let worker = std::thread::spawn(move || {
            Worker {
                waveforms,
                radio,
                job_rx,
                frame_tx,
                quality: worker_quality,
                shutdown: worker_shutdown,
                in_flight: worker_in_flight,
                modes: ModeTable::default(),
            }
            .run();
        });

        Self {
            tx_jobs,
            rx_frames,
            quality,
            shutdown,
            worker: Some(worker),
            next_token: 0,
            in_flight,
        }
    }

    /// Signal the worker to stop and join it. Idempotent. Called by `Drop`, but
    /// exposed so tests can join deterministically.
    pub fn shutdown(&mut self) {
        if let Ok(mut flag) = self.shutdown.lock() {
            *flag = true;
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SondePhy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl PhyTransport for SondePhy {
    fn send_frame(&mut self, payload: &[u8], hint: ModeHint) -> Result<TxToken, PhyError> {
        // Count the frame as in-flight BEFORE handing it to the worker so the
        // count is already live the instant `send_frame` returns — the link
        // must never observe a queued over as "idle". The worker decrements it
        // when `Radio::transmit` returns (PTT released).
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        if self
            .tx_jobs
            .send(TxJob {
                payload: payload.to_vec(),
                hint,
            })
            .is_err()
        {
            // Worker is gone: the frame will never be keyed, so undo the bump.
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            return Err(PhyError::AudioIo("phy worker has stopped".into()));
        }
        let token = TxToken(self.next_token);
        self.next_token += 1;
        Ok(token)
    }

    fn poll_rx(&mut self) -> Option<RxFrame> {
        self.rx_frames.try_recv().ok()
    }

    fn tx_in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    fn channel_quality(&self) -> ChannelQualityReport {
        match self.quality.lock() {
            Ok(q) => q.report(),
            Err(_) => ChannelQualityReport::empty(),
        }
    }
}

struct Worker<R: Radio> {
    waveforms: Vec<Box<dyn Waveform>>,
    radio: R,
    job_rx: Receiver<TxJob>,
    frame_tx: Sender<RxFrame>,
    quality: Arc<Mutex<QualitySnapshot>>,
    shutdown: Arc<Mutex<bool>>,
    in_flight: Arc<AtomicUsize>,
    modes: ModeTable,
}

impl<R: Radio> Worker<R> {
    fn run(mut self) {
        loop {
            if *self.shutdown.lock().unwrap() {
                return;
            }
            // TX has priority: drain one queued job if present.
            match self.job_rx.try_recv() {
                Ok(job) => self.do_tx(job),
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => self.do_rx(),
            }
        }
    }

    fn do_tx(&mut self, job: TxJob) {
        // Transmit ONLY with a waveform that actually serves the resolved mode's
        // family. If none is registered we DROP the over rather than substitute a
        // different family's waveform — silently keying floor RF for an
        // OFDM-requested over (the old `.or_else(first)` fallback) is a real
        // correctness footgun: the link's MODE byte would claim one mode while the
        // air carries another. A dropped over surfaces as a missed over (the link's
        // turn-recovery / P1 BASE-fallback handles it); a wrong-family over does
        // not surface at all. The link is responsible for only requesting modes the
        // PHY registers (ladder-from-registry, sonde-lcw.1).
        let family = self.modes.resolve(job.hint, None).family();
        match self.waveforms.iter().find(|w| w.family() == family) {
            Some(waveform) => {
                // A transmit/encode error is a soundcard hiccup, not an RX channel
                // measurement — we do not crash the worker, and we do NOT fold it
                // into the RX channel-quality window (that would poison the link's
                // FER with TX-side faults).
                if let Ok(samples) = waveform.encode(&job.payload) {
                    let _ = self.radio.transmit(&samples);
                }
            }
            None => {
                // No registered waveform serves this family — drop, do not key the
                // wrong waveform. (Worker has no error channel back to the caller;
                // the over simply does not go on the air.)
            }
        }
        // The over is off the air (PTT released, dropped on no-match, or never made
        // it there on an encode error) — drop it from the in-flight count last, so
        // a link polling `tx_in_flight()` sees the frame keyed for the whole TX
        // window.
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    fn do_rx(&mut self) {
        let samples = match self.radio.receive(RX_WINDOW_SAMPLES) {
            Ok(s) => s,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                return;
            }
        };
        // AUTO-DETECT: run each registered waveform's (cheap-gated) self-syncing
        // decode_scan; the first clean decode wins. This is the sonde-99l answer —
        // a mid-session mode switch is never deafening, because the receiver
        // decodes whatever family actually arrives (design §3). With one waveform
        // this is a single attempt; the loop only does more work when >1 family is
        // registered AND its cheap `detect` pre-gate passes (Codex review C3).
        let mut detected_snr: Option<Option<f32>> = None;
        for waveform in &self.waveforms {
            if !waveform.detect(&samples) {
                continue;
            }
            match waveform.decode_scan(&samples) {
                DecodeScan::Frame(frame) => {
                    let mode = self.modes.resolve(hint_for_family(frame.family), None);
                    if let Ok(mut q) = self.quality.lock() {
                        q.record_over(false, frame.snr_2500_db);
                    }
                    let snr = frame.snr_2500_db.unwrap_or(f32::NAN);
                    let rx = RxFrame::new(frame.payload, mode, None, snr, true);
                    let _ = self.frame_tx.send(rx);
                    return; // first clean decode wins
                }
                // Detected but failed to decode: remember it (with its measured
                // SNR) in case no other waveform decodes cleanly — then it counts
                // as one failed over (no survivorship bias).
                DecodeScan::Detected { snr_2500_db } => {
                    detected_snr.get_or_insert(snr_2500_db);
                }
                DecodeScan::NoSignal => {}
            }
        }
        // No clean decode this window. A detected-but-failed over is a real frame
        // error; pure silence only ages the report.
        if let Ok(mut q) = self.quality.lock() {
            match detected_snr {
                Some(snr) => q.record_over(true, snr),
                None => q.record_no_signal(),
            }
        }
    }
}

/// Representative `ModeHint` for a decoded frame's family, so the delivered
/// `RxFrame` carries a mode of the right family. (The link's MODE byte is the
/// authoritative per-over mode; this is the family-level tag.)
fn hint_for_family(family: ModeFamily) -> ModeHint {
    match family {
        ModeFamily::OfdmMain => ModeHint::MainAuto,
        ModeFamily::RobustnessFloor => ModeHint::Floor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FloorWaveform, LoopbackRadio};
    use sonde_phy::modes::ModeHint;
    use sonde_phy::phy_api::PhyTransport;
    use std::time::{Duration, Instant};

    /// Poll `poll_rx` until a frame arrives or the deadline passes.
    fn wait_for_frame(phy: &mut SondePhy, timeout: Duration) -> Option<RxFrame> {
        let start = Instant::now();
        loop {
            if let Some(f) = phy.poll_rx() {
                return Some(f);
            }
            if start.elapsed() > timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A radio that counts `transmit` calls (and the last buffer length) so a test
    /// can assert whether an over actually went on the air.
    #[derive(Clone, Default)]
    struct CountingRadio {
        tx_calls: std::sync::Arc<AtomicUsize>,
    }
    impl crate::Radio for CountingRadio {
        fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError> {
            if !samples.is_empty() {
                self.tx_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
        fn receive(&mut self, max: usize) -> Result<Vec<f32>, PhyError> {
            std::thread::sleep(Duration::from_millis(2));
            Ok(vec![0.0; max])
        }
    }

    #[test]
    fn tx_drops_an_over_with_no_waveform_for_its_family_instead_of_substituting() {
        // Only a floor-family waveform is registered. A MainAuto hint resolves to
        // the OFDM family — which no registered waveform serves — so the over MUST
        // be dropped, NOT keyed as floor RF (the silent-fallback footgun, 99l.5).
        let radio = CountingRadio::default();
        let calls = std::sync::Arc::clone(&radio.tx_calls);
        let mut phy = SondePhy::new(FloorWaveform::new(), radio);

        phy.send_frame(b"ofdm-please", ModeHint::MainAuto).unwrap();
        // Give the worker time to process the job.
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an OFDM-requested over must NOT be transmitted as floor when no OFDM waveform is registered"
        );

        // A Floor hint DOES match the registered floor waveform → it transmits.
        phy.send_frame(b"floor-ok", ModeHint::Floor).unwrap();
        let start = Instant::now();
        while calls.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a Floor-requested over IS transmitted by the registered floor waveform"
        );
        phy.shutdown();
    }

    #[test]
    fn send_frame_round_trips_through_loopback_to_poll_rx() {
        let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());
        let payload = b"hello tuxlink";

        let token = phy
            .send_frame(payload, ModeHint::Floor)
            .expect("send accepted");
        assert_eq!(token.0, 0, "first token is 0");

        let frame = wait_for_frame(&mut phy, Duration::from_secs(5))
            .expect("a frame round-trips within the deadline");
        assert_eq!(frame.payload(), payload);
        assert!(frame.decode_ok());

        phy.shutdown();
    }

    #[test]
    fn quality_window_is_recent_and_bounded() {
        let mut q = QualitySnapshot::default();
        // Fill past the window with clean overs; FER stays 0, count caps at FER_WINDOW.
        for _ in 0..(FER_WINDOW + 4) {
            q.record_over(false, Some(20.0));
        }
        let r = q.report();
        assert_eq!(
            r.recent_frames_total(),
            FER_WINDOW as u32,
            "window is bounded"
        );
        assert_eq!(r.frame_error_rate(), 0.0);
        assert!(
            (r.aggregate_snr_db() - 20.0).abs() < 1e-3,
            "raw latest-over SNR"
        );

        // A burst of failures pushes FER up within the window …
        for _ in 0..FER_WINDOW {
            q.record_over(true, Some(2.0));
        }
        let r = q.report();
        assert_eq!(r.frame_error_rate(), 1.0, "window now all failures");

        // … and a run of clean overs recovers it (recent, not lifetime).
        for _ in 0..FER_WINDOW {
            q.record_over(false, Some(18.0));
        }
        assert_eq!(
            q.report().frame_error_rate(),
            0.0,
            "FER recovers within window"
        );
    }

    #[test]
    fn quality_window_goes_stale_after_sustained_silence() {
        let mut q = QualitySnapshot::default();
        q.record_over(false, Some(25.0));
        assert!(q.report().aggregate_snr_db().is_finite(), "fresh reading");

        // Sustained dead air ages the report back to "no measurement" (NaN),
        // so a clean pre-fade SNR cannot survive a fade.
        for _ in 0..STALE_AFTER_NOSIGNAL_WINDOWS {
            q.record_no_signal();
        }
        assert!(
            q.report().aggregate_snr_db().is_nan(),
            "stale report reverts to no-measurement"
        );
    }

    #[test]
    fn tokens_are_monotonic() {
        let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());
        let t0 = phy.send_frame(b"a", ModeHint::Floor).unwrap();
        let t1 = phy.send_frame(b"b", ModeHint::Floor).unwrap();
        assert_eq!(t1.0, t0.0 + 1);
        phy.shutdown();
    }

    #[test]
    fn channel_quality_counts_a_received_frame() {
        let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());

        // Before any traffic: zero frames, FER 0.0.
        let before = phy.channel_quality();
        assert_eq!(before.frame_error_rate(), 0.0);

        phy.send_frame(b"quality", ModeHint::Floor).unwrap();
        let _ = wait_for_frame(&mut phy, Duration::from_secs(5)).expect("frame round-trips");

        // Give the worker a beat to update the snapshot.
        std::thread::sleep(Duration::from_millis(50));
        let after = phy.channel_quality();
        assert!(
            after.frame_error_rate().is_finite(),
            "FER is a finite number after a frame"
        );
        phy.shutdown();
    }

    // ─── tx_in_flight (sonde-jt6, seam 1) ──────────────────────────────

    /// A radio that parks inside `transmit` (PTT held) until the test releases
    /// it, so the in-flight count can be observed mid-key deterministically.
    struct BlockingRadio {
        entered: Sender<()>,
        release: Receiver<()>,
    }
    impl Radio for BlockingRadio {
        fn transmit(&mut self, _samples: &[f32]) -> Result<(), PhyError> {
            let _ = self.entered.send(()); // tell the test we are keying
            let _ = self.release.recv(); // park until released
            Ok(())
        }
        fn receive(&mut self, max: usize) -> Result<Vec<f32>, PhyError> {
            std::thread::sleep(Duration::from_millis(1));
            Ok(vec![0.0; max])
        }
    }

    #[test]
    fn tx_in_flight_tracks_a_keyed_over() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut phy = SondePhy::new(
            FloorWaveform::new(),
            BlockingRadio {
                entered: entered_tx,
                release: release_rx,
            },
        );
        assert_eq!(phy.tx_in_flight(), 0, "idle before any send");

        phy.send_frame(b"keyed", ModeHint::Floor).unwrap();
        assert!(
            phy.tx_in_flight() >= 1,
            "send_frame bumps in-flight synchronously — the link must never see a queued over as idle"
        );

        // The worker is now parked inside Radio::transmit (PTT asserted).
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("worker entered transmit");
        assert_eq!(
            phy.tx_in_flight(),
            1,
            "exactly one over is keyed while transmit blocks"
        );

        // Release the over; the count must drain back to 0 once PTT releases.
        release_tx.send(()).unwrap();
        let start = Instant::now();
        while phy.tx_in_flight() != 0 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "in-flight never drained to 0 after the over completed"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        phy.shutdown();
    }

    // ─── detected-but-failed RX (sonde-jt6, seam 2) ────────────────────

    /// A radio that hands back one pre-built capture on the first `receive`,
    /// then silence — to inject a specific RX window into the worker.
    struct ReplayRadio {
        capture: Vec<f32>,
        delivered: bool,
    }
    impl Radio for ReplayRadio {
        fn transmit(&mut self, _samples: &[f32]) -> Result<(), PhyError> {
            Ok(())
        }
        fn receive(&mut self, max: usize) -> Result<Vec<f32>, PhyError> {
            if !self.delivered {
                self.delivered = true;
                Ok(self.capture.clone())
            } else {
                std::thread::sleep(Duration::from_millis(1));
                Ok(vec![0.0; max])
            }
        }
    }

    #[test]
    fn detected_but_failed_frame_counts_as_an_rx_error() {
        use sonde_fec::codec::FloorRate14Codec;
        use sonde_phy::robustness_floor::wideband_lowdensity::{
            WidebandLowDensityFloor, PREAMBLE_LEN_SAMPLES,
        };

        // Build a capture that holds a real preamble + a real first block, but
        // is cut short before the later blocks the header declares. The demod
        // acquires sync (so this is NOT silence) then fails to decode the
        // missing blocks — exactly the case the old `Option<DecodedFrame>` hid
        // as "no frame", leaving FER structurally blind to RX failures.
        let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
        let payload: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
        let full = floor.transmit_multi_with_preamble(&payload).unwrap();
        let keep = PREAMBLE_LEN_SAMPLES + (full.len() - PREAMBLE_LEN_SAMPLES) / 2;
        let capture = full[..keep].to_vec();

        let mut phy = SondePhy::new(
            FloorWaveform::new(),
            ReplayRadio {
                capture,
                delivered: false,
            },
        );

        // Wait until the worker has processed the replayed window and recorded
        // the failure. A clean miss (silence) would never move the FER.
        let start = Instant::now();
        loop {
            if phy.channel_quality().frame_error_rate() > 0.0 {
                break;
            }
            assert!(
                phy.poll_rx().is_none(),
                "a failed decode must not deliver a payload"
            );
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the detected-but-failed frame was never counted"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            phy.channel_quality().frame_error_rate(),
            1.0,
            "the only frame seen was a failure, so FER is 1.0"
        );
        assert!(
            phy.poll_rx().is_none(),
            "no payload is delivered for a failed decode"
        );
        phy.shutdown();
    }
}
