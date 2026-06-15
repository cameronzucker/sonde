//! sonde-99l.3(a): the wideband floor's FER-vs-SNR_2500 decode threshold, measured
//! in the SAME reference the runtime reports (`SNR_2500`) and with the SAME
//! estimator — an "estimator-domain knee" (Codex review C5). This is the real,
//! honest `snr_floor_db` the link bakes into its floor ladder rung, replacing the
//! illustrative placeholder. Gate on physics, not artifacts.
//!
//! The floor occupies ~2.3 kHz (≈ the 2.5 kHz reference), so its `SNR_2500` is
//! close to its per-tone SNR. The full sweep is `#[ignore]`d (slow LDPC); a fast
//! smoke proves the knee is real (high SNR decodes, low SNR does not) and records
//! the reported `SNR_2500` either side of it.
//!
//! RADIO-1: nothing keyed. `sonde-fec` is a dev-only dependency (the real codec).

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::{
    SyncDecodeOutcome, WidebandLowDensityFloor,
};

const SR: f64 = 48_000.0;
const PAYLOAD_BYTES: usize = 58; // one FloorRate14 LDPC block

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn payload(seed: u64) -> Vec<u8> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..PAYLOAD_BYTES).map(|_| rng.gen()).collect()
}

/// Add AWGN sized so the channel SNR referenced to 2500 Hz equals `snr_2500_db`
/// (same convention as `phy_quality_reporting_gate`).
fn add_awgn_at_snr_2500(signal: &[f32], snr_2500_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let n = signal.len() as f64;
    let s: f64 = signal.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n;
    let t = 10f64.powf(snr_2500_db / 10.0);
    let sigma2 = s * (SR / 2.0) / (2500.0 * t);
    let sigma = sigma2.sqrt() as f32;
    signal.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}

/// `(decoded?, reported SNR_2500)` for one captured frame at a target SNR.
fn trial(
    floor: &WidebandLowDensityFloor,
    snr_2500_db: f64,
    seed: u64,
    rng: &mut ChaCha8Rng,
) -> (bool, Option<f32>) {
    let sig = floor.transmit_multi_with_preamble(&payload(seed)).unwrap();
    let mut captured = vec![0.0f32; 400];
    captured.extend_from_slice(&add_awgn_at_snr_2500(&sig, snr_2500_db, rng));
    match floor.receive_multi_with_sync_scan(&captured) {
        SyncDecodeOutcome::Frame { snr_2500_db, .. } => (true, snr_2500_db),
        SyncDecodeOutcome::DetectedDecodeFailed { snr_2500_db, .. } => (false, snr_2500_db),
        SyncDecodeOutcome::NoSignal => (false, None),
    }
}

/// `(fer, mean reported SNR_2500)` over `n` seeds at a target injected SNR.
fn sweep_point(floor: &WidebandLowDensityFloor, snr_2500_db: f64, n: u64) -> (f64, f64) {
    let mut rng = ChaCha8Rng::seed_from_u64(0xF100 ^ (snr_2500_db as u64));
    let mut fails = 0u64;
    let mut snr_acc = Vec::new();
    for seed in 0..n {
        let (ok, snr) = trial(floor, snr_2500_db, seed, &mut rng);
        if !ok {
            fails += 1;
        }
        if let Some(s) = snr {
            snr_acc.push(s as f64);
        }
    }
    let mean_snr = if snr_acc.is_empty() {
        f64::NAN
    } else {
        snr_acc.iter().sum::<f64>() / snr_acc.len() as f64
    };
    (fails as f64 / n as f64, mean_snr)
}

/// Fast smoke: a clear decode knee exists in `SNR_2500` — well above it the floor
/// decodes (FER low), well below it fails (FER high). Records the reported SNR at
/// both points so the link's floor `snr_floor_db` can be read off the curve.
#[test]
fn floor_decode_knee_exists_in_snr_2500() {
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));

    // Above the knee: comfortable margin should decode reliably.
    let (fer_hi, snr_hi) = sweep_point(&floor, 16.0, 6);
    // Well below: starved channel should mostly fail.
    let (fer_lo, snr_lo) = sweep_point(&floor, 0.0, 6);

    eprintln!(
        "floor knee smoke: @16 dB FER={fer_hi:.2} (reported {snr_hi:.1} dB); \
         @0 dB FER={fer_lo:.2} (reported {snr_lo:.1} dB)"
    );

    assert!(
        fer_hi <= 0.34,
        "floor should decode reliably above the knee (FER {fer_hi:.2} @16 dB)"
    );
    assert!(
        fer_lo > fer_hi,
        "FER must rise as SNR_2500 falls (knee is real): {fer_lo:.2} @0 vs {fer_hi:.2} @16"
    );
    // The reported SNR must itself be honest at the measurement points.
    assert!(
        snr_hi.is_finite() && (snr_hi - 16.0).abs() <= 3.5,
        "reported {snr_hi:.1} ~ 16 dB"
    );
}

/// Full FER-vs-SNR_2500 sweep — emits the curve to read the floor's real
/// `snr_floor_db` knee for the link ladder. Slow (LDPC); run explicitly:
/// `cargo test -p sonde-phy --test floor_threshold_sweep -- --ignored --nocapture`.
#[test]
#[ignore]
fn floor_fer_vs_snr_2500_sweep() {
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    eprintln!("snr_2500_db,fer,mean_reported_snr_2500_db");
    let mut knee = None;
    for step in 0..=16 {
        let snr = step as f64; // 0..16 dB
        let (fer, reported) = sweep_point(&floor, snr, 20);
        eprintln!("{snr:.1},{fer:.3},{reported:.2}");
        if knee.is_none() && fer <= 0.1 {
            knee = Some(snr);
        }
    }
    if let Some(k) = knee {
        eprintln!("# floor FER<=0.1 knee at injected SNR_2500 ≈ {k:.1} dB");
    }
}
