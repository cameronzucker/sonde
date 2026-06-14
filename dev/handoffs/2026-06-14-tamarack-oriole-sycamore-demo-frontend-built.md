# Handoff — Sonde interactive demo: frontend BUILT, reviewed, Codex-converged

- **Agent:** tamarack-oriole-sycamore
- **Date:** 2026-06-14
- **Epic:** sonde-669 — Sonde interactive adaptive-modem demo (live WASM)
- **Branch / worktree:** `sonde-669/interactive-demo` at `worktrees/sonde-interactive-demo` → **PR #4** (pushed; `5cdb02f..10d5178`)

## TL;DR
The demo **frontend is built, working, reviewed, Codex-adrev-converged, and pushed.**
`demo/site/` is a static, GitHub-Pages-hostable page that runs Sonde's real DSP in the
browser via WASM. Verified live: clean link at high SNR/Ideal (BER 0.00%), corrupting
recon image in the marginal regime, graceful failure on multipath. **bd sonde-669.4 is
closed.** PR #4 is updated and ready for review/merge — **not yet merged** (left to the
operator; merging to `main` is a deliberate step).

## What shipped this session (12 frontend commits + bd commit, all under PR #4)
`demo/build-assets.sh` (NOAA ERI public-domain image → `payload.bin`/offsets → wasm bundle),
vendored Three.js r160, `engine.js` (the only wasm-aware module), tested pure helpers
(`format.js`), the **frontend-design style-B shell** (`index.html`+`app.css` — dark "EmComm
SDR instrument" console), `waterfall.js` (3D STFT surface), `console.js` (TX|RX byte console),
`image-reveal.js` (progressive + corrupting recon image), `playback.js`+`controls.js`+`main.js`
wiring, a Playwright smoke test (`demo/` harness: package.json + playwright.config.mjs), and
`demo/README.md`. Two follow-up fix commits: code-review findings, then the Codex P2 fix.

## Verification (all green)
- **Rust gate:** `cargo fmt --all --check` OK; `cargo clippy --workspace --all-targets -- -D warnings` exit 0. (No Rust source changed this session.)
- **JS:** `node demo/tests/format.test.mjs` passes; `node --check` clean on every module.
- **Playwright smoke** (`cd demo && npx playwright test`): 2/2 pass.
- **Live in-browser** (served, not file://): high-SNR/Ideal → BER 0.00% on floor-wblo; -6 dB/Ideal → synced-but-corrupted, recon image renders (6422 non-dark px); -6/Poor → degrades without crashing.

## Reviews
- **Subagent two-stage review** (per task) + a **final whole-impl review**: SHIP-WITH-MINORS; all Important + high-value Minor findings fixed in `58a5991`.
- **Codex adrev** (`codex review --base` at the pre-frontend commit): **1 P2**, fixed in `10d5178` —
  the recon image was gated on `recovered_ok`, but the engine returns `recovered_ok:false`
  for BOTH a synced-but-corrupted decode (recovered_bytes PRESENT) and a true sync failure
  (recovered_bytes EMPTY). Old code showed "DECODE FAILED" for the corrupting regime — the
  demo's headline feature. Now gated on `recovered_bytes` presence, with a third "syncs but
  bit errors corrupt the payload" narrative state. Verified via engine probe + UI.

## State
- Branch synced with origin; **working tree clean**. No stashes. Temp review base branch
  `codex-review-base` created + deleted (used for `codex review --base`).
- **Generated/gitignored** (not committed; rebuild via `./demo/build-assets.sh` + `cd demo && npm install`):
  `demo/site/pkg/` (wasm bundle), `demo/site/assets/source.jpg`, `demo/node_modules/`,
  `demo/test-results/`. Committed payload (`payload.bin`/offsets/credit) makes the page work
  on Pages without a Rust rebuild; only `pkg/` must be built at publish time.

## ⚠️ Environment gotchas hit this session (read before committing in a worktree)
1. **Multi-session commit discipline.** Another session was live on the **main checkout**.
   To commit from this worktree: keep the persisted shell cwd **inside the worktree**
   (never `cd` to the main checkout inline — use subshells `(cd /main && …)` for reads),
   and **never use `git -C`** for writes (it trips `block-main-checkout-race.sh`'s
   git-target-override path). The hook reads the *tool payload cwd* to classify worktree vs main.
2. **`check-commit-discipline.sh` had a real bug** (it read the main checkout's branch via
   `BASH_SOURCE`, blocking all worktree commits as "direct to main"). Another agent
   (`opossum-magnolia-taiga`, commit `cd7d4ea`) introduced it; it was fixed mid-session by the
   operator's other session. If worktree commits start failing as "direct to main" again,
   that fix regressed — resolve the branch from the commit's cwd like `block-main-checkout-race.sh` does.

## bd status
- `sonde-669.4` (frontend build) — **CLOSED** ✅ (local bd DB).
- Filed two P3 follow-ups: "Demo waterfall: lift visual contrast/vividness" and
  "Demo smoke test: cover the synced-but-corrupted (marginal) regime."
- `sonde-669.1` (engine) — closes when PR #4 merges. `sonde-669.2` (propagation game) — P3, deferred.
- **bd sync caveat:** `bd dolt push` printed usage (no dolt remote configured on this host);
  the git-tracked `.beads/issues.jsonl` is a partial export. The local bd DB is authoritative
  on this machine; cross-machine bd sync needs the dolt/JSONL remote configured.

## What's next (options, not yet done)
- **Review + merge PR #4** (`gh pr merge 4 --merge --delete-branch`) once the operator is happy.
- Optional polish: the two P3 follow-ups (waterfall vividness; marginal-regime smoke coverage).
- `sonde-669.2` propagation-game mode remains deferred.
