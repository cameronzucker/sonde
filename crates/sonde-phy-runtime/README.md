# sonde-phy-runtime

The first production implementation of `sonde_phy::phy_api::PhyTransport`. This
is the crate the Tuxlink link/MAC (#5) and link-adaptation (#7) layers depend on
to drive a real modem (the `NullPhy` in `sonde-phy` is only a loopback test
fixture).

`SondePhy` is a thin handle over a background worker thread running a
**half-duplex pump**: it bridges the queued / async-shaped `PhyTransport` trait
(`send_frame` / `poll_rx` / `channel_quality`) to the inherently *blocking*
encode → PTT → audio and capture → demod stack. It is generic over two seams:

- **`Waveform`** — modulation/demodulation. `FloorWaveform` wraps the HF
  wide-band low-density floor today.
- **`Radio`** — half-duplex hardware. `LoopbackRadio` is the hardware-free test
  double; `SoundcardRadio` (feature `hardware`) is the production soundcard+PTT.

## Consuming it from Tuxlink

```rust
use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::PhyTransport;
use sonde_phy_runtime::{FloorWaveform, LoopbackRadio, SondePhy};

// Hardware-free (CI / tests): a perfect loopback channel.
let mut phy = SondePhy::new(FloorWaveform::new(), LoopbackRadio::new());

let _token = phy.send_frame(b"CQ CQ", ModeHint::Floor).unwrap();
while let Some(frame) = phy.poll_rx() {
    println!("rx {} bytes", frame.payload().len());
}
```

For a real bench, build with `--features hardware` and swap in `SoundcardRadio`:

```rust,ignore
use sonde_phy_runtime::soundcard::SoundcardRadio;
use std::{path::Path, time::Duration};

let radio = SoundcardRadio::open(
    Some("plughw:CARD=Device"),   // output device
    Some("plughw:CARD=Device"),   // input device
    Path::new("/dev/ttyUSB0"),    // RTS PTT
    Duration::from_secs(30),      // max airtime
)?;
let mut phy = SondePhy::new(FloorWaveform::new(), radio);
```

> **RADIO-1.** `SoundcardRadio` keys a real transmitter. No agent / automation /
> CI keys the radio — the operator (licensee) does, with per-invocation consent.
> See `docs/pitfalls/implementation-pitfalls.md` (RADIO-1).

## FM extension point ("Sonde FM")

The runtime is waveform-agnostic. To add a VHF/UHF FM variant:

1. Implement `Waveform` for an `FmWaveform` (FM-appropriate pre-emphasis
   handling, deviation/PAPR control, and channel assumptions — the HF
   `FloorWaveform` makes none of these, so nothing leaks).
2. Return the FM mode family from `Waveform::family()` (add a dedicated
   `ModeFamily::Fm` to `sonde_phy::modes` if the OFDM/floor split doesn't fit FM).
3. Construct `SondePhy::new(FmWaveform::new(), radio)`. The worker thread,
   half-duplex pump, PTT, queues, and `channel_quality` are all reused unchanged.

`tests/waveform_family_agnostic.rs` is the guard that keeps this true — it drives
the runtime through a non-floor waveform. The `Radio` seam is shared too: FM keys
PTT the same way (RTS / USB-HID).

## Status / follow-on work

- **FEC:** the HF floor transmits *uncoded* today. Wiring `sonde-fec`'s LDPC into
  the waveform is a separate effort; it does not change this crate's interface.
- **Channel-sim gate:** validating the waveform against `hf-channel-sim`
  impairments (vs. the perfect `LoopbackRadio`) is a separate effort.
- **SNR reporting:** `channel_quality` reports frame counts + FER today;
  per-sub-carrier SNR lands when the floor exposes its estimator.

See [ADR 0003](../../docs/adr/0003-sonde-phy-runtime-adapter.md) for the
architecture decision and `docs/superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md`
for the implementation plan.
