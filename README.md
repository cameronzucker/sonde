<h1 align="center">Sonde</h1>

<p align="center">
  A clean-sheet HF data modem, written in Rust.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-AGPL_v3-blue.svg" alt="License: AGPL v3"></a>
  <a href="CHANGELOG.md"><img src="https://img.shields.io/github/v/release/cameronzucker/sonde?label=release" alt="Latest release"></a>
  <a href="https://github.com/cameronzucker/sonde/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/cameronzucker/sonde/ci.yml?label=build" alt="Build status"></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/rust-1.75+-orange.svg?logo=rust" alt="Rust 1.75+"></a>
</p>

> [!WARNING]
> **🚧 Sonde is pre-alpha and not yet a working modem.**
>
> One waveform decodes end-to-end today, and only on a clean (noise-only)
> *simulated* channel. The throughput ladder, the fading equalizer, and on-air
> operation are unfinished.
>
> [release-please](https://github.com/googleapis/release-please) tags versions on
> this repository automatically from conventional-commit activity; those tags
> track repository velocity, not capability. Anything below `v1.0` is incomplete,
> breakable, and unsuitable for moving real traffic. Watch for the first tagged
> release that drops this banner.

## Status

**Pre-alpha** (see the banner above). Working code paths exist for the wideband
robustness-floor waveform (`floor-wblo`, BPSK), LDPC forward-error-correction
primitives, the transmit and receive composition layers, two PTT keying backends
(serial-RTS and CM108 USB-HID), and a browser demo that runs the real DSP against
a synthetic HF channel. The OFDM throughput modes, the pilot-aided equalizer, and
the radio bring-up are under active construction. See
[Maturity](#maturity-what-is-and-isnt-proven) for the proven-vs-aspirational
breakdown.

## What it is

When the internet is gone, amateur-radio operators move email-style traffic over
high-frequency (shortwave) radio. A *data modem* is the layer that makes this
possible — it turns bytes into audio a transceiver can transmit on single
sideband, and turns the received audio back into bytes through noise, fading, and
a channel only a few kilohertz wide. On Linux the established HF modems are
[VARA](https://rosmodem.wordpress.com/) (proprietary, x86-Windows-only, run under
WINE) and [ARDOP](https://github.com/Rhizomatica/ardopcf) (open source).

Sonde is a third option: a modem designed **clean-sheet** — from open
signal-processing foundations rather than by examining the internals of any
existing modem (no study of VARA, ARDOP, FLDigi, Trimode, Pat, or wl2k-go). It is
written in Rust, structured as a Cargo workspace, and licensed AGPLv3-only. Sonde
is developed alongside its sibling project **Tuxlink**, a native Linux Winlink
client (private), which is the modem's first consumer.

## Try it in your browser

The [interactive demo](demo/) compiles **Sonde's real DSP to WebAssembly** and
runs it against a *simulated* HF channel. Drag the SNR and multipath sliders and
watch the modem deliver an emergency SITREP: a spectral waterfall, a TX|RX packet
console, a progressively-revealed recon photo, and live link telemetry.

> [!NOTE]
> **Simulated channel · software DSP · not on-air.** The modulation and
> demodulation are real; the channel is synthetic. No radio is keyed and nothing
> is transmitted. The page is fully static — see [`demo/`](demo/) to build and
> host it yourself.

## Maturity: what is and isn't proven

Sonde is honest about its edges:

- **Validated.** `floor-wblo` — the wideband low-density robustness floor (BPSK)
  — encodes and decodes end-to-end, and recovers payloads cleanly on a clean
  (AWGN-only) simulated channel. The demo exercises exactly this path.
- **In progress.** The OFDM throughput ladder (QPSK / 16-QAM / 64-QAM with
  per-subcarrier bit-loading) and LDPC FEC exist as library code but are not yet
  wired end-to-end. The pilot-aided equalizer is unbuilt, so Watterson-fading
  conditions in the demo intentionally degrade — shown on purpose, to illustrate
  *why* channel adaptation matters.
- **Operator-pending (Part 97).** On-air operation over a real radio is
  designed but not yet performed, and is the licensee's to carry out. Sonde never
  transmits without explicit, per-invocation operator consent — see
  [Amateur radio / Part 97](#amateur-radio--part-97).

## Not yet shipped

- **The throughput modes.** Only the BPSK floor decodes end-to-end today; the
  faster OFDM/QAM modes appear in the demo's picker as *pending*.
- **The equalizer.** Without it, multipath/fading conditions are expected to
  fail. Adaptation is the next major PHY milestone.
- **On-air anything.** No transmission path has been run over a real radio.
- **Published crates.** Nothing is on crates.io yet; build from source.

## Workspace

Sonde is a Cargo workspace. The library crates:

| Crate | Role |
|---|---|
| [`sonde-phy`](crates/sonde-phy/) | PHY waveform layer — OFDM main family, the wideband low-density and narrow-FSK robustness floors, sync (preamble / carrier-offset / symbol-timing), constellations, bit-loading |
| [`sonde-fec`](crates/sonde-fec/) | LDPC forward error correction |
| [`sonde-phy-runtime`](crates/sonde-phy-runtime/) | Production `PhyTransport` runtime — the half-duplex pump that drives the PHY over a pluggable waveform + radio seam |
| [`sonde-tx`](crates/sonde-tx/) | Payload → PHY → PTT + audio composition (**keys a real radio**) |
| [`sonde-rx`](crates/sonde-rx/) | Capture → demod → BER composition |
| [`sonde-rig-rts`](crates/sonde-rig-rts/) | Serial-RTS PTT keying primitive |
| [`sonde-rig-cm108`](crates/sonde-rig-cm108/) | CM108-family USB-HID PTT keying primitive |
| [`hf-channel-sim`](hf-channel-sim/) | Vendored AGPLv3 HF channel simulator (Watterson fading) for tests and benchmarks |
| [`sonde-wasm`](crates/sonde-wasm/) | Real Sonde DSP over a simulated channel, exported to JavaScript/WASM — powers the demo |
| [`sonde-demo-builder`](crates/sonde-demo-builder/) | Turns an image into the demo's SITREP payload |

Architecture decisions are recorded as ADRs under [`docs/adr/`](docs/adr/); the
git workflow (main-as-integration, per-task branches in worktrees, no-squash
merges) lives in [`docs/git-strategy.md`](docs/git-strategy.md). The agent
workflow, ethos, and safety rails this project operates under are in
[CLAUDE.md](CLAUDE.md).

## Build

Build from source with a stable Rust toolchain (1.75+):

```bash
cargo build  --workspace
cargo test   --workspace
cargo clippy --workspace --all-targets -- -D warnings   # warnings are errors
cargo fmt --all --check
```

The audio crates link against ALSA through `cpal`, so a fresh Linux host needs
the system packages **`libasound2-dev`** and **`pkg-config`** before any cargo
step:

```bash
sudo apt-get install -y libasound2-dev pkg-config
```

## Amateur radio / Part 97

The transmit path ([`sonde-tx`](crates/sonde-tx/) and the rig/PTT crates) can key
a physical transmitter. Keying a transmitter under an amateur callsign is a
responsibility that rests with a licensed operator, who bears responsibility for
ensuring every transmission complies with Part 97 of the FCC rules (or the
equivalent regulations in the operator's jurisdiction).

**Sonde prohibits automated or agent-initiated transmissions absent explicit,
per-invocation operator consent.** No test, CI job, or agent ever keys a radio;
code may *prepare* a transmission, and a human licensee *runs* it. The canonical
rule lives in [CONTRIBUTING.md](CONTRIBUTING.md) and [CLAUDE.md](CLAUDE.md).

## License

[AGPL-3.0-only](LICENSE). Copyright 2026 Cameron Zucker and contributors.

## Contributing

Contributions are welcome — start with [CONTRIBUTING.md](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md). To report a security issue privately, see
[SECURITY.md](SECURITY.md). The build prerequisites, commit discipline, and
project ethos are documented in [CLAUDE.md](CLAUDE.md).
