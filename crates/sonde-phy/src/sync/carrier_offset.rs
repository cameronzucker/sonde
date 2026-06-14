//! Carrier-frequency-offset estimation via Schmidl-Cox-style
//! repeat-pair correlation. Given a known-repeating preamble segment,
//! the phase of the per-sample cross-correlation between the first and
//! second halves is proportional to the residual frequency offset.
//!
//! Also home to the receiver-side channelization helpers used to apply a
//! frequency correction to a REAL passband capture: form the analytic
//! (Hilbert) signal, multiply by `e^(−j2π·f·n/SR)`, take `Re{·}`. CFO
//! correction MUST happen in the time domain before the per-symbol FFT — a
//! ±100 Hz offset is ~4 OFDM sub-carrier spacings (Δf = 23.4 Hz @ FFT 2048),
//! which slides the spectrum off the pilot bins and injects inter-carrier
//! interference the per-symbol pilot equalizer cannot undo.

use num_complex::Complex;
use rustfft::FftPlanner;

/// Carrier-frequency-offset estimator. Stateless; constructed once per
/// sample-rate, reused across detections.
pub struct CfoEstimator {
    sample_rate_hz: f32,
}

impl CfoEstimator {
    /// Construct a CFO estimator for the given sample rate in Hz.
    pub fn new(sample_rate_hz: f32) -> Self {
        Self { sample_rate_hz }
    }

    /// Estimate the residual carrier-frequency offset (Hz) of a signal
    /// known to contain a repeated `half_len`-sample preamble segment.
    ///
    /// The unambiguous range is `±SR/(2·half_len)`: a true offset beyond that
    /// wraps in the `arg()` and is mis-estimated. The repeated-pair preamble
    /// (`half_len = PREAMBLE_HALF_LEN = 160`) gives `±150 Hz`.
    pub fn estimate_repeat(&self, signal: &[Complex<f32>], half_len: usize) -> f32 {
        let mut acc = Complex::new(0.0, 0.0);
        for i in 0..half_len {
            acc += signal[i].conj() * signal[i + half_len];
        }
        let phase = acc.arg();
        phase * self.sample_rate_hz / (2.0 * std::f32::consts::PI * half_len as f32)
    }
}

/// Analytic (complex) signal of a real passband signal via FFT: zero the
/// negative-frequency bins, double the positives (DC / Nyquist unchanged).
/// `Re{analytic(x)} == x` for any real `x`, so taking `Re{·}` after a no-op
/// correction is lossless — the clean path stays bit-identical.
///
/// This is the same Hilbert lift the channel-sim harnesses use; here it runs
/// in reverse at RX so a frequency correction can be applied to a real capture.
pub fn analytic_signal(real: &[f32]) -> Vec<Complex<f32>> {
    let n = real.len();
    if n == 0 {
        return Vec::new();
    }
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf: Vec<Complex<f32>> = real.iter().map(|&x| Complex::new(x, 0.0)).collect();
    fwd.process(&mut buf);
    let half = n / 2;
    for (k, b) in buf.iter_mut().enumerate() {
        if k == 0 || (n % 2 == 0 && k == half) {
            // DC / Nyquist unchanged.
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

/// Derotate an analytic signal in place by `−cfo_hz`: multiply sample `n` by
/// `e^(−j2π·cfo_hz·n/SR)`. A constant phase offset (choice of `n`'s origin) is
/// irrelevant — the per-symbol pilot equalizer absorbs a constant phase; only
/// the frequency slope matters.
pub fn derotate(signal: &mut [Complex<f32>], cfo_hz: f32, sample_rate_hz: f32) {
    let w = -2.0 * std::f32::consts::PI * cfo_hz / sample_rate_hz;
    for (n, c) in signal.iter_mut().enumerate() {
        let ph = w * n as f32;
        *c *= Complex::new(ph.cos(), ph.sin());
    }
}

/// Correct a real passband capture for a carrier-frequency offset: lift to
/// analytic, derotate by `−cfo_hz`, project back to real. Returns a real signal
/// the same length as `real`. With `cfo_hz == 0` this is the identity (to FP).
pub fn correct_cfo_real(real: &[f32], cfo_hz: f32, sample_rate_hz: f32) -> Vec<f32> {
    let mut a = analytic_signal(real);
    derotate(&mut a, cfo_hz, sample_rate_hz);
    a.iter().map(|c| c.re).collect()
}
