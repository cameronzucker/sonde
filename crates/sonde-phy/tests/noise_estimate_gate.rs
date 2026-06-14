//! Physics gate (sonde-gtg): the per-symbol noise estimate must EXTEND the
//! floor's decode envelope toward lower Eb/N0 versus the legacy hardcoded
//! `n0 = 0.1`. Differential, on identical Watterson + AWGN seeds: the
//! estimated-`n0` demod decodes more reliably than the fixed-`0.1` demod.
//!
//! ## Why differential (gate on physics)
//!
//! sonde-xhw.3 measured the floor's demod cliff at ~Eb/N0 25 dB with the fixed
//! `n0=0.1`; below it the channel-aware LLR magnitudes are mis-scaled and the
//! rate-1/4 LDPC fails. Replacing the constant with a measured per-bin effective
//! noise (`n0_thermal` from the empty bins + a pilot-curvature / channel-estimate
//! -error term) tracks the real operating SNR. A differential gate — estimated
//! vs fixed on the SAME channel/noise — isolates the estimator's contribution
//! from the equalizer and sync (both identical across arms). The fixed arm is the
//! production demod with `with_fixed_n0(0.1)`. The improvement is gradual (a
//! ~2–3 dB shift of the Good 50%-decode point; pooled 16/16 vs 11/16 at 25 dB),
//! not a cliff-collapse — deeper low-SNR gains are pilot-density-limited (a null
//! narrower than the 4-bin pilot grid is under-sampled), tracked separately.
//!
//! ## Eb/N0 standard (discrete-time AWGN, consistent with Step 0/xhw.3)
//! `snr_db_arg = Eb/N0(dB) + 10·log10(N_info / buffer_len)`.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SR: f64 = 48_000.0;
const PAYLOAD: &[u8] = b"FLOOR FADING GATE PAYLOAD";
/// The legacy fixed noise variance the estimate replaces.
const LEGACY_N0: f32 = 0.1;

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

/// real audio → analytic → Watterson(condition) → AWGN(Eb/N0) → real audio.
/// No CFO/clock offset is injected — sonde-xhw.3 owns sync, so this gate
/// isolates the demod's noise scaling. Decode still runs through the full
/// production path (`receive_multi_with_sync`); at zero offset acquisition is
/// trivial, leaving the per-bin LLR noise model as the only variable.
fn through_channel(
    clean: &[f32],
    condition: ChannelCondition,
    ebn0_db: f64,
    seed: u64,
) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, condition, SR);
    let mut a = analytic(clean);
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(2048));
    let mut faded = ch.process_block(&a);
    let n_info = (PAYLOAD.len() * 8) as f64;
    let snr_db = ebn0_db + 10.0 * (n_info / faded.len() as f64).log10();
    AwgnGenerator::new(seed ^ 0xA5A5).add_noise(&mut faded, snr_db);
    faded.iter().map(|c| c.re).collect()
}

fn seed_for(i: u64) -> u64 {
    0x9E37_79B9_7F4A_7C15u64.wrapping_mul(i + 1) ^ 0xC0DE_6402
}

/// Decode count over `n` seeds through the production sync path, with either the
/// estimated `n0` (`fixed=None`) or the legacy fixed `n0` (`fixed=Some(0.1)`).
fn decode_rate(
    clean: &[f32],
    cond: ChannelCondition,
    ebn0: f64,
    n: u64,
    fixed_n0: Option<f32>,
) -> usize {
    let floor = match fixed_n0 {
        Some(n0) => {
            WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new())).with_fixed_n0(n0)
        }
        None => WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new())),
    };
    (0..n)
        .filter(|&i| {
            let rx = through_channel(clean, cond, ebn0, seed_for(i));
            matches!(floor.receive_multi_with_sync(&rx), Ok((_, ref d)) if d == PAYLOAD)
        })
        .count()
}

const SEEDS: u64 = 8;
/// Differential operating point. The legacy fixed `n0=0.1` cliff is ~25 dB
/// (sonde-xhw.3); here both arms are partially up, so the gap is the estimate's
/// contribution rather than a coincidental all-or-nothing seed split.
const GATE_EBN0_DB: f64 = 25.0;

/// THE GATE. At a representative HF operating point, the per-symbol noise
/// estimate must decode MORE reliably than the legacy fixed `n0=0.1` on
/// identical Watterson + AWGN seeds — pooled over Good + Moderate so the result
/// is a decode-rate improvement, not one lucky condition. Measured: estimated
/// 16/16 vs fixed 11/16 at 25 dB; the `#[ignore]`d curve shows the full shift.
#[test]
fn estimated_n0_extends_decode_envelope_vs_fixed() {
    let clean = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()))
        .transmit_multi_with_preamble(PAYLOAD)
        .unwrap();

    let mut est_pool = 0;
    let mut fixed_pool = 0;
    for cond in [ChannelCondition::Good, ChannelCondition::Moderate] {
        let est = decode_rate(&clean, cond, GATE_EBN0_DB, SEEDS, None);
        let fixed = decode_rate(&clean, cond, GATE_EBN0_DB, SEEDS, Some(LEGACY_N0));
        println!(
            "{cond:?} @ Eb/N0={GATE_EBN0_DB} dB: estimated-n0 {est}/{SEEDS}  vs  \
             fixed-0.1 {fixed}/{SEEDS}"
        );
        est_pool += est;
        fixed_pool += fixed;
    }
    let pool = 2 * SEEDS as usize;
    println!(
        "POOLED (Good+Moderate): estimated {est_pool}/{pool}  vs  fixed-0.1 {fixed_pool}/{pool}"
    );
    // The estimate must decode a strong majority...
    assert!(
        est_pool >= 14,
        "estimated-n0 decoded only {est_pool}/{pool} at {GATE_EBN0_DB} dB — envelope not extended"
    );
    // ...and beat the fixed-n0 control by ≥3 pooled seeds (the operating-Eb/N0 win).
    assert!(
        est_pool >= fixed_pool + 3,
        "estimated-n0 ({est_pool}/{pool}) did not clearly beat fixed-0.1 \
         ({fixed_pool}/{pool}) at {GATE_EBN0_DB} dB — the noise estimate buys nothing"
    );
}

/// Diagnostic curve (no assertions; `--ignored`). Decode-rate vs Eb/N0 for both
/// arms — shows the cliff shift the estimate buys.
#[test]
#[ignore = "slow diagnostic sweep; run with --ignored for the cliff-shift curve"]
fn report_n0_cliff_shift() {
    let clean = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()))
        .transmit_multi_with_preamble(PAYLOAD)
        .unwrap();
    println!("\n===== sonde-gtg n0 cliff shift (estimated vs fixed-0.1, {SEEDS} seeds) =====");
    for cond in [ChannelCondition::Good, ChannelCondition::Moderate] {
        for &ebn0 in &[14.0_f64, 16.0, 18.0, 20.0, 22.0, 25.0, 30.0] {
            let est = decode_rate(&clean, cond, ebn0, SEEDS, None);
            let fixed = decode_rate(&clean, cond, ebn0, SEEDS, Some(LEGACY_N0));
            println!("{cond:?} Eb/N0={ebn0:>4.1} dB -> estimated {est}/{SEEDS}  fixed-0.1 {fixed}/{SEEDS}");
        }
    }
    println!("==========================================================================\n");
}
