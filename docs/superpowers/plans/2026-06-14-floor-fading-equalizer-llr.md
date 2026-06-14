# Plan — floor fading decode: channel-aware soft LLR (sonde-64w.2)

**Status:** in progress · **Agent:** bayou-crag-granite · **Date:** 2026-06-14
**Branch:** `sonde-64w.2/floor-equalizer` · **Epic:** sonde-64w

## Root cause (instrumented + Codex-converged — supersedes the prior timing theory)

The prior session's "late preamble sync → ISI" hypothesis is **refuted**:

- Instrumentation (`zz_fading_instrument.rs`): Watterson Good detects at
  `start_sample = 0` (perfect, *not* 24 late); **perfect-alignment decode
  (correlator bypassed) FAILS** for both Good and Moderate; the decode-basin
  sweep over body-start ∈ [−320, +160] is **empty**. So no timing fix (A1
  guard, larger G, earliest-peak A2) can green the gate.
- The real cause is a **deep frequency-selective null** from the 2-tap Watterson
  channel `H[k] = (f1 + f2·e^(−jθk))/√2`. Measured pilot-|Y| spread: 16.5× (Good,
  one notch) / 27× (Moderate, two notches), notch at ~4–6 % of peak.
- The receiver does **zero-forcing** (`r·conj(h)/|h|²`) then computes LLRs with a
  **fixed scalar `n0 = 0.1`**. At the null this amplifies a wrong-phase
  interpolated estimate into **high-confidence WRONG-sign LLRs** — poison to the
  LDPC SPA decoder (worse than erasures). ZF discards the per-subcarrier
  reliability the code needs. Even the real rate-1/4 LDPC fails today (CRC
  mismatch).

A spectral null destroys *uncoded* bits irrecoverably — that is physics. The
floor's real mode is rate-1/4 LDPC + interleaver, designed to bridge nulls. The
IdentityFec fading gate was the wrong harness.

## Change set (converged with Codex; no substantial disagreement)

1. **`constellations.rs`** — add `Mapper::compute_llr_channel(&self, syms, chans, n0)`:
   max-log `metric(c) = −|Y − H·c|² / N0` over the alphabet. For BPSK this equals
   `4·Re(conj(H)·Y)/N0`, so the LLR magnitude ∝ |H|² → nulled subcarriers become
   low-confidence near-erasures. Keep `compute_llr` for hard decisions/tests.
   Unit test: BPSK equivalence.
2. **`ofdm_main/equalizer.rs`** — add `estimate_channel()` (pilot estimate + linear
   interpolation + **edge extrapolation** with the nearest pilot beyond the pilot
   range — bins past the last pilot currently default to `1+0j`, a latent bug:
   Wide's last pilot is bin 111, so data bins 112–113 are unestimated). Keep
   `equalize()` (ZF) for any hard-decision callers.
3. **`ofdm_main/receiver.rs`** — FFT → `estimate_channel` → channel-aware LLRs via
   `compute_llr_channel`, with **LLR clipping** (±clamp) so a wrong-phase but
   non-small |H_est| can't re-poison SPA.
4. **`crates/sonde-phy/tests/robustness_floor_fading.rs`** — rewrite to use
   `with_fec(FloorRate14Codec)`; assert payload decode through Good + Moderate
   @ 30 dB. Add an IdentityFec control that asserts **sync/preamble detection
   only** (never a load-bearing "uncoded fails" — fragile per Codex). Add
   `sonde-fec` as a documented dev-dep (Cargo permits the dev-only back-edge;
   `sonde-fec → sonde-phy` is the normal edge).
5. **`crates/sonde-tx/tests/fec_differential_gate.rs`** — add an operational
   Watterson fading condition (end-to-end `encode_payload → channel → decode`),
   now that the equalizer can survive it.
6. **`crates/sonde-phy-runtime/src/waveform.rs`** — inject the real codec into
   `FloorWaveform` (currently the IdentityFec floor → can't decode fading). Closes
   the seam; sonde-fec is hardware-free so the runtime core stays ALSA-free.

## Verification

`cargo test -p sonde-phy --test robustness_floor_fading` GREEN (Good + Moderate),
then full `cargo build/test/clippy/fmt --workspace` (warnings = errors).
