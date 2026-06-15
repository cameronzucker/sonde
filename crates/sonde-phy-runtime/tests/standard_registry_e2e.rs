//! sonde-cyo: the `standard_waveforms()` registry — the full ladder the
//! `sonde-modem` runtime binary ships — round-trips through SondePhy over a
//! loopback radio. Proves the operator-facing entrypoint's registry is wired end
//! to end (fast modes here; nFSK's long over is covered by nfsk_e2e).
//!
//! RADIO-1: loopback only; nothing keyed.

use sonde_phy::modes::{ModeFamily, ModeHint};
use sonde_phy::phy_api::{PhyTransport, RxFrame};
use sonde_phy_runtime::{standard_waveforms, LoopbackRadio, SondePhy};
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
fn standard_registry_round_trips_across_the_ladder() {
    let mut phy = SondePhy::with_waveforms(standard_waveforms(), LoopbackRadio::new());

    // Fast rungs: pin each, confirm it round-trips and RX-labels its own mode
    // through the full 5-waveform registry. (floor-nfsk's ~9 s over is exercised
    // in nfsk_e2e; kept out of this fast CI gate.)
    for (hint, expect_mode, family) in [
        (
            ModeHint::MainPinned("ofdm-wide"),
            "ofdm-wide",
            ModeFamily::OfdmMain,
        ),
        (ModeHint::Floor, "floor-wblo", ModeFamily::RobustnessFloor),
    ] {
        let payload = format!("standard registry {expect_mode}").into_bytes();
        phy.send_frame(&payload, hint).unwrap();
        let f = wait_for_frame(&mut phy, Duration::from_secs(20))
            .unwrap_or_else(|| panic!("{expect_mode} round-trips through the standard registry"));
        assert_eq!(f.payload(), payload.as_slice());
        assert_eq!(f.mode().short_name(), expect_mode);
        assert_eq!(f.mode().family(), family);
    }

    phy.shutdown();
}
