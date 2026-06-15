//! OFDM receiver: time-domain samples → CP stripping → FFT →
//! pilot-aided equalization → per-bit LLR computation across data
//! sub-carriers.

use crate::constellations::{Constellation, Mapper};
use crate::ofdm_main::equalizer::OfdmEqualizer;
use crate::ofdm_main::ofdm_params::OfdmParams;
use num_complex::Complex;
use rustfft::FftPlanner;

/// Numerator of the noise-scaled soft-LLR clamp: the clamp magnitude is
/// `LLR_CLAMP_NUM / n0`. Bounds the damage a wrong-SIGN LLR (from
/// channel-estimate interpolation error near a spectral notch) can do to the
/// soft FEC decoder, WITHOUT flattening the per-sub-carrier reliability range.
///
/// The clamp MUST scale with `1/n0`: the channel-aware LLR is `∝ |h|²/n0`, so a
/// fixed absolute clamp that only caps the strongest sub-carrier at one noise
/// level (the legacy `±20` at `n0=0.1`) collapses ALL LLRs to the rail once `n0`
/// shrinks at high SNR — destroying the `|h|²` reliability ordering the LDPC
/// rides to bridge frequency-selective nulls. Scaling the clamp by `1/n0` keeps
/// the SNR-independent nulled-vs-strong dynamic range (~140×) at every noise
/// level. `2.0 = 20 · 0.1` reproduces the legacy clamp exactly at `n0 = 0.1`.
const LLR_CLAMP_NUM: f32 = 2.0;

/// Centered triangular kernel for time-smoothing the pilot channel observations
/// across consecutive OFDM symbols (see [`OfdmReceiver::demodulate_frame`],
/// sonde-vb9). Span 9 symbols ≈ 0.48 s @ 53 ms/symbol: ~8 dB pilot-noise
/// reduction on a static channel while attenuating a 1 Hz Doppler only ≈ −2 dB,
/// safe for the floor's ≤1 Hz coherence. The weights need not pre-sum to 1 —
/// `demodulate_frame` renormalizes by whatever weights fall in-range, so the
/// estimate stays unbiased at the frame edges.
const PILOT_TIME_SMOOTH_KERNEL: [f32; 9] = [1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0, 1.0];

/// Guard, in FFT bins, kept clear around the occupied band, its real-signal
/// mirror, DC, and Nyquist when sampling the noise floor. 8 bins (~190 Hz @
/// FFT 2048) clears soft-clip / windowing spectral leakage at the band edges.
const NOISE_GUARD_BINS: usize = 8;

/// Channel-estimate-error floor as a fraction of the mean pilot power. Bounds
/// demod confidence at the pilot grid's between-pilot blind spot (a null
/// narrower than the pilot spacing — see [`effective_noise_per_bin`]). Calibrated
/// against the Watterson Good/Moderate fading gate: `0.5 · mean|H_pilot|²`
/// reproduces the high-SNR behaviour the legacy fixed `n0 = 0.1` provided
/// (measured `mean|H_pilot|² ≈ 0.18`, so floor ≈ 0.09) while letting the thermal
/// term dominate — and lower the operating Eb/N0 — once the real noise exceeds it.
const CHAN_EST_ERROR_FLOOR_FRAC: f32 = 0.5;

/// Estimate the per-bin noise variance `n0` (in FFT-bin power units) from the
/// EMPTY sub-carriers — those carrying no signal. OFDM orthogonality puts every
/// occupied sub-carrier on an integer bin, so it contributes ~zero to other
/// integer bins; the unoccupied bins (outside the occupied slab, its real-cast
/// mirror, and guards around DC / Nyquist) carry only noise, with the SAME
/// per-bin variance the occupied bins see (`E[|freq[k]|²] = σ²` under the
/// unitary FFT for real white noise). This is exactly the `n0` the channel-aware
/// metric `−|y − h·c|²/n0` needs — same `freq` domain, no 3 dB correction (the
/// real-passband halving is already folded into both `y` and the pilot-derived
/// `h`).
///
/// `|freq[k]|²` is ~exponential (mean σ²), whose MEDIAN is `σ²·ln(2)`; we take
/// `median / ln(2)` to debias back to the variance. The median (vs the mean) is
/// robust to soft-clip leakage, spurs, and narrowband interference that bias the
/// tail (Codex-converged, sonde-gtg). Estimated per symbol over the ~hundreds of
/// empty bins — ample for a few-percent estimate — so it tracks nonstationary
/// noise without frame-averaging smear.
fn estimate_noise_variance(freq: &[Complex<f32>], params: &OfdmParams) -> f32 {
    let n = freq.len();
    let occ = params.subcarrier_indices();
    let (occ_min, occ_max) = match (occ.first(), occ.last()) {
        (Some(&a), Some(&b)) => (a, b),
        _ => return 1e-12,
    };
    let g = NOISE_GUARD_BINS;
    let nyq = n / 2;
    let near = |k: usize, lo: usize, hi: usize| k + g >= lo && k <= hi + g;
    // A bin is EXCLUDED (carries signal or sits at a band edge) if it is within
    // the guard of: the occupied slab, its mirror (N-occ_max .. N-occ_min), DC,
    // or Nyquist (N/2). Everything else is an empty noise bin.
    let excluded = |k: usize| {
        near(k, occ_min, occ_max)
            || near(k, n - occ_max, n - occ_min)
            || k <= g
            || (k + g >= nyq && k <= nyq + g)
    };
    let mut powers: Vec<f32> = (0..n)
        .filter(|&k| !excluded(k))
        .map(|k| freq[k].norm_sqr())
        .collect();
    if powers.is_empty() {
        return 1e-12;
    }
    let mid = powers.len() / 2;
    powers.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
    let median = powers[mid];
    (median / std::f32::consts::LN_2).max(1e-12)
}

/// Per-bin EFFECTIVE noise variance `n0_eff[k] = n0_thermal + var(e[k])`, where
/// `e` is the pilot-aided channel-estimate error. The thermal term alone is
/// insufficient: the channel-aware LLR's residual is `y − ĥ·c = w − e·c`, so its
/// variance is `n0_thermal + var(e)`. Near a deep frequency-selective null the
/// linear pilot interpolation has a large curvature bias that does NOT vanish at
/// high SNR — that bias is exactly what makes a nulled sub-carrier a
/// low-confidence near-erasure the LDPC can bridge (the legacy fixed `n0=0.1`
/// was implicitly this channel-estimate-error floor). `var(e)` has two parts:
///
/// - **pilot-noise propagation** `((1−u)²+u²)·n0_thermal` — the variance of a
///   linear interpolation of two noisy pilot observations at fractional
///   position `u`;
/// - **curvature bias** `(u(1−u))²·q_local` — the deterministic interpolation
///   error, where `q` is a leave-one-out pilot residual
///   `|H[pᵢ] − ½(H[pᵢ₋₁]+H[pᵢ₊₁])|²` with the residual's own `1.5·n0_thermal`
///   noise removed, taken as the adjacent-pilot `max` for the data bin.
///
/// Returns a full-spectrum vector; only occupied data bins are consulted by the
/// demod. Design Codex-converged (sonde-gtg round 2).
fn effective_noise_per_bin(
    freq: &[Complex<f32>],
    params: &OfdmParams,
    n0_thermal: f32,
) -> Vec<f32> {
    let n = freq.len();
    let pilots = params.pilot_indices();
    if pilots.len() < 2 {
        return vec![n0_thermal; n];
    }

    // Channel-estimate-error FLOOR, scaled by the mean pilot power (mode- and
    // gain-independent). The leave-one-out curvature `q` below is measured AT
    // pilots, but the worst interpolation error is at DATA bins where a null
    // falls BETWEEN pilots — invisible to the pilot residual (a sub-`D`-bin null
    // is under-sampled by the every-4th-bin grid). This floor bounds demod
    // confidence at those blind spots: at high SNR `n0_thermal → 0` and `n0_eff`
    // settles to ≈ `floor` (the role the legacy fixed `n0=0.1` played, here
    // derived from the signal instead of pinned); at low SNR `n0_thermal`
    // dominates and the estimate tracks the real noise — the operating-Eb/N0 win.
    let mean_pilot_pow: f32 =
        pilots.iter().map(|&p| freq[p].norm_sqr()).sum::<f32>() / pilots.len() as f32;
    let floor = CHAN_EST_ERROR_FLOOR_FRAC * mean_pilot_pow;
    let base = n0_thermal + floor;
    let mut n0_eff = vec![base; n];
    // De-noised curvature power per pilot (leave-one-out). Edge pilots inherit
    // their nearest interior neighbour's value.
    let mut q = vec![0.0_f32; pilots.len()];
    for i in 1..pilots.len() - 1 {
        let r = freq[pilots[i]] - 0.5 * (freq[pilots[i - 1]] + freq[pilots[i + 1]]);
        q[i] = (r.norm_sqr() - 1.5 * n0_thermal).max(0.0);
    }
    if pilots.len() >= 3 {
        q[0] = q[1];
        let last = pilots.len() - 1;
        q[last] = q[last - 1];
    }
    // Fill each pilot-bounded interval's data bins with the per-bin model, on
    // top of the floor: n0_eff = n0_thermal + floor + prop + curv  (= base + …).
    for w in 0..pilots.len() - 1 {
        let (a, b) = (pilots[w], pilots[w + 1]);
        let q_local = q[w].max(q[w + 1]);
        let span = (b - a) as f32;
        // `k` is the load-bearing absolute bin index (it sets both the
        // interpolation weight `u` and the write target); the enumerate/skip
        // rewrite clippy suggests obscures the math without buying safety.
        #[allow(clippy::needless_range_loop)]
        for k in a..=b {
            let u = (k - a) as f32 / span;
            let prop = ((1.0 - u) * (1.0 - u) + u * u) * n0_thermal;
            let curv = (u * (1.0 - u)).powi(2) * q_local;
            n0_eff[k] = base + prop + curv;
        }
    }
    // Edge data bins outside the pilot span hold the nearest pilot estimate
    // (one noisy observation, propagation ≈ n0_thermal).
    let first = pilots[0];
    let last = *pilots.last().unwrap();
    for e in n0_eff.iter_mut().take(first) {
        *e = base + n0_thermal;
    }
    for e in n0_eff.iter_mut().skip(last + 1) {
        *e = base + n0_thermal;
    }
    n0_eff
}

/// Single-symbol OFDM receiver bound to one resolved [`OfdmParams`]
/// mode.
pub struct OfdmReceiver<'a> {
    params: &'a OfdmParams,
    /// When `Some`, use this fixed noise variance instead of the per-symbol
    /// empty-bin estimate. Only the legacy/diagnostic path sets it (e.g. the
    /// sonde-gtg differential gate's fixed-`n0=0.1` control arm); production
    /// leaves it `None` so `n0` tracks the measured noise floor.
    n0_override: Option<f32>,
}

impl<'a> OfdmReceiver<'a> {
    /// Construct a receiver bound to the given mode parameters. `n0` is
    /// estimated per symbol from the empty bins.
    pub fn new(params: &'a OfdmParams) -> Self {
        Self {
            params,
            n0_override: None,
        }
    }

    /// Construct a receiver that uses a FIXED noise variance instead of the
    /// per-symbol estimate. Diagnostic / differential-gate use only.
    pub fn with_n0_override(params: &'a OfdmParams, n0: Option<f32>) -> Self {
        Self {
            params,
            n0_override: n0,
        }
    }

    /// Demodulate one OFDM symbol: drop the CP, FFT, equalize against
    /// pilot positions, then emit per-bit LLRs across the data
    /// sub-carriers (in the same transmission order the matching
    /// [`crate::ofdm_main::transmitter::OfdmTransmitter::modulate_one_symbol`]
    /// consumed).
    ///
    /// `samples.len()` must equal `params.fft_size() + params.cp_len()`.
    /// `bits_per_subcarrier` follows the same indexing as the
    /// transmitter side.
    pub fn demodulate_one_symbol(&self, samples: &[f32], bits_per_subcarrier: &[u8]) -> Vec<f32> {
        self.llrs_from_freq(&self.symbol_fft(samples), bits_per_subcarrier)
    }

    /// Demodulate a whole frame of CONSECUTIVE OFDM symbols, time-smoothing the
    /// pilot channel observations across symbols before each symbol's
    /// equalization. Returns one LLR vector per input symbol, in order.
    ///
    /// Per-symbol pilot estimation is too noisy at the coded path's low
    /// per-symbol SNR (a rate-1/4 code spreads each info bit over 4 symbols, so
    /// every coded symbol sits ~6 dB below the uncoded per-symbol SNR) — the
    /// dominant ~4 dB loss behind sonde-vb9. But the HF floor's channel is
    /// slowly varying (Doppler ≤1 Hz ⇒ coherence ≥1 s ≫ the 53 ms symbol), so
    /// the pilot at a given bin is nearly constant across neighbouring symbols.
    /// A short centered triangular time-average ([`PILOT_TIME_SMOOTH_KERNEL`])
    /// therefore cuts pilot-estimate noise ~8 dB while attenuating ≤1 Hz Doppler
    /// only ~2 dB. The thermal-noise estimate stays per-symbol (the empty bins
    /// the smoothing never touches), so it still tracks nonstationary noise.
    pub fn demodulate_frame(
        &self,
        symbols: &[&[f32]],
        bits_per_subcarrier: &[u8],
    ) -> Vec<Vec<f32>> {
        let freqs: Vec<Vec<Complex<f32>>> = symbols.iter().map(|s| self.symbol_fft(s)).collect();
        let n_sym = freqs.len();
        if n_sym == 0 {
            return Vec::new();
        }
        let p = self.params;
        let pilots = p.pilot_indices();
        let half = (PILOT_TIME_SMOOTH_KERNEL.len() / 2) as isize;
        // Per-symbol thermal noise (empty-bin estimate), measured on the RAW FFTs
        // before any pilot substitution — it drives the adaptive blend's noise
        // model below.
        let n0: Vec<f32> = freqs
            .iter()
            .map(|f| estimate_noise_variance(f, p))
            .collect();
        (0..n_sym)
            .map(|t| {
                let mut freq = freqs[t].clone();
                for &pbin in pilots {
                    freq[pbin] = self.blend_pilot(&freqs, &n0, t, pbin, half, n_sym);
                }
                self.llrs_from_freq(&freq, bits_per_subcarrier)
            })
            .collect()
    }

    /// Innovation-gated LMMSE blend of the per-symbol pilot `h_raw = freq[t][p]`
    /// with its centered triangular time-average `h_smooth` (sonde-vb9, Codex
    /// round 3). The blend weight separates "noisy but STATIC" (→ smooth, the
    /// low-SNR coding-gain win) from "the channel is MOVING" (→ trust raw, no
    /// fading smear):
    ///
    /// ```text
    /// smear = max(|h_raw − h_smooth|² − var(h_raw − h_smooth | noise), 0)
    /// β     = clamp((var_raw − cov) / (var_diff + smear), 0, 1)
    /// h_used = h_raw + β·(h_smooth − h_raw)
    /// ```
    ///
    /// `smear` is the part of the raw-vs-smoothed disagreement NOT explained by
    /// thermal noise — i.e. real channel motion. Static + noisy ⇒ `smear≈0`,
    /// `β→1` (full smoothing); fading ⇒ `smear` large, `β→0` (raw per-symbol, the
    /// original demod, so high-SNR Watterson is untouched). Using `var`/`smear`
    /// (not `|h_smooth|²/n0`) avoids the trap that coherent smoothing ATTENUATES
    /// `h_smooth` under fading and would otherwise smooth hardest exactly when it
    /// must not.
    #[allow(clippy::too_many_arguments)]
    fn blend_pilot(
        &self,
        freqs: &[Vec<Complex<f32>>],
        n0: &[f32],
        t: usize,
        pbin: usize,
        half: isize,
        n_sym: usize,
    ) -> Complex<f32> {
        let mut h_sum = Complex::new(0.0, 0.0);
        let mut wsum = 0.0_f32;
        let mut var_smooth_num = 0.0_f32; // Σ wᵢ²·n0ᵢ  (before /wsum²)
        let mut center_w = 0.0_f32;
        for (k, &w) in PILOT_TIME_SMOOTH_KERNEL.iter().enumerate() {
            let tt = t as isize + k as isize - half;
            if tt >= 0 && (tt as usize) < n_sym {
                let tt = tt as usize;
                h_sum += freqs[tt][pbin] * w;
                wsum += w;
                var_smooth_num += w * w * n0[tt];
                if (k as isize) == half {
                    center_w = w;
                }
            }
        }
        let h_raw = freqs[t][pbin];
        if wsum <= center_w {
            return h_raw; // only the center tap in range — nothing to average
        }
        let h_smooth = h_sum / wsum;
        let a_center = center_w / wsum; // normalized center weight
        let var_raw = n0[t];
        let var_smooth = var_smooth_num / (wsum * wsum);
        let cov = a_center * var_raw; // shared center-tap noise
        let var_diff = (var_raw + var_smooth - 2.0 * cov).max(0.0);
        let innovation = (h_raw - h_smooth).norm_sqr();
        let smear = (innovation - var_diff).max(0.0);
        let beta = ((var_raw - cov) / (var_diff + smear + 1e-20)).clamp(0.0, 1.0);
        h_raw + (h_smooth - h_raw) * beta
    }

    /// CP-strip → forward FFT → unitary scale: one symbol's time samples to its
    /// frequency bins. `samples.len()` must equal `fft_size + cp_len`.
    fn symbol_fft(&self, samples: &[f32]) -> Vec<Complex<f32>> {
        let p = self.params;
        let expected = p.fft_size() + p.cp_len();
        assert_eq!(samples.len(), expected, "OFDM RX symbol length mismatch");
        let mut freq: Vec<Complex<f32>> = samples[p.cp_len()..]
            .iter()
            .map(|s| Complex::new(*s, 0.0))
            .collect();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(p.fft_size());
        fft.process(&mut freq);
        let scale = 1.0 / (p.fft_size() as f32).sqrt();
        for c in freq.iter_mut() {
            *c *= scale;
        }
        freq
    }

    /// Per-bit LLRs across the data sub-carriers from an already-FFT'd symbol
    /// `freq`. The pilot bins in `freq` are the channel-estimation reference, so
    /// a caller that has time-smoothed them ([`Self::demodulate_frame`]) gets a
    /// correspondingly less-noisy channel estimate for free.
    fn llrs_from_freq(&self, freq: &[Complex<f32>], bits_per_subcarrier: &[u8]) -> Vec<f32> {
        let p = self.params;
        // Estimate the per-bin complex channel from the pilots (with edge
        // extrapolation). We do NOT zero-force: ZF normalizes every bin to the
        // constellation scale and so discards the per-subcarrier reliability
        // |h|² that the soft decoder needs to ride out a frequency-selective
        // null. Instead we feed (y, h) straight to the channel-aware LLR.
        let eq = OfdmEqualizer::new(p.pilot_indices().to_vec(), p.fft_size());
        let chan_est = eq.estimate_channel(freq);

        // Per-bin effective noise `n0_eff[k] = n0_thermal + var(e)`. The thermal
        // term tracks the real operating SNR; the channel-estimate-error term
        // keeps nulled sub-carriers near-erasures at high SNR (see
        // `effective_noise_per_bin`). The fixed override reproduces the legacy
        // flat-n0 demod for gate control / diagnostics.
        let n0_eff: Vec<f32> = match self.n0_override {
            Some(n0) => vec![n0; freq.len()],
            None => {
                let n0_thermal = estimate_noise_variance(freq, p);
                effective_noise_per_bin(freq, p, n0_thermal)
            }
        };

        // LLR per data sub-carrier in transmission order.
        let pilot_set: std::collections::HashSet<usize> =
            p.pilot_indices().iter().copied().collect();
        let mut all_llr = Vec::new();
        for (idx_in_sc, &sc) in p.subcarrier_indices().iter().enumerate() {
            if pilot_set.contains(&sc) {
                continue;
            }
            let bpc = bits_per_subcarrier[idx_in_sc] as usize;
            if bpc == 0 {
                continue;
            }
            let constellation = match bpc {
                1 => Constellation::Bpsk,
                2 => Constellation::Qpsk,
                4 => Constellation::Qam16,
                6 => Constellation::Qam64,
                _ => panic!("unsupported bit-loading: {bpc}"),
            };
            let mapper = Mapper::new(constellation);
            // Channel-aware LLR scaled by the per-bin effective noise: magnitude
            // scales with |h|²/n0_eff, so a nulled sub-carrier (small |h|, large
            // channel-estimate error → large n0_eff) yields a low-confidence
            // near-erasure rather than a fixed-magnitude wrong-sign value.
            let n0 = n0_eff[sc];
            let llrs = mapper.compute_llr_channel(&[freq[sc]], &[chan_est[sc]], n0);
            // Clip so a wrong-phase but non-small |h| estimate (e.g. interp
            // error near a notch) cannot inject an unbounded WRONG-sign LLR that
            // poisons the soft decoder. The clamp scales as 2.0/n0_eff to keep
            // the reliability ordering intact at every SNR (see LLR_CLAMP_NUM).
            let llr_clamp = LLR_CLAMP_NUM / n0;
            all_llr.extend(llrs.into_iter().map(|l| l.clamp(-llr_clamp, llr_clamp)));
        }
        all_llr
    }
}
