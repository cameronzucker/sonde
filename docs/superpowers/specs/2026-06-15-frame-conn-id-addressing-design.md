# Design — drop per-frame callsigns; conn_id addressing; ID at start/end/≤10 min

**bd:** `sonde-sbt` (P1) · **Status:** draft for Codex adversarial review · **Agent:** lupine-yew-harrier
**Amends:** `2026-06-14-link-mac-arq-mvp-design.md` §3 (frame format), §4 (SM), §10 (Part 97).
**Memory:** `sonde-part97-id-at-start-end-not-per-frame`.

## 1. Problem

Every link frame currently carries `SRC`(10) + `DST`(10) callsigns (frame.rs
offsets 5–24). That is **20 of the 42 header bytes** — on a deep-floor over (tens
of bytes of payload) the per-frame callsign is *larger than the payload*. It is
also **not** a Part-97 convention: §97.119 requires station ID at the **end** of
a communication and **at least every 10 minutes** during it (by convention also
at the **start**) — never on every frame. Per-frame callsign is wasted airtime
masquerading as a regulatory requirement. (Operator, 2026-06-14: "This isn't TLS.")

## 2. Change (BREAKING wire change — pre-release, `VER` 1 → 2)

### 2.1 New header (no callsigns)

```
off field        size  notes
0   MAGIC 53 4C   2
2   VER           1    = 2   (bumped; a v1 frame now fails BadVersion, never mis-parses)
3   TYPE          1    DATA/ACK/NAK/CONN/CONN_ACK/DISC/DISC_ACK/KEEPALIVE/ID(9)
4   FLAGS         1    END_OF_OVER (bit0), END_OF_MSG (bit1)
5   CONN_ID       2    session id — THE demux/routing key
7   SEQ           4
11  ACK_THROUGH   4
15  SACK          4
19  MODE          1    ladder-rung id (link adaptation)
20  LEN           2
22  PAYLOAD       N
22+N CRC32        4    IEEE, over [0 .. 22+N)
```

`HEADER_LEN` 42 → **22**, `LINK_OVERHEAD` 46 → **26**. Saves **20 bytes/frame** on
the data plane (the SRC+DST block). Offsets, contiguous: CONN_ID 5, SEQ 7,
ACK_THROUGH 11, SACK 15, MODE 19, LEN 20, PAYLOAD 22 (no gap). Exact-length +
CRC-first parse is unchanged. (Codex review blocker: the first draft skipped a
byte; this is the corrected, gap-free layout.)

### 2.2 Callsigns become ID-payload on ID-bearing frames only

ID-bearing frame types = **{CONN, CONN_ACK, DISC, DISC_ACK, ID}**. Their payload
region is a fixed 20-byte station-ID block: `SRC[10] NUL-pad ++ DST[10] NUL-pad`
(LEN = 20, no other payload — these are control frames). The data plane
**{DATA, ACK, NAK, KEEPALIVE}** carries no callsigns.

`LinkFrame` gains `id: Option<StationId>` (where `StationId { src, dst }`) and
keeps `payload: Vec<u8>` for host data. Invariant enforced in `encode`:
`frame_type.is_id_bearing() ⟺ id.is_some()`. `decode` branches on the (fixed,
pre-payload) TYPE byte: ID-bearing ⇒ require `LEN == 20`, split into `src`/`dst`;
else ⇒ `id = None`, payload is raw host data. The wire stays uniform (fixed
header + LEN-counted, CRC-covered region); only the *interpretation* of the
payload region is type-dependent.

### 2.3 Addressing by conn_id (conn.rs)

`handle_frame`'s opening callsign guard
(`frame.src != remote || frame.dst != local`) is removed. New routing:

- **Data plane** (DATA/ACK/NAK/KEEPALIVE): demuxed solely by `session_ok` =
  `state == Connected && conn_id == self.conn_id`. No callsign to check.
- **CONN** (bootstrap): an acceptor learns the peer. `remote: Option<Callsign>`
  — `None` for a fresh acceptor, set to `frame.id.src` on CONN receipt **iff**
  `frame.id.dst == self.local` (the CONN is addressed to me; else ignore).
  **Learning happens only in `Closed`** (a fresh acceptor). In `Connecting` the
  station is an initiator that already knows `remote` (configured); it does **not**
  learn — it validates `frame.id.src == remote` and only then runs the CONN/CONN
  collision tie-break (`local` vs `remote`), so learning can never corrupt the
  tie-break (Codex review).
- **CONN_ACK / DISC / DISC_ACK / ID**: primary check `conn_id == self.conn_id`;
  defense-in-depth `id.dst == self.local` (and `id.src == remote` once known).

**Constructor change:** `Connection::acceptor(local, profile, window)` drops its
`remote` arg (now learned). `initiator(local, remote, conn_id, profile, window)`
unchanged. `remote` is modeled so the initiator's configured peer and the
acceptor's learned peer are distinguishable: **`reset_session` clears a *learned*
`remote` back to `None`** (so a generic listener can accept a *different* peer on
the next session) but **preserves a *configured* `remote`** (Codex review — else
the first caller binds a listener forever).

### 2.4 Periodic ID (≤10 min) — REAL-time, owned by the Driver

A new `FrameType::Id (9)` carries the station-ID block and nothing else.

**Why not the sans-IO Connection (Codex review blocker).** §97.119's 10-minute
window is **real** wall-clock. But the `Driver` deliberately runs the Connection
on a *logical* clock that **freezes for the duration of keying** (turn-recovery
means "after I stop keying"). A logical-time ID timer would therefore drift past
10 *real* minutes by exactly the accumulated keyed airtime — non-compliant under
any TX duty cycle. So the periodic cadence lives where real time lives: the
real-time adapter (`Driver`), not the Connection.

**Split of responsibility:**
- **Connection** owns the *primitive* and the *peer side*: `make_id_frame()`
  returns a fully-stamped `ID` frame (current `conn_id`, `mode`, and the
  `StationId{ local, remote }` — `remote` is always known mid-session); `on_id`
  treats a received ID like a keepalive (proves liveness: `silent_overs = 0`,
  `follow_mode`; passes the floor only if it carries `END_OF_OVER`; no host
  event). Start/end ID (CONN/CONN_ACK/DISC/DISC_ACK callsigns) is wholly inside
  the Connection — no clock subtlety.
- **Driver** owns the *cadence*, in real time: `ID_INTERVAL = 9 min` (< 10 for
  margin) tracked against `clock.now()`. When it is about to flush an over
  (`tx_in_flight == 0`, ≥1 frame to send) **and** `now − last_id_real ≥
  ID_INTERVAL` (or never sent), it transmits `conn.make_id_frame()` **first** in
  that over, then the normal frames. It resets `last_id_real` whenever it
  transmits *any* ID-bearing frame (so CONN/CONN_ACK/DISC count as the start ID
  and the periodic timer does not double-ID right after connect). ID attaches to
  *keying*: a station that is not flushing an over is not transmitting and need
  not ID.

The deterministic gates drive the Connection through `Link<P>` on a logical
clock; periodic real-time ID is a real-radio concern, so it is covered by a
**Driver** test (the `ManualClock` is real time) rather than the logical-time
gates. RADIO-1: still nothing keys a radio.

## 3. Accepted trade-offs / risks (Codex-reviewed)

- **R1 — conn_id is now the sole data-plane demux (Codex: correctness, not just
  auth).** Two unrelated sessions on a shared channel that pick the same `u16`
  conn_id could cross-talk; the old per-frame callsign filtered that. Accepted for
  this PR: conn_id is initiator-random over 65 536 values, bound to the peer pair
  by the CONN handshake (VARA/ARDOP-class session-id model); per-frame sender auth
  was never a goal ("not TLS"); the channel-model gates are 2-station, no
  collision. Codex recommends widening conn_id (u16 → u32) for collision
  robustness — deferred to a **follow-up bd issue** (a second breaking wire bump;
  keep this PR's blast radius to the callsign removal). Must not be silently
  dropped.
- **R2 — `frame_from_a_third_party_is_ignored` premise dies.** That test injected
  a DATA frame with the right conn_id but a foreign SRC; DATA no longer carries
  SRC. Repurpose to wrong-conn_id rejection (already covered by
  `frame_with_wrong_conn_id_is_rejected_half_open`); drop the callsign variant.
- **R3 — DISC_ACK is ID-bearing** (symmetry: it is that station's last
  transmission, so it IDs at end). Costs 20 bytes on a rare frame. Kept.
- **R4 — type-dependent payload *interpretation*.** decode trusts the TYPE byte
  (CRC-verified before use) to decide whether the 20-byte payload region is the
  station-ID block. A flipped TYPE is caught by CRC first; a valid-CRC frame with
  a surprising type is rejected by the SM. No CRC-offset walk (LEN still bounds
  it). Hardened: ID-bearing decode requires `LEN == 20` and canonical NUL-padded
  callsigns; encode enforces `is_id_bearing() ⟺ id.is_some()` and forbids host
  payload on ID-bearing frames.
- **R5 — PeerLost ends a communication without a final ID (Codex).** Accepted:
  on a dead link you cannot transmit, so a "final ID" would go into a dead
  channel — and ceasing transmission *is* the end of communicating. A clean
  host-initiated teardown sends DISC (which IDs). Documented, not coded.
- **R6 — half-open reconnect with a new conn_id (Codex).** Pre-existing: a fresh
  CONN bearing a *different* conn_id while `Connected` is dropped (a peer reboot /
  lost-DISC then re-CONN is ignored until death-detection closes the old session).
  This PR must **not regress** it; the proper fix (accept a fresh CONN from the
  known remote, resetting the stale session) is a **follow-up bd issue**.

## 4. Blast radius (all in `crates/sonde-link/`)

`frame.rs` (struct gains `id: Option<StationId>`, encode/decode, offsets, unit
tests), `conn.rs` (constructors, `make_*`, `handle_frame`, `on_conn` learns remote
in `Closed` only, `make_id_frame()` + `on_id`, learned-vs-configured `remote`
reset, tests), `driver.rs` (real-time periodic-ID cadence + `next_wakeup`;
acceptor signature in tests), `tests/link_gates.rs` + `tests/g5_wiring_smoke.rs`
(acceptor signature; frame construction). `link.rs` (logical-time gate adapter —
no periodic ID), `host.rs`, `lib.rs` (export `Id`/`StationId`): no logic change.
No other crate references `LinkFrame`/`Callsign`/`Connection` (demo-builder, wasm,
tx, rx do not).

## 5. Gates (link-correct over the channel model — never HF-viable; RADIO-1)

Existing G1–G9 must stay green. New unit tests: `LINK_OVERHEAD == 26`;
data/ack/keepalive round-trip with no callsigns; CONN/CONN_ACK/DISC/DISC_ACK/ID
round-trip the station-ID block; ID-bearing decode rejects `LEN != 20`; acceptor
learns remote from CONN and clears it on close; CONN to a different DST is
ignored; CONN/CONN collision tie-break still resolves (validated against the
configured remote). Driver test (real `ManualClock`): periodic ID fires after a
real `ID_INTERVAL` of keying and not before, and is reset by start-ID
(CONN/CONN_ACK) so it does not double-ID right after connect. Nothing keys a
radio.
