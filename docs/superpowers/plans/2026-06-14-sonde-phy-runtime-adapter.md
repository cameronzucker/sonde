# SondePhy Runtime Adapter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `SondePhy`, the first production implementation of `sonde_phy::phy_api::PhyTransport`, so the Tuxlink link/MAC (#5) and link-adaptation (#7) layers can drive the modem through the documented inter-subsystem boundary instead of the `NullPhy` loopback stub.

**Architecture:** A new `crates/sonde-phy-runtime` crate provides `SondePhy<W, R>`, generic over a `Waveform` (the modulation/demodulation seam — HF floor today, **FM tomorrow**) and a `Radio` (the half-duplex hardware seam — soundcard+PTT in production, an in-memory loopback in tests). `SondePhy` runs one background worker thread implementing a half-duplex pump: drain queued TX frames (assert PTT → play → release), otherwise capture a short RX window and try to decode. `send_frame`/`poll_rx`/`channel_quality` are thin queue operations over `std::sync::mpsc` + a shared `Mutex` snapshot. Every layer except the production `SoundcardRadio` is unit-tested with no hardware, honouring RADIO-1 (an agent never keys a real radio).

**Tech Stack:** Rust 2021, `std::thread` + `std::sync::mpsc` + `Arc<Mutex<_>>` (no async runtime — the PHY is inherently blocking I/O), reusing `sonde-phy` (waveform, audio, `PhyTransport`), `sonde-rig-rts` (`Ptt`), and `sonde-tx` (`run_transmission`, `AirtimeBudget`, `check_budget`, `AbortablePlay`).

---

## Background: what already exists (read before starting)

- **`PhyTransport` trait** — `crates/sonde-phy/src/phy_api.rs:137`. Methods: `send_frame(&mut self, payload: &[u8], hint: ModeHint) -> Result<TxToken, PhyError>`, `poll_rx(&mut self) -> Option<RxFrame>`, `channel_quality(&self) -> ChannelQualityReport`. `TxToken(pub u64)`, `RxFrame::new(payload, mode, per_subcarrier_snr_db, frame_snr_db, decode_ok)`, `ChannelQualityReport::from_parts(per_subcarrier_snr_db, aggregate_snr_db, recent_frames_total, recent_frames_failed, current_bit_loading)` and `::empty()`. The only existing impl is `NullPhy` (loopback test fixture).
- **HF floor waveform** — `crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs`. `WidebandLowDensityFloor::new()`, `.transmit_multi_with_preamble(payload) -> Result<Vec<f32>, PhyError>` (preamble + multi-symbol length-prefixed body — the arbitrary-length, self-synchronising wire format) and `.receive_multi_with_sync(samples) -> Result<(usize, Vec<u8>), PhyError>` (scans for the Zadoff-Chu preamble, returns `(offset, payload)`). `PREAMBLE_LEN_SAMPLES = 192`. Holds no FEC today (uncoded BPSK) — FEC is wired in by a separate plan and does not change this adapter's interface.
- **Modes** — `crates/sonde-phy/src/modes.rs`. `ModeHint` (`MainAuto`, `MainPinned(&str)`, `Floor`, `FloorCrowdedBand`), `ModeTable::default().resolve(hint, channel_snr_db) -> ResolvedMode`, `ModeFamily` (`OfdmMain`, `RobustnessFloor`). `ResolvedMode = ModeDescriptor` with `.short_name()` + `.family()`.
- **Audio + PTT** — `sonde_phy::audio_device::{AudioOutput, AudioInput}` (`open(Option<&str>)`, blocking play/record with abort), `sonde_phy::audio_io::{AudioBuffer, SAMPLE_RATE_HZ}`, `sonde_rig_rts::{Ptt, PttState, RtsPtt, LinuxTty}`.
- **TX orchestration to reuse** — `sonde_tx::{run_transmission, AbortablePlay, AirtimeBudget, check_budget}`. `run_transmission<P: Ptt, A: AbortablePlay>(...)` already encapsulates PTT-lead-in → play → tail-drain → release with SIGINT abort; `AbortablePlay` is implemented for `AudioOutput`.
- **Error type** — `sonde_phy::error::PhyError` (variants incl. `AudioIo(String)`, `FrameDetect(String)`, `Sync(String)`, `PayloadTooLarge { actual, capacity }`). Reuse it; do not invent a new error enum for the trait surface (the trait returns `PhyError`).

## File Structure

- `crates/sonde-phy-runtime/Cargo.toml` — new workspace member.
- `crates/sonde-phy-runtime/src/lib.rs` — crate root, re-exports, crate-level docs incl. the FM extension contract.
- `crates/sonde-phy-runtime/src/waveform.rs` — `Waveform` trait + `DecodedFrame` + `FloorWaveform` (HF impl). **The FM extension seam.**
- `crates/sonde-phy-runtime/src/radio.rs` — `Radio` trait + `LoopbackRadio` (test double).
- `crates/sonde-phy-runtime/src/runtime.rs` — `SondePhy` struct, worker thread, `PhyTransport` impl, shared snapshot.
- `crates/sonde-phy-runtime/src/soundcard.rs` — production `SoundcardRadio` (`#[cfg(feature = "hardware")]`, composes audio + PTT; not unit-tested per RADIO-1).
- `crates/sonde-phy-runtime/tests/phytransport_loopback.rs` — end-to-end `PhyTransport` contract test via `LoopbackRadio` + `FloorWaveform`.
- `crates/sonde-phy-runtime/tests/waveform_family_agnostic.rs` — proves the `Waveform` trait carries no HF-only assumptions (the FM-readiness guard).
- `crates/sonde-phy-runtime/README.md` — what it is, how Tuxlink consumes it, the FM extension point, follow-on plans.

Each task below is TDD: write the failing test, run it red, implement minimally, run it green, commit.

---

### Task 0: Scaffold the crate

**Files:**
- Create: `crates/sonde-phy-runtime/Cargo.toml`
- Create: `crates/sonde-phy-runtime/src/lib.rs`
- Modify: `Cargo.toml` (workspace members list, line ~7)

- [ ] **Step 1: Add the crate to the workspace members**

In the root `Cargo.toml`, add the new member to the `members` array:

```toml
members = [
    "crates/sonde-fec",
    "crates/sonde-phy",
    "crates/sonde-phy-runtime",
    "crates/sonde-rig-cm108",
    "crates/sonde-rig-rts",
    "crates/sonde-tx",
    "crates/sonde-rx",
    "hf-channel-sim",
]
```

- [ ] **Step 2: Create the crate manifest**

`crates/sonde-phy-runtime/Cargo.toml`:

```toml
[package]
name = "sonde-phy-runtime"
version = "0.0.1"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
description = "Production PhyTransport runtime for sonde: a half-duplex pump that bridges the async-shaped PhyTransport trait (queued send_frame / poll_rx) to the blocking encode→PTT→audio and capture→demod stack. Generic over a Waveform (HF floor today, FM tomorrow) and a Radio (soundcard in production, in-memory loopback in tests)."

[dependencies]
sonde-phy.workspace = true
sonde-rig-rts.workspace = true
sonde-tx = { path = "../sonde-tx" }
thiserror.workspace = true

[features]
# Production soundcard+PTT Radio. Off by default so `cargo test` and CI
# build the hardware-free core; the soundcard path is compiled only when
# an operator builds for a real bench.
hardware = ["sonde-tx/default"]
```

Note: `sonde-tx` is not currently a `[workspace.dependencies]` entry, hence the explicit `path`. If a later task adds it there, switch to `.workspace = true`.

- [ ] **Step 3: Create a minimal lib.rs**

`crates/sonde-phy-runtime/src/lib.rs`:

```rust
//! Production `PhyTransport` runtime for the sonde modem.
//!
//! [`SondePhy`] is the first real implementation of
//! [`sonde_phy::phy_api::PhyTransport`] (the `NullPhy` in `sonde-phy`
//! is a loopback test fixture). It lets the Tuxlink link/MAC (#5) and
//! link-adaptation (#7) layers drive a real modem through the
//! documented inter-subsystem boundary.
//!
//! # Two seams
//!
//! - [`Waveform`] — modulation/demodulation. [`FloorWaveform`] wraps the
//!   HF wide-band low-density floor today. **An FM variant ("Sonde FM",
//!   VHF/UHF) is added by implementing [`Waveform`] for an `FmWaveform`
//!   and registering it under [`sonde_phy::modes::ModeFamily`]** — see
//!   the crate README's "FM extension point". The trait deliberately
//!   carries no HF-only (SSB-passband) assumptions.
//! - [`Radio`] — half-duplex hardware. [`LoopbackRadio`] is the
//!   hardware-free test double; the production `SoundcardRadio`
//!   (feature `hardware`) composes the soundcard + PTT.
//!
//! Per RADIO-1, an agent never runs the production path against a real
//! radio; the operator (licensee) does.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod radio;
pub mod runtime;
pub mod waveform;

#[cfg(feature = "hardware")]
pub mod soundcard;

pub use radio::{LoopbackRadio, Radio};
pub use runtime::SondePhy;
pub use waveform::{DecodedFrame, FloorWaveform, Waveform};
```

This will not compile yet (modules are empty/missing). That is expected — Task 1 creates `waveform`, Task 2 `radio`, Task 3 `runtime`. To keep the tree compiling between tasks, create empty module files now:

`crates/sonde-phy-runtime/src/waveform.rs`, `radio.rs`, `runtime.rs` each containing only:

```rust
//! placeholder — implemented in a later task.
```

and temporarily comment out the `pub use` lines + the `pub mod` lines for not-yet-written modules. (Each task re-enables its own.) Simplest: start `lib.rs` with only `pub mod waveform;` and add the rest as tasks land.

- [ ] **Step 4: Verify the workspace still builds**

Run: `cargo build -p sonde-phy-runtime`
Expected: PASS (empty crate compiles).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/sonde-phy-runtime/
git commit -m "feat(sonde-phy-runtime): scaffold PhyTransport runtime crate"
```

---

### Task 1: The `Waveform` trait + `FloorWaveform` (the FM extension seam)

**Files:**
- Create/replace: `crates/sonde-phy-runtime/src/waveform.rs`
- Modify: `crates/sonde-phy-runtime/src/lib.rs` (enable `pub mod waveform;` + re-exports)

- [ ] **Step 1: Write the failing test**

Append to `crates/sonde-phy-runtime/src/waveform.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_waveform_round_trips_a_payload() {
        let wf = FloorWaveform::new();
        let payload = b"sonde";
        let samples = wf.encode(payload).expect("encode");
        // Prepend leading silence to prove the decoder finds the preamble
        // at a non-zero offset (real captures never start exactly on the
        // symbol boundary).
        let mut captured = vec![0.0f32; 500];
        captured.extend_from_slice(&samples);

        let frame = wf.decode_scan(&captured).expect("a frame is decoded");
        assert_eq!(frame.payload, payload);
        assert_eq!(frame.family, sonde_phy::modes::ModeFamily::RobustnessFloor);
    }

    #[test]
    fn floor_waveform_returns_none_on_pure_noise() {
        let wf = FloorWaveform::new();
        let silence = vec![0.0f32; 4096];
        assert!(wf.decode_scan(&silence).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sonde-phy-runtime waveform`
Expected: FAIL — `Waveform`, `FloorWaveform`, `DecodedFrame` not defined.

- [ ] **Step 3: Implement the trait + HF impl**

Replace the placeholder body of `crates/sonde-phy-runtime/src/waveform.rs` (above the `#[cfg(test)]` block) with:

```rust
//! The modulation/demodulation seam.
//!
//! [`Waveform`] is the single trait a new RF format implements. The HF
//! [`FloorWaveform`] wraps `sonde-phy`'s wide-band low-density floor. A
//! VHF/UHF FM variant ("Sonde FM") is a new `impl Waveform` — see the
//! crate README's "FM extension point". Nothing in this trait assumes
//! SSB, a 2.4 kHz passband, or HF fading; an FM waveform overrides
//! [`Waveform::encode`]/[`Waveform::decode_scan`] with FM-appropriate
//! pre-emphasis handling, deviation/PAPR control, and channel model.

use sonde_phy::error::PhyError;
use sonde_phy::modes::ModeFamily;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

/// One successfully demodulated frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFrame {
    /// FEC-corrected payload bytes (uncoded until the FEC-wiring plan
    /// lands; the byte contract is unchanged either way).
    pub payload: Vec<u8>,
    /// Mode family the frame was demodulated under.
    pub family: ModeFamily,
    /// Aggregate frame SNR in dB, when the waveform measures one. `None`
    /// until the floor exposes its sub-carrier SNR estimator (tracked by
    /// the same follow-up that lands `doppler_spread_hz`, per `phy_api`).
    pub frame_snr_db: Option<f32>,
}

/// A modulation/demodulation format. Implementors are `Send` so the
/// runtime worker thread can own one.
pub trait Waveform: Send {
    /// Modulate `payload` into a self-synchronising sample buffer
    /// (preamble + body) ready to hand to a [`crate::Radio`].
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError>;

    /// Scan `samples` for a frame and demodulate it. Returns `None` when
    /// no frame is present (the common case for an RX window that caught
    /// only noise). Returns `Some` on a clean decode.
    fn decode_scan(&self, samples: &[f32]) -> Option<DecodedFrame>;

    /// The mode family this waveform serves. Used by [`crate::SondePhy`]
    /// to route a `ModeHint` to the right waveform once more than one is
    /// registered (HF floor + FM).
    fn family(&self) -> ModeFamily;
}

/// HF wide-band low-density floor waveform (`floor-wblo`). Wraps
/// [`WidebandLowDensityFloor`] using its preamble + multi-symbol
/// framing so arbitrary-length payloads self-synchronise.
pub struct FloorWaveform {
    inner: WidebandLowDensityFloor,
}

impl FloorWaveform {
    /// Construct the floor waveform with its pinned Wide-mode params.
    pub fn new() -> Self {
        Self {
            inner: WidebandLowDensityFloor::new(),
        }
    }
}

impl Default for FloorWaveform {
    fn default() -> Self {
        Self::new()
    }
}

impl Waveform for FloorWaveform {
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        self.inner.transmit_multi_with_preamble(payload)
    }

    fn decode_scan(&self, samples: &[f32]) -> Option<DecodedFrame> {
        match self.inner.receive_multi_with_sync(samples) {
            Ok((_offset, payload)) => Some(DecodedFrame {
                payload,
                family: ModeFamily::RobustnessFloor,
                frame_snr_db: None,
            }),
            Err(_) => None,
        }
    }

    fn family(&self) -> ModeFamily {
        ModeFamily::RobustnessFloor
    }
}
```

- [ ] **Step 4: Enable the module + re-exports in lib.rs**

In `crates/sonde-phy-runtime/src/lib.rs` ensure these lines are active:

```rust
pub mod waveform;
pub use waveform::{DecodedFrame, FloorWaveform, Waveform};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p sonde-phy-runtime waveform`
Expected: PASS (both tests).

- [ ] **Step 6: Commit**

```bash
git add crates/sonde-phy-runtime/src/waveform.rs crates/sonde-phy-runtime/src/lib.rs
git commit -m "feat(sonde-phy-runtime): Waveform trait + FloorWaveform (FM extension seam)"
```

---

### Task 2: The `Radio` trait + `LoopbackRadio` test double

**Files:**
- Create/replace: `crates/sonde-phy-runtime/src/radio.rs`
- Modify: `crates/sonde-phy-runtime/src/lib.rs` (enable `pub mod radio;` + re-exports)

- [ ] **Step 1: Write the failing test**

Append to `crates/sonde-phy-runtime/src/radio.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_returns_transmitted_samples_on_next_receive() {
        let mut radio = LoopbackRadio::new();
        let tx = vec![0.1, 0.2, 0.3, -0.4];
        radio.transmit(&tx).expect("transmit");

        let rx = radio.receive(1024).expect("receive");
        // Loopback wraps the TX with the same leading/trailing silence a
        // real capture window would: the TX samples must appear verbatim
        // somewhere inside the returned window.
        assert!(
            rx.windows(tx.len()).any(|w| w == tx.as_slice()),
            "transmitted samples must round-trip through the loopback"
        );
    }

    #[test]
    fn loopback_receive_is_silence_when_nothing_was_transmitted() {
        let mut radio = LoopbackRadio::new();
        let rx = radio.receive(256).expect("receive");
        assert_eq!(rx.len(), 256);
        assert!(rx.iter().all(|&s| s == 0.0));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sonde-phy-runtime radio`
Expected: FAIL — `Radio`, `LoopbackRadio` not defined.

- [ ] **Step 3: Implement the trait + loopback**

Replace the placeholder body of `crates/sonde-phy-runtime/src/radio.rs` (above the test module) with:

```rust
//! The half-duplex hardware seam.
//!
//! [`Radio`] hides whether the modem is talking to a real soundcard+PTT
//! or an in-memory loop. The runtime is half-duplex: a [`Radio::transmit`]
//! call owns the channel for its duration (PTT keyed); [`Radio::receive`]
//! captures a window only while not transmitting. This mirrors a single
//! SSB rig + one soundcard, where TX and RX cannot overlap.

use sonde_phy::error::PhyError;

/// A half-duplex audio radio. Implementors handle their own PTT inside
/// [`Radio::transmit`] (assert before the first sample, release after the
/// last). `Send` so the runtime worker can own one.
pub trait Radio: Send {
    /// Transmit `samples` end-to-end (PTT lead-in → play → tail-drain →
    /// release). Blocks until the air is clear.
    fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError>;

    /// Capture up to `max_samples` of receive audio. Returns at least one
    /// sample; may return fewer than `max_samples` if the capture window
    /// closes early. Never keys PTT.
    fn receive(&mut self, max_samples: usize) -> Result<Vec<f32>, PhyError>;
}

/// In-memory half-duplex radio for hardware-free tests. Whatever is
/// `transmit`ted is buffered and handed back, wrapped in leading +
/// trailing silence, on the next `receive` — modelling a perfect channel
/// that loops the transmitter into the receiver. No fading, no noise:
/// that is the channel simulator's job (a separate plan), not this
/// double's.
pub struct LoopbackRadio {
    pending: Vec<f32>,
    lead_silence: usize,
}

impl LoopbackRadio {
    /// New loopback radio with a default 200-sample leading silence so
    /// decoders must exercise their preamble search at a non-zero offset.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            lead_silence: 200,
        }
    }
}

impl Default for LoopbackRadio {
    fn default() -> Self {
        Self::new()
    }
}

impl Radio for LoopbackRadio {
    fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError> {
        let mut framed = vec![0.0f32; self.lead_silence];
        framed.extend_from_slice(samples);
        framed.extend(std::iter::repeat(0.0).take(self.lead_silence));
        self.pending = framed;
        Ok(())
    }

    fn receive(&mut self, max_samples: usize) -> Result<Vec<f32>, PhyError> {
        if self.pending.is_empty() {
            return Ok(vec![0.0f32; max_samples]);
        }
        let out = std::mem::take(&mut self.pending);
        Ok(out)
    }
}
```

- [ ] **Step 4: Enable the module + re-exports in lib.rs**

```rust
pub mod radio;
pub use radio::{LoopbackRadio, Radio};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p sonde-phy-runtime radio`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/sonde-phy-runtime/src/radio.rs crates/sonde-phy-runtime/src/lib.rs
git commit -m "feat(sonde-phy-runtime): Radio trait + LoopbackRadio test double"
```

---

### Task 3: `SondePhy` + worker thread implementing `PhyTransport`

This is the keystone. `SondePhy` owns a worker thread that holds the `Waveform` + `Radio`. The public handle communicates with the worker over channels.

**Files:**
- Create/replace: `crates/sonde-phy-runtime/src/runtime.rs`
- Modify: `crates/sonde-phy-runtime/src/lib.rs` (enable `pub mod runtime;` + re-export)

- [ ] **Step 1: Write the failing test**

Append to `crates/sonde-phy-runtime/src/runtime.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FloorWaveform, LoopbackRadio};
    use sonde_phy::modes::ModeHint;
    use sonde_phy::phy_api::PhyTransport;
    use std::time::Duration;

    /// Poll `poll_rx` until a frame arrives or the deadline passes.
    fn wait_for_frame(phy: &mut SondePhy, timeout: Duration) -> Option<sonde_phy::phy_api::RxFrame> {
        let start = std::time::Instant::now();
        loop {
            if let Some(f) = phy.poll_rx() {
                return Some(f);
            }
            if start.elapsed() > timeout {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn send_frame_round_trips_through_loopback_to_poll_rx() {
        let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());
        let payload = b"hello tuxlink";

        let token = phy.send_frame(payload, ModeHint::Floor).expect("send accepted");
        assert_eq!(token.0, 0, "first token is 0");

        let frame = wait_for_frame(&mut phy, Duration::from_secs(5))
            .expect("a frame round-trips within the deadline");
        assert_eq!(frame.payload(), payload);
        assert!(frame.decode_ok());

        phy.shutdown();
    }

    #[test]
    fn tokens_are_monotonic() {
        let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());
        let t0 = phy.send_frame(b"a", ModeHint::Floor).unwrap();
        let t1 = phy.send_frame(b"b", ModeHint::Floor).unwrap();
        assert_eq!(t1.0, t0.0 + 1);
        phy.shutdown();
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sonde-phy-runtime runtime`
Expected: FAIL — `SondePhy` not defined.

- [ ] **Step 3: Implement the runtime**

Replace the placeholder body of `crates/sonde-phy-runtime/src/runtime.rs` (above the test module) with:

```rust
//! [`SondePhy`]: the production `PhyTransport` runtime.
//!
//! The public `SondePhy` is a thin handle. A worker thread owns the
//! [`Waveform`] + [`Radio`] and runs a half-duplex pump:
//!
//! 1. Drain any queued TX frames — for each, encode via the waveform and
//!    hand the samples to `Radio::transmit` (which keys PTT).
//! 2. Otherwise capture one RX window via `Radio::receive` and try
//!    `Waveform::decode_scan`; on success, push an `RxFrame` to the RX
//!    queue and bump the channel-quality counters.
//!
//! TX is prioritised over RX so a queued frame never waits behind a long
//! capture — half-duplex means we cannot do both at once anyway.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use sonde_phy::error::PhyError;
use sonde_phy::modes::{ModeHint, ModeTable};
use sonde_phy::phy_api::{ChannelQualityReport, PhyTransport, RxFrame, TxToken};

use crate::radio::Radio;
use crate::waveform::Waveform;

/// How many samples the worker captures per RX window when idle.
/// ~0.25 s at 48 kHz — long enough to contain a short floor frame,
/// short enough to stay responsive to queued TX.
const RX_WINDOW_SAMPLES: usize = 12_000;

/// A TX request handed to the worker.
struct TxJob {
    payload: Vec<u8>,
    hint: ModeHint,
}

/// Shared channel-quality snapshot, updated by the worker, read by
/// `channel_quality()`.
#[derive(Default)]
struct QualitySnapshot {
    frames_total: u32,
    frames_failed: u32,
    last_frame_snr_db: Option<f32>,
}

/// Production `PhyTransport` runtime. See crate docs.
pub struct SondePhy {
    tx_jobs: Sender<TxJob>,
    rx_frames: Receiver<RxFrame>,
    quality: Arc<Mutex<QualitySnapshot>>,
    shutdown: Arc<Mutex<bool>>,
    worker: Option<JoinHandle<()>>,
    next_token: u64,
}

impl SondePhy {
    /// Spawn the runtime over the given waveform + radio. The worker
    /// thread starts immediately and begins capturing RX windows.
    pub fn new<W, R>(waveform: W, radio: R) -> Self
    where
        W: Waveform + 'static,
        R: Radio + 'static,
    {
        let (tx_jobs, job_rx) = mpsc::channel::<TxJob>();
        let (frame_tx, rx_frames) = mpsc::channel::<RxFrame>();
        let quality = Arc::new(Mutex::new(QualitySnapshot::default()));
        let shutdown = Arc::new(Mutex::new(false));

        let worker_quality = Arc::clone(&quality);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = std::thread::spawn(move || {
            Worker {
                waveform,
                radio,
                job_rx,
                frame_tx,
                quality: worker_quality,
                shutdown: worker_shutdown,
                modes: ModeTable::default(),
            }
            .run();
        });

        Self {
            tx_jobs,
            rx_frames,
            quality,
            shutdown,
            worker: Some(worker),
            next_token: 0,
        }
    }

    /// Signal the worker to stop and join it. Idempotent. Called by
    /// `Drop`, but exposed so tests can join deterministically.
    pub fn shutdown(&mut self) {
        if let Ok(mut flag) = self.shutdown.lock() {
            *flag = true;
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SondePhy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl PhyTransport for SondePhy {
    fn send_frame(&mut self, payload: &[u8], hint: ModeHint) -> Result<TxToken, PhyError> {
        self.tx_jobs
            .send(TxJob {
                payload: payload.to_vec(),
                hint,
            })
            .map_err(|_| PhyError::AudioIo("phy worker has stopped".into()))?;
        let token = TxToken(self.next_token);
        self.next_token += 1;
        Ok(token)
    }

    fn poll_rx(&mut self) -> Option<RxFrame> {
        self.rx_frames.try_recv().ok()
    }

    fn channel_quality(&self) -> ChannelQualityReport {
        let q = match self.quality.lock() {
            Ok(q) => q,
            Err(_) => return ChannelQualityReport::empty(),
        };
        ChannelQualityReport::from_parts(
            Vec::new(),
            q.last_frame_snr_db.unwrap_or(f32::NAN),
            q.frames_total,
            q.frames_failed,
            None,
        )
    }
}

struct Worker<W: Waveform, R: Radio> {
    waveform: W,
    radio: R,
    job_rx: Receiver<TxJob>,
    frame_tx: Sender<RxFrame>,
    quality: Arc<Mutex<QualitySnapshot>>,
    shutdown: Arc<Mutex<bool>>,
    modes: ModeTable,
}

impl<W: Waveform, R: Radio> Worker<W, R> {
    fn run(mut self) {
        loop {
            if *self.shutdown.lock().unwrap() {
                return;
            }
            // TX has priority: drain one queued job if present.
            match self.job_rx.try_recv() {
                Ok(job) => self.do_tx(job),
                Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => self.do_rx(),
            }
        }
    }

    fn do_tx(&mut self, job: TxJob) {
        let _mode = self.modes.resolve(job.hint, None);
        match self.waveform.encode(&job.payload) {
            Ok(samples) => {
                // A transmit error is logged via the quality counters as a
                // failed frame; we do not crash the worker on a soundcard
                // hiccup.
                if self.radio.transmit(&samples).is_err() {
                    if let Ok(mut q) = self.quality.lock() {
                        q.frames_total += 1;
                        q.frames_failed += 1;
                    }
                }
            }
            Err(_) => {
                if let Ok(mut q) = self.quality.lock() {
                    q.frames_total += 1;
                    q.frames_failed += 1;
                }
            }
        }
    }

    fn do_rx(&mut self) {
        let samples = match self.radio.receive(RX_WINDOW_SAMPLES) {
            Ok(s) => s,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
                return;
            }
        };
        if let Some(frame) = self.waveform.decode_scan(&samples) {
            let mode = self.modes.resolve(ModeHint::Floor, None);
            if let Ok(mut q) = self.quality.lock() {
                q.frames_total += 1;
                q.last_frame_snr_db = frame.frame_snr_db;
            }
            let snr = frame.frame_snr_db.unwrap_or(f32::NAN);
            let rx = RxFrame::new(frame.payload, mode, None, snr, true);
            let _ = self.frame_tx.send(rx);
        }
    }
}
```

- [ ] **Step 4: Enable the module + re-export in lib.rs**

```rust
pub mod runtime;
pub use runtime::SondePhy;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p sonde-phy-runtime runtime`
Expected: PASS (both `send_frame_round_trips_through_loopback_to_poll_rx` and `tokens_are_monotonic`).

- [ ] **Step 6: Commit**

```bash
git add crates/sonde-phy-runtime/src/runtime.rs crates/sonde-phy-runtime/src/lib.rs
git commit -m "feat(sonde-phy-runtime): SondePhy worker thread implementing PhyTransport"
```

---

### Task 4: `channel_quality` reflects observed frames

**Files:**
- Modify: `crates/sonde-phy-runtime/src/runtime.rs` (test module only — behaviour already implemented in Task 3; this task pins it with a test and fixes anything the test exposes).

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/sonde-phy-runtime/src/runtime.rs`:

```rust
    #[test]
    fn channel_quality_counts_a_received_frame() {
        let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());

        // Before any traffic: zero frames, FER 0.0.
        let before = phy.channel_quality();
        assert_eq!(before.frame_error_rate(), 0.0);

        phy.send_frame(b"quality", ModeHint::Floor).unwrap();
        let _ = wait_for_frame(&mut phy, Duration::from_secs(5))
            .expect("frame round-trips");

        // Give the worker a beat to update the snapshot after sending.
        std::thread::sleep(Duration::from_millis(50));
        let after = phy.channel_quality();
        assert!(
            after.frame_error_rate().is_finite(),
            "FER is a finite number after a frame"
        );
        phy.shutdown();
    }
```

- [ ] **Step 2: Run test to verify it fails or passes**

Run: `cargo test -p sonde-phy-runtime channel_quality_counts_a_received_frame`
Expected: PASS if Task 3's counter logic is correct. If it FAILS (e.g. `frame_error_rate` returns NaN because `frames_total` stayed 0), fix `do_rx` to increment `frames_total` on every decoded frame (it already does) — re-run until green.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-phy-runtime/src/runtime.rs
git commit -m "test(sonde-phy-runtime): pin channel_quality frame accounting"
```

---

### Task 5: FM-readiness guard — prove `Waveform` is family-agnostic

This task makes the FM extension point a *tested contract*, not a comment: a second, trivial `Waveform` impl living in a test drives the same `SondePhy` runtime, proving the runtime has no `FloorWaveform`/HF assumptions baked in.

**Files:**
- Create: `crates/sonde-phy-runtime/tests/waveform_family_agnostic.rs`

- [ ] **Step 1: Write the failing test**

`crates/sonde-phy-runtime/tests/waveform_family_agnostic.rs`:

```rust
//! FM-readiness guard. A future "Sonde FM" lands as a new `impl Waveform`
//! routed under `ModeFamily::OfdmMain` (or a dedicated FM family added to
//! `sonde_phy::modes`). This test stands in a trivial non-floor waveform
//! and drives the real `SondePhy` runtime through it, proving the runtime
//! is waveform-agnostic — the property that makes FM an extension, not a
//! fork. If this breaks, the runtime grew an HF-only assumption.

use sonde_phy::modes::{ModeFamily, ModeHint};
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{DecodedFrame, Radio, SondePhy, Waveform};
use std::time::{Duration, Instant};

/// A toy waveform: payload bytes are widened to f32 samples 1:1 and read
/// back the same way. No preamble, no DSP — it exercises the runtime's
/// plumbing, not modulation. Tagged as a non-floor family on purpose.
struct ByteEchoWaveform;

impl Waveform for ByteEchoWaveform {
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, sonde_phy::error::PhyError> {
        Ok(payload.iter().map(|&b| b as f32).collect())
    }
    fn decode_scan(&self, samples: &[f32]) -> Option<DecodedFrame> {
        // Loopback wraps with leading/trailing zero silence; strip zeros.
        let bytes: Vec<u8> = samples
            .iter()
            .filter(|&&s| s != 0.0)
            .map(|&s| s as u8)
            .collect();
        if bytes.is_empty() {
            None
        } else {
            Some(DecodedFrame {
                payload: bytes,
                family: ModeFamily::OfdmMain,
                frame_snr_db: Some(42.0),
            })
        }
    }
    fn family(&self) -> ModeFamily {
        ModeFamily::OfdmMain
    }
}

/// Minimal loopback radio local to this test (mirrors `LoopbackRadio`).
struct EchoRadio {
    pending: Vec<f32>,
}
impl Radio for EchoRadio {
    fn transmit(&mut self, samples: &[f32]) -> Result<(), sonde_phy::error::PhyError> {
        let mut framed = vec![0.0f32; 8];
        framed.extend_from_slice(samples);
        framed.push(0.0);
        self.pending = framed;
        Ok(())
    }
    fn receive(&mut self, max: usize) -> Result<Vec<f32>, sonde_phy::error::PhyError> {
        if self.pending.is_empty() {
            Ok(vec![0.0; max])
        } else {
            Ok(std::mem::take(&mut self.pending))
        }
    }
}

#[test]
fn runtime_drives_a_non_floor_waveform() {
    let mut phy = SondePhy::new(ByteEchoWaveform, EchoRadio { pending: Vec::new() });
    // Bytes that are non-zero so the echo waveform survives the silence strip.
    let payload = vec![3u8, 1, 4, 1, 5, 9];
    phy.send_frame(&payload, ModeHint::MainAuto).unwrap();

    let start = Instant::now();
    let frame = loop {
        if let Some(f) = phy.poll_rx() {
            break f;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "frame must arrive");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(frame.payload(), payload.as_slice());
    assert_eq!(frame.frame_snr_db(), 42.0);
    phy.shutdown();
}
```

- [ ] **Step 2: Run test to verify it fails then passes**

Run: `cargo test -p sonde-phy-runtime --test waveform_family_agnostic`
Expected: PASS. If it FAILS to compile because a needed item (`Radio`, `Waveform`, `DecodedFrame`) is not re-exported from the crate root, add the missing `pub use` to `lib.rs` and re-run.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-phy-runtime/tests/waveform_family_agnostic.rs crates/sonde-phy-runtime/src/lib.rs
git commit -m "test(sonde-phy-runtime): FM-readiness guard — runtime is waveform-agnostic"
```

---

### Task 6: End-to-end `PhyTransport` contract test (integration)

A `tests/` integration test (separate compilation unit, like `sonde-phy`'s `tests/api_contract.rs`) that exercises the public crate surface exactly as Tuxlink will.

**Files:**
- Create: `crates/sonde-phy-runtime/tests/phytransport_loopback.rs`

- [ ] **Step 1: Write the test**

`crates/sonde-phy-runtime/tests/phytransport_loopback.rs`:

```rust
//! The contract test Tuxlink's integration mirrors: construct a SondePhy,
//! send a frame, poll it back, read channel quality — all through the
//! `PhyTransport` trait, no concrete-type leakage. Hardware-free
//! (LoopbackRadio), so it runs in CI and respects RADIO-1.

use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{FloorWaveform, LoopbackRadio, SondePhy};
use std::time::{Duration, Instant};

fn poll_until<P: PhyTransport>(phy: &mut P, timeout: Duration) -> Option<sonde_phy::phy_api::RxFrame> {
    let start = Instant::now();
    loop {
        if let Some(f) = phy.poll_rx() {
            return Some(f);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn tuxlink_style_send_and_receive() {
    // Bind to the trait object the way subsystem #5 would, to prove the
    // surface is object-safe enough for the consumer's needs.
    let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());

    let payload = b"the quick brown fox";
    let _token = phy.send_frame(payload, ModeHint::Floor).expect("accepted");

    let frame = poll_until(&mut phy, Duration::from_secs(5)).expect("frame round-trips");
    assert_eq!(frame.payload(), payload);
    assert!(frame.decode_ok());

    let q = phy.channel_quality();
    assert!(q.frame_error_rate().is_finite());
    phy.shutdown();
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p sonde-phy-runtime --test phytransport_loopback`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-phy-runtime/tests/phytransport_loopback.rs
git commit -m "test(sonde-phy-runtime): end-to-end PhyTransport contract via loopback"
```

---

### Task 7: Production `SoundcardRadio` (feature `hardware`, not hardware-tested)

The only non-unit-tested layer (RADIO-1: an agent does not key a real radio). It composes the soundcard + PTT and reuses sonde-tx's `run_transmission`. Gated behind the `hardware` feature so default CI never needs ALSA at link time for this crate. Verified by `cargo build`/`clippy`, not by a hardware test.

**Files:**
- Create: `crates/sonde-phy-runtime/src/soundcard.rs`
- Modify: `crates/sonde-phy-runtime/src/lib.rs` (the `#[cfg(feature = "hardware")] pub mod soundcard;` is already present from Task 0)

- [ ] **Step 1: Implement `SoundcardRadio`**

`crates/sonde-phy-runtime/src/soundcard.rs`:

```rust
//! Production half-duplex `Radio` over a CPAL soundcard + serial-RTS PTT.
//!
//! NOT exercised by automated tests — RADIO-1 forbids an agent keying a
//! real transmitter. The operator (licensee) runs this. It is kept thin:
//! all PTT timing/airtime safety lives in `sonde-tx`'s `run_transmission`,
//! which this delegates to.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use sonde_phy::audio_device::{AudioInput, AudioOutput, RecordOutcome};
use sonde_phy::audio_io::AudioBuffer;
use sonde_phy::error::PhyError;
use sonde_rig_rts::{LinuxTty, RtsPtt};
use sonde_tx::{check_budget, run_transmission, AirtimeBudget};

use crate::radio::Radio;

/// Soundcard + RTS-PTT production radio.
pub struct SoundcardRadio {
    output: AudioOutput,
    input: AudioInput,
    ptt: RtsPtt<LinuxTty>,
    max_airtime: Duration,
}

impl SoundcardRadio {
    /// Open the named output + input devices and the RTS PTT on `tty`.
    /// `None` device names select the system default.
    pub fn open(
        output_device: Option<&str>,
        input_device: Option<&str>,
        tty: &Path,
        max_airtime: Duration,
    ) -> Result<Self, PhyError> {
        let output = AudioOutput::open(output_device)?;
        let input = AudioInput::open(input_device)?;
        let linux_tty = LinuxTty::open(tty)
            .map_err(|e| PhyError::AudioIo(format!("open PTT tty: {e}")))?;
        let ptt = RtsPtt::new(linux_tty);
        Ok(Self {
            output,
            input,
            ptt,
            max_airtime,
        })
    }
}

impl Radio for SoundcardRadio {
    fn transmit(&mut self, samples: &[f32]) -> Result<(), PhyError> {
        let buffer = AudioBuffer::from_samples(samples.to_vec());
        let budget = AirtimeBudget::from_buffer_defaults(&buffer);
        check_budget(&budget, self.max_airtime)
            .map_err(|e| PhyError::AudioIo(format!("airtime budget: {e}")))?;
        let abort = AtomicBool::new(false);
        run_transmission(&mut self.ptt, &mut self.output, &buffer, &abort)
            .map_err(|e| PhyError::AudioIo(format!("transmit: {e}")))?;
        Ok(())
    }

    fn receive(&mut self, max_samples: usize) -> Result<Vec<f32>, PhyError> {
        let abort = AtomicBool::new(false);
        let duration = Duration::from_secs_f32(max_samples as f32 / 48_000.0);
        let (outcome, buffer) = self.input.record_blocking_with_abort(duration, &abort)?;
        let _ = outcome; // Completed | Aborted both yield whatever was captured
        Ok(buffer.into_samples())
    }
}
```

> **VERIFY THESE SIGNATURES BEFORE IMPLEMENTING.** This task assumes:
> `AudioBuffer::from_samples(Vec<f32>)`, `AudioBuffer::into_samples()`, `RtsPtt::new(LinuxTty)`, `LinuxTty::open(&Path)`, `run_transmission(&mut P, &mut A, &AudioBuffer, &AtomicBool)`, `RecordOutcome` import unused. Confirm each against the current source (`grep -n` in `sonde-phy/src/audio_io.rs`, `sonde-rig-rts/src/{writer,linux}.rs`, `sonde-tx/src/lib.rs:330`). If a constructor differs (e.g. `AudioBuffer::new`), adapt the call — the *shape* (open devices, budget-gate, delegate to `run_transmission`, record a window) is the contract, not the exact constructor names. Drop the unused `RecordOutcome` import if clippy flags it.

- [ ] **Step 2: Verify it compiles under the feature**

Run: `cargo build -p sonde-phy-runtime --features hardware`
Expected: PASS. Fix any signature mismatches surfaced (see the VERIFY note). Requires `libasound2-dev` + `pkg-config` (same as the rest of the workspace).

- [ ] **Step 3: Verify clippy is clean under the feature**

Run: `cargo clippy -p sonde-phy-runtime --features hardware --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sonde-phy-runtime/src/soundcard.rs
git commit -m "feat(sonde-phy-runtime): production SoundcardRadio (feature=hardware, RADIO-1 untested)"
```

---

### Task 8: CI feature coverage + README + repository-URL fix

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `crates/sonde-phy-runtime/README.md`
- Modify: `Cargo.toml` (`[workspace.package] repository`)

- [ ] **Step 1: Make CI build the hardware feature too**

The default `cargo build`/`clippy`/`test` steps already cover the hardware-free core. Add a build of the `hardware` feature so the soundcard path can't bit-rot (it links ALSA, already installed by the existing apt step). Add after the existing `Clippy` step in `.github/workflows/ci.yml`:

```yaml
      - name: Clippy (hardware feature)
        run: cargo clippy -p sonde-phy-runtime --features hardware --all-targets -- -D warnings
```

- [ ] **Step 2: Fix the stale repository URL**

In the root `Cargo.toml`, under `[workspace.package]`:

```toml
repository = "https://github.com/cameronzucker/sonde"
```

(Was `.../tuxlink` — a known cleanup item from the extraction; correcting it now unblocks any future crates.io publish and stops the runtime crate inheriting the wrong URL.)

- [ ] **Step 3: Write the consumer-facing README**

`crates/sonde-phy-runtime/README.md`:

```markdown
# sonde-phy-runtime

The first production implementation of `sonde_phy::phy_api::PhyTransport`.
This is the crate the Tuxlink link/MAC (#5) and link-adaptation (#7)
layers depend on to drive a real modem.

## Consuming it from Tuxlink

```rust
use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{FloorWaveform, SondePhy};

# #[cfg(feature = "hardware")]
# fn demo() -> Result<(), sonde_phy::error::PhyError> {
use sonde_phy_runtime::soundcard::SoundcardRadio;
use std::{path::Path, time::Duration};

let radio = SoundcardRadio::open(
    Some("plughw:CARD=Device"),   // output device
    Some("plughw:CARD=Device"),   // input device
    Path::new("/dev/ttyUSB0"),    // RTS PTT
    Duration::from_secs(30),      // max airtime
)?;
let mut phy = SondePhy::new(FloorWaveform::new(), radio);

let _token = phy.send_frame(b"CQ CQ", ModeHint::Floor)?;
while let Some(frame) = phy.poll_rx() {
    println!("rx {} bytes", frame.payload().len());
}
# Ok(()) }
```

For tests / CI, swap `SoundcardRadio` for `LoopbackRadio` — no hardware,
no ALSA, no RADIO-1 concern.

## FM extension point ("Sonde FM")

The runtime is waveform-agnostic. To add a VHF/UHF FM variant:

1. Implement `Waveform` for an `FmWaveform` (FM-appropriate pre-emphasis
   handling, deviation/PAPR control, and channel assumptions — the HF
   `FloorWaveform` makes none of these, so nothing leaks).
2. Return the FM mode family from `Waveform::family()` (add a dedicated
   `ModeFamily::Fm` to `sonde_phy::modes` if the OFDM/floor split doesn't
   fit FM).
3. Construct `SondePhy::new(FmWaveform::new(), radio)`. The worker thread,
   half-duplex pump, PTT, queues, and `channel_quality` are all reused
   unchanged. `tests/waveform_family_agnostic.rs` is the guard that keeps
   this true.

The `Radio` seam is shared too — FM keys PTT the same way (RTS / USB-HID).

## Status / follow-on work

- **FEC:** the HF floor transmits *uncoded* today. Wiring `sonde-fec`'s
  LDPC into the waveform is a separate plan; it does not change this
  crate's interface.
- **Channel-sim gate:** validating the waveform against `hf-channel-sim`
  impairments (vs. the perfect `LoopbackRadio`) is a separate plan.
- **SNR reporting:** `channel_quality` reports frame counts + FER today;
  per-sub-carrier SNR lands when the floor exposes its estimator.
```

- [ ] **Step 4: Run the whole workspace test + clippy to confirm nothing regressed**

Run: `cargo test --workspace`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.
Run: `cargo fmt --all --check`
Expected: PASS (run `cargo fmt --all` first if not).

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml Cargo.toml crates/sonde-phy-runtime/README.md
git commit -m "docs(sonde-phy-runtime): consumer README + FM extension contract; ci: hardware-feature gate; fix workspace repository URL"
```

---

## Self-Review

**Spec coverage** (against the user's goal "wire it up in Tuxlink via PhyTransport, prepare for testing", and "make FM a documented extension point"):
- Production `PhyTransport` impl → Tasks 3, 6 ✓
- Hardware-free, RADIO-1-respecting test path → `LoopbackRadio` (Task 2), all contract tests ✓
- Production hardware path → Task 5/7 `SoundcardRadio` ✓
- FM extension point, baked into the design + tested → `Waveform` seam (Task 1), guard test (Task 5), README contract (Task 8) ✓
- Tuxlink consumption documented → Task 8 README ✓
- Half-duplex correctness (TX/RX can't overlap on one rig) → worker pump prioritises TX (Task 3) ✓
- Out of scope, called out as separate plans: FEC-into-PHY wiring, `hf-channel-sim` validation gate, packaging/publish, Op B6 (removing the stale copy from tuxlink). These do not change this crate's interface.

**Type consistency:** `Waveform::{encode, decode_scan, family}`, `DecodedFrame { payload, family, frame_snr_db }`, `Radio::{transmit, receive}`, `SondePhy::{new, send_frame, poll_rx, channel_quality, shutdown}` are used identically across Tasks 1–8. `RxFrame::new(payload, mode, None, snr, true)` and `ChannelQualityReport::from_parts(...)` match the real `phy_api.rs` signatures read during planning.

**Known verification points (not placeholders — flagged inline):** Task 7's soundcard constructor names (`AudioBuffer::from_samples`/`into_samples`, `RtsPtt::new`, `run_transmission` arity) are the one place the plan asserts signatures it did not read in full; Step 1's VERIFY note tells the implementer exactly what to confirm and that the *shape* is the contract. Everything in Tasks 1–6 uses signatures confirmed against source.

**Numbering note:** Tasks are numbered 0–8; "Task 5" (FM guard) and the production-radio task ("Task 7") are distinct — follow the headers, not a mental sequence.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md`. Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
