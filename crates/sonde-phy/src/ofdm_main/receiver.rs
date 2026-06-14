//! OFDM receiver: time-domain samples → CP stripping → FFT →
//! pilot-aided equalization → per-bit LLR computation across data
//! sub-carriers.

use crate::constellations::{Constellation, Mapper};
use crate::ofdm_main::equalizer::OfdmEqualizer;
use crate::ofdm_main::ofdm_params::OfdmParams;
use num_complex::Complex;
use rustfft::FftPlanner;

/// Symmetric clip applied to each soft LLR before it leaves the demodulator.
/// Bounds the damage an over-confident WRONG-sign LLR (from channel-estimate
/// interpolation error near a spectral notch) can do to the soft FEC decoder.
/// At `n0 = 0.1` a BPSK sub-carrier saturates here once `|h| ≳ 0.71`, so the
/// strongest constructive fades are intentionally capped — fine, since past
/// that point the bit is already maximally reliable and the extra magnitude
/// buys the decoder nothing.
const LLR_CLAMP: f32 = 20.0;

/// Single-symbol OFDM receiver bound to one resolved [`OfdmParams`]
/// mode.
pub struct OfdmReceiver<'a> {
    params: &'a OfdmParams,
}

impl<'a> OfdmReceiver<'a> {
    /// Construct a receiver bound to the given mode parameters.
    pub fn new(params: &'a OfdmParams) -> Self {
        Self { params }
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
        let p = self.params;
        let expected = p.fft_size() + p.cp_len();
        assert_eq!(samples.len(), expected, "OFDM RX symbol length mismatch");

        // Drop CP, promote to complex baseband for FFT.
        let body: Vec<Complex<f32>> = samples[p.cp_len()..]
            .iter()
            .map(|s| Complex::new(*s, 0.0))
            .collect();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(p.fft_size());
        let mut freq = body;
        fft.process(&mut freq);
        let scale = 1.0 / (p.fft_size() as f32).sqrt();
        for c in freq.iter_mut() {
            *c *= scale;
        }

        // Estimate the per-bin complex channel from the pilots (with edge
        // extrapolation). We do NOT zero-force: ZF normalizes every bin to the
        // constellation scale and so discards the per-subcarrier reliability
        // |h|² that the soft decoder needs to ride out a frequency-selective
        // null. Instead we feed (y, h) straight to the channel-aware LLR.
        let eq = OfdmEqualizer::new(p.pilot_indices().to_vec(), p.fft_size());
        let chan_est = eq.estimate_channel(&freq);

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
            // Channel-aware LLR: magnitude scales with |h|², so a nulled
            // sub-carrier yields a low-confidence near-erasure rather than a
            // fixed-magnitude (possibly wrong-sign) zero-forced value. N0 is a
            // fixed proxy; Phase 11 refines via a residual-noise estimator.
            let n0 = 0.1_f32;
            let llrs = mapper.compute_llr_channel(&[freq[sc]], &[chan_est[sc]], n0);
            // Clip so a wrong-phase but non-small |h| estimate (e.g. interp
            // error near a notch) cannot inject an unbounded WRONG-sign LLR
            // that poisons the soft decoder.
            all_llr.extend(llrs.into_iter().map(|l| l.clamp(-LLR_CLAMP, LLR_CLAMP)));
        }
        all_llr
    }
}
