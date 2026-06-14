# Converge round 2: the FIX for sonde-vb9 (coded-path SNR loss localized)

Follow-up to round 1. I ran your oracle-LLR experiment. Decisive results below.
Now I need the fix design. Be terse, concrete, gate-on-physics.

## What the experiment proved (oracle_llr_localize.rs, one rate-1/4 block, bare
## OFDM: no clip/window/framing; identical noisy samples across 3 arms)

`|g| mean = 0.5000` (the real-part TX halves the per-subcarrier amplitude).

```
 Eb/N0 |  (a) production    |  (b) fixed-true-n0  |  (c) oracle (known flat h)
   3.0 |  FER 1.00 raw .351 |  FER 1.00 raw .351  |  FER 0.93 raw .228
   5.0 |  FER 1.00 raw .284 |  FER 1.00 raw .284  |  FER 0.00 raw .173
   7.0 |  FER 0.20 raw .206 |  FER 0.23 raw .206  |  FER 0.00 raw .117
   9.0 |  FER 0.00 raw .124 |  FER 0.00 raw .124  |  FER 0.00 raw .069
```

- (a) production == (b) fixed-true-n0  ⇒ **`effective_noise_per_bin` / `n0_eff`
  is NOT the bottleneck** (your round-1 lead suspect, refuted).
- (c) oracle (true flat `h`, true σ²) decodes at **Eb/N0 = 5 dB**; production
  needs **~9 dB** ⇒ **pilot channel estimation costs ~4 dB.**
- Even the oracle decodes ~2 dB worse than the codec-with-textbook-LLR result
  (3 dB). That residual is the **real-part mirror discard** (|g|=0.5; the RX
  reads only the positive occupied bins, discarding the conjugate copy at N−k).

So the coded path loses ~4 dB to pilot channel estimation + ~2–3 dB to the
mirror discard. The FEC, SPA, and LLR formula are all correct.

## Constraints / context for the fix

- The floor targets slow HF fading: Watterson Good/Mod/Poor, Doppler ≤1 Hz,
  coherence ≥ ~1 s. One OFDM symbol = 2048 FFT + 512 CP = 2560 samples @ 48 kHz
  = **53 ms**. A rate-1/4 block ≈ 29 symbols ≈ **1.5 s** — so the channel is NOT
  constant across a block under fading (it IS across an AWGN block). Pilots are
  every 4th occupied subcarrier; per-symbol pilot estimate interpolated to data
  bins (`equalizer.rs`).
- The TX must emit a REAL passband audio signal for SSB (`modulate_one_symbol`
  step 5 takes `c.re`). The conjugate mirror at N−k is a redundant copy of each
  data bin (for BPSK, real data) — currently unused by the RX.
- `crates/sonde-phy/src/ofdm_main/{equalizer.rs,receiver.rs,transmitter.rs}` and
  `robustness_floor/wideband_lowdensity.rs` are the relevant files.

## Questions

1. **Channel estimation (the ~4 dB lever).** Per-symbol pilot estimation is too
   noisy at the coded path's low per-symbol SNR (rate-1/4 ⇒ Es/N0 ~6 dB below
   uncoded at the same Eb/N0). How do I cut that ~4 dB while staying valid under
   ≥1 s coherence fading? Candidates to rank/critique:
   - Time-smooth the per-bin channel estimate across symbols (EMA / Wiener)
     with a window matched to coherence (~1 s ≈ ~19 symbols) — heavy averaging
     for AWGN, still tracking for ≤1 Hz Doppler.
   - Decision-directed / iterative refinement (re-estimate h from tentative
     decisions after a first decode pass).
   - 2-D (time×freq) pilot interpolation instead of per-symbol freq-only.
   - Denser pilots (costs rate) — last resort.
   Which gives the best dB/complexity, and what window/forgetting factor is
   defensible for ≤1 Hz Doppler @ 53 ms symbols?

2. **Mirror combining (the ~3 dB lever).** Is maximal-ratio combining the
   positive bin `freq[k]` with the conjugate mirror `conj(freq[N−k])` the right
   way to recover the real-part loss, and is it sound here (does the real-cast
   put an exact conjugate replica at N−k with independent noise)? Any catch for
   BPSK vs the pilots? Or is a Hermitian-symmetric (real-baseband) OFDM redesign
   the cleaner fix?

3. **Sequencing.** Which fix first, and what's the expected dB recovery for each?
   I want a credible coded-vs-uncoded coding-gain gate (Step 3 / sonde-xhw.4)
   afterward — what end-to-end coded operating Eb/N0 should I expect once both
   are fixed (codec alone is ~3 dB; how much PHY overhead is irreducible:
   pilots + CP + guard)?

4. Anything I'm missing or any way this localization could still be wrong?

Read the listed files. Give me a ranked, sequenced fix plan with expected dB.
