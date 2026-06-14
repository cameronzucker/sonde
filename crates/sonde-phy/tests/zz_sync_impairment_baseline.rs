//! Step 2 baseline (sonde-xhw.3): prove the CURRENT production sync collapses
//! under a carrier-frequency offset and a sample-clock error — the impairments
//! a real HF link always has and that the modem presently does NOT correct.
//! Defines what P2 (Schmidl–Cox CFO + Gardner timing + clock tracking) must fix.
//! Measurement-first: numbers before building.
use hf_channel_sim::AwgnGenerator;
use num_complex::Complex;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SR: f64 = 48_000.0;

fn analytic(x: &[f32]) -> Vec<Complex<f32>> {
    let n = x.len();
    let mut p = FftPlanner::<f32>::new();
    let f = p.plan_fft_forward(n);
    let inv = p.plan_fft_inverse(n);
    let mut b: Vec<Complex<f32>> = x.iter().map(|&v| Complex::new(v, 0.0)).collect();
    f.process(&mut b);
    let h = n / 2;
    for (k, c) in b.iter_mut().enumerate() {
        if k == 0 || (n % 2 == 0 && k == h) {
        } else if k < h {
            *c *= 2.0;
        } else {
            *c = Complex::new(0.0, 0.0);
        }
    }
    inv.process(&mut b);
    let s = 1.0 / n as f32;
    for c in b.iter_mut() {
        *c *= s;
    }
    b
}

/// Apply a carrier-frequency offset of `cfo_hz` to a real passband signal via
/// the analytic lift, plus AWGN at `snr_db`, then project back to real.
fn with_cfo(clean: &[f32], cfo_hz: f64, snr_db: f64, seed: u64) -> Vec<f32> {
    let mut a = analytic(clean);
    for (n, c) in a.iter_mut().enumerate() {
        let ph = 2.0 * std::f64::consts::PI * cfo_hz * n as f64 / SR;
        *c *= Complex::new(ph.cos() as f32, ph.sin() as f32);
    }
    AwgnGenerator::new(seed).add_noise(&mut a, snr_db);
    a.iter().map(|c| c.re).collect()
}

#[test]
fn current_sync_collapses_under_cfo() {
    let payload = b"FLOOR FADING GATE PAYLOAD";
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let clean = floor.transmit_multi_with_preamble(payload).unwrap();
    println!("\n===== current production sync vs carrier offset (AWGN 25 dB) =====");
    for cfo in [0.0_f64, 1.0, 5.0, 20.0, 50.0, 100.0] {
        let rx = with_cfo(&clean, cfo, 25.0, 0xC0DE_6402);
        let ok = matches!(floor.receive_multi_with_sync(&rx), Ok((_, ref d)) if d == payload);
        println!("CFO {cfo:>6.1} Hz -> decode_ok = {ok}");
    }
    println!("==================================================================\n");
    // No assertion yet — this is the baseline that motivates P2. The P2 gate
    // (E2E decode through CFO +/-100 Hz + clock error + fractional timing via the
    // production sync path) replaces this once Schmidl-Cox/CFO/Gardner land.
}
