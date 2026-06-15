# Design: Mid-session link mode adaptation (receiver-feedback downshift)

> Status: design draft for review (Codex adversarial pass + PHY dependency
> answer pending). Extends the connected-mode link
> (`2026-06-14-link-mac-arq-mvp-design.md`). bd epic `sonde-lcw`.
> **Honesty:** the mechanism is exercised over the channel model with synthetic
> mode profiles; the real *speed* payoff lands only when the PHY exposes >1 real
> mode. Labeled "link-correct over channel model", never "HF-viable". RADIO-1.

## 1. Goal (operational)

Ride through changing HF conditions **mid-transfer** — speed up when the band
opens, drop toward the robust floor when it fades — **without dropping the link
or requiring operator intervention.** Today the mode is fixed at connect; this
makes it adapt during a session.

## 2. Control loop — receiver-feedback downshift (operator's proposed design)

1. **CONN** negotiates a starting mode (initial channel estimate / operator pick).
2. The **floor holder (sender)** keys an over at the current mode.
3. The **receiver** classifies the over — clean / detected-but-failed / nothing —
   and on its *own* reply over (it already owes an ack; half-duplex turn-taking)
   **piggybacks a quality report**, scaled by how bad it was. No extra round-trip.
4. The **sender is the single decider**: it maps the feedback through the existing
   `route()` to a new (lower) rung, **announces** the new mode (mode-id in the
   header), and switches for the next over.
5. Repeat → converges to base in **1–2 steps**. Upshift is *opportunistic*: probe
   one rung up only after K sustained clean overs (failure to upshift is
   non-catastrophic, so it can be lazy + hysteretic — no thrashing).

## 3. Invariants (the two pins that make it robust by construction)

- **P1 — Floor is the universal failure-convergence point. (MUST BE BUILT — it
  does not exist today.)** Adversarial review (Codex) correctly flagged that the
  current turn-recovery does *not* change mode: it retransmits / idles / declares
  `PeerLost`, and `profile`/window/strategy are construction-time only
  (`conn.rs:89`, `conn.rs:161-174`, `conn.rs:263-280`). So "a lost switch
  degrades to both-at-floor" is **a new requirement**, not free reuse. The rule:
  when a station hears no decodable peer over for **K consecutive recovery
  intervals (K < `DEAD_OVERS_TOLERATED`)**, it **falls to BASE mode +
  WholeMessage and keeps trying there**; `PeerLost` only if BASE *also* stays
  silent for the remaining budget. Both ends apply this symmetrically ⇒ they meet
  at BASE. This is what makes a botched renegotiation degrade to "slow but
  connected" instead of a deaf-retransmit death spiral.
- **P2 — Single decider = the floor holder (sender).** The receiver only *feeds
  back* quality; it never independently switches mode. P2 stops dual
  *decision-making* — but **not dual mode *belief* after a lost switch** (a
  receiver that missed the announcement still believes the old mode and may even
  seize the `Idle` floor and originate at it; `conn.rs:289-297`). Mode *belief*
  is reconciled only by **P1** (sustained no-decode ⇒ both fall to BASE) plus the
  post-decode mode-id check below. P1 is therefore load-bearing, not a backstop.
- **Mode-id in every frame header = post-decode *confirmation*, NOT discovery.**
  The header is visible only *after* PHY demod + decode (`frame.rs:3-19`), so it
  cannot tell a deaf receiver what to listen for — mode *discovery* is a PHY
  preamble concern (§6). The mode-id lets a station that *did* decode confirm the
  peer's mode and follow a downshift immediately (when the PHY auto-detects),
  and lets either end detect divergence. It is control metadata, not a substitute
  for P1.

## 4. The totally-garbled edge

If an over is undecodable (`DecodeScan::Detected`, or `NoSignal` with no usable
header), the receiver cannot send a *targeted* notice that over; its reply is
driven by the **turn-recovery timer** ("I heard nothing / quality bad") → sender
downshifts. So "1–2 retries" is sometimes "1–2 *recovery intervals*" — still
fast, still converges, and **P1** catches the tail. Designed for explicitly so
it is not a surprise.

## 5. Severity → step (intelligent downshift)

Reuse `route(payload_len, &ChannelQualityReport)` (`mac.rs`): the receiver's
feedback (the over's FER / SNR / fraction decoded) feeds `route()`, which already
yields a target rung. Big failure ⇒ jump toward floor (the operator's "1–2
retries to base"); marginal ⇒ one rung. The decision function already exists; we
are only wiring *when* it runs and *who* acts on it (the sender).

## 6. PHY dependency — the one open question (graceful vs blunt)

`Waveform::decode_scan(&self, samples) -> DecodeScan` takes **no mode hint** —
each waveform self-syncs on its own preamble. With one mode today RX is trivially
mode-agnostic. The open question for the multi-mode future:

> When >1 waveform is registered, does `SondePhy`'s RX pump run **each**
> registered waveform's `decode_scan` on the window (auto-detect the mode from
> whichever preamble syncs), or only the **currently-negotiated** mode's?

- **Auto-detect** ⇒ the receiver *never* goes deaf on a mode change (it decodes
  whatever arrives and reads the mode-id); graceful per-rung stepping; adaptation
  is nearly free. This is the operator's "it's not that hard" case.
- **Pre-tuned** ⇒ a lost downshift announcement = deafness until **P1**
  (recovery → floor); correct but *blunt* (any meta-signal glitch → jump to floor
  → climb back).

**Correctness holds either way (P1).** Auto-detect only buys *graceful* stepping.
This does not block building the link side; it sets the expected adaptation
*quality*. (Tracked as a PHY question.)

## 7. Mapping to existing code (`sonde-link`)

- `mac.rs route()` — the decision function; reuse for severity → rung.
- `conn.rs` — make **mode / window / strategy reconfigurable at an over
  boundary** (today fixed at construction via `with_strategy`). The DATA seq
  stream **continues** across a mode change (no reset — only framing/window
  change).
- `frame.rs` — add a 1-byte **`MODE_ID`** header field (a breaking wire change,
  acceptable pre-release) so every frame self-describes its mode.
- ACK/reply — carry a few bits of **quality / "request-lower" feedback** (or
  derive it at the receiver from `channel_quality()` and set a flag).
- `driver.rs` — **does need a change** (review correction): it currently sends
  every frame at a *fixed* `self.hint` (`driver.rs:159`). It must transmit at the
  connection's **current** mode, so `Connection` exposes `current_hint()` and the
  driver reads it per send. (The timing-freeze on `tx_in_flight()` is unaffected —
  a different over airtime is absorbed automatically, since the freeze reads the
  real signal, not an assumed airtime.)

## 8a. Review corrections — these are REQUIREMENTS, not caveats

Codex adversarial review (2026-06-14) found the v1 draft assumed machinery that
does not exist. Building proceeds only with these as explicit, tested requirements:

1. **Build P1.** Add a `current_mode` to `Connection`; on K<`DEAD` silent
   recovery intervals, fall to BASE + WholeMessage and keep trying before
   `PeerLost`. Symmetric on both ends ⇒ convergence to BASE. (Gate B/E below.)
2. **Per-over mode plumbing.** `current_hint()` on `Connection`; driver sends at
   it; `MODE_ID` header byte; receiver follows a decoded downshift.
3. **Safe ARQ reconfigure.** `SendWindow`/`RecvBuffer` change window + SACK
   enablement **in place**, preserving `base`/`next_seq`/in-flight + buffered
   state (no rebuild that drops outstanding frames). Tested: byte-exact delivery
   across a mid-session mode change (Gate D).
4. **Real hysteresis + a recent-quality window.** The link tracks its **own**
   recent per-over decode outcomes (it knows whether it decoded each peer over) —
   it does not rely on the PHY's *cumulative* counters for the adapt decision.
   Downshift immediately on bad feedback; upshift only after **K consecutive
   clean overs** (Gate C).
5. **Mode discovery is the PHY's job, not the header's** (open question, §6).

## 8. Gates (over the channel model)

- A: a transfer that hits a sustained fade **downshifts and keeps delivering**
  (no link drop, no operator action), converging toward floor.
- B: a lost downshift **announcement** still converges (P1 → floor), no deadlock.
- C: **no thrashing** — a brief glitch does not ping-pong the mode (upshift
  hysteresis).
- D: byte-exact in-order delivery is preserved **across** a mid-session mode
  change (the ARQ seq stream is continuous).

Exercised with synthetic mode profiles now; the real speed benefit is realized
when the PHY ladder exposes >1 mode. Labeled accordingly.
