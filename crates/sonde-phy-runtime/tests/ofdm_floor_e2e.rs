//! sonde-c7i end-to-end: SondePhy with BOTH real waveform families registered —
//! `OfdmMainWaveform` (ofdm-wide QPSK) + `FloorWaveform` (BPSK floor) — auto-detects
//! the received mode with NO hint, through the actual PhyTransport seam over a
//! loopback radio. This is the genuine "multi-mode PHY end-to-end" proof: real
//! modulation/sync/FEC for two families, not test doubles.
//!
//! RADIO-1: loopback only; nothing keyed.

use sonde_phy::modes::{ModeFamily, ModeHint};
use sonde_phy::phy_api::{PhyTransport, RxFrame};
use sonde_phy_runtime::{FloorWaveform, LoopbackRadio, OfdmMainWaveform, SondePhy, Waveform};
use std::time::{Duration, Instant};

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
fn ofdm_and_floor_both_round_trip_and_auto_detect_through_the_runtime() {
    // Register OFDM-main first, floor second: the RX pump tries each registered
    // waveform's self-syncing decode and the first to decode wins, so a floor over
    // proves the pump does not stop at the OFDM waveform.
    let waveforms: Vec<Box<dyn Waveform>> = vec![
        Box::new(OfdmMainWaveform::new()),
        Box::new(FloorWaveform::new()),
    ];
    let mut phy = SondePhy::with_waveforms(waveforms, LoopbackRadio::new());

    // A MainAuto over: TX selects the OFDM-family waveform; RX auto-detects it as
    // OFDM with no hint. (Payload fits one N1296 R1/2 block.)
    let ofdm_payload = b"ofdm-wide over the runtime";
    phy.send_frame(ofdm_payload, ModeHint::MainAuto).unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(10)).expect("ofdm over round-trips");
    assert_eq!(f.payload(), ofdm_payload);
    assert_eq!(
        f.mode().family(),
        ModeFamily::OfdmMain,
        "auto-detected the OFDM-main family with no hint"
    );

    // A Floor over: TX selects the floor waveform; RX still decodes it — the pump
    // tried both real waveforms.
    let floor_payload = b"floor fallback over the runtime";
    phy.send_frame(floor_payload, ModeHint::Floor).unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(10)).expect("floor over round-trips");
    assert_eq!(f.payload(), floor_payload);
    assert_eq!(
        f.mode().family(),
        ModeFamily::RobustnessFloor,
        "auto-detected the floor family — pump did not stop at the OFDM waveform"
    );

    phy.shutdown();
}
