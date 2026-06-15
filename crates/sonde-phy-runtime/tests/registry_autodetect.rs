//! sonde-99l.2: the RX pump AUTO-DETECTS the received mode across a registry of
//! waveforms — it runs each registered waveform's self-syncing `decode_scan` and
//! the first to decode wins, with NO external mode hint. This is the architectural
//! answer that lets a mid-session mode switch never go deaf (design
//! `2026-06-15-phy-mode-adaptation-quality-design.md` §3). Proven with two
//! distinct in-memory waveforms of different families over a loopback radio.
//!
//! RADIO-1: in-memory doubles only; nothing keyed.

use sonde_phy::error::PhyError;
use sonde_phy::modes::{ModeFamily, ModeHint};
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{DecodeScan, DecodedFrame, Radio, SondePhy, Waveform};
use std::time::{Duration, Instant};

/// A trivial self-describing waveform: `encode` prepends a family marker byte;
/// `decode_scan` returns `Frame` iff the first non-zero sample is its own marker,
/// else `NoSignal`. Distinct markers ⇒ exactly one waveform decodes any frame, so
/// it stands in for "different families self-sync on different preambles".
struct MarkerWaveform {
    family: ModeFamily,
    marker: u8,
}

impl Waveform for MarkerWaveform {
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        let mut out = vec![self.marker as f32];
        out.extend(payload.iter().map(|&b| b as f32));
        Ok(out)
    }
    fn decode_scan(&self, samples: &[f32]) -> DecodeScan {
        let nonzero: Vec<u8> = samples
            .iter()
            .filter(|&&s| s != 0.0)
            .map(|&s| s as u8)
            .collect();
        match nonzero.split_first() {
            Some((&first, rest)) if first == self.marker => DecodeScan::Frame(DecodedFrame {
                payload: rest.to_vec(),
                family: self.family,
                snr_2500_db: Some(15.0),
            }),
            _ => DecodeScan::NoSignal,
        }
    }
    fn family(&self) -> ModeFamily {
        self.family
    }
}

/// Loopback radio: replays the last transmitted buffer (with a little leading
/// silence to exercise the non-zero-offset path).
struct EchoRadio {
    pending: Vec<f32>,
}
impl Radio for EchoRadio {
    fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError> {
        let mut framed = vec![0.0f32; 4];
        framed.extend_from_slice(samples);
        self.pending = framed;
        Ok(())
    }
    fn receive(&mut self, max: usize) -> Result<Vec<f32>, PhyError> {
        if self.pending.is_empty() {
            Ok(vec![0.0; max])
        } else {
            Ok(std::mem::take(&mut self.pending))
        }
    }
}

fn wait_for_frame(phy: &mut SondePhy, timeout: Duration) -> Option<sonde_phy::phy_api::RxFrame> {
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
fn rx_pump_auto_detects_mode_across_a_two_waveform_registry() {
    // Register OFDM first, floor second: order is the RX try-order, so a floor
    // frame proves the pump does NOT stop at the first (non-matching) waveform.
    let waveforms: Vec<Box<dyn Waveform>> = vec![
        Box::new(MarkerWaveform {
            family: ModeFamily::OfdmMain,
            marker: 0xA1,
        }),
        Box::new(MarkerWaveform {
            family: ModeFamily::RobustnessFloor,
            marker: 0xB2,
        }),
    ];
    let mut phy = SondePhy::with_waveforms(
        waveforms,
        EchoRadio {
            pending: Vec::new(),
        },
    );

    // A MainAuto over: TX selects the OFDM-family waveform; RX auto-detects it.
    phy.send_frame(&[1, 2, 3], ModeHint::MainAuto).unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(5)).expect("ofdm frame round-trips");
    assert_eq!(f.payload(), &[1, 2, 3]);
    assert_eq!(
        f.mode().family(),
        ModeFamily::OfdmMain,
        "auto-detected the OFDM family with no hint"
    );

    // A Floor over: TX selects the floor-family waveform (the SECOND in the
    // registry); RX still decodes it — the pump tried both candidates.
    phy.send_frame(&[9, 8], ModeHint::Floor).unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(5)).expect("floor frame round-trips");
    assert_eq!(f.payload(), &[9, 8]);
    assert_eq!(
        f.mode().family(),
        ModeFamily::RobustnessFloor,
        "auto-detected the floor family — pump did not stop at the first waveform"
    );

    phy.shutdown();
}
