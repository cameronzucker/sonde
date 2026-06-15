# Handoff — Link/MAC + selective-repeat ARQ + lossy-channel gates G1–G5

- **Agent:** moraine-willow-grouse · **Date:** 2026-06-14
- **Epic:** `sonde-lcw` (Modem link layer #5/#6/#8)
- **PR:** [#32](https://github.com/cameronzucker/sonde/pull/32) — branch
  `sonde-o0s.1/link-mac-arq` (off current `origin/main`).
- **Worktree:** `worktrees/sonde-o0s-link-mac-mvp` (local branch
  `sonde-o0s/link-mac-mvp`, now tracking the new remote branch).

## What this session built (all TDD)

The modem-owned **Link/MAC + connected-mode selective-repeat ARQ** (#5/#6),
gated by reliable in-order delivery over a realistic lossy channel — not a clean
loopback. Implements design v3 `docs/superpowers/specs/2026-06-14-link-mac-arq-mvp-design.md`.

- `crates/sonde-link/src/profile.rs` — `ModeProfile`: all link timers are
  mode-derived (airtime-aware) multiples of `over_airtime` (§6).
- `crates/sonde-link/src/arq.rs` — selective repeat: `SendWindow` (cumulative +
  SACK ack, retransmit only gaps), `RecvBuffer` (dedup + in-order + ACK/SACK),
  `Reassembler` (fragment → message on `END_OF_MSG`).
- `crates/sonde-link/src/conn.rs` — **sans-IO** half-duplex connection state
  machine. No I/O, no wall-clock. In-band floor token (`END_OF_OVER`, no
  carrier-sense, §3.5); **quiescence rule** avoids the empty-ACK ping-pong
  deadlock; turn-recovery re-take; explicit `PeerLost`. Hardening: idempotent
  CONN/CONN_ACK, half-open rejection, callsign collision tie-break, dup/reorder
  safety.
- `crates/sonde-link/src/link.rs` — `Link<P: PhyTransport>` driver.
- `crates/sonde-link/src/frame.rs` — added `FLAG_END_OF_MSG`.
- `crates/sonde-link/tests/link_gates.rs` — neutral Gilbert-Elliott lossy medium
  (bursty loss, corruption, RTT, collision, blackout) + **G1–G4**.
- `crates/sonde-link/tests/g5_wiring_smoke.rs` — **G5** over the real `SondePhy`
  + `FloorWaveform` (`LoopbackRadio`), labeled wiring only.

**57 sonde-link tests green; workspace build/test/clippy --all-targets/fmt clean.**

## Honesty / labeling (operator directive — honored)

Results are **"link-correct over channel model {params}"**, never "HF-viable."
**RADIO-1:** nothing keys a radio; in-memory doubles / `LoopbackRadio` only.

## bd state

- Closed: `sonde-o0s` (#5 frame + conn SM), `sonde-se5` (harness + gates).
- Updated open: `sonde-5yg` (SR core landed & gated; jittered backoff + mode-STEP
  remain), `sonde-pwh` (W=1 floor mechanically supported; explicit route() path
  pending).
- New follow-up issue filed for: `mac.rs route()`/`adapt.rs`, host `#8`
  (`sonde-a0p`), jittered backoff, and a full two-party **threaded** G5.

## Branch / working-tree state

- Branch `sonde-o0s.1/link-mac-arq` pushed; **PR #32 open, CI pending at handoff.**
- The old remote branch `sonde-o0s/link-mac-mvp` was superseded after the
  start-of-session rebase diverged it (operator chose "new branch + PR"); it is
  stale and can be deleted once #32 lands.
- Working tree clean after this commit. No stashes. `worktrees/` is gitignored;
  no untracked stateful content in this worktree.
- Concurrent sessions are on other branches (`sonde-64w.*`, `sonde-669.8`,
  `sonde-xhw.*`) — untouched.

## Pending decision / next session

1. **Merge #32** once CI is green: `gh pr merge 32 --merge --delete-branch`,
   then `git push origin --delete sonde-o0s/link-mac-mvp` to retire the stale
   branch.
2. Then the follow-up issue: `mac.rs route()` + `adapt.rs` (wire
   `ArqStrategy::WholeMessage` floor path + mode-STEP), host `#8`, jittered
   backoff, two-party threaded G5.
