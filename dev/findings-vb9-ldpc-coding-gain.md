# sonde-vb9 investigation: the "LDPC coding gain" bug is a PHY channel-estimation bug

**Agent:** bison-slate-gorge · **Date:** 2026-06-14 · **Status:** root cause
confirmed + fix design Codex-converged; fix NOT yet implemented.

## TL;DR

`sonde-vb9` was filed as "rate-1/4 FloorRate14 LDPC delivers ~0 net coding gain."
It is **not a FEC bug.** The FEC code, SPA decoder, and H-matrix are excellent.
The coding gain is lost in the **OFDM receiver's per-symbol pilot channel
estimation**, which is too noisy at the coded path's low per-symbol SNR and flips
LLR signs. Fix: **time-smooth the pilot channel estimate across symbols.**

## Evidence chain (all reproducible; tests on this branch, `#[ignore]`d)

1. **FEC is fine** — `sonde-fec/tests/awgn_coding_gain.rs`. Codec-level AWGN with
   textbook LLRs (`2y/σ²`): coded FER → 0 by **Eb/N0 = 3 dB** (waterfall ~1–2 dB),
   the full ~7 dB gain vs uncoded BPSK. H-matrix / SPA(50) / interleaver all good.

2. **PHY loses it** — `sonde-phy/tests/floor_awgn_coding_gain.rs`. Full floor path
   over flat AWGN: coded ≈ / worse than uncoded (at Eb/N0=10, coded FER 0.90 vs
   uncoded BER 6.25e-4). (Test payload corrected 60 B→58 B so it's exactly one
   block; 60 B spilled to 2 blocks — Codex round-1 catch.)

3. **Not the clamp** — `LLR_CLAMP_NUM=2e9` (clamp off) barely moved coded FER
   (0.90→0.825 @10 dB).

4. **Not n0; it's the channel estimate** — `sonde-phy/tests/oracle_llr_localize.rs`
   (Codex-designed). Three arms on identical noisy one-block bare-OFDM samples:

   ```
    Eb/N0 |  (a) production   | (b) fixed-true-n0 | (c) oracle (known flat h)
      5.0 | FER 1.00 raw .284 | FER 1.00 raw .284 | FER 0.00 raw .173
      9.0 | FER 0.00 raw .124 | FER 0.00 raw .124 | FER 0.00 raw .069
   ```
   - (a) == (b) ⇒ **`effective_noise_per_bin` / `n0_eff` is NOT the bottleneck.**
   - oracle decodes @5 dB, production @9 dB ⇒ **pilot channel estimation ≈ 4 dB.**
   - `|g| = 0.5000` confirms the real-part TX amplitude convention.

5. **Raw (sign) BER is the tell** — `sonde-phy/tests/ofdm_llr_quality.rs`: bare-OFDM
   raw BER 24% @Eb/N0=6 (theory ~8%); |LLR| *does* rank reliability (lo-third 41%
   vs hi-third 8%) ⇒ soft info is usable, the channel the demod presents is just
   too noisy.

## Why it hits the coded path specifically

Rate-1/4 ⇒ each coded OFDM symbol sits ~6 dB below the uncoded per-symbol SNR at
the same Eb/N0. The every-4th-pilot, per-symbol channel estimate is interpolated
to data bins; at that low per-symbol SNR the estimate is noisy enough to flip LLR
signs. Uncoded runs 6 dB higher per-symbol where pilots are clean — so uncoded
BER looks fine while coded gain vanishes.

## Refuted hypotheses (tested, not assumed)

- Clamp flattening confident LLRs (probe: no change).
- `n0_eff` channel-est-error floor / pessimism (oracle arm (a)==(b)).
- Mirror/maximal-ratio combining recovering ~3 dB: **invalid.** After `c.re` the
  RX FFT input is real, so `freq[N−k] = conj(freq[k])` *including noise* — an
  exact conjugate duplicate, not an independent branch. MRC double-counts noise.
  The oracle-vs-codec residual is irreducible PHY overhead (pilots+pad 1.31 dB +
  CP 0.97 dB = 2.28 dB), not lost mirror energy.

## Converged fix plan (Codex round 2)

1. **Time-smooth pilot channel estimates across symbols** (the ~4 dB lever).
   Restructure to a block/frame demod: FFT all symbols, collect pilot
   observations `Y[t][p]`, smooth in TIME per pilot, then run the existing
   frequency interpolation per symbol. Centered 9-symbol triangular
   `[1 2 3 4 5 4 3 2 1]/25` (~0.48 s span, N_eff≈7.4 ⇒ ~8 dB pilot-noise cut,
   ~−2 dB at 1 Hz Doppler — safe for ≤1 Hz). Streaming fallback: EMA α≈0.3–0.4.
2. **Re-check `n0_eff` floor** after smoothing (may become the next limiter). Gate
   arms: production / smoothed+true-n0 / smoothed+n0_eff / oracle. Target:
   smoothed production within ~0.5 dB of oracle on flat AWGN.
3. 2-D time×freq Wiener: only if Watterson Poor still fails (+0.5–1.5 dB, fading).
4. Decision-directed refinement: last, soft + CRC-gated.
5. Denser pilots: rejected (costs ~1.7 dB overhead first).

**Expected:** coded flat-AWGN threshold ~9 dB → ~5–5.5 dB. Credible Step-3
(sonde-xhw.4) coded-vs-uncoded gate target ≈ **5.5–6 dB** (codec 3 dB + 2.3 dB
PHY overhead + margin), NOT the codec-only 3 dB.

## Interaction with other issues

- `sonde-xhw.5` (Eb/N0 labels inflated ~6.7 dB): consistent — much of that
  "inflation" is real PHY overhead (pilots+CP+guard) + the amplitude convention,
  measured here as ~2.3 dB irreducible + the estimation loss.
- `sonde-xhw.4` (Step 3 coded-fading gate): blocked on this fix; the gate's
  stated coded operating point should be ~5.5–6 dB, not 3 dB.
- The fix touches the demod the Watterson fading gate rides — regression-test the
  fading gate (the smoother must not smear ≤1 Hz Doppler).
