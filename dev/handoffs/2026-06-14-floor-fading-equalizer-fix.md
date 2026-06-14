# Handoff — floor fading fix shipped (channel-aware LLR), sync defect split out

**Date:** 2026-06-14 · **Agent:** bayou-crag-granite · **Session focus:** sonde-64w.2 (floor fading decode).

## TL;DR

The floor's Watterson-fading decode failure is **fixed and merged** (PR #24, merge
`f1f734e`). The prior session's "late preamble sync → ISI" root cause was
**refuted by instrumentation**; the real cause was a frequency-selective null
defeating the zero-forcing + fixed-noise LLR. A **second, independent defect**
(preamble correlator mislocation under channel phase rotation) was discovered,
characterized, and **split to bd `sonde-64w.3`** — not fixed here.

## What shipped (PR #24, merged to main)

Root cause (instrumented): a deep frequency-selective null from the 2-tap
Watterson channel (|Y| spread 16.5× Good / 27× Moderate). The receiver
zero-forced then used a fixed `n0`, producing high-confidence WRONG-sign LLRs at
the null — poison to the LDPC SPA decoder, discarding per-subcarrier reliability.

Fix (Codex-converged across 4 adversarial rounds):
- `sonde-phy/src/constellations.rs`: `compute_llr_channel` — max-log `−|y−h·c|²/N0`
  (BPSK = `4·Re(conj(h)·y)/N0`); LLR magnitude ∝ |h|² → nulls become near-erasures.
- `sonde-phy/src/ofdm_main/equalizer.rs`: `estimate_channel` split out; fixed an
  edge-extrapolation bug (bins past the last pilot defaulted to `1+0j`).
- `sonde-phy/src/ofdm_main/receiver.rs`: channel-aware LLRs (no ZF) + clamp.
- `sonde-phy-runtime/src/waveform.rs`: `FloorWaveform` injects the real
  `FloorRate14Codec` (was IdentityFec — couldn't decode fading). **Seam closed.**
- Gates: `sonde-phy/tests/robustness_floor_fading.rs` (equalizer proof over 8
  seeds sync-bypassed + E2E smoke + sync-detect control); `sonde-tx`
  operational Watterson gate.
- Plan: `docs/superpowers/plans/2026-06-14-floor-fading-equalizer-llr.md`.

All workspace gates green (build/test/clippy `-D warnings`/fmt). CI passed on PR.

## Key evidence (for the next session on sonde-64w.3)

- Perfect-alignment coded decode (correlator bypassed): **Good 40/40, Moderate
  37/40** across the fixed golden-ratio seed stride. The equalizer fix is
  seed-robust; Moderate's 3/40 misses are genuine deep two-notch nulls (physics).
- Production path (with sync): **Good 27/40, Moderate 29/40**. EVERY production
  failure has `perfect_align_ok=true, sync_ok=false` → the gap is **purely the
  preamble correlator** mislocating, not the equalizer.
- So sonde-64w.3's fix (complex matched filter for the preamble) is the second
  half needed to make the production fading path seed-robust. After it lands,
  upgrade the E2E gate from converged-seed-only to seed-swept and tighten the
  sync control to assert offset accuracy (not just `is_some`).

## State

- **Branch:** sonde-64w.2/floor-equalizer — **merged + remote-deleted.** This
  handoff lands via a separate small branch (`bayou-crag-granite/session-handoff`).
- **Working tree (worktree `worktrees/sonde-64w.2-floor-equalizer`):** clean after
  the handoff commit; **scheduled for disposal** this session (bd issue closed,
  no active claim).
- **bd:** `sonde-64w.2` CLOSED; `sonde-64w.3` OPEN (P2, under epic sonde-64w).
  `bd dolt push` is a no-op (no Dolt remote configured); state is local + in the
  git-tracked `.beads/issues.jsonl`. Note: the `.beads/issues.jsonl` export lives
  in the MAIN checkout's tree and shows as modified there — reconciles on the main
  checkout's next bd-bearing commit (multi-worktree bd export quirk; harmless).
- **Other worktrees:** `worktrees/sonde-interactive-demo` (sonde-669) belongs to
  another live session — untouched.

## Pending / next

1. **sonde-64w.3** — complex matched-filter preamble sync (the discovered second
   defect). Converge the DSP with Codex before building (standing directive).
2. After 64w.3: upgrade fading E2E gates to seed-swept; consider Poor/Flutter
   conditions (need time-varying tracking the static per-symbol estimator lacks)
   and a residual-based N0 estimator (currently fixed `n0=0.1` proxy).
3. Epic `sonde-64w` ladder work (OFDM main modes, bit-loading, link adaptation)
   remains open.
