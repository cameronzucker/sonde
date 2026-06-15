# Converge round 3: fading-safe pilot smoothing (sonde-vb9 fix regressed fading)

Implemented your round-2 fix: time-smooth pilot channel estimates across symbols
(centered 9-tap triangular [1 2 3 4 5 4 3 2 1]/25), scoped per coded block, in
crates/sonde-phy/src/ofdm_main/receiver.rs::demodulate_frame.

RESULT — mixed:
- FLAT AWGN (no fading): coded threshold 12 dB → ~6 dB. Big win, as predicted.
- WATTERSON FADING (noise_estimate_gate, Good+Moderate, 8 seeds @ Eb/N0=25 dB):
  REGRESSED hard. estimated-n0 decode dropped 16/16 → 3/16 (Good 0/8, Mod 3/8).
  The fixed-n0=0.1 control also ~4/16. Pre-fix it was 16/16 vs 11/16.

Diagnosis: at 25 dB the pilots are ALREADY clean, so smoothing buys no noise
reduction — it only SMEARS the time-varying channel (and complex-averaging across
the Doppler phase rotation attenuates |h|, mismatching the full-magnitude y in the
-|y-h c|^2/n0 metric). So a FIXED-strength smoother trades fading margin for AWGN
gain. I need it adaptive: smooth hard only when pilots are noisy (the coded path's
real ~6 dB operating point), and barely at all when pilots are clean (high SNR /
fading).

Proposed fix — SNR-adaptive (Wiener-like) per-pilot blend:
  h_raw[t]    = freq[t][pbin]                       (per-symbol; tracks fading, noisy)
  h_smooth[t] = triangular time-average over symbols (de-noised; smears fading)
  beta[t]     = 1 / (1 + SNR_pilot[t])              SNR_pilot = |h_smooth|^2 / n0_thermal[t]
  h_used[t]   = (1-beta)*h_raw[t] + beta*h_smooth[t]
n0_thermal[t] is the existing per-symbol empty-bin estimate (estimate_noise_variance).
At high SNR beta→0 (no smoothing, fading preserved); at low SNR beta→~0.5 (heavy
smoothing, AWGN gain). Substitute h_used into the pilot bins, then the existing
equalizer/interpolation + channel-aware LLR run unchanged.

Questions (terse):
1. Is this SNR-adaptive blend the right shape? Refine beta (e.g. beta = 1/(1+SNR)
   vs SNR/(1+SNR) weighting, or a different SNR definition)? Should the blend be
   on the complex h, or should I smooth magnitude and phase separately to avoid
   the rotation-attenuation?
2. Is the gate's 25 dB operating point even still meaningful post-fix? The floor
   now operates ~6 dB; the noise_estimate_gate (sonde-gtg) was calibrated when the
   floor needed ~25 dB. Should that gate's GATE_EBN0_DB move down, independent of
   my change — and is it legitimate for me to re-point it (with re-measurement)?
3. Any simpler fading-safe option I'm missing (e.g. cap the kernel by measured
   Doppler, or only smooth the pilot MAGNITUDE not phase)?

Read crates/sonde-phy/src/ofdm_main/receiver.rs (demodulate_frame, llrs_from_freq,
estimate_noise_variance, effective_noise_per_bin) and
crates/sonde-phy/tests/noise_estimate_gate.rs. Give the adaptive formulation +
whether to re-point the gate.
