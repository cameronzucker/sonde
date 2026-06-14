# 3. SondePhy runtime adapter architecture

Date: 2026-06-14
Status: Accepted
Deciders: cameronzucker, sonde-ge9

## Context

`sonde-phy` defines the `PhyTransport` trait (`crates/sonde-phy/src/phy_api.rs`) — the documented inter-subsystem boundary between the modem PHY and its consumers. Its only implementation today is `NullPhy`, a loopback test fixture. The Tuxlink link/MAC (#5) and link-adaptation (#7) layers need a *real* PHY to drive: something that turns `send_frame` / `poll_rx` / `channel_quality` calls into actual encode → PTT → audio transmission and capture → demodulation, while honouring RADIO-1 (an agent never keys a real radio).

The trait surface is queued and async-shaped (`send_frame` returns a `TxToken` immediately; `poll_rx` is non-blocking), but the underlying PHY I/O is inherently blocking and half-duplex: a single SSB rig with one soundcard cannot transmit and receive at once. The adapter must bridge those two shapes. It must also be ready for an FM variant — "Sonde FM" (VHF/UHF) — without re-architecting, since FM is on the roadmap.

The full implementation strategy is captured in the plan at [`docs/superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md`](../superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md). This ADR records the *architectural decision* that plan enacts, so the "why" survives independent of the task-by-task plan.

## Decision

Add a new crate, **`crates/sonde-phy-runtime`**, providing **`SondePhy<W, R>`** — the first production `PhyTransport` implementation. Its architecture:

### Half-duplex worker-thread pump

`SondePhy` is a thin public handle. A single background worker thread owns the waveform and radio and runs a half-duplex pump: it drains any queued TX frame first (encode → assert PTT → play → release), and otherwise captures a short RX window and attempts a decode. TX is prioritised over RX because half-duplex means the two cannot overlap anyway, and a queued frame must not wait behind a long capture. The public methods are thin queue operations: `send_frame` enqueues a job over `std::sync::mpsc` and returns a monotonic `TxToken`; `poll_rx` is a non-blocking `try_recv`; `channel_quality` reads a shared `Arc<Mutex<_>>` snapshot the worker updates. This is what bridges the queued/async-shaped trait to the blocking encode/PTT/audio and capture/demod stack.

### Two seams: `Waveform` and `Radio`

`SondePhy` is generic over two traits:

- **`Waveform`** — the modulation/demodulation seam. `FloorWaveform` (the HF wide-band low-density floor) is the implementation today. The trait deliberately carries **no HF-only assumptions** (no SSB passband, no HF-fading bias). **An FM variant — "Sonde FM" — is an explicit future extension point:** a new `impl Waveform` for an `FmWaveform`, with no runtime changes. A test (`tests/waveform_family_agnostic.rs`) drives the real runtime through a non-floor toy waveform, turning the FM-readiness property into a tested contract rather than a comment.
- **`Radio`** — the half-duplex hardware seam. `LoopbackRadio` is the in-memory test double that makes the entire stack hardware-free in CI, honouring RADIO-1. `SoundcardRadio` (behind the `hardware` feature) is the production path: it composes the soundcard + RTS PTT and delegates PTT timing / airtime safety to `sonde-tx`'s `run_transmission`. The production radio is the only layer not unit-tested, per RADIO-1 — it is verified by `cargo build` / `clippy` under the feature, and the operator (licensee) runs it.

### Explicitly out of scope (sibling efforts that do not change the interface)

- **FEC-into-PHY wiring** — the HF floor transmits uncoded today; wiring `sonde-fec`'s LDPC into the waveform is a separate plan and does not change the `PhyTransport` or `Waveform` surface.
- **The `hf-channel-sim` validation gate** — validating the waveform against simulated channel impairments (vs. the perfect `LoopbackRadio`) is a separate plan.
- **Packaging / publish** and the removal of the stale copy from Tuxlink are separate cleanup items.

## Consequences

**Positive:**

- Subsystems #5 and #7 can depend on a real `PhyTransport` through the documented boundary instead of the `NullPhy` stub, with no concrete-type leakage.
- The entire stack is testable hardware-free (`LoopbackRadio` + `FloorWaveform`), so the contract runs in CI and respects RADIO-1; only the soundcard layer touches real hardware.
- FM becomes an *extension, not a fork*: the worker thread, half-duplex pump, PTT handling, queues, and `channel_quality` are all reused unchanged for a new `Waveform`. The family-agnostic guard test keeps that property honest.
- Blocking PHY I/O is modelled honestly by a dedicated worker thread; the trait's async-shaped surface is preserved for consumers without dragging an async runtime into inherently-blocking code.

**Negative:**

- A background thread + channels + a shared mutex add concurrency surface (shutdown/join discipline, snapshot locking) that a synchronous design would not have. Bounded by keeping the worker's loop small and the public handle thin.
- `channel_quality` reports frame counts + FER today; per-sub-carrier SNR is `None` until the floor exposes its estimator — consumers must tolerate partial quality reporting initially.
- The `hardware`-feature split means the production path can bit-rot if CI does not build it; the plan adds a `hardware`-feature clippy/build step to CI to prevent that.

## Alternatives considered

- **CLI-orchestration instead of a library `PhyTransport` impl** — have the consumer shell out to a modem CLI rather than link a crate. Rejected per the consumer decision: subsystems #5/#7 consume the modem *as a library through `PhyTransport`*, which gives type-safe, in-process, testable integration (the loopback contract test mirrors exactly how Tuxlink calls it). A CLI boundary would reintroduce process management and serialization the trait was designed to avoid.
- **An async runtime (e.g. tokio) instead of `std::thread`** — model the pump as async tasks. Rejected: the PHY I/O is inherently blocking (soundcard play/record, PTT assert/release) and half-duplex. `std::thread` + `mpsc` + `Arc<Mutex<_>>` expresses "one worker owns the radio, the handle talks to it over channels" directly, with no runtime dependency and no async-over-blocking-I/O impedance mismatch.
- **Bake the HF floor in directly (no `Waveform` seam).** Rejected — it would make FM a fork of the runtime rather than a new `impl Waveform`, exactly the outcome the family-agnostic design and its guard test exist to prevent.
- **Unit-test the production `SoundcardRadio` against hardware.** Rejected — RADIO-1 forbids an agent keying a real transmitter. The production radio is verified by compilation under the `hardware` feature; on-air verification is the licensee's, per-invocation.
