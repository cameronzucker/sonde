//! The contract test Tuxlink's integration mirrors: construct a SondePhy, send
//! a frame, poll it back, read channel quality — all through the `PhyTransport`
//! trait, no concrete-type leakage. Hardware-free (LoopbackRadio), so it runs
//! in CI and respects RADIO-1.

use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{FloorWaveform, LoopbackRadio, SondePhy};
use std::time::{Duration, Instant};

fn poll_until<P: PhyTransport>(
    phy: &mut P,
    timeout: Duration,
) -> Option<sonde_phy::phy_api::RxFrame> {
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
fn tuxlink_style_send_and_receive() {
    let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());

    let payload = b"the quick brown fox";
    let _token = phy.send_frame(payload, ModeHint::Floor).expect("accepted");

    let frame = poll_until(&mut phy, Duration::from_secs(5)).expect("frame round-trips");
    assert_eq!(frame.payload(), payload);
    assert!(frame.decode_ok());

    let q = phy.channel_quality();
    assert!(q.frame_error_rate().is_finite());
    phy.shutdown();
}
