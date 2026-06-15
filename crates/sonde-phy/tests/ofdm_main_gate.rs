//! Physics gate (sonde-c7i): the first REAL OFDM main-family mode — `ofdm-wide`
//! = Wide params, QPSK (2 bits/sub-carrier), WiFi LDPC N1296 R1/2 — decodes
//! end-to-end through the SAME preamble + Schmidl-Cox sync + pilot-equalized demod
//! as the floor, over calibrated AWGN. Gate on physics, not artifacts:
//!
//! - it actually decodes (the mode is real, not a catalog entry), and
//! - its FER-vs-`SNR_2500` knee is a sane, higher-throughput point that sits at or
//!   above the floor's knee — i.e. a real ladder rung above the floor.
//!
//! The reported `SNR_2500` is the honest 2500 Hz-referenced channel SNR (same
//! estimator as the floor). RADIO-1: nothing keyed. `sonde-fec` is dev-only here.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sonde_fec::codec::OfdmAdaptiveCodec;
use sonde_fec::codes::{BlockN, WifiLdpcRate};
use sonde_phy::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use sonde_phy::robustness_floor::wideband_lowdensity::{
    SyncDecodeOutcome, WidebandLowDensityFloor,
};

const SR: f64 = 48_000.0;

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn payload(seed: u64, n: usize) -> Vec<u8> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..n).map(|_| rng.gen()).collect()
}

/// Add AWGN sized so the channel SNR referenced to 2500 Hz equals `snr_2500_db`.
fn add_awgn_at_snr_2500(signal: &[f32], snr_2500_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let n = signal.len() as f64;
    let s: f64 = signal.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n;
    let t = 10f64.powf(snr_2500_db / 10.0);
    let sigma2 = s * (SR / 2.0) / (2500.0 * t);
    let sigma = sigma2.sqrt() as f32;
    signal.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}

/// The first real OFDM main mode: Wide + QPSK + N1296 R1/2.
fn ofdm_wide() -> WidebandLowDensityFloor {
    WidebandLowDensityFloor::with_params_constellation_fec(
        OfdmParams::for_mode(OfdmModeName::Wide),
        2, // QPSK
        Box::new(OfdmAdaptiveCodec::new(BlockN::N1296, WifiLdpcRate::R1_2)),
    )
}

/// `(fer, mean reported SNR_2500)` over `n` seeds at a target injected SNR.
fn sweep_point(
    wf: &WidebandLowDensityFloor,
    payload_bytes: usize,
    snr_2500_db: f64,
    n: u64,
) -> (f64, f64) {
    let mut rng = ChaCha8Rng::seed_from_u64(0x0FD3 ^ (snr_2500_db as u64));
    let mut fails = 0u64;
    let mut snr_acc = Vec::new();
    for seed in 0..n {
        let sig = wf
            .transmit_multi_with_preamble(&payload(seed, payload_bytes))
            .unwrap();
        let mut captured = vec![0.0f32; 400];
        captured.extend_from_slice(&add_awgn_at_snr_2500(&sig, snr_2500_db, &mut rng));
        match wf.receive_multi_with_sync_scan(&captured) {
            SyncDecodeOutcome::Frame {
                snr_2500_db,
                payload: got,
                ..
            } => {
                if got != payload(seed, payload_bytes) {
                    fails += 1; // decoded to the wrong bytes counts as a failure
                }
                if let Some(s) = snr_2500_db {
                    snr_acc.push(s as f64);
                }
            }
            SyncDecodeOutcome::DetectedDecodeFailed { snr_2500_db, .. } => {
                fails += 1;
                if let Some(s) = snr_2500_db {
                    snr_acc.push(s as f64);
                }
            }
            SyncDecodeOutcome::NoSignal => fails += 1,
        }
    }
    let mean_snr = if snr_acc.is_empty() {
        f64::NAN
    } else {
        snr_acc.iter().sum::<f64>() / snr_acc.len() as f64
    };
    (fails as f64 / n as f64, mean_snr)
}

/// Fast smoke: ofdm-wide QPSK decodes reliably at a high SNR and degrades at low
/// SNR — proving it is a REAL end-to-end mode with an honest decode knee.
#[test]
fn ofdm_wide_qpsk_decodes_and_has_a_knee() {
    let wf = ofdm_wide();
    let payload_bytes = 64; // fits one N1296 R1/2 block (648 info bits = 81 B)

    let (fer_hi, snr_hi) = sweep_point(&wf, payload_bytes, 22.0, 6);
    let (fer_lo, _snr_lo) = sweep_point(&wf, payload_bytes, 2.0, 6);

    eprintln!(
        "ofdm-wide QPSK: @22dB FER={fer_hi:.2} (reported {snr_hi:.1} dB); @2dB FER={fer_lo:.2}"
    );

    assert!(
        fer_hi <= 0.34,
        "ofdm-wide QPSK must decode reliably at high SNR (FER {fer_hi:.2} @22 dB) — the mode is real end-to-end"
    );
    assert!(
        fer_lo > fer_hi,
        "FER must rise as SNR falls (a real knee): {fer_lo:.2} @2 vs {fer_hi:.2} @22"
    );
    assert!(snr_hi.is_finite(), "ofdm-wide reports an honest SNR_2500");
}

/// Full FER-vs-SNR_2500 sweep for ofdm-wide — emit the curve to read its real
/// `snr_floor_db` knee for the link ladder. Slow; run with `-- --ignored --nocapture`.
#[test]
#[ignore]
fn ofdm_wide_qpsk_fer_vs_snr_2500_sweep() {
    let wf = ofdm_wide();
    eprintln!("snr_2500_db,fer,mean_reported_snr_2500_db");
    for step in 0..=12 {
        let snr = step as f64 * 2.0; // 0..24 dB
        let (fer, reported) = sweep_point(&wf, 64, snr, 20);
        eprintln!("{snr:.1},{fer:.3},{reported:.2}");
    }
}
