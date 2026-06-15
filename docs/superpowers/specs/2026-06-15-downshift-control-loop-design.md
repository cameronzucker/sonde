# Design — mid-session downshift control loop (Fork B, the remaining half of sonde-ruu)

**bd:** `sonde-ruu` (P2, in_progress) · **Status:** draft for Codex review · **Agent:** lupine-yew-harrier
**Extends:** `2026-06-14-link-mode-adaptation-design.md` (Fork B, Codex-reviewed). P1 already landed (PR #40).
**Honesty:** link-correct over the channel model with synthetic mode profiles; the real *speed*
payoff awaits a PHY ladder >1 mode (open question sonde-99l). Never HF-viable. RADIO-1: nothing keyed.

## 1. What P1 already gives (landed, do not rebuild)

`Connection::current_rung`/`apply_rung(id)` (restamp mode + in-place ARQ window/SACK reconfigure,
seq stream continuous) · `follow_mode(peer_mode)` (a station adopts the rung the peer is
transmitting under, read from the `MODE` header byte) · BASE-fallback (`DOWNSHIFT_TO_BASE_OVERS`
silent overs ⇒ fall to `BASE_RUNG`, symmetric ⇒ both meet at BASE = invariant **P1**) ·
`current_hint()` driven on the wire by the Driver · `mac::route`/`recommended_rung`/`rung`.

## 2. The remaining control loop — RECEIVER-AUTHORITATIVE downshift, SENDER-GATED upshift

**Architecture (Codex comparative adrev, 2026-06-15): Option C.** A second Codex pass compared the
operator-spitballed "sender-decides-on-feedback" (A) against "receiver-chooses + default-to-lowest"
(B) and a hybrid (C). **C wins.** Pure B is rejected: in this sans-IO half-duplex SM *ambiguity is
the common case* (ACK loss, idle silence, lost keepalive, a missed announcement all look alike until
timers classify them), so "floor on any ambiguity" collapses to BASE too often and still does not fix
a *lost command*. The correct lost-control mechanism is the landed **P1 BASE-fallback** (timer-driven
"fail to floor" *invariant*, not a reflex). C splits authority by direction:

- **Downshift is receiver-AUTHORITATIVE (mandatory).** The receiver is the only party that observed
  whether the sender's last over decoded. When a frame from the peer decodes and its advertised
  `rx_rung` is **more robust** than the floor holder's `current_rung`, the floor holder applies it
  **immediately** (`apply_rung`) — it may not overrule downward. Severity can jump several rungs.
- **Upshift is sender-GATED (hysteretic, receiver-permitted).** Upshift failure is non-catastrophic,
  so faster moves are paced by the floor holder: step up **one** rung only after `UPSHIFT_HYSTERESIS`
  consecutive permits, on **active DATA/ACK** exchange (not keepalive/ID).
- **`MODE` byte = confirmation; `rx_rung` (FLAGS) = command.** `MODE` says "this frame was sent under
  rung N" (the listener adopts it via the landed `follow_mode` so its replies match → both ends stay
  on one rung). `rx_rung` says "for your next overs, do not exceed this fragility." Distinct
  meanings, distinct directions.

### 2.1 Receiver-side recent-quality window (req 4)

The sans-IO `Connection` sees **only successfully-decoded frames** — a failed PHY decode is
dropped at the `Link`/`Driver` layer before `handle_frame`. So the quality window is built from
the **Connection's own observable per-over outcome**, not the PHY's cumulative counters:

- **clean** — a decodable peer over arrived when expected (recorded at `on_over_end`).
- **missed** — the turn-recovery timer fired while awaiting the peer (recorded in `handle_timeout`).

A small fixed ring (`QUALITY_WINDOW`, e.g. 4) of these outcomes. (Finer "detected-but-failed vs
nothing" needs PHY `DecodeScan` info the Connection lacks today — the §6 PHY open question,
sonde-99l; binary clean/missed is what is honestly observable now and is exactly "track your own
decode outcomes.")

### 2.2 Feedback on the wire — packed into spare FLAGS bits (zero added header bytes)

Honoring the airtime ethos (sonde-sbt just removed 20 B/frame), the feedback adds **no header
byte**. `FLAGS` bits 0–1 are `END_OF_OVER`/`END_OF_MSG`; **bits 2–4 carry `rx_rung`** = the
receiver's recommended rung (0..`NUM_RUNGS`-1 = 5 rungs ≤ 3 bits), computed from its recent-quality
window. Every frame a station sends advertises its current recommendation for the *peer's* TX rung.
(Alternative considered: a dedicated `RX_RUNG` header byte — rejected as 1 wasted B/frame against
the airtime priority. Reviewer: is overloading FLAGS acceptable, or is a byte cleaner?)

`recommended_rung_from_window()`: a **full** clean window ⇒ recommend `current_rung - 1` (permit one
faster — only when the window is full, never on a single clean over, finding 1); any recent miss ⇒
recommend a more-robust rung scaled by miss severity (toward BASE on a full window of misses); a
partial/empty window ⇒ recommend `current_rung` (no change — bootstrapping, H4). Clamped to
`[0, BASE_RUNG]`.

**FLAGS guardrails (finding 6):** `rx_rung` accessors mask only bits 2–4 and **preserve bits 0–1**
(`END_OF_OVER`/`END_OF_MSG`); a decoded `rx_rung > BASE_RUNG` is clamped to `BASE_RUNG` (never
trusted to index past the ladder); **bits 5–7 are reserved (must be 0)**. Unit-tested with both END
bits set alongside `rx_rung`.

### 2.3 Role-gated application (resolves the `follow_mode`-vs-decision ordering hazard)

A peer frame carries both `MODE` (peer's TX rung) and `rx_rung` (peer's command for me). Applying
both blindly races (correctness finding 2). Gate by **floor role**, which the SM already tracks:

- **I am awaiting a reply** (I sent an over; this frame is the peer's reply ⇒ I am the floor-holding
  decider): apply the peer's `rx_rung` **command** — downshift immediately if more robust; advance
  the upshift streak if faster (and on active DATA/ACK). Do **not** `follow_mode` here (I lead).
- **I am Listening** (the peer holds the floor; this is its over ⇒ I am the follower): `follow_mode`
  the peer's `MODE` so my replies match, and **record the quality outcome** (clean). Do not treat
  `rx_rung` as a command (I am not deciding this direction).

This keeps both ends on **one** rung (no persistent desync, finding 2 / H2) and never lets
`follow_mode` clobber a downshift the floor holder just applied.

### 2.4 Self-downshift on a missed reply (closes the Idle-floor blind spot — finding 4)

The receiver can only feed back about overs it *decoded*; data lost on an Idle-originated over yields
**no feedback at all**. So on the floor holder's `awaiting_reply` turn-recovery (no decodable reply):
record a **missed** outcome **and downshift its own next-TX rung one step**, independent of feedback.
Sustained misses still cascade to P1 BASE-fallback (landed). This makes graceful downshift trigger
even when the only evidence is "my reply never came."

### 2.5 Anti-thrash / dwell rules (findings 1, 3, 5)

- On **any rung change** (up or down), **clear the quality window** and reset the upshift streak —
  stale clean samples from a faster rung must not justify climbing back immediately (finding 1).
- After **P1 BASE-fallback**, additionally require a **full clean window dwell at BASE** before any
  upshift probe (a BASE cooldown), so the graceful loop does not fight P1's "meet at floor" role
  (finding 3).
- Only **active DATA/ACK** overs advance the upshift streak; **keepalive/ID** overs give liveness
  (and a miss is still downshift evidence) but never count as clean upshift evidence (finding 5).

## 3. Hazards → resolutions (both Codex passes folded in)

- **H1 — upshift sawtooth / feedback-vs-`follow_mode` oscillation.** Resolved by §2.5 (clear the
  window + streak on every rung change; full-window-clean before a probe) and §2.3 (role-gating, so
  `follow_mode` never clobbers a just-applied downshift). Damping = immediate-downshift /
  hysteretic-upshift asymmetry.
- **H2 — both-ends desync under half-duplex.** Resolved by §2.3 role-gating: only the floor holder
  acts on `rx_rung`; the listener only `follow_mode`s the announced `MODE`. Both ends converge to one
  rung; an `Idle`-floor seize uses the latest applied rung. A transient simultaneous-seize collapses
  via the existing collision/P1 paths, not a persistent split.
- **H3 — fighting P1.** Resolved by §2.5 BASE cooldown (full clean dwell at BASE before any probe),
  so the loop cannot upshift out from under a P1 hold. P1 stays the lost-announcement catch.
- **H4 — bootstrapping.** Resolved: a partial/empty window recommends `current_rung` (no change).
- **H5 — Idle-originated data lost yields no feedback.** Resolved by §2.4 self-downshift on a missed
  reply (the floor holder steps down on its own turn-recovery), with P1 as the sustained-loss catch.

## 4. Gates (channel model; never HF-viable; RADIO-1)

- **A** — a sustained fade (loss burst) downshifts and keeps delivering, converging toward floor.
- **C** — no thrash: a brief 1-over glitch does not ping-pong the rung (upshift hysteresis).
- **D** — byte-exact in-order delivery across a mid-session rung change (ARQ seq continuity; P1's
  in-place reconfigure — re-assert here under the live loop).
- **A2 (upshift)** — after a fade clears, sustained clean overs upshift one rung at a time.
- (B — lost announcement → P1 floor — already covered by the landed BASE-fallback gate G9.)

## 5. Blast radius (all `crates/sonde-link/`)

`frame.rs` — `rx_rung` accessors over FLAGS bits 2–4 (helpers + round-trip tests; no new field/offset
change). `conn.rs` — recent-quality ring + `recommended_rung_from_window`, stamp `rx_rung` on
outgoing frames, sender decision (downshift-now / upshift-hysteresis) on inbound, record clean/missed
outcomes. `mac.rs` — reuse `recommended_rung`/`rung` (maybe a window→rung helper). New gates in
`tests/link_gates.rs`. No other crate affected.
