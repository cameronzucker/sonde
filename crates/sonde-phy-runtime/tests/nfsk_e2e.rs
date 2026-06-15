//! sonde-99l.4 end-to-end: the narrow-FSK deep-floor and the wide-band floor —
//! TWO modes in the SAME RobustnessFloor family — route per-mode through SondePhy
//! and each decodes to its own mode. Proves per-mode routing (sonde-99l.6)
//! distinguishes modes WITHIN a family, not just across families.
//!
//! RADIO-1: loopback only; nothing keyed.

use sonde_phy::modes::{ModeFamily, ModeHint};
use sonde_phy::phy_api::{PhyTransport, RxFrame};
use sonde_phy_runtime::{FloorWaveform, LoopbackRadio, NfskWaveform, SondePhy, Waveform};
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
fn nfsk_and_wideband_floor_route_per_mode_within_the_floor_family() {
    let waveforms: Vec<Box<dyn Waveform>> = vec![
        Box::new(NfskWaveform::new()),
        Box::new(FloorWaveform::new()),
    ];
    let mut phy = SondePhy::with_waveforms(waveforms, LoopbackRadio::new());

    // FloorCrowdedBand resolves to "floor-nfsk" → the nFSK waveform keys + decodes.
    let nfsk_payload = b"crowded-band nfsk over runtime";
    phy.send_frame(nfsk_payload, ModeHint::FloorCrowdedBand)
        .unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(10)).expect("nfsk over round-trips");
    assert_eq!(f.payload(), nfsk_payload);
    assert_eq!(
        f.mode().short_name(),
        "floor-nfsk",
        "RX labels the nFSK mode"
    );
    assert_eq!(f.mode().family(), ModeFamily::RobustnessFloor);

    // Floor resolves to "floor-wblo" → the wide-band floor keys + decodes, even
    // though both are RobustnessFloor family (per-mode routing, not family-first).
    let wblo_payload = b"wideband floor over runtime";
    phy.send_frame(wblo_payload, ModeHint::Floor).unwrap();
    let f = wait_for_frame(&mut phy, Duration::from_secs(10)).expect("wblo over round-trips");
    assert_eq!(f.payload(), wblo_payload);
    assert_eq!(
        f.mode().short_name(),
        "floor-wblo",
        "RX labels the wide-band floor mode"
    );

    phy.shutdown();
}
