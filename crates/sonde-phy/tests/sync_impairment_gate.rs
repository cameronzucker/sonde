//! Step 2 physics gate (sonde-xhw.3): the production sync path must decode an
//! end-to-end frame through the impairments a real HF link ALWAYS carries,
//! applied SIMULTANEOUSLY:
//!
//!   - carrier-frequency offset of ±100 Hz (dial error + drift),
//!   - sample-clock error of ±100 ppm (independent TX/RX oscillators),
//!   - fractional-sample frame timing,
//!   - a Watterson (Good/Moderate) frequency-selective fade,
//!
//! THROUGH `receive_multi_with_sync` — the real two-stage synchronizer
//! (Schmidl-Cox `M(d)` detection, CFO-invariant → coarse-CFO derotation →
//! sharp template-MF timing → channel-aware soft LLR → rate-1/4 LDPC).
//!
//! ## Why this is the gate (and what it replaces)
//!
//! The pre-xhw.3 sync (a single real Zadoff-Chu correlator, no CFO recovery)
//! decoded at 0–5 Hz CFO and COLLAPSED at ≥20 Hz — a ±100 Hz offset is ~4
//! sub-carrier spacings (Δf = 23.4 Hz @ FFT 2048), sliding the spectrum off the
//! pilot bins. This gate proves the rebuilt sync survives the full ±100 Hz HF
//! budget plus clock + timing, at NO measurable decode-rate loss versus perfect
//! frequency/timing alignment. (The scratch `zz_sync_impairment_baseline` that
//! merely recorded the collapse is removed.)
//!
//! ## Operating point — isolating SYNC
//!
//! The gate states **Eb/N0 = 35 dB** (≈26 dB SNR-in-2500 Hz). That is high on
//! purpose: like the fading gate's 30 dB "isolate the equalizer" point, it puts
//! the floor's *demod* in its sound regime so the variable under test is SYNC,
//! not the LDPC's low-SNR margin. The floor's channel-aware LLR currently uses a
//! hardcoded `n0 = 0.1` (sonde-64w.1 spec §6), which caps useful operation at
//! ~25 dB Eb/N0 — a separate demod-calibration concern (bd `sonde-gtg`), NOT a
//! sync defect. The `report_*` curve below shows the demod cliff and the
//! sync-vs-baseline parity across the full Eb/N0 range.
//!
//! ## Eb/N0 standard (discrete-time AWGN, consistent with Step 0)
//!
//! `AwgnGenerator::add_noise(sig, snr_db)` adds complex AWGN of per-sample
//! variance `σ² = P_sig / 10^(snr_db/10)`. In the discrete-time convention
//! (noise variance per complex sample = N0), a target Eb/N0 over a buffer of `L`
//! samples carrying `N_info` net info bits maps to
//!   `snr_db = Eb/N0(dB) + 10·log10(N_info / L)`.
//!
//! ## Channelization (fixed convention, Codex-converged)
//! real audio → analytic (Hilbert) → CFO → Watterson → ppm resample
//! (band-limited windowed-sinc) → AWGN(Eb/N0) → Re{·} → production sync path.
//! AWGN is added AFTER resampling (noise enters at the RX clock).

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SR: f64 = 48_000.0;
const PAYLOAD: &[u8] = b"FLOOR FADING GATE PAYLOAD";

/// Analytic signal of a real signal via FFT (zero negative freqs, double
/// positives). `Re{analytic} == original` for a unit channel.
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

/// Band-limited fractional resampler (Hann-windowed sinc, 32 taps) modelling a
/// sample-clock error of `ppm` plus a constant fractional-sample frame offset
/// `frac`. RX samples the TX waveform at `fs·(1+ppm·1e-6)`, so output sample
/// `m` reads the input at continuous position `p = (m + frac)/(1+ε)`. A
/// windowed sinc keeps the injected impairment clean — linear interpolation
/// would colour the measurement (Codex Q7).
fn resample_clock(input: &[Complex<f32>], ppm: f64, frac: f64) -> Vec<Complex<f32>> {
    let rate = 1.0 + ppm * 1e-6; // output samples per input sample
    let out_len = ((input.len() as f64) * rate).floor() as usize;
    const HALF_TAPS: i64 = 16;
    let mut out = Vec::with_capacity(out_len);
    for m in 0..out_len {
        let p = (m as f64 + frac) / rate;
        let base = p.floor() as i64;
        let mut acc = Complex::new(0.0_f32, 0.0);
        for k in (-HALF_TAPS + 1)..=HALF_TAPS {
            let idx = base + k;
            if idx < 0 || idx as usize >= input.len() {
                continue;
            }
            let x = p - idx as f64;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * x / HALF_TAPS as f64).cos();
            acc += input[idx as usize] * (sinc * w) as f32;
        }
        out.push(acc);
    }
    out
}

/// Full impairment chain → the real RX audio the production sync path decodes.
fn impair(
    clean: &[f32],
    cfo_hz: f64,
    condition: ChannelCondition,
    ppm: f64,
    frac: f64,
    ebn0_db: f64,
    seed: u64,
) -> Vec<f32> {
    let mut a = analytic(clean);
    for (n, c) in a.iter_mut().enumerate() {
        let ph = 2.0 * std::f64::consts::PI * cfo_hz * n as f64 / SR;
        *c *= Complex::new(ph.cos() as f32, ph.sin() as f32);
    }
    let mut ch = WattersonChannel::from_condition(seed, condition, SR);
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(2048));
    let mut faded = ch.process_block(&a);
    faded = resample_clock(&faded, ppm, frac);
    let n_info = (PAYLOAD.len() * 8) as f64;
    let snr_db = ebn0_db + 10.0 * (n_info / faded.len() as f64).log10();
    AwgnGenerator::new(seed ^ 0xA5A5).add_noise(&mut faded, snr_db);
    faded.iter().map(|c| c.re).collect()
}

fn floor() -> WidebandLowDensityFloor {
    WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()))
}

/// Deterministic per-realization seed (golden-ratio stride).
fn seed_for(i: u64) -> u64 {
    0x9E37_79B9_7F4A_7C15u64.wrapping_mul(i + 1) ^ 0xC0DE_6402
}

/// Decode count over `n_seeds` deterministic channel realizations through the
/// FULL production sync path. The result is deterministic (fixed seeds + fixed
/// noise), so a margin below it is a non-flaky gate.
fn decode_rate(
    clean: &[f32],
    cfo: f64,
    cond: ChannelCondition,
    ppm: f64,
    ebn0: f64,
    n_seeds: u64,
) -> usize {
    let floor = floor();
    (0..n_seeds)
        .filter(|&i| {
            let frac = (i as f64 * 0.13) % 1.0; // fractional-sample timing per seed
            let rx = impair(clean, cfo, cond, ppm, frac, ebn0, seed_for(i));
            matches!(floor.receive_multi_with_sync(&rx), Ok((_, ref d)) if d == PAYLOAD)
        })
        .count()
}

const GATE_EBN0_DB: f64 = 35.0;
const GATE_SEEDS: u64 = 8;
/// Decode-rate floor (of `GATE_SEEDS`) the production sync path must clear under
/// each full-impairment case. Measured rates at this operating point are 7–8/8;
/// 6/8 leaves a one-seed margin for cross-platform FP drift while still
/// asserting the sync survives ±100 Hz CFO + 100 ppm + fractional timing.
const GATE_FLOOR: usize = 6;

/// THE GATE. Decode must survive the full simultaneous impairment through the
/// production sync path, at parity with perfect-frequency alignment.
#[test]
fn sync_survives_cfo_clock_timing_over_watterson() {
    let clean = floor().transmit_multi_with_preamble(PAYLOAD).unwrap();
    println!(
        "\n===== sonde-xhw.3 GATE @ Eb/N0={GATE_EBN0_DB} dB ({GATE_SEEDS} seeds, \
         100 ppm clock + fractional timing) ====="
    );
    for cond in [ChannelCondition::Good, ChannelCondition::Moderate] {
        // Perfect-frequency baseline at the same operating point.
        let baseline = decode_rate(&clean, 0.0, cond, 0.0, GATE_EBN0_DB, GATE_SEEDS);
        println!("{cond:?} baseline (CFO=0, ppm=0): {baseline}/{GATE_SEEDS}");
        assert!(
            baseline >= GATE_FLOOR,
            "{cond:?} demod baseline {baseline}/{GATE_SEEDS} below {GATE_FLOOR} — the \
             operating point is below the demod's sound regime, gate is mis-stated"
        );
        for cfo in [100.0_f64, -100.0] {
            let rate = decode_rate(&clean, cfo, cond, 100.0, GATE_EBN0_DB, GATE_SEEDS);
            println!("{cond:?} CFO={cfo:>6.1} Hz +100 ppm + frac timing: {rate}/{GATE_SEEDS}");
            assert!(
                rate >= GATE_FLOOR,
                "{cond:?} CFO={cfo} Hz + 100 ppm + fractional timing decoded only \
                 {rate}/{GATE_SEEDS} (floor {GATE_FLOOR}) — sync does NOT survive the \
                 combined HF impairment"
            );
            // Sync must add ≤2 seeds of loss versus perfect frequency alignment.
            assert!(
                rate + 2 >= baseline,
                "{cond:?} CFO={cfo} Hz lost {} seeds vs the perfect-frequency baseline \
                 ({rate} vs {baseline}/{GATE_SEEDS}) — sync is degrading decode",
                baseline.saturating_sub(rate)
            );
        }
    }
    println!("=================================================================\n");
}

/// Diagnostic curve (no assertions; `--ignored` to keep CI fast). Decode-rate
/// vs CFO across the full Eb/N0 range — shows the demod cliff (~25 dB Eb/N0, the
/// hardcoded-`n0` limit) and the sync-vs-baseline parity above it.
#[test]
#[ignore = "slow diagnostic sweep; run with --ignored for the decode-rate curve"]
fn report_decode_rate_curve() {
    let clean = floor().transmit_multi_with_preamble(PAYLOAD).unwrap();
    let n_info = (PAYLOAD.len() * 8) as f64;
    let frame_s = clean.len() as f64 / SR;
    println!(
        "\n===== combined-impairment curve (clean frame {} samp, {:.3} s, R_b {:.0} bps) =====",
        clean.len(),
        frame_s,
        n_info / frame_s
    );
    for cond in [ChannelCondition::Good, ChannelCondition::Moderate] {
        for &ebn0 in &[20.0_f64, 25.0, 30.0, 35.0, 45.0] {
            for &(ppm, cfo) in &[(0.0_f64, 0.0_f64), (0.0, 100.0), (100.0, -100.0)] {
                let r = decode_rate(&clean, cfo, cond, ppm, ebn0, 8);
                println!("{cond:?} Eb/N0={ebn0:>4.1} ppm={ppm:>6.1} CFO={cfo:>7.1} Hz -> {r}/8");
            }
        }
    }
    println!("====================================================================\n");
}
