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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use sonde_phy::error::PhyError;
use sonde_phy::modes::{ModeHint, ModeTable};
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

/// Shared channel-quality snapshot, updated by the worker, read by
/// `channel_quality()`.
#[derive(Default)]
struct QualitySnapshot {
    frames_total: u32,
    frames_failed: u32,
    last_frame_snr_db: Option<f32>,
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
                waveform,
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
        let q = match self.quality.lock() {
            Ok(q) => q,
            Err(_) => return ChannelQualityReport::empty(),
        };
        ChannelQualityReport::from_parts(
            Vec::new(),
            q.last_frame_snr_db.unwrap_or(f32::NAN),
            q.frames_total,
            q.frames_failed,
            None,
        )
    }
}

struct Worker<W: Waveform, R: Radio> {
    waveform: W,
    radio: R,
    job_rx: Receiver<TxJob>,
    frame_tx: Sender<RxFrame>,
    quality: Arc<Mutex<QualitySnapshot>>,
    shutdown: Arc<Mutex<bool>>,
    in_flight: Arc<AtomicUsize>,
    modes: ModeTable,
}

impl<W: Waveform, R: Radio> Worker<W, R> {
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
        let _mode = self.modes.resolve(job.hint, None);
        match self.waveform.encode(&job.payload) {
            Ok(samples) => {
                // A transmit error is logged via the quality counters as a
                // failed frame; we do not crash the worker on a soundcard
                // hiccup.
                if self.radio.transmit(&samples).is_err() {
                    if let Ok(mut q) = self.quality.lock() {
                        q.frames_total += 1;
                        q.frames_failed += 1;
                    }
                }
            }
            Err(_) => {
                if let Ok(mut q) = self.quality.lock() {
                    q.frames_total += 1;
                    q.frames_failed += 1;
                }
            }
        }
        // The over is off the air (PTT released, or the job never made it there
        // on an encode error) — drop it from the in-flight count last, so a link
        // polling `tx_in_flight()` sees the frame keyed for the whole TX window.
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
        match self.waveform.decode_scan(&samples) {
            DecodeScan::Frame(frame) => {
                let mode = self.modes.resolve(ModeHint::Floor, None);
                if let Ok(mut q) = self.quality.lock() {
                    q.frames_total += 1;
                    q.last_frame_snr_db = frame.frame_snr_db;
                }
                let snr = frame.frame_snr_db.unwrap_or(f32::NAN);
                let rx = RxFrame::new(frame.payload, mode, None, snr, true);
                let _ = self.frame_tx.send(rx);
            }
            // A frame was acquired but failed to decode: a real RX frame error.
            // Count it (total + failed) so `channel_quality().frame_error_rate()`
            // reflects it — this is the signal subsystem #5 adapts on. No
            // `RxFrame` is delivered (there is no payload).
            DecodeScan::Detected => {
                if let Ok(mut q) = self.quality.lock() {
                    q.frames_total += 1;
                    q.frames_failed += 1;
                }
            }
            // Only noise in this window — not a frame error; count nothing.
            DecodeScan::NoSignal => {}
        }
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
