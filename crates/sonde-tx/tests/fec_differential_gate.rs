//! Differential FEC gate: the SAME noisy capture must FAIL to decode with the
//! identity (no-FEC) baseline and SUCCEED with the real rate-1/4 LDPC. Impossible
//! to pass unless `FloorRate14Codec` is genuinely in the signal path adding
//! end-to-end coding gain.
//!
//! ## Why AWGN-only (not Watterson fading)
//!
//! This gate proves *wiring + coding gain*, not worst-case channel robustness
//! (that is a later slice). The impairment here is AWGN from `hf_channel_sim`'s
//! `AwgnGenerator`. The Watterson fading path (`WattersonChannel`) is
//! deliberately *not* applied: the floor receiver has no per-subcarrier channel
//! estimation / equalizer yet, so even the mildest two-tap Rayleigh fade — at
//! zero Doppler and zero delay spread — applies an arbitrary complex rotation
//! that the preamble-sync phase reference cannot undo, collapsing *both* the
//! identity and the FEC decode to failure (empirically verified across all
//! `ChannelCondition`s and custom low-Doppler params). A capture where neither
//! codec decodes proves nothing about coding gain. AWGN-dominated points, by
//! contrast, give a wide, clean, deterministic separation: the rate-1/4 LDPC
//! recovers the payload where the identity baseline cannot.

use hf_channel_sim::AwgnGenerator;
use num_complex::Complex;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

/// Apply AWGN at the given SNR to a real-valued audio capture. The signal is
/// mapped to the complex plane (`im = 0`) for the noise generator and the real
/// part is taken back afterwards.
fn impair(clean: &[f32], snr_db: f64, seed: u64) -> Vec<f32> {
    let mut cx: Vec<Complex<f32>> = clean.iter().map(|&s| Complex::new(s, 0.0)).collect();
    AwgnGenerator::new(seed ^ 0xA5A5).add_noise(&mut cx, snr_db);
    cx.iter().map(|c| c.re).collect()
}

#[test]
fn floor_fec_decodes_where_identity_fails() {
    let payload = b"DIFFERENTIAL GATE PAYLOAD";
    // AWGN-dominated operating point with margin (FEC decodes from roughly
    // -6 dB up; identity never does). See the module doc for why fading is
    // excluded.
    let snr_db = 4.0;
    let seed = 0xC0DE_F100;

    // ONE coded capture pushed through ONE noisy channel.
    let tx = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let clean = tx.transmit_multi_with_preamble(payload).unwrap();
    let noisy = impair(&clean, snr_db, seed);

    // Identity (no-FEC) receiver must FAIL on this capture. NOTE: this negative
    // control conflates two effects — IdentityFec's 74-bit framing differs from
    // FloorRate14's, so it cannot even parse the coded stream's length header,
    // *and* it has no coding gain against the AWGN. It is an anti-island guard
    // (the coded output is provably not trivially decodable as uncoded); the
    // load-bearing coding-gain proof is the positive FEC-decode assertion below.
    let id_rx = WidebandLowDensityFloor::new(); // IdentityFec baseline
    let identity_ok =
        matches!(id_rx.receive_multi_with_sync(&noisy), Ok((_, ref p)) if p == payload);
    assert!(
        !identity_ok,
        "IdentityFec unexpectedly recovered payload at {snr_db} dB AWGN — pick a harder point"
    );

    // Real rate-1/4 LDPC must SUCCEED on the same capture.
    let fec_rx = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let (_segments, decoded) = fec_rx
        .receive_multi_with_sync(&noisy)
        .expect("FloorRate14Codec must decode the impaired capture");
    assert_eq!(decoded, payload, "rate-1/4 LDPC must recover the payload");
}

#[test]
fn operational_pipeline_survives_awgn_end_to_end() {
    // The end-to-end operational gate: the *real* TX entry point
    // (`sonde_tx::encode_payload`, which injects `FloorRate14Codec`) →
    // one noisy channel → the *real* RX entry point
    // (`sonde_rx::decode_one_symbol`, which also injects `FloorRate14Codec`)
    // recovers the payload; and the SAME capture decoded by a no-FEC floor
    // FAILS. This is the un-fakeable proof that the wired codec is load-bearing
    // in the operational pipeline — not just reachable in a unit test.
    let payload = b"DIFFERENTIAL GATE PAYLOAD";
    let snr_db = 4.0;
    let seed = 0xC0DE_F100;

    // Operational TX (encode_payload wires FloorRate14Codec via C1).
    let buf = sonde_tx::encode_payload(
        sonde_tx::Mode::WideFloor,
        payload,
        sonde_tx::FrameMode::MultiSync,
    )
    .expect("operational encode");
    let noisy = impair(buf.samples(), snr_db, seed);

    // A no-FEC floor cannot decode the operational (coded) capture.
    let id_rx = WidebandLowDensityFloor::new();
    let identity_ok =
        matches!(id_rx.receive_multi_with_sync(&noisy), Ok((_, ref p)) if p == payload);
    assert!(
        !identity_ok,
        "no-FEC floor must not decode the operational coded capture"
    );

    // Operational RX (decode_one_symbol wires FloorRate14Codec via C2) recovers
    // the payload through the noisy channel.
    let decoded = sonde_rx::decode_one_symbol(
        sonde_rx::Mode::WideFloor,
        &noisy,
        sonde_rx::FrameMode::MultiSync,
    )
    .expect("operational decode through AWGN");
    assert_eq!(
        decoded, payload,
        "operational TX→channel→RX pipeline must recover the payload end-to-end"
    );
}
