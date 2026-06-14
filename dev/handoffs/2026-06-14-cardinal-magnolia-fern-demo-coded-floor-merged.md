# Handoff — Sonde interactive demo: PR #4 conflict resolved, re-pointed onto coded floor, MERGED

- **Agent:** cardinal-magnolia-fern
- **Date:** 2026-06-14
- **Epic:** sonde-669 — Sonde interactive adaptive-modem demo (live WASM)
- **Issue:** sonde-669.5 (P1 bug) — closed

## TL;DR
PR #4 (the demo Phase 1 engine + frontend) could **not** merge: while it sat open,
`main` advanced and PR #5 (fec-floor-wiring) **rewrote `WidebandLowDensityFloor`** into
the FEC-coded path — deleting `data_bytes_per_symbol()` / `decode_symbol_bytes()` and
replacing byte-stream framing with per-FEC-block soft-LLR decode. The demo engine was
built on the removed APIs, so the PR was `CONFLICTING` and would have produced a **red
`main`** (sonde-wasm wouldn't compile). This session resolved that and **PR #4 is now
merged** (`origin/main` = `c6b92e1`, no-ff merge commit).

## What this session did
1. **Diagnosed** the conflict as architectural, not textual (two branches editing the
   same floor API incompatibly). Evidence: `git grep` confirmed the removed symbols
   absent on `main`; the demo called all three.
2. **Merged `origin/main` into the branch** (the only policy-legal path — the branch was
   pushed, so rebase/force-push is banned). Resolved two conflicts:
   - `Cargo.toml`: union of workspace members (`sonde-phy-runtime` + the two demo crates)
     + main's repo URL. (Merge commit `54f305c`.)
   - `wideband_lowdensity.rs`: took main's coded floor **wholesale**, dropping the
     branch's `DecodedFrame` / `receive_multi_detailed` additions.
3. **Re-pointed `sonde-wasm`** at the coded floor with **zero `sonde-phy` changes**
   (`sonde-phy` is now byte-identical to `main`). Fix commit `22513ed`:
   - Decode whole-frame via `receive_multi_with_sync`; the three narrative states map onto
     the result (clean / synced-but-corrupted / sync-fail).
   - `build_symbols` rebuilt from the floor's **real bit-level framing** — reuses public
     `coded_framing::{blocks_for_payload, HEADER_BITS}` + `params().data_indices()`; each
     payload byte attributed to the symbol holding its first (MSB) bit (contiguous,
     non-overlapping despite 74-bit symbols).
   - `modes.rs` reports floor capacity from the Wide OFDM grid directly.
   - Rewrote the now-false tests (`==9 bytes/symbol`, per-symbol byte asserts, e2e comment).

## Verification (all green)
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` — all clean.
- `cargo build -p sonde-wasm --target wasm32-unknown-unknown` — OK.
- Rebuilt the wasm bundle (`demo/site/pkg/`) and ran **Playwright smoke 2/2**.
- **Engine probe** (throwaway, not committed): clean recovery ≥ −3 dB, **synced-but-
  corrupted at −6/−9 dB AWGN** (recovered_bytes present-but-wrong — the headline image-
  corruption state), sync-fail on multipath. All three states intact.
- **Codex adrev** (`codex review --commit 22513ed`): ran its own independent byte-range
  contiguity check across payload lengths 0–1000 + the committed `payload.bin` (3025 B →
  328 symbols); **converged with no findings.**
- PR #4 CI green (`check` 1m28s); merged `--merge --delete-branch`. Remote branch deleted.

## State at handoff
- `origin/main` = `c6b92e1` ("Merge pull request #4"). Demo crates (`sonde-wasm`,
  `sonde-demo-builder`) + the static demo (`demo/`) are now on `main`.
- This handoff lands via branch `sonde-669.5/session-handoff` → its own PR.
- The `worktrees/sonde-interactive-demo` worktree is being **disposed** this session
  (ritual: inventory clean — only rebuildable artifacts; no bd DB inside; no stashes).
- **Local `main` checkout** (`/home/administrator/Code/sonde`) was behind all session and
  had 2 pre-existing uncommitted files (`.beads/issues.jsonl`,
  `.claude/hooks/check-commit-discipline.sh`) belonging to another session — **untouched
  by this session**; left for that session/operator to reconcile.

## bd status
- `sonde-669.5` **closed** (this rework). `sonde-669.1` **closed** (Phase 1 landed via #4).
  Epic `sonde-669` now 4/5 (80%).
- **Open:** `sonde-669.2` (propagation-game mode, P3, deferred); `sonde-1yh` (waterfall
  vividness, P3); `sonde-blr` (smoke coverage for the synced-but-corrupted regime, P3).
- **bd sync caveat (unchanged):** no dolt remote configured on this host (`bd dolt push`
  prints usage). Local bd DB is authoritative on this machine; the git-tracked
  `.beads/issues.jsonl` is a best-effort export and may lag the DB.

## What's next (options)
- **Publish to GitHub Pages:** the committed `payload.bin` makes the page work without a
  Rust rebuild, but `demo/site/pkg/` (the wasm bundle) is gitignored and must be built at
  publish time via `./demo/build-assets.sh` (needs `wasm-bindgen-cli`, network for the
  NOAA image fetch). No Pages workflow exists yet — that's unfiled work if desired.
- `sonde-blr`: add a Playwright case for the −6 dB synced-but-corrupted regime (the smoke
  test only covers clean + multipath today).
- `sonde-1yh`: waterfall contrast polish.
- `sonde-669.2`: propagation-game mode.
