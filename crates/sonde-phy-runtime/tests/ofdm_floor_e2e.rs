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

    // An OFDM over: TX selects the ofdm-wide waveform by mode; RX auto-detects it
    // as OFDM with no hint and labels the specific mode. (Payload fits one N1296
    // R1/2 block.) Pinned to ofdm-wide because that is the OFDM rung registered
    // here — MainAuto's SNR ladder would resolve to ofdm-mid, which per-mode
    // routing correctly declines to substitute (sonde-99l.6).
    let ofdm_payload = b"ofdm-wide over the runtime";
    phy.send_frame(ofdm_payload, ModeHint::MainPinned("ofdm-wide"))
        .unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(10)).expect("ofdm over round-trips");
    assert_eq!(f.payload(), ofdm_payload);
    assert_eq!(
        f.mode().family(),
        ModeFamily::OfdmMain,
        "auto-detected the OFDM-main family with no hint"
    );
    assert_eq!(
        f.mode().short_name(),
        "ofdm-wide",
        "RX labels the specific OFDM rung that decoded"
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

#[test]
fn full_ofdm_ladder_routes_each_rung_to_its_own_mode_and_labels_it_on_rx() {
    // All three OFDM rungs + the floor registered. Pinning each rung must (a) TX
    // the RIGHT waveform (per-mode routing, not family-first) and (b) RX-label the
    // specific mode that decoded — the multi-rung ladder sonde-99l.6 enables.
    let waveforms: Vec<Box<dyn Waveform>> = vec![
        Box::new(OfdmMainWaveform::narrow()),
        Box::new(OfdmMainWaveform::mid()),
        Box::new(OfdmMainWaveform::wide()),
        Box::new(FloorWaveform::new()),
    ];
    let mut phy = SondePhy::with_waveforms(waveforms, LoopbackRadio::new());

    for (mode, family) in [
        ("ofdm-narrow", ModeFamily::OfdmMain),
        ("ofdm-mid", ModeFamily::OfdmMain),
        ("ofdm-wide", ModeFamily::OfdmMain),
        ("floor-wblo", ModeFamily::RobustnessFloor),
    ] {
        let payload = format!("payload for {mode}").into_bytes();
        phy.send_frame(&payload, ModeHint::MainPinned(mode))
            .unwrap();
        let f = wait_for_frame(&mut phy, Duration::from_secs(10))
            .unwrap_or_else(|| panic!("{mode} over round-trips"));
        assert_eq!(f.payload(), payload.as_slice(), "{mode} payload");
        assert_eq!(
            f.mode().short_name(),
            mode,
            "RX labels the pinned rung {mode}"
        );
        assert_eq!(f.mode().family(), family, "{mode} family");
    }

    phy.shutdown();
}
