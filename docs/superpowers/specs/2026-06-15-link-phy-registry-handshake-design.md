# Dynamic link↔PHY registry handshake — seam design (sonde-3tm, keystone B1+B2+B3)

**Date:** 2026-06-15 · **Agent:** salamander-pika-pine (link-layer agent) · **Epic:** sonde-98e
**Status:** DRAFT for Codex adversarial convergence (before building, per operator directive).
**Source of gaps:** `docs/superpowers/specs/2026-06-15-link-layer-gap-inventory.md` (B1+B2+B3).

## Problem

`crates/sonde-link/src/mac.rs` hardcodes a static `[Rung; 5]` ladder — a hand-edited
*mirror* of what the PHY registers via `sonde-phy-runtime::standard_waveforms()`.
Three consequences (the gap inventory's B1/B2/B3):

- **B1** — the mirror is stale: OFDM/nFSK rungs are pinned `available:false` with
  placeholder knees, while the PHY registers all five waveforms. Production
  adaptation is frozen at the floor (rung 3). Adding a PHY waveform requires a
  hand-edit of `mac.rs`.
- **B2** — OFDM rungs hint `MainAuto`, so the link cannot request the *exact* rung
  matching its wire MODE byte; the PHY may resolve `MainAuto` to a different OFDM
  mode than the link believes it is on.
- **B3** — `apply_rung` resizes ARQ but never swaps the `ModeProfile`. Per-mode
  airtime/MTU/retry/death timers are wrong because every rung shares one profile
  injected at `Connection` construction.

These are one coherent change: **the PHY publishes its registered modes through the
`PhyTransport` seam; the link builds its ladder from that publication.**

## What is PHY-truth vs link-policy (the load-bearing split)

| Datum | Owner | Why |
|---|---|---|
| mode `short_name` (`"ofdm-wide"`) | PHY | mode identity |
| `family` (OfdmMain / RobustnessFloor) | PHY | architectural waveform family |
| **FER knee** (estimator-domain SNR_2500 dB) | **PHY** | a *measured physics* property of the waveform (floor knee 16 dB measured in `floor_threshold_sweep.rs`; OFDM knees in sonde-8xl). The link must NOT hardcode it. |
| **over-airtime** (wall-clock of one over) | **PHY** | physics: symbol rate × frame symbols |
| **per-over MTU** (payload bytes/over) | **PHY** | physics: frame capacity |
| canonical **wire-id** (MODE byte) | **shared const (PHY crate)** | a cross-build wire contract; both ends must agree id↔identity |
| ARQ strategy (SelectiveRepeat/WholeMessage) | **link** | adaptation policy keyed off family |
| window size | **link** | adaptation policy |
| "upshift ≤ one family-step", dead-band, FER gating | **link** | adaptation policy (symmetric-SNR design) |
| availability (is this rung selectable) | **derived** | a rung is available *iff* the PHY published it. No separate flag. |

The keystone moves the **bolded** rows out of `mac.rs` and across the seam.

## The seam — enrich `ModeDescriptor` + add `PhyTransport::modes()`

### Type (in `sonde-phy`, shared by both crates)

`sonde_phy::modes::ModeDescriptor` today carries only `{short_name, family}`. Enrich it
with the physics the link needs (new fields; existing accessors unchanged):

```rust
pub struct ModeDescriptor {
    short_name: &'static str,
    family: ModeFamily,
    wire_id: u8,            // NEW — canonical stable MODE-byte id (see "wire-id" below)
    knee_snr_db: f32,       // NEW — estimator-domain (SNR_2500) FER knee, measured
    over_airtime: Duration, // NEW — one over's wall-clock at this mode
    per_over_mtu: usize,    // NEW — payload bytes carriable in one over
}
```

with `wire_id()`, `knee_snr_db()`, `over_airtime()`, `per_over_mtu()` accessors.

### Publication method (on `PhyTransport`)

```rust
/// The modes this transport can key/decode RIGHT NOW, fastest-first. The link
/// builds its adaptation ladder from this: a rung exists and is selectable iff
/// its mode appears here. Empty/absent ⇒ the link falls back to the floor-only
/// default (so legacy/loopback transports keep working).
fn modes(&self) -> Vec<ModeDescriptor> { /* default: floor-only */ }
```

A default keeps `NullPhy`, `WireEnd`, and other test doubles compiling unchanged.
`SondePhy` overrides it, assembling descriptors from its registered waveforms.

### Link consumption — `mac.rs` becomes a `Ladder` value

Replace the free-function-over-static-`ladder()` design with a `Ladder` struct built
once from `Vec<ModeDescriptor>` and stored in `Connection`:

- descriptor → `Rung`: `wire_id`, `ModeHint` derived from identity
  (`ofdm-*` → `MainPinned(name)` [**fixes B2**]; `floor-wblo` → `Floor`;
  `floor-nfsk` → `FloorCrowdedBand`), `ArqStrategy`/`window` derived from family
  (link policy), knee from descriptor, `ModeProfile::new(over_airtime, per_over_mtu)`.
- `available` = present in the published set (no hand-edited flag — **fixes B1**).
- the existing `mac::` free fns become methods on `Ladder` (`rung`, `adapt_rung`,
  `clamp_available`, `family_of`, `base_rung`, `default_rung`, `recommended_rung`).
  Call sites in `conn.rs` change `mac::foo(x)` → `self.ladder.foo(x)` (mechanical).
- `apply_rung` additionally swaps `self.profile = self.ladder.rung(id).profile`
  [**fixes B3**], so per-mode timers track the active rung.

`Ladder` is threaded `Driver::new(phy, …)` → reads `phy.modes()` → builds `Ladder` →
into `Connection`. (Both endpoints same-build today ⇒ identical ladders.)

## Decisions to converge with Codex (the contested points)

1. **wire-id assignment.** Proposal: a canonical const table in `sonde-phy` keyed by
   `short_name` (`ofdm-wide=0, ofdm-mid=1, ofdm-narrow=2, floor-wblo=3, floor-nfsk=4`
   — matches today's static rung ids, so no wire change). The descriptor's `wire_id`
   comes from this table. A build missing a mode simply lacks that rung; present
   modes keep stable ids. `clamp_available`/`apply_rung` already round a
   not-present id to the nearest available rung (absorbs version skew).
   *Alternative rejected:* registry-index ids (fragile under version skew →
   cross-decode chaos). **Q: is a const id table the right home, or should the
   descriptor own the id with a uniqueness gate?**

2. **family granularity mismatch.** Link uses 3 adaptation tiers (OFDM=0,
   floor-wblo=1, deep-floor-nfsk=2) for the "upshift ≤ one family-step" rule; PHY
   `ModeFamily` has 2 (OfdmMain, RobustnessFloor). Proposal: the link derives its
   adaptation tier from `(family, short_name)` — `RobustnessFloor`+`floor-nfsk` is
   its own deepest tier — keeping the 3-tier behavior without changing PHY's
   architectural 2-family split (family-step is link policy, not PHY truth).
   *Alternative:* collapse to PHY's 2 families (wblo↔nfsk become free within-floor
   upshifts, still SNR+FER-gated). **Q: keep 3 link tiers, or adopt PHY's 2?**

3. **Lane boundary.** This keystone necessarily touches `sonde-phy` (descriptor +
   trait method) and `sonde-phy-runtime` (`SondePhy::modes()` + a source for each
   waveform's knee/airtime/MTU). The knees/airtime are *already-measured* physics;
   publishing them is seam work, not waveform/routing/gate work. But it borders
   sonde-b60's PHY lane. **Q: do the per-waveform physics constants live as new
   `Waveform` accessors (trait change, sonde-b60 territory) or as a descriptor
   table colocated with `standard_waveforms()` in `sonde-phy-runtime`?** The latter
   avoids a `Waveform` trait change and keeps the registry's metadata in one place.

4. **Default `modes()` semantics.** Floor-only default vs empty. Proposal:
   floor-only (the link always has at least the universal failure-convergence rung),
   so a transport that doesn't publish still yields a working stop-and-wait link.
   **Q: is a floor-only default honest, or should a non-publishing transport be a
   hard error at `Driver` construction?**

5. **Mid-session re-fragmentation (B4 interaction).** With per-mode MTU now swapping
   on `apply_rung`, queued-but-unsent host data fragmented at the old MTU can be
   oversized for a smaller mode. B4 (sonde-p4g, fragment-at-transmit) is the proper
   fix and is sequenced next; this keystone should at minimum NOT make B4 harder.
   **Q: should the keystone keep host messages whole now (deferring fragmentation to
   transmit), or is that strictly B4's scope?**

## Gates (physics-honest, per project ethos)

- **Unit:** `Ladder` built from a synthetic 5-mode descriptor set selects/clamps/
  adapts identically to today's `all_available_ladder()` algorithm tests (port them).
- **Unit:** a floor-only descriptor set reproduces today's "only rung 3 selectable"
  invariants (the real-ladder tests in `mac.rs`).
- **Integration:** `apply_rung` swaps the `ModeProfile` — assert the over-airtime-
  derived timers change across a rung step (currently they cannot).
- **Integration (B8 later):** two `Driver<SondePhy>` over `standard_waveforms()`
  through connect/send/mode-shift/disconnect — proves the published ladder drives a
  real handshake. (Sequenced as sonde-mua after the keystone.)

No RADIO-1 surface: link/sim only, nothing keyed.

---

## v2 — Codex adversarial convergence outcome (2026-06-15, salamander-pika-pine)

Codex review (read-only, grounded in the real code) returned: *goal right, seam needs
rework before building*. Resolved decisions, each verified against the code:

1. **Split identity from capability.** `ModeDescriptor` stays the catalog *identity*
   (`short_name`, `family`) + a canonical **`wire_id`** (the cross-build MODE-byte
   contract). The runtime *physics* (knee, airtime, MTU) move to a **separate
   capability type** (`ModeCapability` / `RegisteredMode`) — a catalog mode is not the
   same as a runtime-registered, physics-measured mode.
   *Why:* `ModeTable::default()` ([modes.rs:65]) lists conceptual modes regardless of
   what the runtime registered; `standard_waveforms()` ([waveform.rs:21]) is the real
   registry. Conflating them re-introduces the staleness B1 is fixing.

2. **Publish per-FRAME airtime, not per-over.** The wire model is *one link frame = one
   PHY `send_frame`* ([frame.rs:3]), but ARQ sends up to `W` frames per over
   ([arq.rs:86]). So "over airtime" is link-computed = `frame_airtime × frames_per_over`.
   The PHY publishes **per-frame airtime + per-frame payload capacity** (both honest
   physics); `ModeProfile` derives over-airtime from the window policy. Note: today's
   `per_over_mtu` is *already* used as the per-frame fragment size in `send()`
   ([conn.rs:274]) — the name is misleading; it is per-frame capacity.

3. **Same-build-only; drop the version-skew claim.** `clamp_available` ([mac.rs:185])
   only absorbs *inbound* ids this build lacks — it does NOT prove the peer can decode a
   mode this endpoint selects. Real cross-build safety needs a peer-capability
   intersection handshake (a separate, larger feature — file as follow-up). For the
   keystone: **declare same-build-only** and remove the "absorbs version skew" claim.

4. **Canonical ids must be 0..=7.** The MODE byte is a full `u8` ([frame.rs:237]), but
   the `rx_rung` *feedback* field is **3 bits** (FLAGS bits 2–4, masked → 0..=7)
   ([frame.rs:344-353]). Canonical wire ids therefore must stay `0..=7`; validate
   uniqueness + monotonic robustness ordering + range at ladder-build time.

5. **Family granularity: keep 3 link adaptation tiers.** The upshift algorithm depends
   on tiered one-step limits ([mac.rs:350]); PHY `ModeFamily` stays the 2-family
   architecture. The link derives its 3 tiers from `(family, short_name)` — policy, not
   PHY truth.

6. **Physics constants live in a `sonde-phy-runtime` metadata table** keyed by
   `mode_name()`, validated against the registered waveforms — **no `Waveform` trait
   change** (keeps the PHY agent's trait stable). The capability snapshot is stored on
   `SondePhy` *before* waveforms move into the worker ([runtime.rs:157]). **(Owner of
   this PHY-side table = open coordination with the sonde-b60 PHY session — see the
   handoff prompt; the link half is built independently against synthetic capabilities.)**

7. **No silent floor-only default.** A production transport must publish capabilities or
   fail `Driver`/`Connection` construction. `NullPhy` + wire doubles get *explicit*
   floor/test capabilities. (Truthful floor-only is fine as an *explicit* production
   stopgap until the PHY publishes more — it is today's honest state, not a fabrication.)

8. **`apply_rung` must re-arm the outstanding deadline on profile swap.** Deadlines are
   absolute (`now + profile.turn_recovery_timeout()`, [conn.rs:271/298/415]); swapping to
   a slower mode without re-arming fires a retransmit early. On swap, recompute the
   active deadline from the new profile.

9. **Cross-check `RxFrame.mode()` vs the link MODE byte.** The runtime labels the
   `RxFrame` with the waveform that actually decoded ([runtime.rs:331]); `Driver`
   currently discards it and trusts only the peer's self-declared MODE byte
   ([driver.rs:177]). With dynamic ids, compare them (divergence = a signal). Hardening,
   not strictly keystone-blocking.

10. **Ownership = immutable `Ladder` in `Connection`** via `Connection::{initiator,
    acceptor}_with_ladder` (keep the default ctors for tests; keep pure `mac` helpers
    operating on a `&Ladder` for the algorithm tests). `Driver::new` takes an
    already-built `Connection` ([driver.rs:73]), so the ladder is built at the
    Connection-construction site (the wiring layer reads the PHY's capabilities there).

11. **B4 interaction:** do NOT swap per-mode MTU while `send()` still fragments at
    enqueue ([conn.rs:274]) — a downshift would orphan oversized fragments. Either land
    B4 (transmit-time fragmentation) first, or pin fragmentation to a conservative
    **global-minimum frame size** across the published ladder until B4 lands.

### Build split (pending PHY-agent coordination)

- **LINK half (my lane, collision-free, build now):** `Ladder` + link-side ladder
  construction from `&[ModeCapability]`, `with_ladder` constructors, `MainPinned` hints,
  `ModeProfile` swap + deadline re-arm, 3-tier family derivation, global-min fragment
  pin, all unit/integration gates against synthetic capability sets. No edits to
  `sonde-phy*`.
- **PHY half (sonde-b60's lane):** the `ModeCapability` publication — capability type +
  how the PHY exposes it (a `PhyTransport`/separate-trait method vs a value handed to the
  wiring layer) + the `sonde-phy-runtime` metadata table with real measured knees/
  airtime/capacity + `SondePhy` snapshot. **Contract + ownership to be agreed with the
  sonde-b60 session before either side commits the seam type.**
