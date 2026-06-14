# Plan — Real ARDOP connected-mode testbench (sonde-imh.2)

**Goal:** replace the PHY-only one-way frame demo with the *real* ARDOP protocol —
two live ardopcf stations completing a CONNECT handshake, negotiating a data
rate/mode, and running ARQ — with their audio bridged through `hf-channel-sim` so
the operator can drive SNR/condition and watch a genuine connected session.

ardopcf is external/reference only (clean-sheet, ADR 0014). Virtual audio + WAV
only; **no radio is keyed**.

## Feasibility (de-risked 2026-06-14, agent raven-tamarack-pika)

- `snd-aloop` present at `/lib/modules/.../snd-aloop.ko.xz`; **passwordless sudo
  works** (`sudo modprobe snd-aloop`).
- ardopcf 1.0.4.1.3 enumerates ALSA devices via PortAudio (JACK errors are
  harmless fallback); accepts `-i`/`-o` device names with case-insensitive
  substring matching. Devices seen: `plughw:CARD=Loopback,DEV=0|1`, etc.
- Full ARQ host protocol over TCP host_port (default 8515): `MYCALL`, `LISTEN`,
  `ARQCALL`, `ARQBW`, `ARQTIMEOUT`, `PROTOCOLMODE`, `ARQState`, `ARQBandwidths`.
- No Python ALSA lib needed: `arecord | filter | aplay` moves PCM (alsa-utils
  present). The channel filter is a stdin→stdout S16LE processor.

## Audio topology (the crux — solved)

snd-aloop is bidirectional per card: playback→dev0 is captured on dev1, AND
playback→dev1 is captured on dev0 (independent substreams). So ONE card gives a
full duplex A↔bridge cable. Use **two cards** to keep A and B independent and
avoid subdevice auto-assignment races:

```
sudo modprobe -r snd-aloop 2>/dev/null
sudo modprobe snd-aloop index=10,11 pcm_substreams=1 id=AldA,AldB
```

- **Station A**: `-i plughw:CARD=AldA,DEV=0 -o plughw:CARD=AldA,DEV=0`
- **Bridge side A**: `arecord -D plughw:CARD=AldA,DEV=1` (gets A's TX),
  `aplay -D plughw:CARD=AldA,DEV=1` (feeds A's RX)
- **Station B**: `-i plughw:CARD=AldB,DEV=0 -o plughw:CARD=AldB,DEV=0`
- **Bridge side B**: `arecord -D plughw:CARD=AldB,DEV=1`, `aplay -D plughw:CARD=AldB,DEV=1`

Bridge = two pipelines (ARDOP is half-duplex, so only one carries signal at a time):
```
arecord AldA/dev1 | channel_filter --snr S --cond C | aplay AldB/dev1   # A→B
arecord AldB/dev1 | channel_filter --snr S --cond C | aplay AldA/dev1   # B→A
```
ardopcf audio is mono 12 kHz S16LE — match `arecord/aplay -f S16_LE -r 12000 -c1`.

## Build steps

1. **`channel_filter.py`** — read S16LE frames from stdin, add AWGN at target SNR
   (and optional Watterson taps for good/moderate/poor/flutter), write to stdout.
   Real-time, small block size (~1024 samples) for low latency. Reuse the
   `hf-channel-sim` math; a numpy AWGN is the MVP, Watterson a follow-up. SNR is
   relative to the measured input RMS during a TX burst (squelch silence).
2. **`testbench.py`** — orchestrator:
   - (re)load snd-aloop two-card config (idempotent; skip if already correct).
   - launch ardopcf A and B (`--nologfile`, host ports 8515/8525), wait for
     "listening for host connection".
   - launch the two bridge pipelines at the requested SNR/condition.
   - open host TCP to each: `MYCALL` (A=call sign, B=call sign), `PROTOCOLMODE ARQ`,
     `ARQBW <max>`; B: `LISTEN TRUE`; A: `ARQCALL <B> <repeats>`.
   - watch async host replies (`PTT`, `STATUS`, `ARQState`, `CONNECTED`,
     `NEWSTATE`, mode/`ARQBW` negotiation) and the data port (8516/8526) for the
     transferred payload; send the image as ARQ data from A; collect B's received
     bytes; record negotiated mode, retries, throughput, connect/disconnect.
   - tear everything down cleanly (kill children, leave snd-aloop loaded).
3. **Backend**: new `/api/session` (supersedes `/api/run`'s fire-and-forget) that
   runs one connected session at the given SNR/condition and returns a timeline:
   handshake events, negotiated mode(s), per-block ARQ outcomes, final image bytes,
   throughput, connect→disconnect duration. Audio: capture the on-air audio at the
   bridge (tee the filter output to a WAV) for the live waterfall.
4. **Frontend**: show the connected session — a CONNECT/handshake readout, the
   negotiated mode (replacing the heuristic Auto ladder), an ARQ progress/retry
   view (the frame-log becomes an ARQ-block log), the recon image rebuilding from
   delivered blocks, and the live audio-driven waterfall (already built).

## Risks / open questions

- **Real-time latency on the Pi**: two ardopcf + two arecord|filter|aplay + numpy
  per block. ARQ has timing (ACK turnaround, `ARQTIMEOUT`); excessive bridge
  latency could break the handshake. Mitigate: small blocks, raise `ARQTIMEOUT`,
  measure. This is the #1 thing to validate in the first spike.
- **SNR calibration**: define SNR vs the ARDOP signal RMS in the passband; gate
  silence so noise isn't added to dead air (or add a constant noise floor always —
  more realistic). Feeds sonde-imh.1's deferred SNR-axis calibration.
- **PTT/CODEC**: stations run CODEC with no PTT device; ardopcf keys via the host
  (we drive it) — confirm TX actually emits to the loopback without a PTT line.
- **Determinism**: snd-aloop free-runs on its own clock; if A and B drift, ardopcf
  resamples. Confirm stable sync over a multi-minute session.

## Status at write time

- Frontend re-anchor + live audio waterfall + recognizable recon image: committed
  (`f068a56`) on `sonde-imh.1/ardop-live-backend`; viewable at the LAN URL.
- This testbench: feasibility proven, topology designed, **not yet implemented**.
- First milestone: a single successful CONNECT + small data transfer through the
  bridge (validates the real-time-latency risk). Build out from there.
