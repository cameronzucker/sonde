# Design — symmetric, measurement-based link adaptation (final shape)

**bd:** `sonde-qnq` (P2) · **Status:** draft for Codex review · **Agent:** lupine-yew-harrier
**Supersedes:** the upshift path of `2026-06-15-downshift-control-loop-design.md` (sonde-ruu, #45).
**Honesty:** link-correct over the channel model; the real quality signal + speed payoff await the
PHY (sonde-99l). RADIO-1 — nothing keyed.

## 0. How we got here (the chain of corrections — context for review)

1. **Fork B / P1 (landed #40):** mid-session mode plumbing — `apply_rung`, `follow_mode`, per-over
   `MODE` byte, BASE-fallback on sustained silence (the "fail to floor" invariant).
2. **Downshift loop (landed #45), Codex-converged to "architecture C":** receiver-**authoritative
   immediate downshift**; sender-**gated hysteretic upshift** (probe one rung up after N consecutive
   "faster" permits). The receiver's quality signal was a **binary clean/missed over window** —
   chosen because the sans-IO `Connection` never sees a *failed* decode (the `Link`/`Driver` drops it
   before `handle_frame`), so "missed reply via turn-recovery" was the only observable degradation.
3. **Operator correction A:** "*several consecutive permits* is unrealistic in an amateur-radio
   context." Correct — each permit is a slow half-duplex round-trip; ~7 clean overs per rung to climb
   is useless when a band window is seconds-to-minutes.
4. **Operator correction B (the deeper one):** "the receiver can authoritatively determine *how far*
   to downshift — why not upshift?" Correct. The asymmetry was **an artifact of the binary signal**,
   not a principle: a *miss* authoritatively means "too fast" and its count gives the downshift
   distance, but a *clean decode* only proves the current rung works — with binary info you cannot
   compute the SNR **margin** above it, so upshift distance had to be *probed*.

**The fix (this doc):** give the receiver the magnitude it was missing — a channel **measurement**
(SNR + FER) — and it becomes authoritative in **both** directions in one shot, exactly as the
original MVP adaptation spec intended (§5: "the receiver's feedback feeds `route()`, which already
yields a target rung"). No probe, no round-trip counting.

## 1. Final shape

### 1.1 Receiver is authoritative in both directions, from a measurement

`mac` already maps quality → rung (`recommended_rung(&ChannelQualityReport)` / `route()`), in both
directions, via per-rung `snr_floor_db` thresholds penalized by FER. The receiver:

- observes the PHY's per-over `channel_quality()` (aggregate SNR + windowed FER),
- maps it to a target rung **with threshold hysteresis relative to the current (shared) rung**, and
- advertises that rung as `rx_rung` (FLAGS bits 2–4, unchanged from #45).

The floor-holding sender **obeys it authoritatively, up or down** (`apply_rung`). No probe, no
consecutive-permit counter.

### 1.2 Threshold hysteresis = a small SNR dead-band (the only hysteresis)

A new pure function `mac::adapt_rung(quality, current_rung) -> u8`:

- **Downshift:** if effective SNR drops below the **current** rung's `snr_floor_db`, return the
  rung the SNR actually supports (may be several steps — authoritative distance).
- **Upshift:** return a faster rung only if effective SNR exceeds that faster rung's `snr_floor_db`
  **+ `HYSTERESIS_MARGIN_DB`** (e.g. 2–3 dB). Climbing through several clearly-supported rungs at
  once is allowed and desired (grab the open band).
- **Else stay** (the dead-band). Asymmetric thresholds (up needs margin, down does not) prevent
  flapping right at a boundary without slowing legitimate change.

This replaces *both* the binary window and the consecutive-permit hysteresis from #45.

### 1.3 SNR is rung-independent → no window-clear, persistent estimate

Unlike the binary clean/missed window (which had to be cleared on every rung change because
"clean-at-rung-2" said nothing about rung-1), **SNR is a property of the channel, not the rung**, so
the smoothed estimate persists across rung changes — simpler and more correct. The link keeps a
light **EWMA** of per-over SNR/FER for responsiveness control (open question for review: EWMA in the
link vs. trusting the PHY report's own windowing — `recent_frames_*` is already windowed).

### 1.4 What stays from #45 / P1 (the no-measurement cases)

A *total* loss yields **no measurement at all** (you can't measure the SNR of an over you didn't
receive). So these remain, unchanged:

- **Sender self-downshift one step on a missed reply** (turn-recovery) — graceful step-down when no
  feedback can arrive.
- **P1 BASE-fallback** on sustained silence — the "fail to floor" convergence invariant + the
  lost-announcement catch.
- **`follow_mode`** — the listener adopts the sender's announced `MODE` so replies match; the
  role-gate (act on `rx_rung` only while awaiting a reply) is retained.

### 1.5 Wiring (sans-IO purity preserved)

`channel_quality()` is an IO concern living at the `Driver`/`Link` seam, not the sans-IO core. So:

- `Connection::observe_quality(snr_db: f32, fer: f32)` (or `&ChannelQualityReport`) feeds the
  measurement in; the Connection holds the EWMA + computes `rx_rung` via `mac::adapt_rung`.
- The `Driver`/`Link` call it per decoded over from `phy.channel_quality()`.
- The deterministic gates (the `Pair`, `link_gates`) feed **synthetic** SNR/FER directly (the
  in-memory doubles report no SNR). The loop is thus testable now; it gets real teeth when the PHY
  exposes usable per-mode quality (sonde-99l).

## 1bis. Codex-folded fixes (binding — these are the final rules)

Codex review (2026-06-15) confirmed the symmetric-measurement direction and the chain-of-corrections
diagnosis, with six required fixes. They are now the spec:

- **F1 — authoritative within the measurement domain; conservative across waveform families.** A
  high SNR on the *current* mode does not prove a *different bandwidth/waveform* decodes. So
  multi-rung authoritative upshift is allowed **within one waveform family** (today rungs 0–2 are all
  `MainAuto` OFDM); crossing a family boundary upward (OFDM↔Floor↔deep-Floor) is capped to **one
  family step** per decision (a guarded probe). `mac` tags each rung with a family; `adapt_rung`
  enforces the cap. (Largely latent today — one real PHY mode — but the shape is correct for sonde-99l.)
- **F2 — dead-band = +3 dB on upshift only.** Downshift on **raw/instantaneous** effective SNR
  (protect delivery, no smoothing lag); upshift only when **smoothed** SNR exceeds the target rung's
  `snr_floor_db` **+ 3 dB**. 3 dB clears estimator jitter + shallow fades without blocking real
  openings (the ladder gaps are larger).
- **F3 — asymmetric smoother (not "trust the PHY report").** The `ChannelQualityReport` exposes no
  window length/age, so the link keeps its own estimate: **falling SNR applied immediately**, **rising
  SNR via EWMA α≈0.5** (≈1–2 good overs to climb — fast, not the old permit crawl). Downshift uses raw,
  upshift uses smoothed.
- **F4 — FER is a confidence VETO, not an SNR penalty (was a blocker).** Do **not** use
  `snr − FER·40 dB` for the adapt decision — one failed frame in a 1-frame report makes 30 dB look
  like −10 dB. Instead: **SNR chooses the candidate rung; FER gates it.** Upshift only if the FER
  sample count is credible (`≥ FER_MIN_SAMPLES`) **and** FER is very low. FER-driven downshift
  requires a minimum denominator or consecutive failures. Total-loss self-downshift stays separate.
  (`mac::route`/`recommended_rung` keep the old penalty for *connect-time* selection; the new
  `adapt_rung` does not.)
- **F5 — asymmetric paths: the worse direction wins.** With one shared rung, the good direction must
  not pull the rung up and break the bad one. The sender sets its rung to the **more robust of**
  (the peer's `rx_rung` feedback about *this* TX direction) **and** (its own RX recommendation about
  the *reverse* direction). On a reciprocal channel these agree; under asymmetry the robust one wins.
  (Per-direction TX rung is the more-optimal alternative — noted as a future option, not built now.)
- **F6 — measurement domain validity (corrects "SNR is rung-independent").** Physical SNR is
  rung-independent, but *reported usable* SNR/FER is mode/bandwidth-conditioned. So: **reset FER stats
  on every rung change**, and **reset the SNR EWMA on a cross-family change** (keep it within a family).

**Net upshift cadence:** ≈1–2 good overs to climb a rung within a family (vs the old ~7 + per-step
crawl), multi-rung when SNR is clearly high within a family, one guarded step across a family,
instant authoritative downshift. This is the realistic ham-radio behavior the operator asked for.

## 1ter. End-to-end wiring (Codex completeness review, 3 rounds)

A dedicated end-to-end review (no-unwired-islands) traced the full path and drove three fixes:

- **Measurement ordering + freshness.** `Driver`/`Link` `poll()` **batch-drain** the decoded inbound
  frames, call `observe_quality(channel_quality())` **once, only if the batch is non-empty, before**
  replaying the frames into the SM. This (a) feeds the SM the *fresh* measurement before
  `apply_peer_feedback`'s worse-wins reads it, and (b) never re-applies a stale idle snapshot (which
  would corrupt the SNR EWMA / FER cadence). A total-loss over yields no decode → no measurement →
  the sender's **self-downshift on a missed reply + P1 BASE-fallback** cover it (intentional).
- **`Link` transmits at `current_hint()`.** `Link` previously sent at a fixed constructor hint, so a
  rung change never reached the wire through it; it now matches the `Driver` (`Link::new(phy, conn)`,
  sends at `conn.current_hint()`).
- **`CONN_ACK` bootstrap.** `on_conn_ack` applies the acceptor's initial `rx_rung` once the initiator
  becomes the first-over decider — so the first over starts at the right rung, not blindly at
  `DEFAULT_RUNG`. (A no-op when there is no measurement, so the handshake is undisturbed.)

The confirmed path: **PHY `channel_quality()` → Driver/Link `observe_quality` → conn SNR/FER state →
`recommended_rung` = `mac::adapt_rung` → stamped `rx_rung` on every outgoing frame → peer decode →
`handle_frame` role-gate (decider → `apply_peer_feedback` worse-wins → `apply_rung`; follower →
`follow_mode`) → `current_rung` → `current_hint()`/`MODE` on the wire → convergence.** Integrity
(CRC + ARQ) is independent of all of it.

## 2. Open questions / risks (Codex-reviewed — see §1bis for resolutions)

- **R1 — EWMA vs trust-the-PHY-report.** Does the link need its own SNR smoothing, or is
  `channel_quality()` already "recent" enough? Over-smoothing slows downshift (bad); under-smoothing
  flaps. What time constant, and is the dead-band alone sufficient stability?
- **R2 — FER vs SNR weighting.** `effective_snr = snr − FER·PENALTY` (existing `mac`). With SNR now
  driving upshift too, does the FER penalty interact badly with the dead-band (e.g. a single failed
  frame yanking the effective SNR across a boundary)? Should upshift gate on *both* low FER and SNR
  margin?
- **R3 — measurement vs reciprocity.** The receiver measures the *forward* path (sender→receiver).
  It recommends the sender's TX rung. HF is roughly reciprocal, but not exactly. Acceptable? (It was
  the same assumption in #45.)
- **R4 — both-ends still converge to one rung.** With authoritative two-way obey, does any
  sequence (Idle-floor seize, simultaneous data) leave the ends on different rungs persistently, or
  does `MODE`/`follow_mode` + the role-gate still collapse it?
- **R5 — does removing the window-clear + permit-count reintroduce thrash** anywhere the binary
  design prevented, given SNR persistence + dead-band?
- **R6 — bootstrap:** no measurement yet ⇒ recommend `current_rung` (no change). Same as before.

## 3. PHY dependency (sonde-99l) — what the link needs from below

For real adaptation the PHY must expose, per received over, a **per-mode-meaningful SNR estimate**
and a recent FER through `channel_quality()` (`ChannelQualityReport`), AND answer the multi-mode RX
question (auto-detect vs pre-tuned) so a downshift announcement is not deafening. See §3 prompt below.

## 4. Gates (channel model; never HF-viable; RADIO-1)

- `mac::adapt_rung` unit tests: downshift jumps to the SNR-supported rung; upshift needs margin;
  dead-band holds inside the band; multi-rung climb when SNR is clearly high.
- Integration (Pair w/ synthetic SNR): a fade (SNR drop) downshifts authoritatively + keeps
  delivering; an opening (SNR rise) upshifts promptly (no round-trip counting); a marginal SNR
  sitting in the dead-band does **not** flap; byte-exact delivery across the change.
- Existing G1–G9 + the #45 self-downshift/P1 paths stay green. Nothing keyed.
