//! sonde-b60.8: `MainAuto` resolves with the MEASURED channel SNR, not a fixed
//! 15 dB default. Over a clean loopback (high SNR), once an over has populated the
//! quality snapshot, a MainAuto send must resolve to the FAST `ofdm-wide` rung
//! (SNR ≥ 20 dB). Only `ofdm-wide` is registered, so the pre-fix behaviour
//! (default 15 dB → `ofdm-mid`) would find no waveform and drop the over.
//!
//! RADIO-1: loopback only; nothing keyed.

use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::{PhyTransport, RxFrame};
use sonde_phy_runtime::{LoopbackRadio, OfdmMainWaveform, SondePhy, Waveform};
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
fn mainauto_uses_measured_snr_to_pick_the_fast_rung() {
    // Only ofdm-wide registered. A clean loopback over reports a high SNR_2500.
    let waveforms: Vec<Box<dyn Waveform>> = vec![Box::new(OfdmMainWaveform::wide())];
    let mut phy = SondePhy::with_waveforms(waveforms, LoopbackRadio::new());

    // Prime the quality snapshot with one clean (high-SNR) over.
    phy.send_frame(b"prime the snr", ModeHint::MainPinned("ofdm-wide"))
        .unwrap();
    let primed =
        wait_for_frame(&mut phy, Duration::from_secs(10)).expect("primer over round-trips");
    assert_eq!(primed.mode().short_name(), "ofdm-wide");

    // Now MainAuto: with the MEASURED high SNR it resolves to ofdm-wide and keys
    // it (round-trips). With the old None→15 dB default it would resolve to
    // ofdm-mid, find no registered waveform, and drop → timeout.
    phy.send_frame(b"auto picks wide", ModeHint::MainAuto)
        .unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(10))
        .expect("MainAuto over round-trips (resolved to a registered fast rung via measured SNR)");
    assert_eq!(f.payload(), b"auto picks wide");
    assert_eq!(
        f.mode().short_name(),
        "ofdm-wide",
        "MainAuto resolved to the fast rung from the measured high SNR"
    );

    phy.shutdown();
}
