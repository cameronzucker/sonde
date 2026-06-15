# Link-layer gap inventory — close ALL gaps to true end-to-end (sonde-98e)

**Date:** 2026-06-15 · **Agent:** sparrow-hawk-swallow (the link-layer agent) · **Epic:** sonde-98e
**Source:** Codex read-only adversarial audit of `crates/sonde-link` (2026-06-15), the
counterpart to sonde-b60 (PHY). This is the authoritative gap list; the epic is
done when a fresh Codex audit of sonde-link comes back empty. Build order, design
convergence with Codex, and gates per the operator directive ("the whole feature").

## Blockers

- **B1 — ladder is a stale static mirror.** `mac.rs::ladder()` hardcodes OFDM/nFSK
  rungs `available:false` while `standard_waveforms()` registers all five modes.
  Production adaptation is pinned to rung 3. Fix: a link-visible mode-registry/
  profile API (the dynamic link↔PHY handshake) — pass a shared registry into
  `Driver`/`Connection`; build availability + knees + profiles + mode hints from it.
- **B2 — OFDM rungs use `MainAuto`, not `MainPinned`.** Even when available, the
  link can't request the exact rung matching its wire MODE byte; the PHY may resolve
  `MainAuto` differently. Fix: rungs 0/1/2 → `MainPinned("ofdm-wide"|"-mid"|"-narrow")`;
  map decoded `RxFrame.mode()` back to a link rung.
- **B3 — `apply_rung` resizes ARQ but never swaps `ModeProfile`.** Real per-mode
  airtime/MTU/retry/death timers are wrong (profiles are illustrative). Fix: registry
  descriptors carry measured `ModeProfile`; `apply_rung` updates profile + timers.
- **B4 — fragmentation at enqueue uses the current MTU** (`conn.rs:276`). A later
  downshift to a smaller mode leaves queued fragments oversized → silent drop/fail.
  Fix: keep host messages whole in a queue, fragment at transmit time for the active
  rung (or re-fragment outstanding data on downshift).
- **B5 — `conn_id` is u16, caller-supplied, accepts 0** (sonde-44n + more). On a
  shared channel an ID collision cross-delivers DATA/ACK (data plane has no
  callsigns). Fix: widen to u32, reserve 0, generate random nonzero IDs in the link,
  reject impossible/stale ACK/data for the active session.
- **B6 — fresh CONN from the known peer with a different `conn_id` is dropped while
  Connected** (`conn.rs:712`; sonde-ajn). A peer reboot / lost DISC / half-open
  wedges one side forever (no idle keepalive). Fix: accept a fresh addressed CONN
  from the known peer as a session reset/reconnect; flush old state; reply CONN_ACK.
- **B7 — no idle liveness.** `keepalive_interval()` is unused; idle Connected links
  schedule no liveness timer, so a stale peer stays Connected indefinitely. Fix:
  schedule idle keepalives / liveness probes from the mode profile, with death.
- **B8 — no real multi-mode two-party E2E test.** `g5_wiring_smoke` uses only
  `FloorWaveform` and excludes a handshake. Fix: a hardware-free shared-radio
  integration test with two `Driver<SondePhy>` over `standard_waveforms()` through
  connect/send/disconnect (and a mid-session mode shift).

## Important

- I1 — quality fed from any decodable frame (wrong conn_id / third-party) before
  session validation (`link.rs:79`, `driver.rs:187`) → shared-channel traffic
  corrupts adaptation. Validate session/addressing before applying quality.
- I2 — link CRC/decode failures dropped, not counted toward link-level FER
  (`link.rs:74`, `driver.rs:178`). Count decode failures (with SNR) into adaptation.
- I3 — encode/send errors dropped (`link.rs:95`; `driver.rs` drops encode errors).
  Unified host error event/status path.
- I4 — runtime drops an over after `send_frame` returned Ok when no waveform matches
  the hint (`runtime.rs:297`). The handshake must prevent impossible hints; longer
  term, TX outcome reporting keyed by `TxToken`. (Cross-layer note.)
- I5 — reserved FLAGS bits 5–7 accepted on decode; `rx_rung`/`mode` unvalidated at
  the frame boundary (`frame.rs:394`). Reject nonzero reserved; clamp/validate.
- I6 — control frames (ACK/NAK/KEEPALIVE) accept arbitrary payload on the wire
  (`frame.rs:345`). Require zero payload; ID-bearing = station-ID only.
- I7 — `FrameType::Nak` on the wire is never handled (`conn.rs:309`). Implement
  rate-limited NAK or reserve/remove before the wire hardens.
- I8 — `SendWindow::on_ack` trusts future/stale ACKs (`arq.rs:101`) → can clear
  unsent/needed frames under collision/replay. Bound ACK/SACK to sent ranges.
- I9 — SACK fixed 32 bits but public window width uncapped (`arq.rs:194`). Cap the
  selectable window to the SACK range (or variable SACK).
- I10 — send + reassembly buffers unbounded (`arq.rs:52`,`:208`). Max outbound
  queue, max message size, max partial-reassembly age/size, backpressure/errors.
- I11 — seq u32 has no wrap policy (`arq.rs:54`). Close/renegotiate before wrap.
- I12 — disconnect aborts queued/unacked data with no policy surface (`conn.rs:289`);
  `send()` accepted in any state, returns nothing (`conn.rs:276`). Make host commands
  return Result/events; define drain-vs-abort disconnect. Add `HostStatus`
  (state/peer/conn_id/rung/outstanding/last-quality/errors) — `host.rs` is bare.

## Polish

- P1 — `rung(id)` (pub) returns routes for unavailable rungs (`mac.rs:231`): make
  private / `Option` / clamp.
- P2 — `over_timeout()` unused (`profile.rs:61`): wire into ACK-wait or remove.
- P3 — callsign validation is syntactic only (`frame.rs:133`): document operator
  responsibility or stricter profile validation.
- P4 — mode-degradation gates degenerate (`link_gates.rs:216,792`): fix once the
  registry-backed ladder lands.

## Keystone & build order

B1+B2+B3 are one coherent change — **the dynamic registry handshake**: the PHY
publishes its registered modes (short_name, family, knee, ModeProfile) through the
`PhyTransport` seam; `Driver` reads it at construction and builds the ladder
(availability + MainPinned hints + per-rung profiles) from it; `apply_rung` swaps
the profile. That unblocks P4 and the real ladder. Then B5 (conn_id u32+random),
B6 (reconnect), B7 (keepalive), B4 (fragment-at-transmit), B8 (two-party E2E), then
the Important/Polish set. Physics/protocol decisions (knee sourcing, profile values,
handshake seam shape) converge with Codex before building, per the operator.
