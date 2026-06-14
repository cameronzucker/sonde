//! Differential FEC gate: the SAME noisy capture must FAIL to decode with the
//! identity (no-FEC) baseline and SUCCEED with the real rate-1/4 LDPC. Impossible
//! to pass unless `FloorRate14Codec` is genuinely in the signal path adding
//! end-to-end coding gain.
//!
//! ## Why the differential gate uses AWGN
//!
//! `floor_fec_decodes_where_identity_fails` uses AWGN from `hf_channel_sim`'s
//! `AwgnGenerator` because the *differential* separation (identity fails / FEC
//! succeeds) is cleanest and most deterministic at an AWGN-dominated operating
//! point. It is not a fading test — its job is to prove the codec is wired and
//! carrying coding gain.
//!
//! ## Watterson fading (added sonde-64w.2)
//!
//! With the channel-aware-LLR equalizer fix in place (sonde-phy's
//! `ofdm_main::receiver` now does pilot-aided channel estimation + reliability-
//! weighted soft LLRs), the floor decodes through Watterson fading too.
//! `operational_pipeline_survives_watterson_good` below proves the *operational*
//! entry points (`encode_payload` / `decode_one_symbol`) recover the payload
//! through a frequency-selective fade on the converged seed. It is
//! converged-seed-only (not seed-swept) because the production sync correlator
//! is not yet phase-robust across all realizations — tracked under bd
//! `sonde-64w.3` (complex matched-filter sync). The correctly-aligned equalizer
//! robustness is gated seed-robustly in
//! `sonde-phy/tests/robustness_floor_fading.rs`.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rustfft::FftPlanner;
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

/// Analytic signal (Hilbert) of a real signal: zero negative freqs, double
/// positives. `Re{analytic} == original` for a unit channel.
fn analytic(real_sig: &[f32]) -> Vec<Complex<f32>> {
    let n = real_sig.len();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf: Vec<Complex<f32>> = real_sig.iter().map(|&x| Complex::new(x, 0.0)).collect();
    fwd.process(&mut buf);
    let half = n / 2;
    for (k, b) in buf.iter_mut().enumerate() {
        if k == 0 || (n % 2 == 0 && k == half) {
        } else if k < half {
            *b *= 2.0;
        } else {
            *b = Complex::new(0.0, 0.0);
        }
    }
    inv.process(&mut buf);
    let scale = 1.0 / n as f32;
    for b in buf.iter_mut() {
        *b *= scale;
    }
    buf
}

/// Real audio → analytic → complex Watterson → AWGN → real audio. The faithful
/// application of a complex-baseband fading model to real passband audio.
fn through_watterson(
    clean: &[f32],
    condition: ChannelCondition,
    snr_db: f64,
    seed: u64,
) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, condition, 48_000.0);
    let mut a = analytic(clean);
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(1024));
    let mut faded = ch.process_block(&a);
    AwgnGenerator::new(seed ^ 0xA5A5).add_noise(&mut faded, snr_db);
    faded.iter().map(|c| c.re).collect()
}

#[test]
fn floor_fec_decodes_where_identity_fails() {
    let payload = b"DIFFERENTIAL GATE PAYLOAD";
    // AWGN-dominated operating point with margin (FEC decodes from roughly
    // -6 dB up; identity never does). See the module doc for why the
    // differential gate uses AWGN (fading is covered by the Watterson test).
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

#[test]
fn operational_pipeline_survives_watterson_good() {
    // The operational entry points recover the payload through a frequency-
    // selective Watterson Good fade — the un-fakeable proof that the
    // channel-aware-LLR equalizer fix (sonde-64w.2) is load-bearing in the real
    // encode→channel→decode path, not just a sonde-phy unit test. Converged-seed
    // only: production sync phase-robustness is bd sonde-64w.3.
    let payload = b"DIFFERENTIAL GATE PAYLOAD";
    let seed = 0xC0DE_6402;

    let buf = sonde_tx::encode_payload(
        sonde_tx::Mode::WideFloor,
        payload,
        sonde_tx::FrameMode::MultiSync,
    )
    .expect("operational encode");
    let faded = through_watterson(buf.samples(), ChannelCondition::Good, 30.0, seed);

    let decoded = sonde_rx::decode_one_symbol(
        sonde_rx::Mode::WideFloor,
        &faded,
        sonde_rx::FrameMode::MultiSync,
    )
    .expect("operational decode through Watterson Good fading");
    assert_eq!(
        decoded, payload,
        "operational pipeline must recover the payload through Watterson Good fading"
    );
}
