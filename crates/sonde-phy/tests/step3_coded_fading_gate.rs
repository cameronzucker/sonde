//! Step-3 physics gate (sonde-xhw.4): coded mode validated over realistic
//! fading, with an HONEST stated Eb/N0 and a coded-vs-uncoded coding-gain curve.
//!
//! Composes the whole receive chain — real Schmidl-Cox sync (xhw.3) + per-bin n0
//! estimate (sonde-gtg) + adaptive pilot time-smoothing (sonde-vb9) + rate-1/4
//! LDPC (sonde-fec) — and proves, gate-on-physics:
//!
//! - **Gate A** (`gate_a_*`): frame success-rate over Watterson Good/Moderate/Poor
//!   at a STATED true Eb/N0, through the production sync path.
//! - **Gate B** (`gate_b_*`): coded-vs-uncoded coding gain in dB over AWGN.
//!
//! ## Honest Eb/N0 (resolves the sonde-xhw.5 ~6.7 dB over-statement)
//!
//! Eb/N0 is energy per LDPC info bit, injected from MEASURED received power:
//! `σ² = E_signal / (2·K_info·10^(Eb/N0_dB/10))`, `K_info = n_blocks·block_info_bits`.
//! The legacy harness used `K_info = payload_bits` and complex AWGN (missing the
//! real-cast factor of 2): `10log10(2·480/200) = 6.8 dB` — exactly the inflation.
//! `eb_n0_calibration_matches_bpsk_theory` validates this scale against
//! `Q(√(2Eb/N0))` on a bare BPSK link (Watterson curves are NOT expected to match
//! the AWGN Q-law — fading changes the BER law).
//!
//! Full sweeps are `#[ignore]`d (slow: ~1.5 s/decode); a fast calibrated smoke
//! runs in CI. `sonde-fec` is a dev-only dependency here (the real codec).

use hf_channel_sim::{ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::coded_modulation::FecCodec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SR: f64 = 48_000.0;
/// 58 B payload + 16-bit frame length header = 480 bits = exactly one
/// FloorRate14 LDPC block (k). One block keeps FER clean (no multi-block CRC
/// compounding) and K_info = block_info_bits = 480.
const PAYLOAD_BYTES: usize = 58;
const K_INFO: usize = 480;

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn payload(seed: u64) -> Vec<u8> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..PAYLOAD_BYTES).map(|_| rng.gen()).collect()
}

/// Add real AWGN at a TRUE Eb/N0 (energy per LDPC info bit) from MEASURED signal
/// power: `σ² = E_signal / (2·K_info·10^(Eb/N0/10))`.
fn add_awgn(signal: &[f32], ebn0_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let e_signal: f64 = signal.iter().map(|&x| (x as f64) * (x as f64)).sum();
    let sigma2 = e_signal / (2.0 * K_INFO as f64 * 10f64.powf(ebn0_db / 10.0));
    let sigma = sigma2.sqrt() as f32;
    signal.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}

/// Analytic (Hilbert) lift of a real signal: zero negatives, double positives.
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

/// real audio → analytic → Watterson(condition) → real audio. NO noise (the
/// caller adds calibrated AWGN). Unit-power Watterson ⇒ measured received power
/// ≈ transmitted, so the Eb/N0 reference holds.
fn through_fade(clean: &[f32], condition: ChannelCondition, seed: u64) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, condition, SR);
    let mut a = analytic(clean);
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(2048));
    let faded = ch.process_block(&a);
    faded.iter().map(|c| c.re).collect()
}

fn coded_floor() -> WidebandLowDensityFloor {
    WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()))
}

fn seed_for(i: u64) -> u64 {
    0x9E37_79B9_7F4A_7C15u64.wrapping_mul(i + 1) ^ 0xC0DE_6402
}

/// Coded frame-decode count over `n` seeds through the production sync path over
/// `cond` fading at true `ebn0_db`.
fn coded_fade_decodes(cond: ChannelCondition, ebn0_db: f64, n: u64) -> usize {
    let floor = coded_floor();
    (0..n)
        .filter(|&i| {
            let pl = payload(seed_for(i));
            let clean = floor.transmit_multi_with_preamble(&pl).unwrap();
            let faded = through_fade(&clean, cond, seed_for(i));
            let mut rng = ChaCha8Rng::seed_from_u64(seed_for(i) ^ 0xA5A5);
            let rx = add_awgn(&faded, ebn0_db, &mut rng);
            matches!(floor.receive_multi_with_sync(&rx), Ok((_, ref d)) if *d == pl)
        })
        .count()
}

// ─── Eb/N0 calibration: bare BPSK must match Q(√(2Eb/N0)) ────────────────────

/// Q(x) = 0.5·erfc(x/√2) via Abramowitz-Stegun 7.1.26.
fn q_func(x: f64) -> f64 {
    let z = x / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * z.abs());
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erfc = poly * (-z * z).exp();
    if x >= 0.0 {
        0.5 * erfc
    } else {
        1.0 - 0.5 * erfc
    }
}

#[test]
fn eb_n0_calibration_matches_bpsk_theory() {
    // Bare BPSK over the SAME measured-power AWGN injection (K_info = n bits, so
    // E_signal = n·1 and σ² = 1/(2·Eb/N0)) must track Q(√(2Eb/N0)) within ~1 dB.
    // This validates the honest Eb/N0 scale used by both gates (sonde-xhw.5).
    let n_bits = 2_000_000usize;
    let mut rng = ChaCha8Rng::seed_from_u64(0x00B9_5EE0);
    for ebn0_db in [2.0_f64, 4.0, 6.0, 8.0] {
        let ebn0 = 10f64.powf(ebn0_db / 10.0);
        let sigma = (1.0 / (2.0 * ebn0)).sqrt() as f32; // E_signal/n = 1
        let mut err = 0usize;
        for _ in 0..n_bits {
            let y = 1.0 + sigma * gaussian(&mut rng); // bit 0 → +1
            if y < 0.0 {
                err += 1;
            }
        }
        let measured = err as f64 / n_bits as f64;
        let theory = q_func((2.0 * ebn0).sqrt());
        // dB error: how far the measured BER sits from theory horizontally is
        // hard; compare BER ratio within a factor consistent with ~1 dB.
        let ratio = measured / theory;
        println!(
            "Eb/N0={ebn0_db} dB: measured BER {measured:.3e} vs theory {theory:.3e} (×{ratio:.2})"
        );
        assert!(
            ratio > 0.5 && ratio < 2.0,
            "bare-BPSK BER {measured:.3e} not within ~1 dB of theory {theory:.3e} at {ebn0_db} dB \
             — Eb/N0 scale is miscalibrated"
        );
    }
}

// ─── Gate A: success-rate over Watterson Good/Moderate/Poor ──────────────────

const GATE_A_EBN0_DB: f64 = 20.0; // stated TRUE Eb/N0 (Codex-converged)
const GATE_A_SEEDS: u64 = 16;

#[test]
#[ignore = "slow full sweep; run with --ignored --nocapture"]
fn gate_a_fading_fer_sweep() {
    println!("\n=== Gate A: coded FER over Watterson, TRUE Eb/N0, {GATE_A_SEEDS} seeds ===");
    for cond in [
        ChannelCondition::Good,
        ChannelCondition::Moderate,
        ChannelCondition::Poor,
    ] {
        let row: Vec<String> = [14.0_f64, 16.0, 18.0, 20.0]
            .iter()
            .map(|&e| {
                let ok = coded_fade_decodes(cond, e, GATE_A_SEEDS);
                format!("{:>2}dB:{ok}/{GATE_A_SEEDS}", e as i32)
            })
            .collect();
        println!("{cond:?}: {}", row.join("  "));
    }
    // THE GATE: at the stated operating point, every condition decodes a strong
    // majority of seeds (Poor is allowed one extra failure — it cliffs last).
    for cond in [
        ChannelCondition::Good,
        ChannelCondition::Moderate,
        ChannelCondition::Poor,
    ] {
        let ok = coded_fade_decodes(cond, GATE_A_EBN0_DB, GATE_A_SEEDS);
        let floor_n = if matches!(cond, ChannelCondition::Poor) {
            14
        } else {
            15
        };
        assert!(
            ok >= floor_n,
            "Gate A: {cond:?} decoded {ok}/{GATE_A_SEEDS} at true Eb/N0={GATE_A_EBN0_DB} dB \
             (need ≥{floor_n})"
        );
    }
    println!("=== Gate A PASS ===\n");
}

#[test]
fn gate_a_smoke() {
    // Fast CI subset: a few fixed seeds per condition at the stated Eb/N0 must
    // decode. Proves the composed coded-over-fading path is alive without the
    // full 48-decode sweep.
    for cond in [
        ChannelCondition::Good,
        ChannelCondition::Moderate,
        ChannelCondition::Poor,
    ] {
        let ok = coded_fade_decodes(cond, GATE_A_EBN0_DB, 3);
        assert!(
            ok >= 2,
            "Gate A smoke: {cond:?} decoded {ok}/3 at true Eb/N0={GATE_A_EBN0_DB} dB"
        );
    }
}

// ─── Gate B: coded-vs-uncoded coding gain over AWGN (codec level) ────────────
//
// The "LDPC coding gain in dB" is an AWGN-waterfall concept (over a fade the gain
// is ~unbounded at a null — see the design doc). Measured at the codec level with
// textbook LLRs so it is the CODE's intrinsic gain, free of OFDM overhead: coded
// post-FEC BER (decode-anyway) vs uncoded BPSK BER on a shared Eb/N0 axis. That
// this gain SURVIVES the PHY over fading is what Gate A proves.

/// One AWGN point at true `ebn0_db`: (coded post-FEC BER, uncoded BPSK BER) over
/// `frames` codewords. Coded symbols sit at Es/N0 = R·Eb/N0 (rate 1/4); uncoded
/// BPSK at Es=Eb. Both use textbook BPSK LLRs `2y/σ²`.
fn gate_b_point(ebn0_db: f64, frames: u64) -> (f64, f64) {
    let codec = FloorRate14Codec::new();
    let ebn0 = 10f64.powf(ebn0_db / 10.0);
    let coded_s2 = (1.0 / (2.0 * 0.25 * ebn0)) as f32; // Es = R·Eb, R = 1/4
    let unc_s2 = (1.0 / (2.0 * ebn0)) as f32; // uncoded BPSK: Es = Eb
    let mut rng = ChaCha8Rng::seed_from_u64(0x0B60_u64 ^ ebn0_db.to_bits());
    let (mut c_bit_err, mut u_bit_err) = (0usize, 0usize);
    for i in 0..frames {
        // Coded: random info block → encode → BPSK+AWGN at Es/N0 → decode-anyway.
        let mut br = ChaCha8Rng::seed_from_u64(seed_for(i));
        let info: Vec<u8> = (0..K_INFO).map(|_| br.gen_range(0u8..2)).collect();
        let cw = codec.encode(&info);
        let llr: Vec<f32> = cw
            .iter()
            .map(|&b| {
                let x = if b == 0 { 1.0 } else { -1.0 };
                let y = x + coded_s2.sqrt() * gaussian(&mut rng);
                2.0 * y / coded_s2
            })
            .collect();
        let dec = codec.decode_soft_payload_unchecked(&llr);
        c_bit_err += dec.iter().zip(info.iter()).filter(|(a, b)| a != b).count();

        // Uncoded BPSK reference: K_INFO bits at Eb/N0.
        for _ in 0..K_INFO {
            let y = 1.0 + unc_s2.sqrt() * gaussian(&mut rng);
            if y < 0.0 {
                u_bit_err += 1;
            }
        }
    }
    let denom = (frames as usize * K_INFO) as f64;
    (c_bit_err as f64 / denom, u_bit_err as f64 / denom)
}

#[test]
#[ignore = "slow full sweep; run with --ignored --nocapture"]
fn gate_b_coding_gain_sweep() {
    println!("\n=== Gate B: LDPC coding gain over AWGN (codec, true Eb/N0) ===");
    println!(
        "{:>6} | {:>14} | {:>14}",
        "Eb/N0", "coded post-BER", "uncoded BER"
    );
    let frames = 60u64;
    let mut rows = Vec::new();
    for &e in &[1.0_f64, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] {
        let (cber, uber) = gate_b_point(e, frames);
        println!("{e:>6.1} | {cber:>14.2e} | {uber:>14.2e}");
        rows.push((e, cber, uber));
    }
    let gain = coding_gain_db(&rows, 1e-3);
    println!("CODING GAIN @ BER=1e-3: {gain:.1} dB");
    assert!(
        gain >= 4.0,
        "LDPC coding gain {gain:.1} dB < 4 dB at BER=1e-3 — code/LLR regressed"
    );
    println!("=== Gate B PASS ===\n");
}

/// Horizontal Eb/N0 gap (dB) between uncoded BER and coded post-FEC BER at
/// `ref_ber`, via linear interpolation of Eb/N0 vs log10(BER).
fn coding_gain_db(rows: &[(f64, f64, f64)], ref_ber: f64) -> f64 {
    let coded: Vec<(f64, f64)> = rows.iter().map(|&(e, c, _)| (e, c)).collect();
    let uncoded: Vec<(f64, f64)> = rows.iter().map(|&(e, _, u)| (e, u)).collect();
    ebn0_at_ber(&uncoded, ref_ber) - ebn0_at_ber(&coded, ref_ber)
}

/// Eb/N0 (dB) at which a monotonically-decreasing BER curve crosses `ref_ber`.
/// Linear in (Eb/N0, log10 BER) between bracketing finite points. A steep code
/// waterfall can hit BER = 0 (no errors in the sample) before the next swept
/// point; in that window the true crossing is at most the upper Eb/N0, so we
/// return it (a CONSERVATIVE / lower-bound coding-gain estimate, never inflated).
fn ebn0_at_ber(curve: &[(f64, f64)], ref_ber: f64) -> f64 {
    let lref = ref_ber.log10();
    for w in curve.windows(2) {
        let (e0, b0) = w[0];
        let (e1, b1) = w[1];
        // Looking for the window where BER drops through ref (b0 > ref ≥ b1).
        if b0 > ref_ber && b1 <= ref_ber {
            if b1 <= 0.0 {
                return e1; // hit the measurement floor; crossing ≤ e1
            }
            let (l0, l1) = (b0.log10(), b1.log10());
            return e0 + (e1 - e0) * (lref - l0) / (l1 - l0);
        }
    }
    f64::NAN
}

#[test]
fn gate_b_smoke() {
    // Fast CI subset: at Eb/N0 = 4 dB the coded link is already essentially clean
    // while uncoded BPSK BER is ~1e-2 — i.e. the coding gain exists.
    let (cber, uber) = gate_b_point(4.0, 8);
    assert!(
        cber < 1e-3,
        "Gate B smoke: coded post-FEC BER {cber:.2e} too high at 4 dB"
    );
    assert!(
        uber > 5e-3,
        "Gate B smoke: uncoded BER {uber:.2e} unexpectedly low at 4 dB — gain unclear"
    );
}
