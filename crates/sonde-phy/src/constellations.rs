//! Constellation mapping + LLR computation.
//!
//! Per PHY spec §3.Q3 the constellation set scales from BPSK (used by
//! the wide-band low-density floor) through QPSK, 16-QAM, 64-QAM
//! (bit-loaded per sub-carrier in the OFDM main family). Gray-coded
//! mappings throughout.

use num_complex::Complex;

/// The four supported constellations. Bit-loading (Phase 7) picks one
/// per sub-carrier per OFDM mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Constellation {
    /// 1 bit/symbol; symbols at ±1 on the real axis.
    Bpsk,
    /// 2 bits/symbol; symbols on the unit circle at ±1/√2 ± j/√2.
    Qpsk,
    /// 4 bits/symbol; Gray-coded square 16-QAM, unit average energy.
    Qam16,
    /// 6 bits/symbol; Gray-coded square 64-QAM, unit average energy.
    Qam64,
}

impl Constellation {
    /// Bits per modulated symbol for this constellation.
    pub fn bits_per_symbol(&self) -> usize {
        match self {
            Constellation::Bpsk => 1,
            Constellation::Qpsk => 2,
            Constellation::Qam16 => 4,
            Constellation::Qam64 => 6,
        }
    }
}

/// Bit ↔ complex-symbol mapper for one constellation.
pub struct Mapper {
    constellation: Constellation,
}

impl Mapper {
    /// Construct a mapper for the given constellation.
    pub fn new(c: Constellation) -> Self {
        Self { constellation: c }
    }
    /// Underlying constellation.
    pub fn constellation(&self) -> Constellation {
        self.constellation
    }

    /// Map a bit sequence to a complex-symbol sequence.
    pub fn map(&self, bits: &[u8]) -> Vec<Complex<f32>> {
        match self.constellation {
            Constellation::Bpsk => bits
                .iter()
                .map(|b| {
                    if *b == 0 {
                        Complex::new(1.0, 0.0)
                    } else {
                        Complex::new(-1.0, 0.0)
                    }
                })
                .collect(),
            Constellation::Qpsk => {
                let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
                bits.chunks(2)
                    .map(|c| {
                        let i = if c[0] == 0 { inv_sqrt2 } else { -inv_sqrt2 };
                        let q = if c.get(1).copied().unwrap_or(0) == 0 {
                            inv_sqrt2
                        } else {
                            -inv_sqrt2
                        };
                        Complex::new(i, q)
                    })
                    .collect()
            }
            Constellation::Qam16 => {
                // 4-bit Gray-coded square 16-QAM. Bits laid out
                // [b3 b2 b1 b0] where (b3,b2) selects I and (b1,b0)
                // selects Q from Gray-coded levels {0->-3, 1->-1, 3->+1, 2->+3}.
                let gray_level: [f32; 4] = [-3.0, -1.0, 3.0, 1.0];
                // avg power for square 4x4 levels {-3,-1,1,3} = 10 → norm 1/sqrt(10).
                let norm = 1.0 / 10.0_f32.sqrt();
                bits.chunks(4)
                    .map(|c| {
                        let i_lvl = gray_level[((c[0] << 1) | c[1]) as usize];
                        let q_lvl = gray_level[((c[2] << 1) | c[3]) as usize];
                        Complex::new(i_lvl * norm, q_lvl * norm)
                    })
                    .collect()
            }
            Constellation::Qam64 => {
                let gray_level: [f32; 8] = [-7.0, -5.0, -1.0, -3.0, 7.0, 5.0, 1.0, 3.0];
                // avg power for 64-QAM = (1/64)*sum = 42 for square 8x8 {-7..7} → norm 1/sqrt(42).
                let norm = 1.0 / 42.0_f32.sqrt();
                bits.chunks(6)
                    .map(|c| {
                        let i_idx = ((c[0] << 2) | (c[1] << 1) | c[2]) as usize;
                        let q_idx = ((c[3] << 2) | (c[4] << 1) | c[5]) as usize;
                        Complex::new(gray_level[i_idx] * norm, gray_level[q_idx] * norm)
                    })
                    .collect()
            }
        }
    }

    /// Hard-decision demap: most-likely transmitted bit sequence given received symbols.
    pub fn hard_demap(&self, syms: &[Complex<f32>]) -> Vec<u8> {
        match self.constellation {
            Constellation::Bpsk => syms
                .iter()
                .map(|s| if s.re >= 0.0 { 0 } else { 1 })
                .collect(),
            Constellation::Qpsk => {
                let mut out = Vec::with_capacity(syms.len() * 2);
                for s in syms {
                    out.push(if s.re >= 0.0 { 0 } else { 1 });
                    out.push(if s.im >= 0.0 { 0 } else { 1 });
                }
                out
            }
            Constellation::Qam16 | Constellation::Qam64 => {
                // For QAM, hard_demap routes through max-log LLR + sign,
                // which avoids per-Gray-table demap-axis logic that diverges
                // from the canonical reflected-Gray sequence in subtle ways.
                let llrs = self.compute_llr(syms, 1.0);
                llrs.iter().map(|l| if *l >= 0.0 { 0 } else { 1 }).collect()
            }
        }
    }

    /// Compute per-bit log-likelihood ratios using the max-log approximation.
    /// Returns one LLR per bit in transmission order.
    /// LLR positive ⇒ bit=0 favoured; negative ⇒ bit=1 favoured.
    /// `n0` is the noise-variance estimate.
    pub fn compute_llr(&self, syms: &[Complex<f32>], n0: f32) -> Vec<f32> {
        let inv = 1.0 / n0.max(1e-9);
        let bps = self.constellation.bits_per_symbol();
        let mut out = Vec::with_capacity(syms.len() * bps);
        // Brute-force max-log over the constellation: tractable up to 64-QAM.
        let alphabet = self.alphabet();
        for s in syms {
            for bit_idx in 0..bps {
                let mut max0 = f32::NEG_INFINITY;
                let mut max1 = f32::NEG_INFINITY;
                for (bit_pattern, c) in &alphabet {
                    let dist = (s - c).norm_sqr();
                    let metric = -dist * inv;
                    if (bit_pattern >> (bps - 1 - bit_idx)) & 1 == 0 {
                        if metric > max0 {
                            max0 = metric;
                        }
                    } else if metric > max1 {
                        max1 = metric;
                    }
                }
                out.push(max0 - max1);
            }
        }
        out
    }

    /// Channel-aware per-bit LLRs (max-log) given the received symbol `y`,
    /// the per-symbol complex channel estimate `h`, and noise variance `n0`.
    ///
    /// Metric over the unit-power constellation: `metric(c) = -|y - h·c|² / n0`.
    /// Unlike [`Self::compute_llr`] (which assumes the symbol has already been
    /// equalized to the constellation scale), this folds the channel into the
    /// likelihood, so the LLR magnitude scales with the per-subcarrier
    /// reliability `|h|²`. On a deep frequency-selective null (`|h|→0`) the LLRs
    /// collapse toward zero — a low-confidence near-erasure the FEC can bridge —
    /// rather than a zero-forced, fixed-magnitude WRONG-sign value that poisons
    /// the soft decoder. For BPSK this reduces to `4·Re(conj(h)·y)/n0`.
    ///
    /// `syms` and `chans` must have equal length (one channel tap per symbol).
    /// Returns `bits_per_symbol()` LLRs per symbol in transmission order; LLR
    /// positive ⇒ bit=0 favoured.
    pub fn compute_llr_channel(
        &self,
        syms: &[Complex<f32>],
        chans: &[Complex<f32>],
        n0: f32,
    ) -> Vec<f32> {
        assert_eq!(
            syms.len(),
            chans.len(),
            "compute_llr_channel: syms/chans length mismatch"
        );
        let inv = 1.0 / n0.max(1e-9);
        let bps = self.constellation.bits_per_symbol();
        let mut out = Vec::with_capacity(syms.len() * bps);
        let alphabet = self.alphabet();
        for (y, h) in syms.iter().zip(chans.iter()) {
            for bit_idx in 0..bps {
                let mut max0 = f32::NEG_INFINITY;
                let mut max1 = f32::NEG_INFINITY;
                for (bit_pattern, c) in &alphabet {
                    let dist = (y - h * c).norm_sqr();
                    let metric = -dist * inv;
                    if (bit_pattern >> (bps - 1 - bit_idx)) & 1 == 0 {
                        if metric > max0 {
                            max0 = metric;
                        }
                    } else if metric > max1 {
                        max1 = metric;
                    }
                }
                out.push(max0 - max1);
            }
        }
        out
    }

    fn alphabet(&self) -> Vec<(usize, Complex<f32>)> {
        let bps = self.constellation.bits_per_symbol();
        let n = 1usize << bps;
        let mut bits = vec![0u8; bps];
        let mut out = Vec::with_capacity(n);
        for code in 0..n {
            for (i, b) in bits.iter_mut().enumerate() {
                *b = ((code >> (bps - 1 - i)) & 1) as u8;
            }
            let sym = self.map(&bits);
            out.push((code, sym[0]));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// For BPSK the channel-aware max-log LLR must equal the closed form
    /// `4·Re(conj(h)·y)/n0`. This is the load-bearing property: the LLR
    /// magnitude tracks |h|², so a faded (small-|h|) subcarrier yields a
    /// low-confidence value instead of a fixed-magnitude (possibly wrong-sign)
    /// zero-forced one.
    #[test]
    fn bpsk_channel_llr_matches_closed_form() {
        let mapper = Mapper::new(Constellation::Bpsk);
        let n0 = 0.37_f32;
        let cases = [
            (Complex::new(0.8, 0.1), Complex::new(1.0, 0.0)),
            (Complex::new(-0.6, 0.3), Complex::new(0.2, -0.9)),
            // Deep-null channel: tiny |h| ⇒ tiny LLR (near-erasure).
            (Complex::new(0.5, -0.4), Complex::new(0.03, 0.02)),
        ];
        for (y, h) in cases {
            let got = mapper.compute_llr_channel(&[y], &[h], n0);
            let want = 4.0 * (h.conj() * y).re / n0;
            assert_eq!(got.len(), 1);
            assert!(
                (got[0] - want).abs() < 1e-3,
                "BPSK channel LLR {} != closed form {} for y={y} h={h}",
                got[0],
                want
            );
        }
    }

    /// With a unit channel (`h = 1+0j`), the channel-aware LLR must reproduce
    /// the plain equalized-sample LLR exactly, for every constellation — the
    /// new path is a strict generalization, not a behavior change on the clean
    /// channel the existing OFDM tests rely on.
    #[test]
    fn unit_channel_matches_plain_llr_all_constellations() {
        let h = Complex::new(1.0_f32, 0.0);
        let n0 = 0.2_f32;
        let syms = [
            Complex::new(0.9, -0.2),
            Complex::new(-0.3, 0.7),
            Complex::new(0.1, 0.1),
            Complex::new(-0.8, -0.6),
        ];
        let chans = [h; 4];
        for c in [
            Constellation::Bpsk,
            Constellation::Qpsk,
            Constellation::Qam16,
            Constellation::Qam64,
        ] {
            let mapper = Mapper::new(c);
            let plain = mapper.compute_llr(&syms, n0);
            let channel = mapper.compute_llr_channel(&syms, &chans, n0);
            assert_eq!(plain.len(), channel.len(), "{c:?}: LLR count differs");
            for (a, b) in plain.iter().zip(channel.iter()) {
                assert!(
                    (a - b).abs() < 1e-4,
                    "{c:?}: unit-channel LLR {b} != plain LLR {a}"
                );
            }
        }
    }

    /// A near-null channel must yield a far smaller LLR magnitude than a
    /// strong channel for the same normalized received symbol — the reliability
    /// weighting the soft decoder needs.
    #[test]
    fn bpsk_null_subcarrier_is_low_confidence() {
        let mapper = Mapper::new(Constellation::Bpsk);
        let n0 = 0.1_f32;
        let strong =
            mapper.compute_llr_channel(&[Complex::new(1.0, 0.0)], &[Complex::new(1.0, 0.0)], n0)[0];
        let nulled =
            mapper.compute_llr_channel(&[Complex::new(0.05, 0.0)], &[Complex::new(0.05, 0.0)], n0)
                [0];
        assert!(
            nulled.abs() < strong.abs() * 0.1,
            "null LLR {nulled} should be << strong LLR {strong}"
        );
    }
}
