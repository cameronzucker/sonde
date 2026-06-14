//! Per-sub-carrier single-tap frequency-domain equalizer.
//!
//! The channel estimate is derived from pilot positions (transmitted as
//! `+1+0j`); data-position estimates come from linear interpolation
//! between adjacent pilots in bin order. The interpolation assumes the
//! channel's coherence bandwidth is wider than the pilot spacing — for
//! the every-4th-sub-carrier grid at 23–47 Hz bin width (Wide / Narrow
//! modes), that holds across the FT-818-class SSB passband under
//! ITU-R F.520 "moderate" multipath. Phase 11 may revisit for floors
//! with denser pilot grids.

use num_complex::Complex;

/// Pilot-aided single-tap frequency-domain equalizer bound to a fixed
/// FFT size and pilot-index set.
pub struct OfdmEqualizer {
    pilot_positions: Vec<usize>,
    n_bins: usize,
}

impl OfdmEqualizer {
    /// Construct an equalizer for a given pilot-position set and FFT
    /// bin count.
    pub fn new(pilot_positions: Vec<usize>, n_bins: usize) -> Self {
        Self {
            pilot_positions,
            n_bins,
        }
    }

    /// Estimate the per-bin complex channel from the pilot bins (which the
    /// transmitter emits as `+1+0j`, so the observed pilot bin *is* the
    /// channel). Data bins between pilots are linearly interpolated; bins
    /// outside the pilot span are held at the nearest pilot's estimate
    /// (edge extrapolation). Returns the full-spectrum channel estimate.
    ///
    /// Edge extrapolation matters: with an every-4th-bin pilot grid the
    /// occupied band can extend a few data bins past the last pilot (Wide
    /// mode's last pilot is bin 111, data bins 112–113 follow). Leaving those
    /// at a `1+0j` default would equalize them against a fictitious unit
    /// channel — wrong phase and magnitude under any real fade.
    ///
    /// `freq_bins.len()` must equal the `n_bins` passed at construction.
    pub fn estimate_channel(&self, freq_bins: &[Complex<f32>]) -> Vec<Complex<f32>> {
        assert_eq!(freq_bins.len(), self.n_bins);
        let mut chan_est = vec![Complex::new(1.0_f32, 0.0); self.n_bins];
        if self.pilot_positions.is_empty() {
            return chan_est;
        }
        for &pi in &self.pilot_positions {
            chan_est[pi] = freq_bins[pi]; // pilot was 1, so observed = channel.
        }
        // Linear interpolation between consecutive pilot positions.
        for window in self.pilot_positions.windows(2) {
            let a = window[0];
            let b = window[1];
            if b <= a + 1 {
                continue;
            }
            let h_a = chan_est[a];
            let h_b = chan_est[b];
            let span = (b - a) as f32;
            // `k` carries the load-bearing absolute bin index for the
            // interpolation weight `t = (k - a) / span`; rephrasing as
            // `iter_mut().enumerate().skip(...).take(...)` obscures
            // the math without buying type safety.
            #[allow(clippy::needless_range_loop)]
            for k in (a + 1)..b {
                let t = (k - a) as f32 / span;
                chan_est[k] = h_a * (1.0 - t) + h_b * t;
            }
        }
        // Edge extrapolation: hold the nearest pilot estimate beyond the
        // pilot span (before the first pilot, after the last) so occupied
        // data bins there are equalized against a real channel estimate.
        let first = self.pilot_positions[0];
        let last = *self.pilot_positions.last().unwrap();
        let h_first = chan_est[first];
        let h_last = chan_est[last];
        for h in chan_est.iter_mut().take(first) {
            *h = h_first;
        }
        for h in chan_est.iter_mut().skip(last + 1) {
            *h = h_last;
        }
        chan_est
    }

    /// Estimate the channel (see [`Self::estimate_channel`]) and apply a
    /// zero-forcing equalizer: `y · conj(h) / |h|²`, with a small floor to
    /// keep divisions numerically tame where the channel collapses. Returns
    /// the equalized full-spectrum vector.
    ///
    /// ZF discards per-subcarrier reliability (it normalizes every bin to the
    /// constellation scale regardless of `|h|`), so it is suited to
    /// hard-decision / flat-channel use. For soft-decision decoding under
    /// frequency-selective fading, the receiver computes channel-aware LLRs
    /// directly from `(y, h)` via
    /// [`crate::constellations::Mapper::compute_llr_channel`] instead.
    ///
    /// `freq_bins.len()` must equal the `n_bins` passed at construction.
    pub fn equalize(&self, freq_bins: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let chan_est = self.estimate_channel(freq_bins);
        freq_bins
            .iter()
            .zip(chan_est.iter())
            .map(|(r, h)| {
                let h2 = h.norm_sqr().max(1e-9);
                r * h.conj() / h2
            })
            .collect()
    }
}
