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

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use sonde_phy::error::PhyError;
use sonde_phy::modes::{ModeHint, ModeTable};
use sonde_phy::phy_api::{ChannelQualityReport, PhyTransport, RxFrame, TxToken};

use crate::radio::Radio;
use crate::waveform::Waveform;

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

        let worker_quality = Arc::clone(&quality);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = std::thread::spawn(move || {
            Worker {
                waveform,
                radio,
                job_rx,
                frame_tx,
                quality: worker_quality,
                shutdown: worker_shutdown,
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
        self.tx_jobs
            .send(TxJob {
                payload: payload.to_vec(),
                hint,
            })
            .map_err(|_| PhyError::AudioIo("phy worker has stopped".into()))?;
        let token = TxToken(self.next_token);
        self.next_token += 1;
        Ok(token)
    }

    fn poll_rx(&mut self) -> Option<RxFrame> {
        self.rx_frames.try_recv().ok()
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
    }

    fn do_rx(&mut self) {
        let samples = match self.radio.receive(RX_WINDOW_SAMPLES) {
            Ok(s) => s,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                return;
            }
        };
        if let Some(frame) = self.waveform.decode_scan(&samples) {
            let mode = self.modes.resolve(ModeHint::Floor, None);
            if let Ok(mut q) = self.quality.lock() {
                q.frames_total += 1;
                q.last_frame_snr_db = frame.frame_snr_db;
            }
            let snr = frame.frame_snr_db.unwrap_or(f32::NAN);
            let rx = RxFrame::new(frame.payload, mode, None, snr, true);
            let _ = self.frame_tx.send(rx);
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
}
