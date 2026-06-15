# Handoff — link-layer gap-closure begun (sonde-98e)

**Agent:** sparrow-hawk-swallow · **Date:** 2026-06-15
**Branch:** `sonde-98e/close-link-gaps` → **PR #62** (open). Local `main` synced to
origin/main (`d82dc4f`, #61).

## ⚠️ Lane discipline (read first)
This session's agent is the **LINK-LAYER agent**. Lane = `crates/sonde-link`. The
**PHY** (`crates/sonde-phy*`: waveforms, per-mode routing, gates, power-norm) is
owned by **parallel sessions** (sonde-b60 PHY "close all gaps", #59/#60/#61). Do
**not** build PHY here. This session burned effort building duplicate PHY work
(sonde-8xl, abandoned, never pushed) before the operator corrected the lane —
**always re-fetch origin/main and check what parallel sessions landed before
building.** Memory: `sonde-i-am-the-link-layer-agent`.

## Tasking (operator, 2026-06-15)
"The WHOLE link layer needs built. Not small bits. The whole feature. Converge
physics/protocol decisions with Codex until you substantially converge." Method
mirrors sonde-b60 (PHY): enumerate every gap via a Codex audit, build until a
fresh audit comes back empty.

## What landed this session (PR #62)
1. **Authoritative gap inventory** — `docs/superpowers/specs/2026-06-15-link-layer-gap-inventory.md`.
   Codex audit of `sonde-link`: **8 blockers, 12 important, 4 polish**, with build
   order + the keystone. THIS IS THE ROADMAP.
2. **B5 (sonde-44n)** — `conn_id` u16→u32 + reserve-0. Wire VERSION 2→3; offsets
   single-sourced in `OFF_*` consts; conn_id 0 reserved/rejected. Breaking wire.
3. **B6 (sonde-ajn)** — half-open reconnect: a fresh CONN from the known peer with
   a new conn_id (Connected) is accepted as a reconnect, not dropped.
   - Gates: `cargo build/test/clippy -Dwarnings/fmt` green (sonde-link 94 lib +
     integration; `cargo build --workspace` ok). RADIO-1: nothing keyed.
   - `bd`: sonde-44n + sonde-ajn noted "done in PR #62, close on merge."

## Remaining to close the link layer (tracked; inventory has full detail)
- **KEYSTONE — sonde-32q (B1+B2+B3): dynamic link↔PHY registry handshake.** The
  `mac.rs` ladder is still a static mirror (OFDM/nFSK `available:false`, `MainAuto`,
  illustrative `ModeProfile`s) while the PHY registers all 5 modes
  (`sonde_phy_runtime::standard_waveforms`). Build: `PhyTransport` (the `phy_api`
  seam) exposes registered modes {short_name, family, knee SNR_2500, ModeProfile};
  `Driver` reads it at construction, builds the ladder (availability + `MainPinned`
  hints + per-rung profiles); `apply_rung` swaps the profile/timers; map decoded
  `RxFrame.mode()` back to a rung. **Cross-layer — converge the seam shape +
  knee/profile sourcing with Codex first.** Unblocks the real ladder + the
  degenerate mode-degradation gates (link_gates.rs P4).
- **sonde-3tm (B4):** fragment host messages at transmit time for the active rung
  (not at enqueue) — a downshift currently can oversize queued fragments.
- **sonde-mua (B7):** idle keepalive/liveness timer (`keepalive_interval` is unused)
  + death handling.
- **sonde-p4g (B8):** hardware-free two-party multi-mode E2E (two `Driver<SondePhy>`
  over `standard_waveforms`, with a mid-session mode shift). Depends on the keystone.
- **12 Important + 4 Polish** (inventory §Important/§Polish): quality-from-unvalidated
  frames; link decode-fail not in FER; dropped encode/send errors; reserved-flag
  validation; control-frame payload validation; unhandled NAK; on_ack range bounds;
  SACK-vs-window cap; unbounded buffers; seq wrap; disconnect drain policy + `send()`
  returns nothing in any state; `HostStatus` surface; `rung(id)` returns unavailable;
  `over_timeout` unused; callsign validation depth.

## State
- Working tree clean on the branch (after this handoff commit). No stashes.
- `.claude/local-untracked-backup/` (in the MAIN checkout, gitignored) holds two
  stale untracked handoff drafts set aside during the main sync — not mine, can be
  discarded by their author.
- Other live sessions: sonde-b60 (PHY). Don't touch `sonde-phy*`.
