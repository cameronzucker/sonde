# Sonde Interactive Adaptive-Modem Demo — Design Spec

- **Date:** 2026-06-14
- **Status:** Draft (design approved in brainstorming; pending written-spec review)
- **Branch context:** authored alongside `sonde-fm-validation`; demo work expected on its own branch
- **Related:** `2026-06-14-sonde-fm-validation-design.md` (on-air validation runbook)

## 1. Summary

A single static web page — hostable on GitHub Pages — that runs **Sonde's real DSP in
the browser via WebAssembly** to demonstrate the modem as an *adaptive* system. The
operator manipulates a simulated HF channel (SNR, multipath condition) and watches Sonde
either **adapt automatically** (pick bandwidth/constellation/FEC for the conditions) or
respond to **manual** mode selection, while a fixed payload — an EmComm SITREP with a
compressed drone photo — is sent over the simulated link. The page renders a real 3D
waterfall of the modulated, channel-impaired audio, a per-symbol packet inspector, the
recovered image, and live link statistics (throughput, time-to-deliver, BER).

The engine is **not throwaway demo code**: the channel model, the SNR-driven mode
decision, and a WASM build of the DSP are all capabilities Sonde benefits from
independently.

## 2. Goals / Non-goals

**Goals**
- Show, interactively and in real time, *how Sonde reacts* to changing channel conditions.
- Drive the demo with the **actual** `sonde-phy` + `hf-channel-sim` code, not a mock.
- Be fully static (no backend) so it hosts on GitHub Pages and runs by double-clicking locally.
- Produce a reusable `sonde-wasm` build of the DSP as a side benefit.
- Tell an honest EmComm story: a recon photo pushed through a narrow, noisy channel,
  error-checked end to end.

**Non-goals**
- No real RF, no on-air transmission, no hardware. The channel is simulated.
- No Winlink/VARA/ARDOP protocol (Sonde is clean-sheet per ADR 0014). The payload is a
  Winlink-*style* text blob — opaque bytes to the PHY.
- No claim of broadband speed. Floor mode is the robust mode (~1.35 kbps); full-speed
  trades robustness for rate, which is precisely the adaptation being shown.
- Image codecs do not run in WASM (handled offline; see §5.3).

## 3. The payload (the story)

A fixed EmComm SITREP, built once offline, shipped as a static asset:

```
To: EMCOMM-NET
From: <operator callsign>
Subject: SITREP — Disaster Area Recon
Date: 2026-06-14 18:30Z
Position: 34-12.34N / 118-29.10W (DM04xf)

<situational commentary paragraph>

--- attachment: recon.jpg (~5000 bytes) ---
<raw compressed JPEG bytes>
```

- Source image is **operator-provided** (dropped at a known path; see §9), resized and
  re-compressed to **~5 KB** so the robust-mode link runs ~30 s and the waterfall is
  visually rich without dragging.
- The builder records a **field-offset map**: which payload byte ranges are header,
  commentary, and image. The packet inspector uses this to color-code each symbol's bytes
  by the field it carries.

## 4. User experience

**Levers (controls)**
- **SNR (dB)** — slider; sets AWGN level applied by the channel.
- **Channel condition** — clean / moderate / poor (maps to `ChannelCondition` +
  Watterson multipath/fading).
- **Mode control toggle:**
  - **Auto:** operator sets only the channel; Sonde measures it and *picks* the mode. The
    UI calls out the decision, e.g. *"measured SNR 14 dB → ofdm-mid / QPSK."*
  - **Manual:** operator pins bandwidth + constellation directly and observes the result —
    including watching it fail at low SNR.

**What's shown (reacts live to the levers)**
- **3D waterfall** — real STFT of the real modulated + channel-impaired audio, cropped to
  the occupied band, with orbit/zoom and a playback "now" plane.
- **Packet inspector** — current symbol index, its decoded bytes (hex + ASCII),
  field/color mapping, per-symbol decode-OK indicator, and a message-assembly progress bar.
- **Recovered image** — fills in as symbols arrive; clean and fast at high SNR, slow or
  visibly corrupted at low SNR.
- **Live stats** — chosen mode + constellation, bytes/symbol, symbol count,
  **time-to-deliver**, throughput (bps), measured SNR, and **BER**.
- **Comparison affordance (Phase 3)** — pin two modes (e.g. floor-wblo vs a full-speed
  mode) at the same channel and compare time-to-deliver side by side.

**Liveness (Phase 1):** each lever change re-runs the link once (debounced); the result
animates as a playback sweep. Continuous streaming (perpetual rolling channel) is Phase 3.

## 5. Architecture — four components, one stable WASM boundary

### 5.1 Sonde core (Rust — mostly exists)
- `sonde-phy`: OFDM transmitter/receiver, floor modes, `ModeTable` adaptation decision.
- `hf-channel-sim`: `WattersonChannel`, `AwgnGenerator`, `ChannelCondition`,
  `estimate_subcarrier_snr` (the receiver's own SNR measurement Auto adapts on).
- Parallel work adds QAM bit-loading to the OFDM-Main modes (see §7).

### 5.2 `sonde-wasm` facade (new — `cdylib` + wasm-bindgen)
A small, stable JS-facing API over the real DSP. It must NOT pull in `image`/file-IO
crates (excluded from the wasm build).

```
init(payload_bytes, field_offsets)              // load the shipped SITREP payload
list_modes() -> [ModeInfo]                       // {id, family, label, constellation,
                                                 //  bandwidth_hz, data_bytes_per_symbol,
                                                 //  implemented: bool}
recommend_mode(snr_db) -> mode_id                // wraps ModeTable::resolve(MainAuto, snr),
                                                 //  clamped to implemented modes
estimate_snr(samples) -> f32                     // exposes estimate_subcarrier_snr
run_link(mode_id, {snr_db, condition, seed}) -> LinkResult
```

`LinkResult`:
```
recovered_ok: bool
ber: f32
measured_snr_db: f32
payload_len, preamble_samples, symbol_size_samples
time_to_deliver_s, throughput_bps
symbols: [ {idx, sample_start, sample_end, t_start_s, t_end_s,
            bytes: [u8], byte_range, field: header|body|image, decoded_ok} ]
spectrogram: {freqs_hz: [f32], times_s: [f32], rows, cols,
              mag_db_q: [u8]}            // row-major, quantized 0–255 to keep transfer small
recovered_image_jpeg_b64: string         // what actually came out (may be corrupt at low SNR)
```

STFT computed in WASM via the existing FFT dependency; magnitudes quantized to `u8`.

### 5.3 Offline payload builder (new Rust bin)
Image processing does not belong in WASM. This runs once, offline:
operator image → ~5 KB JPEG (quality-search loop with the `image` crate) → SITREP payload
+ field-offset map → emitted as a static `payload.bin` (+ offsets) asset the WASM consumes.

### 5.4 Frontend (new — `demo/site/`)
Three.js waterfall (vendored, not CDN — works offline and on Pages identically), the lever
panel, Auto/Manual toggle, adaptation readout, packet inspector, recovered-image panel,
live stats, and a short "what you're seeing" explainer. Loads the WASM bundle + payload
asset. Fully static.

## 6. Data flow

```
operator image
   └─[offline builder]→ payload.bin + field offsets  (shipped as static asset)

[browser, live]
 lever change → JS (debounced)
   → wasm.recommend_mode(snr)            (Auto only)
   → wasm.run_link(mode_id, channel)
       → encode (framing + preamble) → Watterson/AWGN channel → decode + BER
       → STFT → quantized spectrogram grid
   → LinkResult → Three.js waterfall + packet inspector + image + stats render/animate
```

## 7. Mode integration contract (for the parallel full-speed session)

The demo is mode-*parametric*; it only needs modes to be reachable through one generic
framing entry point. To make the full-speed work slot in with zero frontend/facade change:

1. Implement QPSK/16-QAM mapping & demapping in `OfdmTransmitter::modulate_one_symbol` /
   `OfdmReceiver::demodulate_one_symbol` for `bits_per_subcarrier ∈ {1, 2, 4}`.
2. Give `ofdm-narrow` / `ofdm-mid` / `ofdm-wide` real `bits_per_subcarrier` profiles.
3. **Extract the multi-symbol + preamble framing off `WidebandLowDensityFloor`** into a
   mode-generic component parameterized by `OfdmParams` + the `bits_per_subcarrier` vector,
   so floor (BPSK) and full-speed (QAM) share one framing/length-header/preamble path.
   `run_link` then calls a single generic `transmit_framed`/`receive_framed`.
4. **Acceptance:** byte-exact clean-channel round-trip per OFDM mode, plus a monotone,
   sane BER-vs-SNR curve through `hf-channel-sim`.

`ModeTable::resolve(MainAuto, snr)` thresholds (`<0: floor-wblo · <10: ofdm-narrow ·
<20: ofdm-mid · ≥20: ofdm-wide`) are provisional (Phase 11 re-pegs from sweeps); the demo
surfaces them as provisional and clamps Auto to implemented modes until §8 Phase 2.

## 8. Phasing

- **Phase 0 — payload builder.** Image → SITREP payload + offsets. Independent; ships now.
- **Phase 1 — WASM + frontend over what exists today.** Floor mode across the SNR range,
  Auto recommendation, full interactive UI (levers, waterfall, inspector, image, stats,
  recompute-on-change playback). A complete, shippable interactive demo with **zero
  dependency** on the parallel work.
- **Phase 2 — full-speed lights up automatically.** When QAM modes land (§7), they appear
  via the data-driven mode list; high-SNR Auto and Manual high-rate options activate with
  no frontend changes.
- **Phase 3 — polish.** Continuous "live link" animation (rolling channel realizations),
  deeper multipath levers, two-mode comparison overlays, README preview via Playwright
  screenshot.

No pause required: Phase 0/1 and the parallel full-speed work proceed concurrently and
meet at the §7 interface.

## 9. File / layout plan

```
demo/
  README.md                 # story + link to live Pages demo + static preview assets
  assets/source.<ext>       # operator-provided image  (path the operator drops into)
  builder/                  # Rust bin: image → payload.bin + offsets
  site/                     # static frontend (committed)
    index.html
    vendor/three.min.js
    pkg/                    # wasm-bindgen output (sonde-wasm)
    payload.bin             # generated asset
crates/
  sonde-wasm/               # cdylib facade over sonde-phy + hf-channel-sim
```

Core modem crates are untouched except the §7 generic-framing extraction (shared by both
sessions). Demo code stays out of the core crates.

## 10. Honesty guardrails (surfaced in-UI)

- Banner: *"Simulated Watterson/AWGN channel · software DSP · not on-air. Adaptation
  thresholds provisional."*
- Until QAM lands, high-SNR Auto clamps to the best implemented mode and labels the rest
  "pending."
- Stats always show that throughput/BER are clean-room simulation results.

## 11. Testing strategy

- **Rust (core + facade):** loopback recovers payload byte-exact at high SNR (BER 0);
  payload builder field offsets correct; spectrogram grid dimensions as specified;
  `run_link` returns monotone BER-vs-SNR for a fixed mode.
- **WASM:** node-harness test that `list_modes`, `recommend_mode`, and `run_link`
  round-trip a small payload and return well-formed `LinkResult`.
- **Frontend:** Playwright smoke test — page loads, WASM initializes, dragging SNR changes
  BER/throughput, Auto readout updates, recovered image renders at high SNR and degrades at
  low SNR.

## 12. Risks / open items

- **WASM re-encode latency** per lever tick (~5 KB → ~570+ symbols + FFTs): debounce;
  expected sub-second, but budget a perf check; reduce payload or spectrogram resolution if
  needed.
- **wasm-bindgen build integration** into the workspace (separate `cdylib` target; exclude
  `image`/file-IO from the wasm build; confirm the FFT dep compiles to `wasm32`).
- **Continuous live-link** is the heaviest piece → deliberately Phase 3, not blocking.
- **Auto resolving to placeholder modes** today → "implemented-modes-only" clamp until
  Phase 2.
- **Spectrogram payload size** → quantize magnitudes to `u8`; decimate time frames.
```
