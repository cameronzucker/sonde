//! Physics gate (sonde-99l.1): the PHY's link-facing channel-SNR report is
//! HONEST — the reported `SNR_2500` (channel SNR referenced to a 2500 Hz noise
//! bandwidth) tracks the SNR actually injected, not a loopback/sim number.
//!
//! Design: `docs/superpowers/specs/2026-06-15-phy-mode-adaptation-quality-design.md`
//! (Codex-converged). The reported reference is `SNR_2500`, NOT Eb/N0 — the link
//! adaptation ladder needs a mode-comparable channel number (Eb/N0 normalizes out
//! data rate). Eb/N0 stays the *gate* reference elsewhere (step3_coded_fading_gate).
//!
//! ## Why this convention is exact (no hidden 3 dB)
//! Inject real white noise of per-sample variance `σ²` to hit a TARGET true
//! `SNR_2500 = T` (linear). The signal is bandlimited to the occupied band
//! (< 2500 Hz), so all its power `S = mean(x²)` lies in any 2500 Hz band, while
//! white noise contributes `σ²·2500/(fs/2)` in that band. Hence
//! `T = S / (σ²·2500/(fs/2))` ⇒ `σ² = S·(fs/2) / (2500·T)`. The estimator
//! independently returns `S·fs / (2·σ²·2500)` — the real-mirror factor of 2
//! cancels — so reported should equal injected with slope 1.
//!
//! RADIO-1: nothing keyed. `sonde-fec` is a dev-only dependency (the real codec).

use hf_channel_sim::{ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::{
    SyncDecodeOutcome, WidebandLowDensityFloor,
};

const SR: f64 = 48_000.0;

/// Analytic (Hilbert) lift of a real signal: zero negatives, double positives.
fn analytic(real_sig: &[f32]) -> Vec<Complex<f32>> {
    let n = real_sig.len();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf: Vec<Complex<f32>> = real_sig.iter().map(|&x| Complex::new(x, 0.0)).collect();
    fwd.process(&mut buf);
    let half = n / 2;
    for (k, c) in buf.iter_mut().enumerate() {
        if k == 0 || (n % 2 == 0 && k == half) {
            // DC / Nyquist unchanged.
        } else if k < half {
            *c *= 2.0;
        } else {
            *c = Complex::new(0.0, 0.0);
        }
    }
    inv.process(&mut buf);
    let scale = 1.0 / n as f32;
    buf.iter().map(|c| c * scale).collect()
}

/// Pass a real signal through Watterson fading (analytic lift → process → real),
/// the same path the step-3 gate uses. Unit-power Watterson preserves signal power.
fn through_fade(clean: &[f32], condition: ChannelCondition, seed: u64) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, condition, SR);
    let mut a = analytic(clean);
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(2048));
    let faded = ch.process_block(&a);
    faded.iter().map(|c| c.re).collect()
}
const PAYLOAD_BYTES: usize = 58; // one FloorRate14 LDPC block (see step3 gate)

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn payload(seed: u64) -> Vec<u8> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..PAYLOAD_BYTES).map(|_| rng.gen()).collect()
}

/// Add real AWGN sized so the channel SNR referenced to 2500 Hz equals
/// `snr_2500_db`. `σ² = S·(fs/2) / (2500·T)` with `S = mean(x²)`.
fn add_awgn_at_snr_2500(signal: &[f32], snr_2500_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let n = signal.len() as f64;
    let s: f64 = signal.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n;
    let t = 10f64.powf(snr_2500_db / 10.0);
    let sigma2 = s * (SR / 2.0) / (2500.0 * t);
    let sigma = sigma2.sqrt() as f32;
    signal.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}

/// Reported `SNR_2500` for one captured frame, or `None` if not measurable.
fn reported_snr_2500(floor: &WidebandLowDensityFloor, captured: &[f32]) -> Option<f32> {
    match floor.receive_multi_with_sync_scan(captured) {
        SyncDecodeOutcome::Frame { snr_2500_db, .. } => snr_2500_db,
        SyncDecodeOutcome::DetectedDecodeFailed { snr_2500_db, .. } => snr_2500_db,
        SyncDecodeOutcome::NoSignal => None,
    }
}

/// Least-squares slope of `ys` against `xs`.
fn slope(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let num: f64 = xs.iter().zip(ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let den: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    num / den
}

/// AWGN: reported SNR_2500 tracks injected with slope ≈ 1 and tight absolute
/// calibration. This is the core honesty property — the report is a real
/// measurement, not a constant or a loopback number.
#[test]
fn reported_snr_2500_tracks_injected_awgn() {
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let mut rng = ChaCha8Rng::seed_from_u64(0xA5A5);
    let injected = [10.0_f64, 14.0, 18.0, 22.0, 26.0];

    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for &snr_db in &injected {
        // Average a few seeds per point to tame estimator variance.
        let mut acc = Vec::new();
        for seed in 0..4u64 {
            let sig = floor.transmit_multi_with_preamble(&payload(seed)).unwrap();
            let mut captured = vec![0.0f32; 400];
            captured.extend_from_slice(&add_awgn_at_snr_2500(&sig, snr_db, &mut rng));
            if let Some(r) = reported_snr_2500(&floor, &captured) {
                acc.push(r as f64);
            }
        }
        assert!(
            !acc.is_empty(),
            "no frame detected at injected SNR_2500 = {snr_db} dB"
        );
        let mean = acc.iter().sum::<f64>() / acc.len() as f64;
        // Absolute calibration: the convention is exact (see module docs), so a
        // generous ±3 dB catches a broken estimator / wrong bandwidth / 3 dB slip.
        assert!(
            (mean - snr_db).abs() <= 3.0,
            "reported {mean:.1} dB vs injected {snr_db:.1} dB (>3 dB off)"
        );
        xs.push(snr_db);
        ys.push(mean);
    }

    // Slope ≈ 1: 1 dB more injected SNR ⇒ ~1 dB more reported. The honesty core.
    let m = slope(&xs, &ys);
    assert!(
        (0.8..=1.2).contains(&m),
        "reported-vs-injected slope {m:.2} not ≈ 1 (report does not track the channel)"
    );
}

/// Watterson (H3): the report stays honest under fading — it still rises with
/// injected SNR (looser, since fading adds variance and notches depress the
/// average). Proves the number is not AWGN-only.
#[test]
fn reported_snr_2500_rises_with_injected_under_watterson() {
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let mut rng = ChaCha8Rng::seed_from_u64(0x1234);

    let measure = |snr_db: f64, rng: &mut ChaCha8Rng| -> f64 {
        let mut acc = Vec::new();
        for seed in 0..6u64 {
            let sig = floor.transmit_multi_with_preamble(&payload(seed)).unwrap();
            // Fade first, then add noise at the target SNR_2500 on the faded signal.
            let faded = through_fade(&sig, ChannelCondition::Moderate, seed);
            let mut captured = vec![0.0f32; 400];
            captured.extend_from_slice(&add_awgn_at_snr_2500(&faded, snr_db, rng));
            if let Some(r) = reported_snr_2500(&floor, &captured) {
                acc.push(r as f64);
            }
        }
        assert!(
            !acc.is_empty(),
            "no frame detected at {snr_db} dB (Watterson)"
        );
        acc.iter().sum::<f64>() / acc.len() as f64
    };

    let low = measure(12.0, &mut rng);
    let high = measure(26.0, &mut rng);
    assert!(
        high > low + 3.0,
        "reported SNR did not rise under fading: {low:.1} dB @12 vs {high:.1} dB @26"
    );
}
