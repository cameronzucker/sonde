# Converge: estimate noise variance n0 for the floor's channel-aware LLR (sonde-gtg)

Context: clean-sheet HF OFDM modem, Rust. The wideband "robustness floor" is
BPSK-per-subcarrier OFDM + rate-1/4 LDPC. Its receiver
(`ofdm_main/receiver.rs::demodulate_one_symbol`) computes channel-aware soft
LLRs with a HARDCODED `n0 = 0.1`. Measured (sonde-xhw.3): the floor decodes
Watterson Good/Moderate only above ~Eb/N0 25 dB; below that the LLR magnitudes
are mis-scaled and the LDPC fails. This caps low-SNR/HF operation and is the
binding constraint for the next coded-fading gate. Replace 0.1 with a real
per-bin noise-variance estimate.

## Fixed mechanics (don't propose changing the waveform)
- Wide mode: FFT N=2048, CP=512, SR=48000. Occupied sub-carriers = a contiguous
  slab `subcarrier_indices` (Wide: bins ~15..113, center 1500 Hz, ~2300 Hz).
  Pilots every 4th occupied bin (emitted as +1+0j). BPSK data on the rest.
- TX: fill occupied bins, IFFT (unitary 1/√N), CP, take `Re{·}` (real passband;
  a fixed 3 dB penalty), raised-cosine inter-symbol windowing in the CP, 12 dB
  soft-clip for PAPR.
- RX per symbol: drop CP → `Complex::new(s,0)` → FFT (unitary 1/√N) → `freq[k]`.
  This real→complex cast mirrors the occupied slab to bins `N-113..N-15`. Pilot-
  aided channel estimate `h = chan_est[sc]` (observed pilot bin = channel, since
  pilot=1; linear interp + edge hold between pilots). LLR metric (max-log):
  `metric(c) = −|y − h·c|² / n0`, `y = freq[sc]`. So n0 must be the noise
  variance ON `y`, in the SAME units as `y` and `h·c`.

## PROPOSED estimator (challenge it)
**Empty-bin noise power.** OFDM orthogonality puts every occupied sub-carrier on
an integer FFT bin, so (ideal) it contributes ZERO to other integer bins. The
~1600 UNOCCUPIED bins (outside `occupied ∪ mirror`, away from DC/Nyquist) carry
only noise. For real white noise of per-sample variance σ², the unitary DFT
gives `E[|freq[k]|²] = σ²` for every bin → the occupied bins' noise variance
equals the empty bins'. So:

```
n0_est = mean over k in EMPTY of |freq[k]|²
EMPTY = { k : k not within GUARD of (occupied ∪ mirror), DC, or Nyquist }
```

Use it per-symbol (≈1600 samples → already stable), feeding `compute_llr_channel`
in place of 0.1. Keep the existing ±20 LLR clamp. Floor n0_est at a small ε to
avoid div-by-zero on a (near-)noiseless clean capture.

## Questions (terse answers)
Q1: Is empty-bin `mean|freq|²` the right n0 here — units consistent with the
`−|y−h·c|²/n0` metric (note the 3 dB real-passband halving applies to BOTH `y`
and `h`, and the noise estimate lives in the same `freq` domain, so I claim it's
self-consistent — confirm or correct)?
Q2: Bias from the 12 dB soft-clip + windowing leakage into empty bins: at HIGH
SNR the empty bins pick up clip/leakage distortion (mask ≤ −26 dBc), biasing
n0_est high. Does that matter — it only inflates n0 where the link already
decodes, and the channel-aware |h|² weighting is preserved; an overall n0 scale
mostly shifts SPA convergence, not the sign? Or should I pick empty bins FAR
from the occupied band (least leakage) / use a robust statistic (median of
|freq|²) instead of mean?
Q3: Per-symbol vs per-frame averaging of n0_est? Per-symbol over ~1600 bins
seems plenty; per-frame is more stable but assumes stationary noise across the
~28-symbol frame. Which?
Q4: Mean vs median vs trimmed-mean of `|freq[k]|²` over EMPTY — the clip
distortion and any stray spectral spurs are positive outliers; is a median (×
the |freq|²→variance bias factor) worth it, or is the mean fine given the large
sample count?
Q5: Anything physically wrong, or a better estimator I'm missing (pilot-residual
vs a smoothed channel fit has only ~25 samples → noisier; decision-directed adds
a feedback loop — I think empty-bin dominates)?
Q6: The GATE (gate-on-physics): I'll assert the n0 estimate LOWERS the operating
Eb/N0 — a differential gate where, on the same Watterson realizations + AWGN, the
n0-estimated demod decodes at an Eb/N0 where the fixed-0.1 demod FAILS, plus a
decode-rate-vs-Eb/N0 curve quantifying the dB improvement. Is a differential
(estimated-beats-fixed) gate the right physics bar, and what decode-rate /
Eb/N0-shift threshold would you call a credible pass (vs noise)?

Be terse; confirm/correct and give the numbers (guard width, mean vs median,
per-symbol vs frame, gate threshold).
