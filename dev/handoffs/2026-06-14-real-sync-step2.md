# Handoff — Step 2: real synchronization in the loop (sonde-xhw.3)

**Date:** 2026-06-14
**Agent:** tamarack-butte-harrier
**bd:** `sonde-xhw.3` (epic `sonde-xhw` — gate on physics, not artifacts)
**Branch:** `sonde-xhw.3/real-sync` (worktree `worktrees/sonde-xhw.3-real-sync`), **pushed**.
**PR:** #33 → `main` (no-squash merge-commit; awaiting/merged — verify with `gh pr view 33 --json state`).

## One-line state

The production floor sync now decodes end-to-end through ±100 Hz CFO + 100 ppm
clock + fractional-sample timing over Watterson Good/Moderate, at parity with
perfect-frequency alignment. Pre-xhw.3 baseline was 0/8 at ≥20 Hz CFO. All
quality gates green; the physics gate (`tests/sync_impairment_gate.rs`) is GREEN.

## What shipped (2 commits)

- `bcdd0e2` feat: Schmidl-Cox repeated-pair preamble (`[h|h]`, H=160, root 29) +
  two-stage CFO-robust detection (`M(d)` CFO-invariant detect → derotate → sharp
  two-half I/Q template MF) + CFO derotation wired into `receive_multi_with_sync`;
  guard 128→32; `sonde-tx` re-exports the phy preamble const.
- `b976efd` test: the Step 2 physics GATE (replaces scratch `zz_` baseline);
  removed the hand-aligned bypass from `robustness_floor_fading.rs`; Codex design doc.

## Key design (Codex-converged — `dev/codex-prompt-real-sync.md`)

- **Why the old sync failed:** ±100 Hz ≈ 4 sub-carrier spacings (Δf=23.4 Hz),
  sliding the spectrum off the pilot bins. CFO must be corrected in the time
  domain before the FFT. A *template* matched filter's magnitude collapses below
  the noise floor at ±100 Hz → detection must use the **CFO-invariant** S&C
  metric `M(d)=|P(d)|/R(d)`. An **energy gate** in `M(d)` rejects spurious peaks
  in the zero-padded trailing symbol / silence.
- **Clock + fractional timing need NO explicit tracker** at this frame length:
  ~7 samples drift at 100 ppm over ~1.5 s is far inside the 512-sample CP,
  absorbed by the per-symbol equalizer's phase ramp. Gardner (`symbol_timing.rs`)
  stays the FSK substrate, intentionally NOT wired into the OFDM floor.
- **Eb/N0 knob** (discrete-time AWGN, matches Step 0):
  `snr_db = Eb/N0 + 10·log10(N_info / buffer_len)`.

## Gate result (measured, `tests/sync_impairment_gate.rs`, Eb/N0=35 dB, 8 seeds)

| condition | baseline CFO=0 | +CFO±100 +100 ppm +frac timing |
|---|---|---|
| Good | 8/8 | 7/8 |
| Moderate | 7/8 | 6–8/8 |

The 35 dB point isolates SYNC (like the fading gate's 30 dB isolates the
equalizer). `#[ignore]`d `report_decode_rate_curve` sweeps the full Eb/N0 range.

## Important caveats / follow-ups (filed)

- **`sonde-gtg` (P2):** the floor's demod uses a hardcoded `n0=0.1` in the
  channel-aware LLR, capping useful decode to ~Eb/N0 ≥ 25 dB. This — NOT sync —
  is why the gate states a high (35 dB) Eb/N0. Replace with a real noise-variance
  estimate to lower the operating SNR. **This is the next lever for credible HF
  operation** and likely gates `sonde-xhw.4` (coded mode over realistic fading).
- **`sonde-wfa` (P3):** fractional resampler / pilot-tracked window repositioning
  for long-frame (>~27 s) or high-SRO clock error. Not needed at current scale.

## Working-tree / environment notes

- Worktree clean, all work committed + pushed. No stashes. Untracked before
  commit were the gate test + design doc (now committed). `.beads/issues.jsonl`
  is bd/dolt export churn — reconcile via `bd`, do not hand-edit.
- The impairment gate runs ~60 s in debug (CI) — acceptable but the slowest
  single test; the diagnostic curve is `#[ignore]`d to keep CI lean.

## Next session — resume

1. Verify PR #33 merged: `gh pr view 33 --json state`. If merged, `bd close sonde-xhw.3`.
2. Start `sonde-xhw.4` (Step 3: integrate + validate coded mode over realistic
   fading) — but FIRST tackle `sonde-gtg` (n0 estimation), since the demod's
   ~25 dB Eb/N0 floor is the binding constraint for a credible coded-fading gate.
