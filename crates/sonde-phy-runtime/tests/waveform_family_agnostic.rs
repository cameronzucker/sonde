//! FM-readiness guard. A future "Sonde FM" lands as a new `impl Waveform`
//! routed under `ModeFamily::OfdmMain` (or a dedicated FM family added to
//! `sonde_phy::modes`). This test stands in a trivial non-floor waveform and
//! drives the real `SondePhy` runtime through it, proving the runtime is
//! waveform-agnostic — the property that makes FM an extension, not a fork. If
//! this breaks, the runtime grew an HF-only assumption.

use sonde_phy::modes::{ModeFamily, ModeHint};
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{DecodeScan, DecodedFrame, Radio, SondePhy, Waveform};
use std::time::{Duration, Instant};

/// A toy waveform: payload bytes are widened to f32 samples 1:1 and read back
/// the same way. No preamble, no DSP — it exercises the runtime's plumbing, not
/// modulation. Tagged as a non-floor family on purpose.
struct ByteEchoWaveform;

impl Waveform for ByteEchoWaveform {
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, sonde_phy::error::PhyError> {
        Ok(payload.iter().map(|&b| b as f32).collect())
    }
    fn decode_scan(&self, samples: &[f32]) -> DecodeScan {
        // Loopback wraps with leading/trailing zero silence; strip zeros.
        let bytes: Vec<u8> = samples
            .iter()
            .filter(|&&s| s != 0.0)
            .map(|&s| s as u8)
            .collect();
        if bytes.is_empty() {
            DecodeScan::NoSignal
        } else {
            DecodeScan::Frame(DecodedFrame {
                payload: bytes,
                family: ModeFamily::OfdmMain,
                frame_snr_db: Some(42.0),
            })
        }
    }
    fn family(&self) -> ModeFamily {
        ModeFamily::OfdmMain
    }
}

/// Minimal loopback radio local to this test (mirrors `LoopbackRadio`).
struct EchoRadio {
    pending: Vec<f32>,
}
impl Radio for EchoRadio {
    fn transmit(&mut self, samples: &[f32]) -> Result<(), sonde_phy::error::PhyError> {
        let mut framed = vec![0.0f32; 8];
        framed.extend_from_slice(samples);
        framed.push(0.0);
        self.pending = framed;
        Ok(())
    }
    fn receive(&mut self, max: usize) -> Result<Vec<f32>, sonde_phy::error::PhyError> {
        if self.pending.is_empty() {
            Ok(vec![0.0; max])
        } else {
            Ok(std::mem::take(&mut self.pending))
        }
    }
}

#[test]
fn runtime_drives_a_non_floor_waveform() {
    let mut phy = SondePhy::new(
        ByteEchoWaveform,
        EchoRadio {
            pending: Vec::new(),
        },
    );
    // Bytes that are non-zero so the echo waveform survives the silence strip.
    let payload = vec![3u8, 1, 4, 1, 5, 9];
    phy.send_frame(&payload, ModeHint::MainAuto).unwrap();

    let start = Instant::now();
    let frame = loop {
        if let Some(f) = phy.poll_rx() {
            break f;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "frame must arrive"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(frame.payload(), payload.as_slice());
    assert_eq!(frame.frame_snr_db(), 42.0);
    phy.shutdown();
}
