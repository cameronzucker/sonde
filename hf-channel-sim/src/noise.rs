// SPDX-License-Identifier: AGPL-3.0-only

//! Additive White Gaussian Noise injection.
//!
//! Per ITU-R F.1487 methodology: channel impairment is applied first;
//! AWGN is added separately at a measured SNR relative to the post-
//! channel signal. Decoupling lets callers sweep noise realizations
//! at a fixed channel realization (and vice versa).

use crate::rng::{complex_gaussian_block, rng_from_seed};
use num_complex::Complex;
use rand_xoshiro::Xoshiro256PlusPlus;

/// Seeded complex-AWGN generator. Deterministic per `seed`; independent of
/// channel state so noise realizations can be swept while the channel is
/// fixed (and vice versa).
pub struct AwgnGenerator {
    seed: u64,
    rng: Xoshiro256PlusPlus,
}

impl AwgnGenerator {
    /// Construct a new generator from a `u64` seed. Same seed produces the
    /// same complex-Gaussian byte-stream across runs and machines.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            rng: rng_from_seed(seed),
        }
    }

    /// Rewind to the initial state (equivalent to `AwgnGenerator::new(seed)`
    /// where `seed` is the seed this generator was constructed with).
    pub fn reset(&mut self) {
        self.rng = rng_from_seed(self.seed);
    }

    /// Add complex AWGN to `signal` in-place such that the signal-to-noise
    /// power ratio is `snr_db` dB, where SIGNAL power is the measured
    /// average power of `signal` BEFORE noise is added.
    ///
    /// `snr_db` interpretation:
    /// - `+∞`: no noise added.
    /// - `0.0`: noise power equals signal power.
    /// - `-3.0`: noise power is 2× signal power.
    pub fn add_noise(&mut self, signal: &mut [Complex<f32>], snr_db: f64) {
        if signal.is_empty() {
            return;
        }
        let sig_power: f64 =
            signal.iter().map(|c| c.norm_sqr() as f64).sum::<f64>() / signal.len() as f64;
        if sig_power == 0.0 {
            return; // nothing to scale noise against
        }
        // SNR linear = 10^(snr_db/10); noise_power = sig_power / snr_linear.
        let snr_linear = 10.0_f64.powf(snr_db / 10.0);
        let noise_power = sig_power / snr_linear;
        let noise_amplitude = (noise_power as f32).sqrt();

        let pairs = complex_gaussian_block(&mut self.rng, signal.len());
        for (s, (nre, nim)) in signal.iter_mut().zip(pairs) {
            // complex_gaussian_block returns unit-variance complex; scale to
            // target amplitude.
            *s += Complex {
                re: nre * noise_amplitude,
                im: nim * noise_amplitude,
            };
        }
    }

    /// Add complex AWGN at a FIXED per-component standard deviation, independent of
    /// the signal's instantaneous power (a constant band-noise floor).
    ///
    /// Unlike [`add_noise`](Self::add_noise), which sets the noise level from the
    /// block's measured signal power, this adds the same noise level regardless of
    /// the signal — so SILENCE still carries the floor and a deeply-faded signal is
    /// genuinely buried. This is the model needed for real-time / streaming use
    /// (e.g. piping a live modem's audio through the channel), where "SNR" is
    /// referenced to a fixed transmit level rather than measured per block. Each
    /// real (and imaginary) noise component is N(0, `std`²).
    pub fn add_noise_fixed(&mut self, signal: &mut [Complex<f32>], std: f32) {
        if signal.is_empty() || std <= 0.0 {
            return;
        }
        // `complex_gaussian_block` returns unit-variance COMPLEX samples (each of the
        // real/imag components has variance 1/2), so scale by std·√2 to make each
        // component N(0, std²) — i.e. `std` is the per-component standard deviation,
        // which is what the real-projected audio noise floor uses.
        let amp = std * std::f32::consts::SQRT_2;
        let pairs = complex_gaussian_block(&mut self.rng, signal.len());
        for (s, (nre, nim)) in signal.iter_mut().zip(pairs) {
            *s += Complex {
                re: nre * amp,
                im: nim * amp,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_signal(n: usize) -> Vec<Complex<f32>> {
        vec![Complex { re: 1.0, im: 0.0 }; n]
    }

    fn power(v: &[Complex<f32>]) -> f64 {
        v.iter().map(|c| c.norm_sqr() as f64).sum::<f64>() / v.len() as f64
    }

    #[test]
    fn same_seed_same_noise() {
        let mut s1 = unit_signal(1024);
        let mut s2 = unit_signal(1024);
        let mut g1 = AwgnGenerator::new(42);
        let mut g2 = AwgnGenerator::new(42);
        g1.add_noise(&mut s1, 10.0);
        g2.add_noise(&mut s2, 10.0);
        assert_eq!(s1, s2);
    }

    #[test]
    fn snr_0db_yields_equal_signal_and_noise_power() {
        let mut s = unit_signal(100_000);
        let p_in = power(&s);
        let mut g = AwgnGenerator::new(0);
        g.add_noise(&mut s, 0.0);
        let p_out = power(&s);
        // Out = signal + noise (uncorrelated): expected power ~ 2× input.
        // Tolerance widened for statistical noise over 100k samples.
        assert!(
            ((p_out / p_in) - 2.0).abs() < 0.05,
            "expected ~2× input power at 0 dB SNR, got ratio {}",
            p_out / p_in,
        );
    }

    #[test]
    fn snr_minus_10db_yields_11x_total_power() {
        // SNR = -10 dB → noise_power = 10 × signal_power → total ≈ 11×.
        let mut s = unit_signal(100_000);
        let p_in = power(&s);
        let mut g = AwgnGenerator::new(0);
        g.add_noise(&mut s, -10.0);
        let p_out = power(&s);
        assert!(
            ((p_out / p_in) - 11.0).abs() < 0.5,
            "expected ~11× input at -10 dB SNR, got {}",
            p_out / p_in,
        );
    }

    #[test]
    fn reset_returns_to_initial_state() {
        let mut s1 = unit_signal(64);
        let mut s2 = unit_signal(64);
        let mut g = AwgnGenerator::new(7);
        g.add_noise(&mut s1, 0.0);
        g.reset();
        g.add_noise(&mut s2, 0.0);
        assert_eq!(s1, s2);
    }

    #[test]
    fn fixed_noise_floors_silence() {
        // Unlike add_noise (which adds nothing to silence), add_noise_fixed adds the
        // floor regardless of signal. Each component is N(0, std²) so E[|n|²] = 2·std².
        let mut silence = vec![Complex { re: 0.0, im: 0.0 }; 100_000];
        AwgnGenerator::new(5).add_noise_fixed(&mut silence, 0.5);
        let p = power(&silence);
        assert!((p - 0.5).abs() < 0.02, "expected ~2·std²=0.5, got {p}");
    }

    #[test]
    fn fixed_noise_same_seed_deterministic() {
        let mut a = vec![Complex { re: 0.0, im: 0.0 }; 256];
        let mut b = a.clone();
        AwgnGenerator::new(9).add_noise_fixed(&mut a, 0.3);
        AwgnGenerator::new(9).add_noise_fixed(&mut b, 0.3);
        assert_eq!(a, b);
    }
}
