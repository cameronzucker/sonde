# Handoff — link adaptation: symmetric SNR + registry-honest ladder (epic sonde-lcw)

**Date:** 2026-06-15
**Agent:** lupine-yew-harrier
**Branch state:** all work merged to `main`; this handoff lands on `sonde-lcw/handoff2`.

## One-line state

The modem-owned **link layer is feature-complete** — connected-mode ARQ, Part-97
framing, and **mid-session mode adaptation are all landed and registry-honest**.
What remains to "finish the link" is mostly **PHY-gated** (real modes to adapt
*across*) plus a few small follow-ups. Everything is **link-correct over the
channel model, never HF-viable; RADIO-1 — nothing keyed.**

## What landed this session (all merged, CI green)

| PR | What |
|---|---|
| **#43** | `feat(sonde-link)!` drop per-frame callsigns — wire **v2**, header 42→22 B, `conn_id` addressing, callsigns only on ID-bearing frames, Part-97 ID at start/end + real-time periodic ID in the `Driver`. |
| **#45** | mid-session downshift control loop (Fork B). |
| **#48** | `feat(sonde-link)!` **superseded #45's upshift** with **symmetric, measurement-based** adaptation (`mac::adapt_rung`: raw-SNR downshift, smoothed-SNR + 3 dB + FER-veto upshift, one-family-step cap; `observe_quality` asymmetric EWMA fed from `channel_quality()`; worse-direction-wins; domain resets; `on_conn_ack` bootstrap). Driver end-to-end test. |
| **#53** | `feat(sonde-link)!` **ladder-from-registry (C7)** — unbacked rungs **unselectable**; today only the wideband floor (rung 3) is real; floor knee = **estimator-domain SNR_2500 ≈ 16 dB**; `base_rung()`/`default_rung()` = most-robust *available*; `clamp_available`; consume `snr_2500_db`; **default ARQ is now the floor's WholeMessage/stop-and-wait** (operator-chosen honest default). |
| — | **sonde-2f0 closed WON'T-BUILD**: RADIO-1 is an agent-discipline ADR, **not** a runtime consent gate (operator-clarified). |

Design docs (canonical): `docs/superpowers/specs/2026-06-15-symmetric-snr-adaptation-design.md`,
`…-ladder-from-registry-design.md`, and the PHY-side `…-phy-mode-adaptation-quality-design.md`.
Memory: `sonde-link-design-v3-codex-converged-x3-connected` (bd) summarizes the arc.

## Key invariants (don't regress)

- **Integrity is independent of adaptation:** CRC32 + ARQ guarantee byte-exact
  delivery at every rung; FER (decode success) is the upshift veto.
- **No fabricated mode is ever selectable** (C7): the ladder mirrors the PHY's
  registered, physics-gated waveforms; `clamp_available` rounds any request to a
  real rung; wire `MODE` ids stay stable (protocol contract).
- **Honest consequence today:** with one registered mode the adaptation loop is
  *inert* (nothing to adapt to) — the machinery is correct and unit-tested over a
  synthetic all-available ladder (`mac::adapt_rung_with`), and gains range when more
  modes register.

## To FINISH the link (next sessions)

1. **sonde-c7i — OFDM family waveforms (PHY).** The biggest lever: registering real,
   gated OFDM `Waveform`s unlocks rungs 0–2, their real `SNR_2500` knees, and brings
   SelectiveRepeat back as a live default. Until then the link is floor-only
   WholeMessage. The link side is *ready* (flip `available` + bake the real knees).
2. **Wrap `NarrowFskFloor` as a `Waveform` (PHY)** → unlocks the deep-floor rung 4.
3. **sonde-44n** — widen `conn_id` u16→u32 (shared-channel collision robustness; a
   breaking wire bump).
4. **sonde-ajn** — half-open reconnect (accept a fresh CONN w/ a new conn_id from the
   known remote).
5. **Dynamic link↔PHY registry handshake** — replace the link's *static* availability
   mirror with the PHY actually enumerating registered modes/knees (single source of
   truth). Needs a `PhyTransport`/runtime registry-enumeration API.
6. **PHY TX-routing-by-family aliasing** (Codex flag): the runtime routes TX by
   `ModeFamily`, so `FloorCrowdedBand` could alias to `FloorWaveform`. The link never
   selects that rung, so it's latent — but the PHY should route by exact mode.

## Working-tree / environment notes

- **Local `main` is stale** (no-ff merges live on `origin/main`; the race hook blocks
  updating the main checkout). Always branch off `origin/main` via
  `new_sonde_worktree.py` (it fetches first).
- **No full Codex end-to-end review on #53** (context wall) — the design was
  Codex-converged and the impl is mechanical + fully gated, but a completeness pass
  (no-unwired-islands, like #48 got) is worth doing if any doubt remains.
- Stale sibling worktrees from prior sessions remain under `worktrees/` (not ours to
  dispose). This session's worktrees are all disposed.

## Pending decision

None blocking. The link is feature-complete; the next substantive throughput comes
from the **PHY** registering real modes (sonde-c7i + nFSK wrap), at which point the
already-built adaptation gains real range with no link refactor.
