# Handoff — Step 3 (coded-fading validation) INVESTIGATION CHECKPOINT

**Date:** 2026-06-14
**Agent:** tamarack-butte-harrier
**bd:** `sonde-xhw.4` (in_progress, **BLOCKED** — see below)
**Branch:** `sonde-xhw.4/coded-fading-validation` (worktree
`worktrees/sonde-xhw.4-coded-fading-validation`). **No production code changed**
on this branch — it carries only the Codex design doc + this handoff. Two earlier
PRs this session merged first: #33 (Step 2 real sync) and #36 (n0 estimate).

## Why this is a checkpoint, not a shipped gate

Step 3's acceptance is "success-rate over Watterson Good/Mod/Poor at a STATED
Eb/N0, AND a coded-vs-uncoded BER curve showing LDPC coding gain in dB." The
measurement work (Codex-converged methodology) surfaced **two blocking findings**
that make an honest gate impossible right now. Shipping a green gate over them
would violate gate-on-physics. Both are filed; xhw.4 depends on them.

## What IS established (good news)

- **No Doppler/time tracker needed** for Good/Moderate/Poor: delay spread
  (≤96 samp Poor) ≪ 512 CP, and Doppler (≤1 Hz, coherence ≥1 s) is tracked
  feedforward by the per-symbol pilot equalizer. (Flutter @10 Hz is out of scope.)
- All three conditions decode 8/8 through the production sync path (real
  Schmidl-Cox sync + n0 estimate) at my-Eb/N0 ≥ 26 dB; Poor's cliff is ~2-4 dB
  worse than Good/Moderate. So the **composition works** at sufficient SNR.
- Codex-converged gate methodology recorded in `dev/codex-prompt-step3.md`
  (Gate A: FER table, 30 dB, ≥15/16, 16 seeds; Gate B: AWGN waterfall for the dB
  coding-gain number; Eb/N0 must be info-bit-energy normalized).

## BLOCKER 1 — `sonde-vb9` (P1 bug): LDPC gives ~0 net coding gain over AWGN

Measured (aligned `receive_multi` over AWGN, my-Eb/N0 scale, 24 frames):

```
my-Eb/N0   coded FER   uncoded BER
   6        24/24       1.25e-1
  10        24/24       1.67e-2
  12        20/24       3.27e-3
  14         0/24       1.30e-4
  16         0/24       0
```

The rate-1/4 LDPC only starts working at my-Eb/N0 ~13-14, where uncoded BER is
ALREADY ~1e-4. A good rate-1/4 LDPC (n=2048) should decode ~7 dB BELOW uncoded.
So the code provides essentially NO net gain. **Ruled out:** the sonde-gtg n0
floor (disabling it left coded FER unchanged). Candidate causes: the
IRA/dual-diagonal H construction (changed mid-flight in sonde-64w.1 due to
rank-deficiency — may be a weak code), SPA decoder `MAX_ITERS`, or LLR
scaling/saturation. **Gate B is impossible until this is fixed.**

## BLOCKER 2 — Eb/N0 calibration (folded into `sonde-xhw.5`)

The harness Eb/N0 (`snr_db = ebn0 + 10·log10(Ninfo/Lbuf)`) **over-states by a
clean ~6.7 dB** vs uncoded BPSK theory (measured: my-Eb/N0 8/12/16 → BER
5.5e-2/3.5e-3/2.6e-5 → true 1.1/5.6/9.1 → offset 6.9/6.4/6.9). Consequence: the
absolute Eb/N0 labels in the MERGED xhw.3 sync gate (stated 35 ≈ true 28) and
gtg n0 gate (stated 25 ≈ true 18) are inflated ~6.7 dB. **Their DIFFERENTIAL
claims still hold** (sync parity, estimate-vs-fixed) — only the absolute labels
are off. A credible "stated Eb/N0" for Gate A needs this fixed first.

## Resume plan (next session)

1. Fix `sonde-vb9` (the FEC coding-gain defect) — this is the bigger lever and
   matters beyond Step 3. Verify with an AWGN coded-vs-uncoded BER waterfall
   (expect ~5-7 dB gain at 1e-4).
2. Calibrate Eb/N0 (`sonde-xhw.5`): anchor to uncoded BPSK theory; subtract
   ~6.7 dB (or fix the analytic-doubling normalization in the harnesses).
   Consider relabeling the xhw.3/gtg gate docs.
3. THEN build Step-3 Gate A (Good/Mod/Poor FER table at honest stated Eb/N0,
   16 seeds, ≥15/16) + Gate B (AWGN LDPC coding-gain dB) per
   `dev/codex-prompt-step3.md`.
4. Dispose merged worktrees (`sonde-xhw.3-real-sync`, `sonde-gtg-n0-estimate`)
   via the `docs/git-strategy.md` ritual.

## Working-tree note
Floor const `CHAN_EST_ERROR_FLOOR_FRAC` restored to 0.5 (a diagnostic temporarily
set it to 0.0 to rule out the floor; reverted). Scratch `zz_*` probes deleted.
