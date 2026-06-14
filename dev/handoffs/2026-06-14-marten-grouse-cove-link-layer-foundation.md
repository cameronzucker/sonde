# Handoff — Link layer foundation (sonde-link)

- **Agent:** marten-grouse-cove · **Date:** 2026-06-14
- **Epic:** `sonde-lcw` (Modem link layer #5/#6/#8)
- **Branch:** `sonde-o0s/link-mac-mvp` (pushed) · **Worktree:** `worktrees/sonde-o0s-link-mac-mvp` (based on `origin/main` @ `a9d5221`)
- **bd:** `sonde-o0s` (#5) in_progress; `sonde-pwh` (#6-MVP), `sonde-5yg` (#6-full), `sonde-a0p` (#8), plus filed PHY-runtime bug for RX-window reassembly.

## What this is

The modem-owned link layer (VARA/ARDOP-vein, **half-duplex turn-taking — not a packet network**).
Scope expanded mid-session via two operator interventions:
1. Half-duplex/simplex realignment (killed full-duplex "Radio DSL" drift).
2. Operator audit "gate on physics, not artifacts" → target is now the **full connected-mode
   selective-repeat ARQ link**, gated by **reliable in-order delivery over a realistic lossy
   channel** (Gilbert–Elliott bursty loss + corruption), NOT a clean loopback. Plus an FYSA: the
   adaptation ladder now extends to an **FT8-class deep-floor** bottom rung; link timers must be
   **mode-derived (airtime-aware)** down to ~tens-of-seconds overs and "trickle, not fail."

## Completed (committed `939e459`)

- **Design doc** `docs/superpowers/specs/2026-06-14-link-mac-arq-mvp-design.md` (v3, Codex-converged
  across 3 rounds). Resolves the spec open questions; §3.5 is the key fix.
- **Frame codec** `crates/sonde-link/src/frame.rs` — **11 tests green, clippy+fmt clean**:
  wire format (MAGIC/VER/TYPE/FLAGS/SRC/DST/CONN_ID/SEQ/ACK_THROUGH/SACK/LEN/PAYLOAD/CRC32),
  exact-length + CRC-first parse, validated Part-97 `Callsign` in every frame, `LINK_MTU`,
  and **`FLAGS.END_OF_OVER` = the in-band turn token** (the seam has no carrier-sense).
- Crate scaffolded + added to workspace members.

## The core design decision the next session MUST honor

**Turn ownership without carrier-sense** (Codex round-3 convergence): `PhyTransport`
(`send_frame`/`poll_rx`) gives no "peer's over ended" or "my TX done" signal, and `SondePhy` is
TX-priority (an early retransmit crowds out the ACK-receive window → collision/livelock). Resolution:
- floor passed **in-band** via `END_OF_OVER`;
- **connection initiator owns the first floor** (deterministic, no split-brain);
- **turn-recovery timer** (mode-derived) re-takes the floor if the token/over is lost;
- **callsign tie-break + jittered backoff** resolves simultaneous re-takes.

## Next chunk (in order)

1. `tests/` **lossy-channel harness** — `HalfDuplexLossyLink` at the *frame* level (two
   `PhyTransport` endpoints, single shared medium, mutual-exclusion+collision), injecting
   **Gilbert–Elliott bursty loss + byte corruption + variable RTT** (seeded/deterministic).
2. `conn.rs` **connection state machine** — CLOSED→CONNECTING→CONNECTED→DISCONNECTING; the
   SENDING_OVER⇄LISTENING floor sub-state driven by `END_OF_OVER` + turn-recovery timer; CONN/CONN
   collision tie-break; half-open rejection via CONN_ID; idempotent CONN/DISC. Exhaustive unit tests.
3. `arq.rs` **selective-repeat** (window-per-over, cumulative ACK_THROUGH + SACK; floor degenerate
   W=1 whole-message) + in-order/no-dup/no-corrupt reassembly + fragmentation.
4. `adapt.rs` / `mac.rs` — `route()` + `ModeProfile` (airtime-aware timers + per-mode MTU; deep-floor
   bottom rung). Single mode today ⇒ adaptation plumbing unit-tested over profiles; mode-STEP gated.
5. `host.rs` — minimal HostCommand/HostEvent.
6. **Gates G1–G5** (design §8): G1 in-order delivery under loss+corruption; G2 connect/teardown under
   control-frame loss; G3 burst recovery; G4 honest failure (explicit PeerLost, no hang/corrupt);
   G5 wiring smoke over real `SondePhy` (labeled wiring, not viability).
7. Full gates → PR → merge → bd close → handoff.

## Honesty / labeling rule (operator directive)
No capability is "done" until its gate passes. Results are "link-correct over channel model {params}",
**never** "HF-viable" — over-the-real-PHY validation waits on the PHY physics gates (program items
0–3) owned by the main modem agent. RADIO-1: nothing keys a real radio; all tests are in-memory doubles.

## Working-tree / coordination state
- Worktree clean after commit `939e459` (pushed). No stashes; no untracked/gitignored stateful content.
- Concurrent agent in `worktrees/sonde-64w.3-complex-correlator` on `crates/sonde-phy/src/sync/preamble.rs` — stay out.
- `main` moves under this branch (was ~69 commits ahead of the stale local checkout at session start; this worktree is based on `origin/main`). Rebase the branch on `origin/main` before PR.
