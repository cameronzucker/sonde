# Handoff — Link layer: correctness, sound real-time driver, mode-adaptation P1

- **Agent:** moraine-willow-grouse · **Date:** 2026-06-14
- **Epic:** `sonde-lcw` (modem link layer) · adaptation under `sonde-ruu`
- **All work merged to `main`** (PRs #38, #40 this stretch; #32/#34/#35 earlier).

## What shipped this session (all on `main`)

1. **Codex-reviewed the whole link** → 6 blockers; converged on each.
2. **PR #38 — correctness + sound driver.** Symmetric bidirectional floor
   (`Idle` free-floor state; an acceptor can originate) + session reset on close
   (no stale-fragment bleed) — the two real defects Codex found. Plus the
   real-time `Driver<P,C>` finalized to **freeze the link clock on the PHY's real
   `tx_in_flight()` signal** (not an airtime estimate) — Codex blocker #3 closed.
3. **PHY seam coordination.** Asked the PHY layer for `tx_in_flight()` (`sonde-5qt`,
   closed) + `DecodeScan{NoSignal,Detected,Frame}` for honest FER (`sonde-wti`).
   PHY delivered both (PR #37); verified against the code before consuming.
4. **PR #40 — mode adaptation design + P1.** Codex-converged design doc
   `docs/superpowers/specs/2026-06-14-link-mode-adaptation-design.md` (Fork B,
   receiver-feedback downshift). **P1 built:** `MODE_ID` header byte; in-place ARQ
   reconfigure; rung-addressable ladder (`mac::rung`/`BASE_RUNG`); `conn`
   `current_rung`/`current_hint`/`apply_rung`/`follow_mode` + **BASE-fallback**
   (after `DOWNSHIFT_TO_BASE_OVERS` silent overs at a non-base mode, fall to BASE
   with a fresh death budget; `PeerLost` only if BASE also fails — symmetric ⇒
   convergence); driver transmits at `current_hint()`; **gate G9** proves the link
   converges to the floor under degradation and delivers (not `PeerLost`).

**84 sonde-link tests; workspace build/test/clippy --all-targets/fmt green.**
Everything labeled "link-correct over channel model", never HF-viable. RADIO-1.

## Process learning (saved to memory)

The main-checkout-race hook reads the **session cwd** (hook payload `.cwd`), not a
`cd` inside a compound command. `cd <wt> && git …` leaves the session on `main`
and the hook blocks it once another agent is live. **Fix:** standalone `cd` *into*
the worktree, then run git plainly (no `cd &&`, no `git -C`). Memory:
`sonde-worktree-cwd-not-compound-cd`.

## NEXT SESSION — build the rest immediately

Two independent work items, both buildable now (a fresh worktree per the cwd rule):

### A. `sonde-2f0` — live-radio safety layer (PHY-INDEPENDENT; do this first)
Part-97 §97.119 periodic-ID cadence at the link (callsign is already in every
frame = continuous ID; encode/verify the ≤10-min + end-of-exchange cadence) **and**
a **consent-gate abstraction that DEFAULTS TO DENIED** so nothing keys without
explicit per-run licensee consent (RADIO-1). Build over in-memory doubles; never
key a real radio. The prerequisite for any live bring-up.

### B. `sonde-ruu` — downshift control loop (full Fork B; mechanism buildable now)
Receiver piggybacks a quality report on its reply over → sender (single decider)
maps it through `route()`/`recommended_rung` to a target rung → announces (MODE
byte) + switches; link-side **recent-quality window** (the link's own per-over
decode outcomes, NOT the PHY's cumulative counters) + **upshift hysteresis** (K
clean overs). Gates: graceful per-rung stepping, no-thrash, byte-exact delivery
across a mid-session mode change. P1 already provides the floor-convergence safety
net and the mode plumbing to build on.
- **Open dependency:** `sonde-99l` (PHY) — does multi-mode RX auto-detect across
  waveforms or pre-tune? Sets *graceful-vs-blunt*; correctness holds either way
  (P1). Real *speed* payoff awaits the PHY ladder exposing >1 mode.

## Working-tree / coordination state

- `main` clean; all PRs merged, their remote branches deleted.
- **Stale local worktrees to dispose** (branches merged/superseded; disposal ritual
  in `docs/git-strategy.md`): `worktrees/sonde-0lx-link-ship-hardening`,
  `worktrees/sonde-0lx-link-ship-on-seam`, `worktrees/sonde-ruu-link-mode-adapt`,
  and the older `worktrees/sonde-o0s-link-mac-mvp`. No untracked stateful content
  in them beyond source already merged.
- Concurrent agents are live on other branches (`sonde-vb9` LDPC, etc.) — the
  main-checkout lease is contended, so **work in a worktree with the shell cwd set
  into it** (see the memory note above).
