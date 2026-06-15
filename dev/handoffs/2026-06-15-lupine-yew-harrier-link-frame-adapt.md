# Handoff — link frame-efficiency + mid-session adaptation (epic sonde-lcw)

**Date:** 2026-06-15
**Agent:** lupine-yew-harrier
**Branch state:** all work merged to `main`; this handoff lands on `sonde-lcw/handoff`.

## One-line state

The three tasks from the session brief are **done and merged with CI green**:
per-frame callsigns removed (sonde-sbt), the consent-gate task resolved as
won't-build (sonde-2f0), and the mid-session downshift control loop landed
(sonde-ruu) — plus a latent Driver bug fix (sonde-i3h). Each design was
Codex-converged before building; every change is **link-correct over the channel
model, never HF-viable; RADIO-1 — nothing keyed.**

## What landed

### PR #43 — `feat(sonde-link)!` drop per-frame callsigns (sonde-sbt, BREAKING wire v2)
- Header **42 → 22 bytes**, `LINK_OVERHEAD` 46 → **26** (−20 B/frame). `VER` 1→2.
- Callsigns now ride only on **ID-bearing** frames (`CONN`/`CONN_ACK`/`DISC`/`DISC_ACK`/`ID`)
  as a `StationId` block in the payload region; the data plane is addressed by **`conn_id`**.
- `Connection::acceptor` **learns** the peer from `CONN.src` (dropped its `remote` arg);
  learned-vs-configured peer reset.
- Part-97 §97.119 ID cadence: start/end via the conn frames; the periodic ≤10-min ID is
  enforced in **real time by the `Driver`** (the sans-IO connection's logical clock freezes
  during keying, so a logical-time timer would drift past 10 real minutes).
- Canonical spec: `docs/superpowers/specs/2026-06-15-frame-conn-id-addressing-design.md`.

### PR #45 — `feat(sonde-link)` mid-session downshift control loop (sonde-ruu, Fork B)
- Architecture **C** (Codex comparative adrev): **receiver-authoritative immediate downshift**
  (`rx_rung` packed into spare FLAGS bits 2–4, zero added header bytes) + **sender-gated
  hysteretic upshift**. Pure "default-to-lowest-on-any-ambiguity" was rejected (ambiguity is
  the common half-duplex case; the landed P1 BASE-fallback is the lost-control catch).
- Recent-quality window of the link's OWN decode-vs-missed-over outcomes; role-gated
  application (act on `rx_rung` only while awaiting a reply; `follow_mode` only while
  Listening); self-downshift on a missed reply (Idle-floor blind spot); window cleared on
  every rung change (anti-sawtooth + BASE cooldown).
- **Also fixed sonde-i3h**: the Driver advanced the ID cadence even when `send_frame` failed.
- Canonical spec: `docs/superpowers/specs/2026-06-15-downshift-control-loop-design.md`.

### sonde-2f0 — CLOSED won't-build
RADIO-1 is an **agent-discipline ADR/policy** (the AI never keys a transmitter), **not** a
runtime consent-gate feature — a default-denied software gate would wrongly block the licensee
operating their own station. Operator-clarified 2026-06-15. (See memory
`radio1-is-agent-discipline-not-runtime-gate`.) The earlier draft consent-gate spec was
discarded (never committed).

## Working-tree / environment

- Workspace gates green at each merge: `cargo test --workspace`, `clippy --workspace
  --all-targets -D warnings`, `fmt --all --check`. sonde-link: 89 lib + 2 g5 + 16 gates.
- **Local `main` is stale** (the no-ff merges live on `origin/main`; the race hook blocks
  updating the main checkout while another session holds the lease). Branch off `origin/main`
  via `new_sonde_worktree.py` (it fetches first) — do NOT rely on local `main`.
- A stale **`worktrees/sonde-ruu-link-mode-adapt`** worktree (merged `sonde-ruu.1/handoff`
  branch) remains from a prior session — left in place (not ours to dispose).
- A harmless stale local branch `sonde-sbt/drop-per-frame-callsign` could not be `-d`'d (race
  hook on the main checkout); merged on the remote, safe to ignore.

## Open follow-ups (filed in bd)

- **sonde-99l** (PHY, open dep): does multi-mode RX auto-detect the waveform? Sets
  graceful-vs-blunt adaptation AND unlocks the **real speed payoff** — today the rung change
  is real but the waveform is one PHY mode ("stubbed-but-not-faked").
- **sonde-44n** (P3): widen `conn_id` u16 → u32 (shared-channel collision robustness; a 2nd
  breaking wire bump).
- **sonde-ajn** (P3): half-open reconnect — accept a fresh `CONN` with a new `conn_id` from the
  known remote (currently dropped; not regressed).

## What is NOT done / pending decision

- Nothing from the brief is outstanding. The link layer is feature-complete for the modem-owned
  ARQ/link/adaptation model; the next substantive lever is the **PHY ladder >1 mode** (sonde-99l),
  which turns the (already-correct) adaptation policy into actual throughput.
