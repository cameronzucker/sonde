# Design: Modem Link/MAC (#5) + ARQ (#6) + host surface (#8) — MVP

> Status: design v2 (post-Codex-round-1, half-duplex realignment). Implements the canonical
> subsystem specs (`tuxlink docs/superpowers/specs/2026-05-31-clean-sheet-modem-{5-link-mac,6-arq,8-host-protocol,overview}.md`).
> bd epic `sonde-lcw`; MVP = `sonde-o0s` (#5) + `sonde-pwh` (#6-MVP) + minimal `sonde-a0p` (#8).
> Moniker: marten-grouse-cove.

## 0. The operating model — half-duplex, push-to-talk, turn-taking (read first)

This is an **amateur HF data link in the VARA/ARDOP vein — NOT a packet network.** The
physical reality the whole design serves:

- **One rig, one channel, half-duplex at best (often effectively simplex).** A station is
  either transmitting **or** receiving — never both. While keyed (PTT down) the station is
  **deaf**.
- **Strictly one station transmits at a time.** Two overlapping transmissions **collide**
  and are mutually destroyed — there is no "both buffered through."
- **Turnaround is explicit and expensive.** Sender drops PTT → peer keys up → peer transmits.
  This dominates HF latency; the protocol is built around taking turns, not pipelining.
- **No free reverse signal.** A sender cannot hear an ACK while transmitting. Confirmation
  only arrives in a *later, separate over* after the sender has turned around to listen.

Everything below is turn-taking by construction. The full selective-repeat ARQ (#6-full,
OFDM) still obeys this — it fills one *over* with a window of frames, then listens for one
ACK over; it does not pipeline against a full-duplex pipe. (Per ADR 0014 this draws on the
*conceptual* half-duplex-ARQ primitive as background only; no examination of VARA/ARDOP
internals.)

## 1. Architecture posture (non-negotiable)

The **modem owns** framing, station ID, sequence numbers, connection state, ACK/retransmit,
and turnaround — a new crate **`crates/sonde-link`**. Tuxlink is the client; no link code in
tuxlink. The host drives the modem over a thin host surface (#8); the modem runs the link
internally. The link is generic over `P: PhyTransport` (`sonde_phy::phy_api`), so it runs
over `SondePhy` (production) and `NullPhy` (loopback) unchanged.

## 2. Seam facts (verified on current `origin/main`)

- `send_frame(&[u8], ModeHint)->TxToken` queues opaque bytes (= the serialized link frame);
  **returns on enqueue, not TX-completion** (runtime.rs:113) — timeouts must budget for
  enqueue + encode + airtime + turnaround.
- `poll_rx()->Option<RxFrame>`; `RxFrame::payload()` = the bytes the peer sent.
- **A failed/absent decode = `poll_rx` returns nothing** (`do_rx` hardcodes `decode_ok=true`,
  runtime.rs:208; `decode_scan`→`None` on no/failed frame). So remote loss is detectable
  **only by absence of a confirming over within a deadline** — exactly the FT8 model.
- `LoopbackRadio` is **self-echo**, not an A↔B channel → we build a shared half-duplex medium (§7).
- **RX-window limit (filed, out of scope):** `SondePhy` decodes one ≤12k-sample capture per
  window with no cross-window reassembly; a full floor frame is larger. Tracked as a separate
  PHY-runtime bug. The MVP's test medium delivers one complete *over* as a single captured
  burst (models "the receiver's capture spans the over") — this still drives the **real**
  `FloorWaveform` encode/decode, so the E2E is not an island; only multi-window reassembly is
  deferred.

## 3. #5 Link/MAC frame (resolves spec open questions)

Variable-length, big-endian, one frame = one PHY `send_frame` = one over.

```
off  field         size  notes
0    MAGIC 0x53 4C  2     "SL" link sanity
2    VER           1     = 1; link-version hook
3    FRAME_TYPE    1     0x01 DATA · 0x02 ACK   (reserve 0x10/0x11/0x12 CONN/CONN_ACK/DISC, #6-full)
4    SRC callsign  10    ASCII, NUL-pad. Part-97 station ID in EVERY frame (validated, §3.2)
14   DST callsign  10    ASCII, NUL-pad
24   SEQ           4     u32. DATA: message seq. ACK: the acked seq (PAYLOAD_LEN=0)
28   PAYLOAD_LEN   2     u16
30   PAYLOAD       N
30+N CRC32         4     IEEE, over bytes [0 .. 30+N)
```

Header = 34 bytes. **Parsing is exact-length** (Codex #8): treat `RxFrame.payload()` as one
complete datagram; require `buf.len() == 34 + PAYLOAD_LEN` and a passing CRC **before** any
field is trusted; otherwise drop. A corrupted `PAYLOAD_LEN` therefore can't walk the CRC
offset — length mismatch → drop.

- **Station ID (§5.Q2): explicit per-frame, validated** (Codex #11). `Callsign` is a parsed,
  validated type (nonempty, ASCII, `[A-Z0-9/]`, ≤10), not opaque bytes. Local SRC must be a
  legal callsign in amateur mode — the MAC is the Part-97 enforcement point. (Opaque/arbitrary
  addressing, if ever needed, becomes a *separate* future address field, not this one.)
- **SEQ u32** (§5.Q8): wide up-front; the v2.0→v2.2 retrofit is the warning. Pre-sizes #6-full's window space.
- **CRC32** (§5.Q5): stronger than CRC-16-CCITT; `crc` crate already a workspace dep.
- **Link MTU** (Codex #7): the floor caps PHY input at `u16::MAX`, and PHY input is the *whole*
  frame, so `FLOOR_LINK_MTU = u16::MAX - 34`. Host messages above MTU are rejected before
  serialization (fragmentation is a later concern).

## 4. #6-MVP — floor reliability: whole-message confirmation, turn-taking

**Mode-conditional ARQ** (overview §5.A.2): the floor does **not** run the selective-repeat
ARQ state machine. It uses the minimal turn-taking confirmation:

- Sender transmits the whole DATA frame in one over, **drops PTT, turns around, listens**.
- Receiver, on a clean decode addressed to it, **delivers once** and, in its **own next over**,
  transmits a single **ACK** (positive confirmation; SEQ=acked seq, no payload). It **never
  NACKs**.
- Sender, if no matching ACK arrives within the cycle deadline, **resends the whole message**;
  repeat up to `MAX_RETRIES`, then `Err(Unacked)`.

**Codex #10 disposition (intentional, documented):** the canonical spec says the floor has
"no ARQ *state machine*" and "the receiver doesn't NACK." We honor both literally — there is
**no window, no selective NACK, no per-frame retransmission**, only a single positive
whole-message ACK and resend-until-confirmed. This is the JS8-style end-of-message
confirmation ("RR73"-shaped), not selective-repeat ARQ (which stays reserved for OFDM
#6-full). A sender that wants pure fire-and-forget (fixed N repeats, zero reverse traffic) is
a future flavor; the MVP delivers the *confirmed* whole-message exchange the deliverable asks
for, because "retry on a forced decode failure" requires the sender to learn of non-delivery,
and on a half-duplex link the only such signal is absence-of-confirmation.

**Turnaround / convergence (Codex #3, #4):**
- One station transmits at a time; the shared medium (§7) enforces mutual exclusion and
  collides overlaps. `SondePhy`'s worker is TX-priority and only receives when idle, so a
  station is naturally deaf during its own over.
- The cycle deadline is **enqueue-relative and conservative**: budgets enqueue + encode +
  DATA airtime + turnaround + peer decode + ACK airtime. MVP uses a generous fixed base.
- Retry backoff is **randomized** (base + per-retry jitter) to break the lost/late-ACK
  livelock where a resend keeps colliding in-phase with the peer's delayed ACK.
- A real TX-complete/airtime signal (via `TxToken`) is a follow-up; noted, not blocking.

**Bounded dedup (Codex #9):** stop-and-wait whole-message ⇒ track **last-accepted SEQ per
source** + a small replay window, not an unbounded `(src,seq)` set. A duplicate (lost-ACK
resend) is re-ACKed but **not re-delivered** (idempotent).

## 5. #6-full selective-repeat ARQ (design-only, gated; `sonde-5yg`)

OFDM-family, not yet built. Connection-oriented (CONN/CONN_ACK/DISC). Each *over* carries a
**window** of frames; the peer replies in its over with one cumulative+selective ACK
(SACK-style bitmap); sender retransmits only gaps next over. Window sized to fill one over at
the link-adaptation-chosen rate (u32 seq pre-sizes it). RTT-tracked (EWMA) cycle timer;
NACK-free (selective ACK, not NACK storms, §8). Implemented later as a unit-tested state
machine; on-air waits for the OFDM waveform. Still strictly turn-taking — a window per over,
never full-duplex pipelining.

## 6. #8 host surface — minimal in-process slice (`sonde-a0p`)

`enum HostCommand { Send{dst, bytes}, Poll, ... }` → `enum HostEvent { Delivered{src,bytes},
SendResult{seq, ok}, ... }`, mapping 1:1 onto `Station`. This is the vocabulary a later
two-port TCP daemon serializes; MVP keeps it in-process. CONN/DISC commands land with #6-full.

## 7. Test strategy — wired through the REAL PhyTransport, faithfully half-duplex

**`SharedChannel` test medium (replaces the v1 two-queue design — Codex #2):** ONE shared
medium, not two independent queues. Properties it enforces:
- **Mutual exclusion + collision:** if a station calls `transmit` while the channel is busy
  with another station's over, **both overs are destroyed** (the medium records a collision;
  neither is delivered). Models "you cannot both talk at once."
- **Deaf while transmitting:** a station never captures an over that was on the air during its
  own transmit.
- **One over delivered as one captured burst** to the listening peer (the documented
  RX-window simplification, §2).
Two stations, **each with its own `SondePhy`** over its own `SharedChannel` endpoint.

**Forced-loss double `DropNthOver`:** wraps an endpoint and silently drops the Nth over a
station transmits (models a missed decode), to exercise whole-message retry.

**Tests (TDD):**
1. Frame codec units: serialize↔deserialize round-trip; CRC rejects corruption; exact-length
   parse rejects truncation/over-length; `Callsign` validation; ACK encoding (SEQ=acked, LEN=0).
2. Station logic over `NullPhy`: duplicate DATA re-ACKs without re-delivering; ACK releases the
   sender; wrong-DST dropped; bounded dedup.
3. **E2E deliverable over `SondePhy<FloorWaveform, SharedChannel>`:** B runs a **concurrent
   pump thread** (Codex #5) draining its link + emitting ACKs; A calls `send_message("B", payload)`
   (blocks, internally polling for the ACK) with **A's first over dropped** (`DropNthOver(1)`).
   Assert: B delivers `payload` **exactly once**, and A returns `Ok` after exactly one retry.
   This exercises the real `SondePhy` worker + real `FloorWaveform` LDPC encode/decode over a
   faithfully half-duplex medium (anti-island).

`SharedChannel`/`DropNthOver` live in `sonde-link`'s test support, implementing the real
`Radio` trait — `SondePhy` is unmodified.

## 8. Crate layout

`crates/sonde-link/`: `frame.rs` (LinkFrame/FrameType/Callsign/CRC/exact-parse), `station.rs`
(Station<P>: send_message, poll/pump, whole-message confirm, bounded dedup), `mac.rs`
(`route(len, quality)->(ModeHint, ArqStrategy)` — MVP hardcodes Floor+WholeMessage),
`host.rs` (HostCommand/HostEvent), `arq/selective_repeat.rs` (gated #6-full), `tests/`
(`SharedChannel`, `DropNthOver`, e2e). Deps: `sonde-phy`, `crc`; dev-dep `sonde-phy-runtime`.

## 9. RADIO-1 / Part 97
No real radio keyed; all tests use in-memory doubles implementing `Radio`. Station ID is in
every frame by construction and validated; the MAC is the enforcement point, satisfied before
any on-air path exists. On-air smoke is operator-run.

## 10. Codex round-1 disposition
P0 #1 RX-window → filed as separate PHY-runtime bug; test medium delivers one-over-burst,
documented (§2). P0 #2 half-duplex medium → `SharedChannel` with mutual-exclusion/collision
(§7). P1 #3 livelock → randomized backoff + collision model (§4). P1 #4 timeout sizing →
enqueue-relative conservative budget (§4). P1 #5 concurrent pump → B pump thread (§7). P2 #6
ACK-timeout-only → accepted (§2). P1 #7 MTU → `u16::MAX-34` enforced (§3). P2 #8 exact-length
parse + ACK semantics → §3. P1 #9 bounded dedup → last-seq+replay window (§4). P1 #10 floor
ACK vs "no ARQ" → documented intentional minimal confirmation (§4). P2 #11 callsign validation
→ validated `Callsign` (§3).

## 11. Codex round-2 disposition (no P0s — half-duplex rework confirmed sound)
- P1: `SharedChannel::receive` is **non-blocking/bounded** — returns silence immediately when
  idle, so a worker never blocks in receive while a TX is queued. Unit-tested.
- P1: the E2E is labeled a **"whole-over capture shim"**; a separate `#[ignore]`d bounded-window
  test documents the deferred PHY-runtime reassembly bug (the filed sibling issue).
- P1: the cycle deadline is **airtime-aware** — `turnaround_base + 2 * frame_len /
  FLOOR_EST_BYTES_PER_SEC` — scaling with message size, not a flat timeout. Retry backoff is
  **deterministic exponential** (base × retry) to break lost/late-ACK phase-lock without RNG
  nondeterminism. Real TX-complete/airtime signaling via `TxToken` remains a follow-up.
- P2: direct medium tests — overlap destroys both overs, no TX self-capture, lost/collided ACK
  → duplicate DATA re-ACK without re-deliver.
- P2: naming — the floor path is `WholeMessageConfirm` "floor confirmation/retry," never "ARQ
  state machine"; selective-repeat ARQ naming is reserved for the OFDM #6-full path.
