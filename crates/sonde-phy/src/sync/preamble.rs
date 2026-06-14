//! Preamble generation + correlation-based detection.
//!
//! Design primitive (per foundation doc §2.5 Meyr/Moeneclaey/Fechtel):
//! a **Schmidl–Cox repeated-pair** preamble — two identical halves `[h | h]`,
//! each half a constant-amplitude zero-autocorrelation (CAZAC) Zadoff–Chu
//! segment projected to the audio band. Two properties earn their keep:
//!
//! 1. **Sharp acquisition.** A ZC half has an impulsive autocorrelation, so a
//!    matched filter against the full pair gives a sharp time-domain peak.
//! 2. **Coarse CFO from the repetition.** Because the two halves are identical
//!    in the complex domain, a carrier-frequency offset rotates the second half
//!    relative to the first by a phase proportional to the offset. The phase of
//!    the half-to-half correlation (see [`crate::sync::carrier_offset`]) is the
//!    coarse CFO estimate — the thing a single non-repeating ZC could NOT give,
//!    and the reason the floor collapsed above ~20 Hz CFO before sonde-xhw.3.
//!
//! Half length `H` trades CFO unambiguous range `±SR/(2H)` against estimator
//! variance. `H = 160` (total 320 samples, 6.7 ms @ 48 kHz) gives `±150 Hz` —
//! comfortable headroom over the `±100 Hz` HF dial/drift budget — at a root
//! coprime with 160 (root 29; the legacy root 25 shares a factor of 5 and is
//! NOT a valid ZC root mod 160). Re-tune in Phase 11 if sweeps motivate it.

use num_complex::Complex;

/// Length of one preamble half (samples). The transmitted preamble is this
/// half repeated once → `2 × PREAMBLE_HALF_LEN` total samples.
pub const PREAMBLE_HALF_LEN: usize = 160;
/// Total preamble length (samples): the repeated pair `[h | h]`.
pub const PREAMBLE_LEN: usize = 2 * PREAMBLE_HALF_LEN; // 320
/// Zadoff–Chu root for the half. MUST be coprime with [`PREAMBLE_HALF_LEN`]
/// for the sequence to retain its CAZAC autocorrelation; 29 is coprime with
/// 160 (= 2⁵·5), the legacy root 25 is not.
pub const PREAMBLE_ROOT: usize = 29;

/// Generator for the canonical Schmidl–Cox repeated-pair preamble waveform.
pub struct PreambleGenerator;

impl PreambleGenerator {
    /// Construct a new preamble generator.
    pub fn new() -> Self {
        Self
    }
    /// Real-valued preamble samples ready to push into the audio buffer: the
    /// real part of a length-`H` Zadoff–Chu half, emitted twice back-to-back.
    ///
    /// Taking `Re{·}` of the complex half is fine pre-Hilbert: the receiver
    /// re-creates the same complex template for correlation, and — crucially —
    /// `Re{c[n]}` of a repeated complex sequence is itself exactly repeated, so
    /// the real passband preamble preserves the two-identical-halves property
    /// the CFO estimator relies on.
    pub fn generate(&self) -> Vec<f32> {
        let half: Vec<f32> = zadoff_chu(PREAMBLE_HALF_LEN, PREAMBLE_ROOT)
            .iter()
            .map(|c| c.re)
            .collect();
        let mut out = Vec::with_capacity(PREAMBLE_LEN);
        out.extend_from_slice(&half);
        out.extend_from_slice(&half);
        out
    }
}

impl Default for PreambleGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// Correlation-based detector for the repeated-pair preamble in a real-valued
/// audio stream.
///
/// The detector is a **CFO-robust, phase-invariant (I/Q) matched filter** that
/// correlates the real RX against the complex Zadoff–Chu HALF template, peaking
/// on the magnitude `√(c_re² + c_im²)`. Two ideas earn their keep:
///
/// - **Phase-invariance.** A real matched filter against only `Re{ZC}` collapses
///   when the channel rotates the preamble ≈90° (`Re{α·ZC}` becomes ≈orthogonal
///   to `Re{ZC}`), mislocating the frame under a complex Watterson channel. The
///   magnitude of the I/Q correlation equals `|⟨rx, ZC⟩|`, invariant to that
///   rotation (sonde-64w.3).
/// - **CFO-robustness via non-coherent two-half combining.** The preamble is the
///   pair `[h | h]`. We correlate each half SEPARATELY and average the two
///   magnitudes: `½(|c₁| + |c₂|)`. A single coherent correlation over the full
///   320-sample pair integrates a phasor that rotates ~240° across the window at
///   a 100 Hz CFO — the true peak is suppressed and a half-overlap sidelobe at
///   lag `H` wins, locking the frame half a preamble late (measured, sonde-xhw.3
///   / Codex Q2). Splitting into two H-sample correlations caps the per-half
///   rotation at ~120° (tolerable), and at the false-lock lag the second half
///   falls on the body (low `|c₂|`), so the true lag's `½(high+high)` beats the
///   sidelobe's `½(high+low)`.
pub struct PreambleDetector {
    /// One half of the complex ZC template (`Re`), length [`PREAMBLE_HALF_LEN`].
    half_re: Vec<f32>,
    /// One half of the complex ZC template (`Im`).
    half_im: Vec<f32>,
    /// `Σ Re{ZC}²` over the half — the normalisation reference (a clean aligned
    /// half scores 1.0 against `‖rx_half‖·√(this)`).
    half_template_energy: f32,
}

impl PreambleDetector {
    /// Construct a detector pre-loaded with the canonical half template (both
    /// quadrature components of the complex Zadoff–Chu half).
    pub fn new() -> Self {
        let half = zadoff_chu(PREAMBLE_HALF_LEN, PREAMBLE_ROOT);
        let half_re: Vec<f32> = half.iter().map(|c| c.re).collect();
        let half_im: Vec<f32> = half.iter().map(|c| c.im).collect();
        let half_template_energy: f32 = half_re.iter().map(|s| s * s).sum();
        Self {
            half_re,
            half_im,
            half_template_energy,
        }
    }
}

impl Default for PreambleDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a preamble scan.
#[derive(Debug, Clone)]
pub struct Detection {
    /// Sample index where the preamble starts in the scanned signal.
    pub start_sample: usize,
    /// Estimated post-correlation SNR in dB.
    pub snr_estimate_db: f32,
}

/// Detection threshold on the **Schmidl-Cox metric** `M(d) = |P(d)|/R(d)`
/// (the half-to-half self-correlation of the RECEIVED signal, normalised by the
/// second half's energy). Unlike a template matched filter, `M(d)` is
/// CFO-INVARIANT: a carrier offset adds a constant phase to every product term
/// in `P(d)`, leaving `|P(d)|` unchanged. That is what lets detection survive a
/// ±100 Hz offset, where the template MF magnitude collapses (a ±100 Hz CFO
/// rotates each half ~120°, shrinking the coherent correlation below the noise
/// floor — measured sonde-xhw.3). A clean aligned preamble scores ≈1.0.
/// Measured separation under CFO=100 Hz + Watterson Good/Moderate at a ~20 dB
/// Eb/N0: signal `M` bottoms at ~0.70, the noise floor (max `M` over many random
/// captures) tops at ~0.40. 0.55 sits in that gap.
const SC_DETECT_THRESHOLD: f32 = 0.55;

/// Full detection result: frame timing plus the coarse CFO read off the
/// repeated-pair's half-to-half phase.
#[derive(Debug, Clone)]
pub struct DetectionFull {
    /// Refined preamble start sample (template-MF sharp, post-derotation).
    pub start_sample: usize,
    /// Coarse carrier-frequency offset (Hz) from the repeated-pair phase.
    pub cfo_hz: f32,
    /// Schmidl-Cox detection metric `M` at the plateau peak (≈1.0 = clean).
    pub sc_metric: f32,
}

impl PreambleDetector {
    /// Raw correlation peak: the `(start_sample, normalised_magnitude)` of the
    /// strongest I/Q match, with NO detection threshold applied. `None` only
    /// when `signal` is shorter than the template. Used by [`Self::scan`] and
    /// for detector characterization/tuning.
    pub fn peak_normalized(&self, signal: &[f32]) -> Option<(usize, f32)> {
        let h = PREAMBLE_HALF_LEN;
        let full = PREAMBLE_LEN; // 2·h
        if signal.len() < full {
            return None;
        }
        let template_norm = self.half_template_energy.sqrt().max(1e-9);

        // Per-half normalised I/Q correlation magnitude at offset `off`.
        let half_corr = |off: usize| -> f32 {
            let mut c_re = 0.0_f32;
            let mut c_im = 0.0_f32;
            let mut sig_energy = 0.0_f32;
            for j in 0..h {
                let s = signal[off + j];
                c_re += s * self.half_re[j];
                c_im += s * self.half_im[j];
                sig_energy += s * s;
            }
            let sig_norm = sig_energy.sqrt().max(1e-9);
            (c_re * c_re + c_im * c_im).sqrt() / (sig_norm * template_norm)
        };

        let mut best_corr = 0.0_f32;
        let mut best_idx = 0usize;
        for i in 0..=(signal.len() - full) {
            // Non-coherent combine of the two halves — CFO-robust (see the type
            // docs). Each half scores ≈1.0 when aligned; the average peaks at
            // the true frame start, not the half-preamble-late sidelobe.
            let metric = 0.5 * (half_corr(i) + half_corr(i + h));
            if metric > best_corr {
                best_corr = metric;
                best_idx = i;
            }
        }
        Some((best_idx, best_corr))
    }

    /// Schmidl-Cox scan over the analytic signal: the argmax lag of
    /// `M(d) = |P(d)|/R(d)`, the metric value there, and the coarse CFO at that
    /// lag. `M(d)` is CFO-invariant (see [`SC_DETECT_THRESHOLD`]); its peak is a
    /// PLATEAU, so the lag is only coarse timing — [`Self::detect_analytic`]
    /// refines it with the template MF.
    fn schmidl_cox(&self, a: &[Complex<f32>], sample_rate_hz: f32) -> (usize, f32, f32) {
        let h = PREAMBLE_HALF_LEN;
        if a.len() < 2 * h {
            return (0, 0.0, 0.0);
        }
        // Prefix sums of sample energy → O(1) per-window energy `R(d)` for the
        // energy gate below.
        let mut prefix = vec![0.0_f32; a.len() + 1];
        for (i, c) in a.iter().enumerate() {
            prefix[i + 1] = prefix[i] + c.norm_sqr();
        }
        let r_of = |d: usize| prefix[d + 2 * h] - prefix[d + h]; // second-half energy

        // Energy gate: M(d) = |P|/R is numerically unstable where R is tiny
        // (silent regions, the zero-padded trailing symbol), spuriously inflating
        // M. The constant-amplitude CAZAC preamble has uniformly HIGH energy, so
        // only consider lags whose window energy is a meaningful fraction of the
        // strongest window — this excludes the low-energy tail without rejecting
        // the (high-energy) preamble.
        let mut r_max = 0.0_f32;
        for d in 0..=(a.len() - 2 * h) {
            r_max = r_max.max(r_of(d));
        }
        let gate = 0.30 * r_max;

        let mut best = (0usize, 0.0_f32, 0.0_f32);
        for d in 0..=(a.len() - 2 * h) {
            let r = r_of(d);
            if r < gate {
                continue;
            }
            let mut p = Complex::new(0.0_f32, 0.0);
            for i in 0..h {
                p += a[d + i].conj() * a[d + i + h];
            }
            let m = p.norm() / r.max(1e-9);
            if m > best.1 {
                let cfo = p.arg() * sample_rate_hz / (2.0 * std::f32::consts::PI * h as f32);
                best = (d, m, cfo);
            }
        }
        best
    }

    /// Coarse CFO (Hz) from the half-to-half correlation phase at a fixed lag.
    fn cfo_at(&self, a: &[Complex<f32>], d: usize, sample_rate_hz: f32) -> f32 {
        let h = PREAMBLE_HALF_LEN;
        let mut p = Complex::new(0.0_f32, 0.0);
        for i in 0..h {
            p += a[d + i].conj() * a[d + i + h];
        }
        p.arg() * sample_rate_hz / (2.0 * std::f32::consts::PI * h as f32)
    }

    /// Two-stage acquisition on the analytic signal:
    /// 1. **Schmidl-Cox `M(d)`** for CFO-invariant detection + coarse CFO. Below
    ///    [`SC_DETECT_THRESHOLD`] ⇒ `None` (no preamble).
    /// 2. **Derotate** a window around the coarse lag by the coarse CFO, then run
    ///    the **two-half template MF** on it — with the CFO removed the MF regains
    ///    full magnitude and a SHARP peak, pinning the exact frame start (the
    ///    `M(d)` plateau alone is too coarse, ±~120 samples under fade).
    ///
    /// The final CFO is recomputed at the refined start for the best estimate.
    pub fn detect_analytic(
        &self,
        a: &[Complex<f32>],
        sample_rate_hz: f32,
    ) -> Option<DetectionFull> {
        let h = PREAMBLE_HALF_LEN;
        let full = PREAMBLE_LEN;
        let (d_sc, m, cfo_coarse) = self.schmidl_cox(a, sample_rate_hz);
        if m < SC_DETECT_THRESHOLD {
            return None;
        }
        // Refinement window around the coarse lag (the plateau peaks at or after
        // the true start, so `[d_sc − H, d_sc + 2H]` reliably brackets it).
        let lo = d_sc.saturating_sub(h);
        let hi = (d_sc + 2 * h).min(a.len()).max((lo + full).min(a.len()));
        if hi - lo < full {
            // Too close to the buffer end to refine; fall back to the SC lag.
            let cfo = self.cfo_at(a, d_sc, sample_rate_hz);
            return Some(DetectionFull {
                start_sample: d_sc,
                cfo_hz: cfo,
                sc_metric: m,
            });
        }
        let mut win: Vec<Complex<f32>> = a[lo..hi].to_vec();
        crate::sync::carrier_offset::derotate(&mut win, cfo_coarse, sample_rate_hz);
        let real: Vec<f32> = win.iter().map(|c| c.re).collect();
        let (local, _corr) = self.peak_normalized(&real)?;
        let start = lo + local;
        let cfo = if start + full <= a.len() {
            self.cfo_at(a, start, sample_rate_hz)
        } else {
            cfo_coarse
        };
        Some(DetectionFull {
            start_sample: start,
            cfo_hz: cfo,
            sc_metric: m,
        })
    }

    /// Scan a real-valued capture for the preamble. Forms the analytic signal
    /// internally and runs [`Self::detect_analytic`]; returns `None` below the
    /// detection threshold. Production callers that already hold the analytic
    /// should call [`Self::detect_analytic`] directly to avoid recomputing it.
    pub fn scan(&self, signal: &[f32]) -> Option<Detection> {
        let a = crate::sync::carrier_offset::analytic_signal(signal);
        let det = self.detect_analytic(&a, crate::audio_io::SAMPLE_RATE_HZ as f32)?;
        // Approximate SNR from the detection metric (|rho|² ≈ SNR/(1+SNR)).
        let rho_sq = (det.sc_metric * det.sc_metric).clamp(1e-6, 1.0 - 1e-6);
        let snr_lin = rho_sq / (1.0 - rho_sq);
        Some(Detection {
            start_sample: det.start_sample,
            snr_estimate_db: 10.0 * snr_lin.log10(),
        })
    }
}

fn zadoff_chu(n: usize, q: usize) -> Vec<Complex<f32>> {
    let pi = std::f32::consts::PI;
    (0..n)
        .map(|k| {
            let arg = -pi * (q as f32) * (k as f32) * ((k + 1) as f32) / (n as f32);
            Complex::new(arg.cos(), arg.sin())
        })
        .collect()
}
