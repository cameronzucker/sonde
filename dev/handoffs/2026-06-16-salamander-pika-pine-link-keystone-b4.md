# Session handoff — link-layer keystone Phase 1 + B4 (sonde-98e)

**Date:** 2026-06-16 · **Agent:** salamander-pika-pine (LINK-layer lane) · **Epic:** sonde-98e

## What happened this session

1. **Merged PR #62** (prior session's gap inventory + B5 conn_id-u32 + B6 reconnect) onto `main` (commit `54edf60`). Closed `sonde-44n`, `sonde-ajn`.
2. **Designed the keystone seam** (dynamic link↔PHY registry handshake, B1+B2+B3) and **converged it with Codex** (adversarial, read-only). Design doc: `docs/superpowers/specs/2026-06-15-link-phy-registry-handshake-design.md` (v1 + v2 outcome).
3. **Coordinated the lane split with the sonde-b60 PHY agent** — locked contract: PHY agent owns **`sonde-ddg`** (the `ModeCapability` type in sonde-phy + canonical `wire_id` consts 0..=4 [5–7 reserved] + `sonde-phy-runtime` metadata table + inherent `SondePhy::capabilities()` snapshot, real measured knees; **no `PhyTransport`/`Waveform` trait change**). Link sorts the ladder by knee; same-build-only; ids must stay `0..=7` (rx_rung is 3-bit). PHY agent lands ddg next (ahead of QAM) and pings with the concrete `ModeCapability` slice + wire_id table on merge.
4. **Shipped PR #70 — keystone link half, Phase 1** (`sonde-3tm/registry-handshake`): `mac.rs` static `ladder()` → injectable **`Ladder` value**; `Connection` holds a `Ladder` + `*_with_ladder` ctors (back-compat uniform-profile ctors unchanged); **B3 fix** — `apply_rung` swaps the `ModeProfile` to the target rung + re-arms any outstanding turn-recovery deadline from the new profile (`now` threaded through the inbound path). Gates: 99/2/16 green, fmt + clippy clean.
5. **Shipped PR #71 — B4** (`sonde-p4g/fragment-at-transmit`, **stacks on #70**): `send()` fragments at `Ladder::min_available_fragment_bytes()` (deepest available rung's per-frame capacity) so committed DATA seqs stay transmittable after a downshift + post-downshift retransmit. Codex-converged. Gates: 102/2/16 green, fmt + clippy clean. Filed P3 follow-up: transmit-time fragment coalescing (fast-mode efficiency, a protocol change).

## Branch / working-tree state

- `main` @ `54edf60` (has #62).
- **PR #70** `sonde-3tm/registry-handshake` — OPEN, CI pending. 2 commits (design doc + Phase 1 impl).
- **PR #71** `sonde-p4g/fragment-at-transmit` — OPEN, **stacks on #70**. Until #70 merges, #71's diff includes #70's commits; the B4-specific changes are the last 2 commits.
- Both worktrees clean (all work committed + pushed). No stashes.

## In-flight worktrees

- `worktrees/sonde-3tm-registry-handshake` (branch `sonde-3tm/registry-handshake`, bd `sonde-3tm` in_progress) — Phase 1 pushed; **Phase 2 not started** (see below). No untracked/gitignored state of note.
- `worktrees/sonde-p4g-fragment-at-transmit` (branch `sonde-p4g/fragment-at-transmit`, bd `sonde-p4g`) — B4 pushed; this handoff lives here.
- `worktrees/sonde-98e-close-link-gaps` — the prior session's epic worktree; its branch merged via #62 and was pruned remotely. **Disposable** via the ritual (docs/git-strategy.md) — left in place, not owned by this session's active work.

## Critical first action for the next session

**Merge order: #70 then #71** (`gh pr merge 70 --merge --delete-branch`, then `71`) once CI is green — #71 stacks on #70. Verify each merge by state (second-parent), not the CLI exit message (the `--delete-branch` local-cleanup error from a worktree is benign).

## Pending decision / blocked

- **Keystone Phase 2 (sonde-3tm)** is BLOCKED on **sonde-ddg** (PHY agent's lane). When ddg merges: add `Ladder::from_capabilities(&[sonde_phy::ModeCapability])` (sort by knee; `MainPinned` per-mode hints; availability = present; derive 3 link tiers from `(family, short_name)`), wire the production assembly (where `Driver`+`Connection`+`SondePhy` are built) to read `phy.capabilities()` into `Connection::*_with_ladder`. That closes B1/B2 fully. The link half is built + tested against synthetic capabilities already; Phase 2 is the real wire-up + `from_capabilities` builder.
- **Remaining non-blocked link gaps** (sonde-98e): `sonde-32q` (B7 idle keepalive — `keepalive_interval()` is wired in `ModeProfile` but no liveness timer scheduled), `sonde-mua` (B8 two-party multi-mode E2E test), the **Important set** (I1–I12: quality-before-session-validation, count decode failures into FER, host error/status surface, NAK handling, ACK-range bounding, buffer caps, seq-wrap, disconnect drain policy), and **Polish** (P1–P4). Epic is done when a fresh Codex audit of sonde-link comes back empty.
- **Pi load:** the dev host is a Raspberry Pi shared with a parallel tuxlink session. Keep builds targeted (`-p sonde-link`), avoid `cargo build --workspace`, don't stack parallel cargo runs.
