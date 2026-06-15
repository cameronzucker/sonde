//! Gate G5 — wiring smoke over the **real** `SondePhy` + `FloorWaveform`
//! (design §8). This is a *wiring* claim only: it proves the generic link's
//! frames survive the real PhyTransport encode/decode path and that `Link<P>`
//! drives the real runtime. It is **NOT** an HF-viability or throughput claim —
//! over-the-real-PHY viability is gated on the PHY physics gates, owned
//! elsewhere. Per RADIO-1 the radio here is the hardware-free `LoopbackRadio`;
//! nothing keys a real transmitter.
//!
//! `LoopbackRadio` loops a phy's TX back to its *own* RX, so a two-party
//! handshake (which the callsign addressing would split) is out of scope here;
//! a full two-station threaded run over a shared-medium radio is a documented
//! follow-up. What this gate establishes is the load-bearing integration fact:
//! a link frame is not an island over the real waveform.

use std::time::{Duration, Instant};

use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{FloorWaveform, LoopbackRadio, SondePhy};

use sonde_link::{Callsign, Connection, Link, LinkFrame, ModeProfile};

fn call(s: &str) -> Callsign {
    Callsign::new(s).unwrap()
}

fn profile() -> ModeProfile {
    ModeProfile::new(Duration::from_millis(10), 8)
}

/// Poll the worker for a decoded frame's bytes, up to a wall-clock deadline.
fn wait_for_bytes(phy: &mut SondePhy, deadline: Duration) -> Option<Vec<u8>> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if let Some(rx) = phy.poll_rx() {
            return Some(rx.payload().to_vec());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

#[test]
fn g5_link_frame_round_trips_through_real_sondephy_floor_waveform() {
    // Take an actual frame the link layer emits (its CONN) and push it through
    // the real FloorWaveform encode → LoopbackRadio → decode path. It must come
    // back byte-identical and re-parse as the same LinkFrame.
    let mut conn = Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8);
    conn.connect(Duration::ZERO);
    let conn_frame = conn
        .poll_transmit(Duration::ZERO)
        .expect("link emits a CONN frame");
    let bytes = conn_frame.encode().expect("link frame encodes");

    let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());
    phy.send_frame(&bytes, ModeHint::Floor)
        .expect("real phy accepts the frame");
    let got = wait_for_bytes(&mut phy, Duration::from_secs(5))
        .expect("frame round-trips through the real waveform within the deadline");
    phy.shutdown();

    let decoded = LinkFrame::decode(&got).expect("link frame survives the real PHY decode");
    assert_eq!(decoded, conn_frame, "byte-exact round-trip, not an island");
}

#[test]
fn g5_link_drives_the_real_sondephy_transport() {
    // The generic Link<P> compiles against and drives a real SondePhy: pumping
    // it serializes the CONN over the real waveform without panicking. Proves
    // the adapter is wired to the production trait, not only to test doubles.
    let conn = Connection::initiator(call("K1ABC"), call("W2XYZ"), 0x1234, profile(), 8);
    let phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());
    let mut link = Link::new(phy, conn, ModeHint::Floor);
    link.connect(Duration::ZERO);
    for i in 0..5 {
        let _ = link.poll(Duration::from_millis(10 * i));
    }
    // Reaching here without panic is the wiring assertion.
}
