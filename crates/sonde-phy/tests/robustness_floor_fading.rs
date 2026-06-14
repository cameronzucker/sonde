//! Robustness-floor fading gate (sonde-64w.2). The floor must decode through
//! Watterson Good/Moderate fading, not just AWGN — this is the channel-sim
//! seam that was never wired (sim_adapter.rs was a placeholder), which is why
//! the floor's clean-only validation hid a sync-timing bug.
//!
//! Channel model: the floor emits REAL passband audio; the Watterson sim is
//! complex-baseband. The faithful application for an OFDM-over-SSB waveform is
//! real audio -> analytic signal (Hilbert) -> complex Watterson -> AWGN ->
//! real projection. High SNR (30 dB) isolates sync/timing from BER limits.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rustfft::FftPlanner;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SR: f64 = 48_000.0;

/// Analytic signal of a real signal via FFT: zero the negative frequencies,
/// double the positives. `Re{analytic} == original` for a unit channel.
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
            // DC / Nyquist unchanged
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

/// real audio -> analytic -> Watterson(condition) -> AWGN(snr) -> real audio.
fn through_channel(clean: &[f32], condition: ChannelCondition, snr_db: f64, seed: u64) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, condition, SR);
    let mut a = analytic(clean);
    // Guard tail (>= one CP) so the timing-corrected window has samples to read.
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(1024));
    let mut faded = ch.process_block(&a);
    AwgnGenerator::new(seed ^ 0xA5A5).add_noise(&mut faded, snr_db);
    faded.iter().map(|c| c.re).collect()
}

fn assert_decodes(condition: ChannelCondition) {
    let payload = b"FLOOR FADING GATE PAYLOAD";
    // IdentityFec floor isolates sync/PHY from FEC — the bug under test is sync.
    let floor = WidebandLowDensityFloor::new();
    let clean = floor.transmit_multi_with_preamble(payload).unwrap();
    let rx = through_channel(&clean, condition, 30.0, 0xC0DE_6402);
    let (_start, decoded) = floor
        .receive_multi_with_sync(&rx)
        .unwrap_or_else(|e| panic!("{condition:?}: receive failed: {e:?}"));
    assert_eq!(decoded, payload, "{condition:?}: payload mismatch");
}

#[test]
fn floor_decodes_through_watterson_good() {
    assert_decodes(ChannelCondition::Good);
}

#[test]
fn floor_decodes_through_watterson_moderate() {
    assert_decodes(ChannelCondition::Moderate);
}
