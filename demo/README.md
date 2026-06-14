# Sonde Interactive Demo

A static, link-shareable web page that runs **Sonde's real DSP in the browser
via WebAssembly**. Drive a simulated HF channel (SNR + multipath condition) and
watch the modem deliver an EmComm SITREP — a 3D spectral waterfall, a TX|RX
packet console, a progressively-revealed recon photo, and live link telemetry.

> **Simulated channel · software DSP · not on-air.** Real modulation and
> demodulation run over a *synthetic* HF channel. No radio is keyed; this page
> transmits nothing. See the honesty banner in the page itself.

## What's real, what's pending

- **`floor-wblo`** (the wideband robustness-floor waveform, BPSK) is the only
  mode implemented end-to-end today, and it decodes cleanly **only on the Ideal
  (AWGN-only) condition**.
- **Watterson multipath** conditions (Good / Moderate / Poor / Flutter)
  intentionally degrade or fail — the receiver has no equalizer yet. This is
  shown on purpose, to illustrate *why adaptation matters*.
- The faster OFDM-Main / QAM modes appear in the picker as **"pending"** until
  the parallel PHY work lands.

## Layout

```
demo/
  build-assets.sh        # fetch image -> payload.bin/offsets -> wasm bundle
  package.json           # Playwright smoke-test harness (dev only)
  playwright.config.mjs
  site/                  # <-- the shippable static site
    index.html
    css/app.css
    js/                  # engine.js (wasm API) + view modules + wiring
    vendor/three.module.js
    pkg/                 # wasm-bindgen output (generated; gitignored)
    assets/              # payload.bin + offsets + credit (committed); source.jpg (generated)
  tests/smoke.spec.mjs   # browser smoke test
```

## Prerequisites

- **Rust + cargo** and the `wasm32-unknown-unknown` target
  (`rustup target add wasm32-unknown-unknown`).
- **`wasm-bindgen-cli`**, version-matched to the `wasm-bindgen` crate
  (currently `0.2.125`): `cargo install wasm-bindgen-cli --version 0.2.125`.
- **`curl`** and **`jq`** (the asset script fetches a public-domain NOAA image
  via the Wikimedia Commons API).
- A static file server — e.g. Python's `http.server` (no extra install).
- For the smoke test only: **Node.js + npm**.

## Build the assets

```bash
./demo/build-assets.sh
```

This (re-)generates:

- `site/assets/payload.bin` + `payload.offsets.json` — the SITREP payload
  (committed, so the page works on a host without a rebuild).
- `site/assets/source.jpg` + `source-credit.txt` — the source image + credit
  (gitignored / generated).
- `site/pkg/sonde_wasm.js` + `sonde_wasm_bg.wasm` — the wasm bundle (gitignored;
  **must be built** before serving — see hosting note below).

## Serve locally

```bash
cd demo/site && python3 -m http.server 8080
# then open http://localhost:8080
```

**`file://` will not work** — ES modules and `fetch()` of the payload/wasm
require a real HTTP origin (CORS). Always serve over HTTP.

## Host on GitHub Pages

The page is fully static. Point Pages at `demo/site/` (or copy that directory
to your Pages branch). Because `site/pkg/` is gitignored, the wasm bundle must
be produced **before** publishing: run `./demo/build-assets.sh` in your Pages
build/CI step, or build locally and copy `site/pkg/` into the published tree.
`payload.bin` + `payload.offsets.json` are committed, so only `pkg/` needs to be
present at publish time.

## Run the smoke test

```bash
cd demo
npm install                      # installs @playwright/test
npx playwright install chromium  # one-time browser download
npx playwright test              # serves site/ automatically + runs the test
```

The config starts a static server for `site/` (reusing one already on `:8080`)
and runs two checks: a clean link at high SNR / Ideal (BER 0.00% on
`floor-wblo`, no page errors), and graceful degradation at low SNR / Poor
multipath. Override the target with `DEMO_BASE=http://host:port`.

## Image attribution

The demo's recon photo is **NOAA Emergency Response Imagery** of the 2020
Hurricane Laura response — a work of the U.S. Government, in the **public
domain**, retrieved via Wikimedia Commons. The exact source line is written to
`site/assets/source-credit.txt` by the build script and shown in the page
footer.

## License

Sonde is **AGPLv3-only**. This demo is part of the Sonde workspace.
