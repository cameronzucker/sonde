# Converge: why does the rate-1/4 LDPC give ~0 coding gain THROUGH THE PHY? (sonde-vb9)

Clean-sheet HF OFDM modem. A rate-1/4 LDPC floor code shows essentially **zero
net coding gain** (sometimes WORSE than uncoded) when run end-to-end through the
OFDM PHY, even though the code itself is excellent. I need your adversarial DSP
read on where the OFDM demod LLR path loses the gain. Be terse and concrete.

## Evidence gathered (all reproducible; this is NOT speculation)

1. **The FEC code + SPA decoder are FINE.** `crates/sonde-fec/tests/awgn_coding_gain.rs`
   feeds the `FloorRate14Codec` textbook BPSK+AWGN LLRs (`LLR = 2y/σ²`),
   bypassing the PHY. Result: coded frame-error-rate → 0 by **Eb/N0 = 3 dB**
   (waterfall ~1–2 dB), the full ~7 dB gain vs uncoded BPSK. So the H-matrix
   (n=2048,k=512 IRA/dual-diagonal), the SPA (50 iters), and the interleaver are
   all good.

2. **Through the PHY, the gain vanishes.** `crates/sonde-phy/tests/floor_awgn_coding_gain.rs`
   runs `WidebandLowDensityFloor` coded (FloorRate14) vs uncoded (IdentityFec)
   over flat AWGN via `receive_multi` (no sync), rate-aware Eb/N0:
   ```
     Eb/N0 | coded FER | uncoded FER  uncoded BER
      8.0  |  1.0000   |  0.9000      8.31e-2
     10.0  |  0.9000   |  0.2750      6.25e-4
     12.0  |  0.0000   |  0.0250      5.21e-5
   ```
   Coded is no better than (worse than, at 10 dB) uncoded. A rate-1/4 code that
   performs WORSE than uncoded is a bug, not a weak code.

3. **Clamp ruled out.** Setting `LLR_CLAMP_NUM = 2e9` (clamp effectively off)
   changed coded FER @10dB only 0.90 → 0.825.

4. **n0 channel-est-error floor ruled out** (prior measurement: disabling it left
   coded FER unchanged).

5. Uncoded BER through the same demod is reasonable (signs are correct), so the
   demod produces correct LLR **signs**; the lost gain is in the **magnitudes /
   reliability information** the soft decoder rides.

## The suspect code

- LLR generation + per-bin n0: `crates/sonde-phy/src/ofdm_main/receiver.rs`
  (`demodulate_one_symbol`, `estimate_noise_variance`, `effective_noise_per_bin`,
  the `LLR_CLAMP_NUM` clamp). The channel-aware LLR is
  `compute_llr_channel` in `crates/sonde-phy/src/constellations.rs`
  (`metric = -|y - h·c|²/n0`, BPSK → `4·Re(conj(h)·y)/n0`).
- Floor TX/RX + framing + PAPR soft-clip + inter-symbol windowing:
  `crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs`
  (`transmit_multi` applies `soft_clip_to_papr` at PAPR_TARGET_DB=12 and
  `window_and_concat`; `receive_multi` demodulates per symbol).
- Pilot equalizer (channel estimate, every-4th-subcarrier pilots interpolated to
  data bins): `crates/sonde-phy/src/ofdm_main/equalizer.rs`.

## Questions

1. Given signs are right but coding gain is gone, what is the **most likely**
   single root cause in this OFDM demod LLR path? Rank your top candidates.
   My current leading suspects to challenge:
   - PAPR **soft-clip** (`soft_clip_to_papr`, 12 dB) distorts the constellation;
     the clip distortion is non-Gaussian and unmodeled in n0. Over the 4×-longer
     coded frame at the rate-reduced per-symbol SNR, could clip distortion be a
     noise floor the code can't bridge? (But uncoded suffers it too…)
   - Pilot channel-estimate noise (3-of-4 bins interpolated) injecting a
     per-bin reliability error that's correlated/structured, not the i.i.d. AWGN
     the codec sweep assumed.
   - n0 (per-bin `n0_eff`) mis-scaled so LLR magnitudes don't rank reliability
     (SPA degenerates toward hard-decision → loses the soft-decoding gain).
   - Something making the per-bit LLR magnitudes ~constant (sign-only),
     destroying the reliability ordering the SPA needs.
2. What is the **single most discriminating experiment** to localize it? I'm
   inclined toward symbol-level instrumentation: drive one OFDM symbol
   (`OfdmTransmitter::modulate_one_symbol`) over AWGN, demod with
   `OfdmReceiver::demodulate_one_symbol`, and compare (a) the demod's LLR
   sign-error-rate to the theoretical BPSK BER at that per-subcarrier Es/N0, and
   (b) the |LLR| distribution / how well |LLR| ranks the actually-wrong bits.
   Better idea?
3. If the soft-clip is the culprit, what's the right fix that keeps PAPR in the
   SSB-PA budget without flattening coding gain (e.g., clip less aggressively,
   model clip distortion in n0, or move clipping out of the per-frame path)?
4. Sanity-check the rate-aware Eb/N0 normalization in the PHY test
   (`add_awgn`: σ² = P_s·L/(K_info·2·Eb/N0)) — is the coded-vs-uncoded
   comparison fair, or is the test itself unfair to the coded (4×-longer, CP+
   windowing overhead) arm?

Read the files above. Give me: ranked root-cause, the one experiment to run next,
and the fix direction. Keep it honest — gate on physics, no hand-waving.
