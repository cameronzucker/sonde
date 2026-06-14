# Design: Modem Link layer — connected-mode selective-repeat ARQ (#5/#6/#8)

> Status: design v3 (post operator audit "gate on physics, not artifacts" + course-correction
> to the full connected-mode link). Implements `tuxlink docs/superpowers/specs/2026-05-31-clean-sheet-modem-{5-link-mac,6-arq,8-host-protocol,overview}`.
> bd epic `sonde-lcw`. Moniker: marten-grouse-cove.
>
> **Discipline:** no capability is "done" until a gate proves it. A clean loopback round-trip
> and green CI are NOT evidence. This layer's gate is **reliable in-order delivery over a
> realistic lossy channel + connection establish/teardown** (§8). Results are labeled
> "link-correct over channel model X," never "HF-viable" — over-the-real-PHY viability is
> integration work gated on the PHY physics gates (program items 0–3), owned elsewhere.

## 0. Operating model — half-duplex, push-to-talk, turn-taking

One rig, half-duplex at best (often simplex): a station transmits **or** receives, never both,
and is deaf while keyed. One station transmits at a time; overlaps **collide**. Turnaround
(drop PTT → peer keys up) is explicit and expensive. **Selective-repeat here is window-PER-OVER,
turn-taking — never full-duplex pipelining.** A station with the floor sends a burst (window) of
frames in one over, reverses, and the peer replies in its over with one cumulative+selective ACK.
(Per ADR 0014 this uses the conceptual half-duplex-ARQ primitive as background; no examination of
VARA/ARDOP internals. "More robust than VARA" is a clean-sheet design posture, §7.)

## 1. Architecture posture

The **modem owns** the link: framing, station ID, sequence numbers, the connection state machine,
selective-repeat ARQ, retransmission, turnaround, link adaptation. New crate **`crates/sonde-link`**.
Tuxlink is the client; no link code in tuxlink. Generic over `P: PhyTransport`
(`sonde_phy::phy_api`) so it runs over any PHY/transport double unchanged.

## 2. Seam facts (current `origin/main`)
- `send_frame(&[u8], ModeHint)->TxToken` queues opaque bytes (= one serialized link frame);
  returns on enqueue, not TX-complete.
- `poll_rx()->Option<RxFrame>`; `RxFrame::payload()` = the peer's bytes; a failed/absent decode =
  `poll_rx` returns nothing (no usable `decode_ok`). Remote loss ⇒ inferred from a missing reply
  within a deadline.
- `channel_quality()->ChannelQualityReport` (FER, aggregate SNR) — consumed by link adaptation (§6).
- **The link gate is at the frame level (PhyTransport boundary), decoupled from the not-yet-valid
  waveform.** A real-SondePhy *wiring smoke test* (clean path) proves the generic link drives the
  real trait (anti-island), labeled as wiring only — not a robustness/viability claim.

## 3. Frame format (#5)

Big-endian, variable-length, one frame = one PHY `send_frame`. Exact-length, CRC-first parse.

```
off field        size  notes
0   MAGIC 53 4C   2
2   VER           1    = 1
3   TYPE          1    1 DATA · 2 ACK · 3 NAK(optional, ACK SACK is primary) · 4 CONN · 5 CONN_ACK · 6 DISC · 7 DISC_ACK · 8 KEEPALIVE
4   SRC callsign  10   Part-97 station ID, validated, EVERY frame
14  DST callsign  10
24  CONN_ID       2    u16 session id (CONN-negotiated; rejects cross-session/half-open frames)
26  SEQ           4    u32. DATA: frame seq. control: context seq (e.g. CONN proposed-initial-seq)
30  ACK_THROUGH   4    u32 cumulative in-order-received high-water (ACK/data-piggyback); 0 if n/a
34  SACK          4    u32 bitmap: bit i set ⇒ (ACK_THROUGH+1+i) received out-of-order
38  LEN           2    u16 payload length
40  PAYLOAD       N
40+N CRC32        4    IEEE, over [0 .. 40+N)
```

Header = 44 bytes. **Parse rule:** require `buf.len() == 44+LEN` AND CRC valid **before** trusting
any field; else drop. A corrupted LEN ⇒ length mismatch ⇒ drop (CRC offset can't walk).
`Callsign` is validated (`[A-Z0-9/]`, 1–10, NUL-pad). **Link MTU per over-frame** = `u16::MAX-44`;
host messages above the per-message cap are fragmented across DATA frames (seq-ordered) and
reassembled in-order at the receiver.

## 3.5 Turn ownership — explicit in-band token (Codex convergence: the core fix)

The `PhyTransport` seam offers **no carrier-sense and no TX-complete signal** —
`poll_rx`/`send_frame` only yield decoded frames or nothing, and the `SondePhy`
worker is TX-priority (an early retransmit crowds out the ACK-receive window).
Turn ownership therefore cannot rely on "hearing the peer go quiet." It is passed
**explicitly in-band**:
- A `FLAGS` byte (frame offset 4) carries `END_OF_OVER` (bit 0). The **last frame
  of an over** sets it; receiving it passes the floor to the peer.
- **The connection initiator owns the first floor** after `CONN_ACK` — deterministic,
  no split-brain.
- A **turn-recovery timer** (mode-derived, §6) runs while LISTENING: if the floor
  token (or the peer's whole over) is lost, the previous holder re-takes the floor
  and retransmits — it never waits forever. Simultaneous re-takes resolve by the
  **callsign tie-break** (same ordering as connect-collision) + jittered backoff.

## 4. Connection state machine (#6 — the top failure mode; Codex-converge this)

Per-peer session. States and the half-duplex floor sub-state are explicit.

```
States:  CLOSED → CONNECTING → CONNECTED → DISCONNECTING → CLOSED
Floor (in CONNECTED): SENDING_OVER ⇄ LISTENING  (whose turn to key up)

Events: HostConnect, HostSend(bytes), HostDisconnect, Rx(frame), Timer(kind)

CLOSED:
  HostConnect      → send CONN(conn_id=rand, init_seq); start connect-retry timer; → CONNECTING
  Rx(CONN)         → accept: send CONN_ACK; adopt conn_id; → CONNECTED (LISTENING/peer has floor per tie-break)
  Rx(other)        → drop (optionally RST to clear a stale half-open peer)
CONNECTING:
  Rx(CONN_ACK)     → → CONNECTED (we hold the floor: SENDING_OVER)
  Rx(CONN) collide → tie-break by callsign ordering: higher wins CONNECTING role, lower becomes acceptor → CONNECTED
  Timer(connect)   → retransmit CONN up to MAX_CONN_RETRIES, jittered backoff; exhausted → CLOSED(Err)
CONNECTED:
  HostSend         → enqueue into send window
  SENDING_OVER     → transmit up to WINDOW unacked/retx DATA frames this over; reverse → LISTENING; start over-timer
  Rx(DATA)         → buffer; deliver in-order; (acceptor side) accumulate ACK/SACK for our next over
  LISTENING + got peer's over end → take floor: send ACK/SACK (+ piggyback our DATA) → SENDING_OVER
  Timer(over)      → peer's reply over not heard: retransmit unacked window next over; RTT-track; after
                     MAX_LINK_RETRIES consecutive silent overs → link dead → CLOSED(Err: PeerLost)
  Idle (no data, no traffic) → KEEPALIVE each keepalive-interval; survives DEAD_OVERS_TOLERATED silent overs
  HostDisconnect / Rx(DISC) → drain or abort per policy → send DISC → DISCONNECTING
DISCONNECTING:
  Rx(DISC_ACK) or Rx(DISC) → → CLOSED(Ok)
  Timer(disc)      → retransmit DISC up to MAX_DISC_RETRIES → CLOSED(Ok, best-effort)
```

Hardening (the spec's watched SM bugs): CONN/CONN collision tie-break (deterministic by callsign);
half-open rejection via CONN_ID; idempotent retransmit of CONN/DISC; DISC-during-data handled;
no both-transmit (floor ownership is explicit, one holder). All transitions table-driven + exhaustively
unit-tested (collision, half-open, lost-handshake, lost-ACK, reordered DATA, duplicate DATA).

## 5. Selective-repeat ARQ (#6, half-duplex, window-per-over)

- **Sender:** send window of up to `W` unacked DATA frames; transmits new + retransmit-needed frames
  in its over; on the peer's ACK/SACK, slides the cumulative high-water and clears SACKed frames;
  retransmits only the **gaps** (cumulative+SACK) next over (burst-error-friendly — no go-back-N waste).
- **Receiver:** buffers out-of-order DATA up to `W`; delivers **in-order** to host (never out of order,
  never duplicated, never corrupt — CRC-gated); replies with cumulative `ACK_THROUGH` + `SACK` bitmap.
- **No NAK storms:** ACK/SACK carries the negative information implicitly; explicit NAK frame optional/
  rate-limited. u32 seq ⇒ no wrap. `W` sized by link adaptation (§6); RTT-tracked over-timer (EWMA) +
  jittered bounded backoff (half-duplex phase-lock-safe).
- **Floor degenerate strategy:** under the floor mode the same router selects `ArqStrategy::WholeMessage`
  — `W=1`, no SACK, no NAK, resend-whole-until-ACKed (the canonical floor "no NACK" model). One code
  path, two strategies.

## 6. Link adaptation + mode-derived timing (#7 plumbing)
`route(payload_len, &ChannelQualityReport) -> (ModeHint, ArqStrategy, WindowParams)`. Consumes FER +
SNR: high FER ⇒ shrink `W`, lengthen timers, and **degrade down the ladder** rather than dropping the
link.

**The ladder now extends to a deep-robustness (FT8-class) floor** (operator FYSA: `floor-nfsk`
promoted from a manual crowded-band sidecar to the bottom rung of `MainAuto`):

```
OFDM (fast → slow)  →  floor-wblo (BPSK wide low-density)  →  deep-floor nFSK (narrow, slow, ~FT8 −21 dB)
   selective-repeat, W>1            WholeMessage, W=1                 WholeMessage, W=1
```

**All link timers are MODE-DERIVED (airtime-aware), not flat** — this is mandatory because a
deep-floor over is ~tens of seconds, vs sub-second for OFDM. A `ModeProfile { over_airtime,
per_over_mtu, .. }` (supplied by the PHY/MAC; link-side defaults until the PHY exposes it) scales:
- turn-recovery timer, retransmit/over timer, keepalive interval — all = f(over_airtime).
- per-over MTU — deep-floor carries only tens of bytes/over ⇒ a long message fragments across **many
  slow overs** (minutes); u32 seq won't wrap.
- **Long ride-through:** `DEAD_OVERS_TOLERATED × over_airtime` at the bottom rung = minutes of
  keepalive persistence before `PeerLost`. The link **trickles instead of failing.**

The link does NOT hardcode mode specifics; it reads the `ModeProfile`, so it is correct across the
whole ladder the moment the deep-floor mode lands (that PHY work is owned elsewhere, gated on the
operator's measured decode-rate-vs-SNR-in-2.5-kHz gate near FT8's −21 dB). Single mode available
today ⇒ window/timer adaptation is live and unit-tested over mode profiles; the actual mode-STEP is
exercised as soon as >1 profile exists, and is stubbed-but-not-faked until then.

## 7. "More robust than VARA" — clean-sheet robustness levers (design goals the gate checks)
1. **No silent corruption/loss:** CRC-32 per frame + hard in-order/no-dup/no-corrupt delivery invariant.
2. **Burst tolerance:** SACK selective-repeat retransmits only gaps; survives multi-over deep fades.
3. **Survive worse channels before death:** `DEAD_OVERS_TOLERATED` keepalive ride-through; link declares
   death explicitly (and reports it) rather than corrupting or hanging.
4. **No phase-lock collisions:** jittered bounded backoff on a half-duplex shared channel.
5. **Graceful degradation** to slower/floor mode via ChannelQualityReport, not connection drop.
6. **Wide u32 seq** (no wrap under long sessions); negotiated `CONN_ID` rejects stale/half-open frames.
7. **Idempotent handshakes** (connect/teardown survive lost control frames).

## 8. The gate — realistic lossy channel (replaces the clean loopback)

`HalfDuplexLossyLink` test harness: a shared **frame-level** medium with two `PhyTransport`
endpoints (decoupled from the waveform, per the audit). Injects, deterministically (seeded):
- **Bursty/correlated loss** — Gilbert–Elliott good/bad states (bad = deep-fade burst dropping
  consecutive frames/overs), NOT uniform random.
- **Byte corruption** — flips in-frame; CRC must catch ⇒ frame dropped (never delivered corrupt).
- **Variable RTT** — per-over delay distribution (multi-second tail).
- **Collisions** — overlapping overs destroy both (turn discipline must avoid; harness can force).

**Acceptance gates (each is a test that must pass):**
- G1 **Reliable in-order delivery:** a multi-frame message over a channel at a defined burst-loss +
  corruption rate is delivered **byte-exact, in order, no dups** (or the link reports failure — never
  silent corruption).
- G2 **Connection establish/teardown** completes under control-frame loss (retransmitted handshakes).
- G3 **Burst recovery:** a deep-fade burst of K consecutive lost overs is recovered by selective-repeat
  once the channel reopens.
- G4 **Honest failure:** sustained loss past `DEAD_OVERS_TOLERATED` ⇒ explicit `PeerLost`, no hang, no
  partial/corrupt delivery.
- G5 **Wiring smoke (labeled, not a viability claim):** the generic link drives a real `SondePhy` +
  `FloorWaveform` over a clean medium for one message — proves the trait integration isn't an island.

Results are reported as "link-correct over channel model {params}", never as HF throughput/viability.

## 9. Crate layout
`crates/sonde-link/`: `frame.rs` (types/Callsign/CRC/exact-parse/fragmentation), `conn.rs`
(connection state machine), `arq.rs` (selective-repeat sender/receiver + floor degenerate),
`mac.rs` (`route`), `adapt.rs` (ChannelQualityReport→params), `host.rs` (HostCommand/HostEvent),
`tests/` (`HalfDuplexLossyLink` Gilbert–Elliott harness; G1–G5). Deps: `sonde-phy`, `crc`, `thiserror`;
dev-dep `sonde-phy-runtime`, `sonde-fec`.

## 10. RADIO-1 / Part 97
No real radio keyed; all tests are in-memory doubles. Station ID validated + in every frame; the MAC
is the enforcement point. On-air smoke is operator-run, post PHY-physics gates.

## 11. Scope honesty
This push lands: the design (Codex-converged SM), frame codec, connection state machine,
selective-repeat ARQ, the lossy-channel harness, and gates G1–G5 it can pass. Link adaptation mode-STEP,
HARQ, and over-the-real-PHY viability are explicitly deferred/gated and will be labeled as such in the
handoff — not claimed done.
