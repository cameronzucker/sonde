# Sonde Demo Frontend (Manual-Levers Mode) — Design Spec

- **Date:** 2026-06-14
- **Status:** Draft (design approved in brainstorming; pending written-spec review)
- **Parent spec:** `2026-06-14-sonde-interactive-adaptive-demo-design.md` (component #4)
- **Depends on:** `sonde-wasm` engine + `sonde-demo-builder` (epic sonde-669, task sonde-669.1, PR #4)
- **Tracking:** new task under epic sonde-669 (frontend); follow-on map mode is **sonde-669.2** (out of scope here)

## 1. Summary

A single, static, link-shareable web page that drives Sonde's real DSP (compiled to
WebAssembly) from interactive controls. The operator sets a simulated HF channel (SNR +
condition) and an Auto/Manual mode choice, and watches a fixed EmComm SITREP payload
(position report + commentary + ~5 KB drone photo) go over the link: a live 3D waterfall,
a per-symbol packet inspector, the recon image filling in, and live BER/throughput/ETA
stats. Built with the **frontend-design skill** for visual sophistication, in the
"modern professional dark dashboard" direction (Tuxlink-vein, chosen in brainstorming as
style **B**).

## 2. Scope / Non-goals

**In scope (this spec):** the **manual-levers** demo over the existing WASM engine — the
foundation both demo modes share.

**Out of scope (separate sub-projects):**
- **Map-linked "propagation game" mode** — place two points, pick band/antenna, drive the
  levers from an HF-propagation prediction. Tracked as **sonde-669.2**; will study
  Tuxlink's *Find a Station* (`/home/administrator/Code/tuxlink`) and gets its own
  brainstorm/spec. The engine seam (`run_link(payload, mode, snr, condition, seed)`) already
  supports it — that mode is purely a second input source producing the same (snr, condition).
- **Full-speed QAM modes** — shown as "pending" until the parallel PHY work lands.
- **Per-symbol live decode** and **corrupted-image rendering** — need engine API additions
  (noted as future).

## 3. Engine contract consumed (already built)

From `crates/sonde-wasm` (built to `demo/site/pkg/` via `wasm-bindgen --target web`):
- `list_modes() -> string` — JSON `ModeInfo[]` (`id, family, constellation, bandwidth_hz,
  data_bytes_per_symbol, implemented`).
- `recommend_mode(snr_db: number) -> string` — plain mode-id string (NOT JSON), clamped to
  implemented modes.
- `run_link(payload: Uint8Array, offsets_json: string, mode_id: string, snr_db: number,
  condition: string, seed: number) -> string` — JSON `LinkResult` or `{"error": "..."}`.

`LinkResult` fields the UI renders: `mode_id, recovered_ok, ber, measured_snr_db,
payload_len, preamble_samples, symbol_size_samples, total_samples, time_to_deliver_s,
throughput_bps, symbols[] {idx, sample_start/end, t_start_s/t_end_s, bytes[], byte_start,
byte_end, field}, spectrogram {rows, cols, freqs_hz[], times_s[], mag_q[] (row-major u8)}`.

Static assets produced by `sonde-demo-builder`: `payload.bin` + `payload.offsets.json`
(field offsets: header / body / image ranges). Requires an operator-provided source image
at build time (a committed sample image is an acceptable default; see §10).

**Engine limitation to surface honestly:** only `floor-wblo` decodes cleanly, and only on
the AWGN-only `"none"` condition; Watterson conditions (`good/moderate/poor/flutter`)
return `recovered_ok:false` because the floor receiver has no equalizer. The UI frames this
as *why adaptation matters* (see §8).

## 4. Information architecture (single page)

- **Hero — 3D waterfall** (Three.js, vendored): time × frequency × magnitude surface from
  `LinkResult.spectrogram`; orbit/zoom; viridis colormap; 250–2700 Hz band; a translucent
  **"now" plane** that sweeps during playback.
- **Lever panel:** SNR slider (dB); channel-condition selector (`none`, `good`, `moderate`,
  `poor`, `flutter`); **Auto ⇄ Manual** toggle (Manual reveals a mode picker built from
  `list_modes()`, with non-`implemented` modes shown disabled/"pending").
- **Adaptation readout:** in Auto, "measured SNR N dB → Sonde chose <mode> / <constellation>."
- **Packet inspector:** the current symbol's bytes (hex) color-coded by `field`
  (header / body / image), with byte range and a message-assembly progress bar.
- **Recovered-image tile:** the recon photo, progressively revealed as playback advances
  (see §6).
- **Stats bar:** mode, constellation, BER, throughput (bps), time-to-deliver (s),
  measured SNR.
- **Explainer + honesty banner** (see §8).

## 5. Interaction model

- **Recompute-on-change + playback** (parent spec, Phase 1): any lever change triggers a
  **debounced** `run_link`; the returned `LinkResult` then **animates** as a playback sweep
  — the waterfall "now" plane advances, and the inspector + image fill in symbol-by-symbol
  along `symbols[].t_start_s`. Play/pause/scrub + speed control over the timeline.
- **Auto:** UI calls `recommend_mode(snrSlider)` → uses that mode_id in `run_link` with the
  current (snr, condition). The adaptation readout shows the decision.
- **Manual:** UI calls `run_link` with the user-picked mode_id directly. Picking a
  non-implemented mode shows the engine's `{"error": ...}` surfaced as a friendly "this mode
  is pending" state (no crash).
- **Determinism:** a fixed `seed` (UI may expose a "re-roll" that bumps the seed) so results
  are reproducible and shareable.

## 6. Recovered-image handling (Phase 1)

`run_link` does not return recovered bytes; it returns `recovered_ok` + `ber`. Therefore:
- **On `recovered_ok` (BER 0):** recovered bytes equal the original, so the tile renders the
  **known image** decoded from `payload.bin`'s image byte-range (from `payload.offsets.json`),
  revealed progressively in step with the playback marker (the "image arriving over radio"
  effect).
- **On failure (multipath / low SNR):** show a "decode failed — no clean image recovered"
  state (e.g., static/noise placeholder) rather than a fake image.
- Rendering *partially corrupted* bytes would require an engine API that returns the
  recovered buffer — explicitly **future**, not Phase 1.

## 7. Visual approach

- Built with the **frontend-design skill** during implementation to achieve a distinctive,
  sophisticated result — not generic AI-dashboard aesthetics. Direction: **modern
  professional dark dashboard** (style B, Tuxlink-vein): dark surfaces, rounded cards, pill
  toggles, a vibrant scientific colormap for the waterfall, system-ui for chrome +
  monospace for data readouts. Exact tokens are the frontend-design skill's to choose within
  this direction (no exact Tuxlink token match required).
- The 3D waterfall is **Three.js, vendored locally** (works offline + on GitHub Pages
  identically).

## 8. Honesty guardrails (in-UI)

- Persistent banner: **"Simulated Watterson/AWGN channel · software DSP · not on-air."**
- Channel conditions other than `none` visibly degrade/fail recovery; the UI labels this as
  the floor receiver lacking equalization — *the reason adaptive modes exist* — rather than
  hiding it.
- Non-implemented modes are shown but disabled/"pending"; no fabricated success.
- Stats are labeled as clean-room simulation results.

## 9. Component / file structure

```
demo/
  site/
    index.html              # shell + frontend-design-produced markup/styles
    css/                    # styles (if not inlined by frontend-design output)
    js/
      main.js               # bootstrap: load wasm + payload assets, wire controls
      engine.js             # thin wrapper over sonde_wasm JS API (parse JSON, types)
      waterfall.js          # Three.js waterfall (build/update surface from spectrogram)
      inspector.js          # packet inspector rendering from symbols[]
      playback.js           # timeline/playback state machine + "now" marker
      image-reveal.js       # progressive image render from payload image-range
      controls.js           # levers, Auto/Manual, debounce
    vendor/three.min.js     # vendored Three.js
    pkg/                    # wasm-bindgen output (sonde_wasm.js + .wasm)
    assets/
      payload.bin           # generated by sonde-demo-builder
      payload.offsets.json  # generated
  README.md                 # how to build assets + run/host; link to live demo
```
Each JS module has one responsibility; `engine.js` is the only module that knows the wasm
API shape. Files stay focused (a large `main.js` is a smell to split).

## 10. Build & asset prerequisites

- Build the wasm bundle into `demo/site/pkg/` (`cargo build -p sonde-wasm --release
  --target wasm32-unknown-unknown` then `wasm-bindgen ... --target web`).
- Generate `payload.bin` + `payload.offsets.json` with `sonde-demo-builder` from a source
  image. **A committed sample "disaster-area" image** (license-clean) is the default so the
  demo builds without external input; the operator can swap in their own.
- Page is fully static: opens via `file://` (data inlined or fetched with a local-server
  note) **and** on GitHub Pages. Prefer loading `payload.bin`/`pkg` via `fetch` (works on
  Pages and `python -m http.server`); document the one-line local-serve command in the
  README since `file://` + `fetch` is restricted in some browsers.

## 11. Testing

- **Playwright smoke test:** page loads; wasm initializes; `list_modes`/`run_link` reachable;
  dragging SNR changes BER/throughput readouts; Auto readout updates; at high SNR + `none`
  the image renders and BER shows 0.00%; at a multipath condition the UI shows the
  degraded/failed state without crashing.
- **Module unit checks** where pure (e.g., colormap mapping, byte→hex formatting,
  field-color mapping) via a lightweight JS test runner or assertions in a headless page.

## 12. Risks / open items

- **`file://` + `fetch`** restrictions → document the local-serve command; consider inlining
  `payload.bin` as base64 if a true double-click experience is required (decide in the plan).
- **Three.js bundle size** (vendored) — acceptable for a demo asset.
- **Waterfall performance** for ~548-symbol captures — spectrogram is pre-decimated to
  ≤ 400 columns by the engine, so the mesh is bounded; verify smoothness.
- **`measured_snr_db` may be `NaN`** for tiny payloads — guard the display (show "—").
- **Sample image licensing** — pick a public-domain/again-license-clean aerial image, or
  generate a synthetic one, for the committed default.

## 13. Future (explicitly deferred)

Map-linked propagation-game mode (sonde-669.2), full-speed QAM modes, per-symbol live
decode coloring, corrupted-image rendering, continuous "live link" streaming animation.
