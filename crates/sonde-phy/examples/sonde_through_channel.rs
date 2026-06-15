//! Sonde's REAL robust mode through the REAL Watterson channel (sonde-imh).
//!
//! A runnable, honest Sonde-vs-channel measurement for the demo comparison. It
//! drives the *production receive chain* — real Schmidl-Cox sync (xhw.3) + per-bin
//! n0 estimate (sonde-gtg) + adaptive pilot time-smoothing (sonde-vb9) + rate-1/4
//! LDPC (sonde-fec) — exactly the path the Step-3 physics gate
//! (`tests/step3_coded_fading_gate.rs`) asserts on. That gate is the AUTHORITATIVE
//! source; this example just makes the same measurement runnable as a CSV.
//!
//! It REPLACES an earlier version of this file that called the naive per-symbol
//! `modulate_one_symbol`/`demodulate_one_symbol` primitives — which bypass sync,
//! channel estimation and FEC, and therefore badly understated the modem (a
//! "gate-on-physics, not artifacts" own-goal). Eb/N0 is the honest energy-per-LDPC-
//! info-bit scale, calibrated to BPSK theory by the gate's calibration test.
//!
//! DSP only: modulates/demodulates audio-band SAMPLES in memory. It does NOT touch
//! sonde-tx or any PTT/rig crate — nothing can key a radio (RADIO-1 safe).
//!
//! Run: `cargo run --release --example sonde_through_channel -p sonde-phy`

use hf_channel_sim::{ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SR: f64 = 48_000.0;
const PAYLOAD_BYTES: usize = 58; // 58 B + 16-bit len = 480 bits = one FloorRate14 block
const K_INFO: usize = 480;
const SEEDS: u64 = 6; // frames per sweep point (each decode ~1.5 s)

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn payload(seed: u64) -> Vec<u8> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..PAYLOAD_BYTES).map(|_| rng.gen()).collect()
}

/// Real AWGN at a TRUE Eb/N0 (energy per LDPC info bit) from MEASURED signal power.
fn add_awgn(signal: &[f32], ebn0_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let e_signal: f64 = signal.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    let sigma = (e_signal / (2.0 * K_INFO as f64 * 10f64.powf(ebn0_db / 10.0))).sqrt() as f32;
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

/// real audio → analytic → Watterson(condition) → real audio (caller adds AWGN).
fn through_fade(clean: &[f32], condition: ChannelCondition, seed: u64) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, condition, SR);
    let mut a = analytic(clean);
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(2048));
    ch.process_block(&a).iter().map(|c| c.re).collect()
}

fn seed_for(i: u64) -> u64 {
    0x9E37_79B9_7F4A_7C15u64.wrapping_mul(i + 1) ^ 0xC0DE_6402
}

/// Coded frame-decode successes over `SEEDS` seeds through the production sync path.
/// `cond = None` = AWGN only (Ideal); `Some(c)` = that Watterson condition.
fn coded_decodes(cond: Option<ChannelCondition>, ebn0_db: f64) -> u64 {
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    (0..SEEDS)
        .filter(|&i| {
            let pl = payload(seed_for(i));
            let clean = floor.transmit_multi_with_preamble(&pl).unwrap();
            let faded = match cond {
                Some(c) => through_fade(&clean, c, seed_for(i)),
                None => clean,
            };
            let mut rng = ChaCha8Rng::seed_from_u64(seed_for(i) ^ 0xA5A5);
            let rx = add_awgn(&faded, ebn0_db, &mut rng);
            matches!(floor.receive_multi_with_sync(&rx), Ok((_, ref d)) if *d == pl)
        })
        .count() as u64
}

fn main() {
    println!("# Sonde robust floor (rate-1/4 LDPC) through hf-channel-sim, REAL receive chain");
    println!("# sample_rate_hz = {SR}, payload = {PAYLOAD_BYTES} B, {SEEDS} frames/point");
    println!("condition,ebn0_db,frames_ok,frames,success_rate");
    let conditions: [(&str, Option<ChannelCondition>); 4] = [
        ("ideal", None),
        ("good", Some(ChannelCondition::Good)),
        ("moderate", Some(ChannelCondition::Moderate)),
        ("poor", Some(ChannelCondition::Poor)),
    ];
    for (cname, cond) in conditions {
        for ebn0_db in [2.0_f64, 5.0, 8.0, 11.0] {
            let ok = coded_decodes(cond, ebn0_db);
            println!(
                "{cname},{ebn0_db},{ok},{SEEDS},{:.3}",
                ok as f64 / SEEDS as f64
            );
        }
    }
}
