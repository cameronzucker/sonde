//! Step 1 characterization (sonde-xhw.2): measure the CURRENT emitted floor
//! waveform's PSD, occupied bandwidth, out-of-band/image energy, and PAPR —
//! BEFORE changing the transmitter — so the fix targets the real defect, not an
//! assumed one. Prints numbers; the pass/fail PSD-mask gate is set from reality.
//!
//! SSB audio passband target: ~300–2700 Hz at 48 kHz. A real signal's spectrum
//! is conjugate-symmetric, so we measure the positive-frequency half (0–24 kHz)
//! and look for energy OUTSIDE the intended band (out-of-band spurs / images)
//! and the −26 dBc occupied bandwidth.

use num_complex::Complex;
use rustfft::FftPlanner;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SR: f32 = 48_000.0;

fn psd_db(signal: &[f32]) -> Vec<f32> {
    // Hann-windowed periodogram over the whole signal (single segment).
    let n = signal.len();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut buf: Vec<Complex<f32>> = signal
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            Complex::new(x * w, 0.0)
        })
        .collect();
    fft.process(&mut buf);
    let half = n / 2;
    let psd: Vec<f32> = buf[..half].iter().map(|c| c.norm_sqr()).collect();
    let peak = psd.iter().cloned().fold(0.0_f32, f32::max).max(1e-30);
    psd.iter().map(|p| 10.0 * (p / peak).log10()).collect()
}

fn bin_to_hz(bin: usize, n: usize) -> f32 {
    bin as f32 * SR / n as f32
}

#[test]
fn characterize_current_floor_waveform() {
    let floor = WidebandLowDensityFloor::new();
    let payload: Vec<u8> = (0..120).map(|i| (i * 7 % 251) as u8).collect();
    let sig = floor.transmit_multi(&payload).unwrap(); // body only (no preamble)
    let n = sig.len();

    // PAPR over the time-domain body.
    let mean_pow: f32 = sig.iter().map(|x| x * x).sum::<f32>() / n as f32;
    let peak_pow: f32 = sig.iter().map(|x| x * x).fold(0.0, f32::max);
    let papr_db = 10.0 * (peak_pow / mean_pow.max(1e-30)).log10();

    // PSD (dB rel peak) over positive frequencies.
    let psd = psd_db(&sig);
    let half = psd.len();

    // −26 dBc occupied-band edges (lowest/highest bin above −26 dBc).
    let thresh = -26.0_f32;
    let above: Vec<usize> = (0..half).filter(|&k| psd[k] >= thresh).collect();
    let (lo_hz, hi_hz) = match (above.first(), above.last()) {
        (Some(&l), Some(&h)) => (bin_to_hz(l, n), bin_to_hz(h, n)),
        _ => (0.0, 0.0),
    };
    let occupied_bw = hi_hz - lo_hz;

    // Energy fraction inside the intended SSB band [300, 2700] Hz vs outside
    // (out-of-band spur / image indicator). Uses linear PSD.
    let lin: Vec<f32> = psd.iter().map(|d| 10f32.powf(d / 10.0)).collect();
    let total: f32 = lin.iter().sum();
    let in_band: f32 = (0..half)
        .filter(|&k| {
            let f = bin_to_hz(k, n);
            (300.0..=2700.0).contains(&f)
        })
        .map(|k| lin[k])
        .sum();
    let oob_frac = 1.0 - in_band / total.max(1e-30);

    // Largest out-of-band peak (dBc) above 3000 Hz (image / spur).
    let oob_peak_dbc = (0..half)
        .filter(|&k| bin_to_hz(k, n) > 3000.0)
        .map(|k| psd[k])
        .fold(f32::NEG_INFINITY, f32::max);

    println!("\n===== CURRENT floor waveform characterization =====");
    println!("samples={n}  ({:.0} ms @ 48k)", n as f32 / SR * 1000.0);
    println!("PAPR = {papr_db:.2} dB");
    println!("-26 dBc occupied band: {lo_hz:.0}..{hi_hz:.0} Hz  (BW = {occupied_bw:.0} Hz)");
    println!(
        "out-of-band energy fraction (outside 300..2700 Hz) = {:.4}",
        oob_frac
    );
    println!("largest spur/image above 3000 Hz = {oob_peak_dbc:.1} dBc");
    println!("PAPR = {papr_db:.2} dB (PAPR reduction is a separate Step-1 item)");
    println!("=====================================================\n");

    // ─── PSD-mask GATE (Step 1 acceptance, spectral half) ───
    // Raised-cosine inter-symbol windowing brings the emitted spectrum within an
    // SSB-class mask. PAPR is measured but not yet gated (reduction pending).
    assert!(
        occupied_bw <= 2700.0,
        "occupied -26 dBc bandwidth {occupied_bw:.0} Hz exceeds the 2.7 kHz mask"
    );
    assert!(
        oob_peak_dbc <= -26.0,
        "out-of-band spur/image {oob_peak_dbc:.1} dBc exceeds the -26 dBc mask"
    );
    assert!(
        oob_frac < 0.01,
        "out-of-band energy fraction {oob_frac:.4} exceeds 1%"
    );
}
