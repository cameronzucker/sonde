# Handoff — Link adaptation + floor WholeMessage + host #8 + jittered backoff

- **Agent:** moraine-willow-grouse · **Date:** 2026-06-14 (continuation)
- **Epic:** `sonde-lcw` (Modem link layer #5/#6/#7/#8)
- **PRs this session:** [#32](https://github.com/cameronzucker/sonde/pull/32)
  (link/MAC + ARQ + gates G1–G5) and [#34](https://github.com/cameronzucker/sonde/pull/34)
  (this increment) — **both merged to `main`.**

## What this increment built (PR #34, all TDD)

- `crates/sonde-link/src/mac.rs` — `route()` link adaptation (#7 plumbing, §6):
  pure decision from `ChannelQualityReport` (SNR+FER) + payload size to
  `(ModeHint, ArqStrategy, WindowParams, ModeProfile)` over a 5-rung ladder
  (OFDM → wide-band floor → FT8-class deep-floor nFSK). High FER degrades down
  the ladder. Mode-STEP stubbed-but-not-faked (gated on >1 real PHY profile).
- `conn.rs` `ArqStrategy::WholeMessage` floor path via `with_strategy` — window
  1, SACK suppressed (floor "no NACK"). Gated by **G6** under burst loss.
- `host.rs` `HostCommand` + `Link::command` (#8). Full-lifecycle gate.
- `conn.rs` jittered backoff — deterministic, bounded, station-specific spread
  on the turn-recovery deadline (anti phase-lock, §3.5).

**69 sonde-link tests green; workspace build/test/clippy --all-targets/fmt clean.**
Results remain **"link-correct over channel model", never HF-viable.** RADIO-1.

## bd state

- Closed: `sonde-pwh` (#6-MVP floor), `sonde-a0p` (#8 host), `sonde-5yg` (#6-full
  SR ARQ + backoff). Plus `sonde-o0s`/`sonde-se5` from PR #32.
- `sonde-8xw` (follow-ups) still open — **remaining work**:
  1. **Live mode-STEP** — wire `route()` into the `Connection` per-over loop to
     re-pick window/profile mid-session. Stubbed-but-not-faked today; gated on
     >1 real PHY `ModeProfile` (the PHY ladder work, owned elsewhere).
  2. **Two-party threaded G5** over a shared-medium radio double (current G5 is a
     bounded single-frame round-trip + driver smoke).

## Working-tree / branch state

- Both PRs merged; their remote branches deleted.
- This worktree `worktrees/sonde-8xw-link-mac-adapt-host` is on the merged local
  branch `sonde-8xw/link-mac-adapt-host`; disposable via the ritual once this
  handoff lands. No stashes.
- Concurrent sessions on `sonde-xhw.*`, `sonde-imh.*` branches — untouched.

## Next session

`bd show sonde-8xw` for the two remaining items. The link is feature-complete for
#5/#6/#7-plumbing/#8; what's left is the live mode-STEP (needs real PHY profiles)
and the threaded real-PHY G5. Honesty rule still holds: nothing is HF-viable
until the PHY physics gates pass; nothing keys a radio without per-run licensee
consent (RADIO-1).
