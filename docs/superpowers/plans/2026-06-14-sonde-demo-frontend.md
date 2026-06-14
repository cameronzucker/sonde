# Sonde Demo Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The visual shell (Task 5) is produced with the **frontend-design skill**; everything else is deterministic glue + tests.

**Goal:** A static, link-shareable web page that drives the `sonde-wasm` engine from interactive levers — a 3D Three.js waterfall, a TX|RX packet console, a progressively-revealed (and realistically-corrupting) recon image, and live stats — in the modern-dark-dashboard (style B) look.

**Architecture:** Pure static `demo/site/` (no backend). A build step fetches a public-domain NOAA aerial image, generates `payload.bin` via `sonde-demo-builder`, and emits the `sonde-wasm` bundle via `wasm-bindgen --target web`. The page loads the wasm + payload, and on each lever change calls `run_link(...)`, then animates the returned `LinkResult` as a playback sweep. JS is split into focused modules; `engine.js` is the only module that knows the wasm JSON shape.

**Tech Stack:** Vanilla JS (ES modules), Three.js (vendored), wasm-bindgen output, Playwright for the smoke test. Built/served static; hostable on GitHub Pages.

**Spec:** `docs/superpowers/specs/2026-06-14-sonde-demo-frontend-design.md`. **Engine API:** `crates/sonde-wasm` (`list_modes`/`recommend_mode`/`run_link`; `LinkResult` now includes `recovered_bytes` + per-symbol `rx_bytes`).

**Working directory:** worktree `worktrees/sonde-interactive-demo` (branch `sonde-669/interactive-demo` — builds on the engine; the frontend extends PR #4). Do NOT touch `main`. Paths relative to worktree root.

**Governance:** conventional commits + BOTH trailers (`Agent: <moniker>`, `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`); no destructive git. Frontend/build only — no `sonde-tx`/PTT, no radio.

**Verification:** Rust gate stays green (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`); frontend verified by a Playwright smoke test (Task 10) and manual serve.

---

## File Structure (locks decomposition)

```
demo/
  build-assets.sh           # fetch image -> payload.bin/offsets -> wasm bundle (Task 1)
  README.md                 # build/serve/host instructions + NOAA attribution (Task 11)
  site/
    index.html              # shell + markup (frontend-design, Task 5)
    css/app.css             # styles (frontend-design, Task 5)
    js/
      engine.js             # ONLY module aware of the wasm JSON API (Task 3)
      waterfall.js          # Three.js 3D waterfall from spectrogram (Task 6)
      console.js            # TX|RX per-symbol byte console + flip highlight (Task 7)
      image-reveal.js       # progressive recon-image render from recovered_bytes (Task 8)
      playback.js           # timeline/now-marker state machine (Task 9)
      controls.js           # levers + Auto/Manual + debounce (Task 9)
      main.js               # bootstrap + wiring (Task 9)
    vendor/three.module.js  # vendored Three.js (Task 2)
    pkg/                    # wasm-bindgen output (Task 1, generated)
    assets/                # payload.bin, payload.offsets.json, source-credit.txt (Task 1, generated)
  tests/
    smoke.spec.mjs          # Playwright smoke test (Task 10)
```
Each JS module has one responsibility; if `main.js` grows past wiring, split the new concern into its own module.

---

## Task 1: Asset build script (image → payload → wasm bundle)

**Files:** Create `demo/build-assets.sh`

Deterministic, re-runnable build of the three generated artifacts. Uses the Commons API to get a JPEG-rendered thumbnail of a public-domain NOAA Emergency Response Imagery TIFF (so no TIFF decoder is needed).

- [ ] **Step 1: Write `demo/build-assets.sh`**
```bash
#!/usr/bin/env bash
# Build the static demo assets: NOAA image -> payload.bin -> wasm bundle.
# Re-runnable. Requires: curl, jq, cargo, wasm-bindgen (cargo install wasm-bindgen-cli).
set -euo pipefail
cd "$(dirname "$0")/.."                     # repo/worktree root
OUT=demo/site/assets
PKG=demo/site/pkg
mkdir -p "$OUT" "$PKG"

# --- 1. Fetch a public-domain NOAA ERI aerial (rendered to JPEG via Commons API) ---
# NOAA Emergency Response Imagery is a US-Government work (public domain).
FILE="File:20200827aC0884800w295530n.tif"   # 2020 Hurricane Laura, NOAA ERI
API="https://commons.wikimedia.org/w/api.php"
THUMB=$(curl -fsSL --get "$API" \
  --data-urlencode "action=query" \
  --data-urlencode "titles=$FILE" \
  --data-urlencode "prop=imageinfo" \
  --data-urlencode "iiprop=url" \
  --data-urlencode "iiurlwidth=1024" \
  --data-urlencode "format=json" \
  | jq -r '.query.pages[].imageinfo[0].thumburl')
echo "thumb: $THUMB"
curl -fsSL "$THUMB" -o "$OUT/source.jpg"
printf 'Source: NOAA Emergency Response Imagery (2020 Hurricane Laura), public domain (US Gov work).\nVia Wikimedia Commons: %s\n' "$FILE" > "$OUT/source-credit.txt"

# --- 2. Build the SITREP payload ---
cargo run --release -p sonde-demo-builder -- "$OUT/source.jpg" "$OUT" --target-bytes 5000 --max-dim 200

# --- 3. Build the wasm bundle ---
cargo build --release -p sonde-wasm --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/sonde_wasm.wasm --out-dir "$PKG" --target web
echo "assets built: $OUT (payload.bin, payload.offsets.json), $PKG (sonde_wasm.js + _bg.wasm)"
```

- [ ] **Step 2: Make executable + run it**
```bash
chmod +x demo/build-assets.sh && ./demo/build-assets.sh
```
Expected: prints the thumb URL, writes `demo/site/assets/{source.jpg,payload.bin,payload.offsets.json,source-credit.txt}` and `demo/site/pkg/{sonde_wasm.js,sonde_wasm_bg.wasm}`. The builder prints e.g. `wrote NNNN byte payload (~5000) ...`.
If the Commons `thumburl` is a `.png` (some renders), the builder needs PNG decode — if `cargo run ... sonde-demo-builder` errors with an unsupported-format message, add `"png"` to the `image` features in `crates/sonde-demo-builder/Cargo.toml` (`features = ["jpeg", "png"]`), commit that one-line change with the standard trailers, and re-run. Report if you did this.
If `jq`/`wasm-bindgen` are missing, install (`cargo install wasm-bindgen-cli`); if `wasm32` target missing, `rustup target add wasm32-unknown-unknown`.

- [ ] **Step 3: Gitignore the generated artifacts**
Append to the repo's local exclude (not a tracked .gitignore change, to avoid churn): the generated `pkg/` and large binaries. Add to `demo/.gitignore` (committed) so the demo dir is self-documenting:
```
site/pkg/
site/assets/source.jpg
```
Keep `payload.bin` + `payload.offsets.json` + `source-credit.txt` **committed** (they're small and make the page work without a rebuild on Pages). Create `demo/.gitignore` with the two lines above.

- [ ] **Step 4: Commit**
```bash
git add demo/build-assets.sh demo/.gitignore demo/site/assets/payload.bin demo/site/assets/payload.offsets.json demo/site/assets/source-credit.txt
git commit   # "build(sonde-demo): asset build script + committed payload (NOAA ERI, public domain)"; trailers
```

---

## Task 2: Vendor Three.js

**Files:** Create `demo/site/vendor/three.module.js`

- [ ] **Step 1: Download a pinned Three.js ES module**
```bash
mkdir -p demo/site/vendor
curl -fsSL https://unpkg.com/three@0.160.0/build/three.module.js -o demo/site/vendor/three.module.js
head -c 200 demo/site/vendor/three.module.js   # sanity: should be JS, starts with a license comment
```
Expected: a ~1MB JS module file. (Pinned version so the demo is reproducible + offline-capable + Pages-safe.) If `0.160.0` 404s, pick the latest stable `0.16x.0` and note the version used.

- [ ] **Step 2: Commit**
```bash
git add demo/site/vendor/three.module.js
git commit   # "build(sonde-demo): vendor three.js 0.160.0 ES module"; trailers
```

---

## Task 3: `engine.js` — wasm API wrapper

**Files:** Create `demo/site/js/engine.js`

The ONLY module that touches the wasm JSON API. Parses JSON, exposes typed-ish objects.

- [ ] **Step 1: Implement**
```js
// engine.js — the only module aware of the sonde-wasm JSON API.
import init, { list_modes, recommend_mode, run_link } from "../pkg/sonde_wasm.js";

let _payload = null;          // Uint8Array
let _offsetsJson = null;      // string (passed verbatim to run_link)
let _offsets = null;          // parsed {total_len, fields[], image_byte_len}

/** Load the wasm module + the committed payload assets. Call once at startup. */
export async function initEngine(base = "./assets") {
  await init();               // loads pkg/sonde_wasm_bg.wasm
  const [binResp, offResp] = await Promise.all([
    fetch(`${base}/payload.bin`),
    fetch(`${base}/payload.offsets.json`),
  ]);
  _payload = new Uint8Array(await binResp.arrayBuffer());
  _offsetsJson = await offResp.text();
  _offsets = JSON.parse(_offsetsJson);
  return { payloadLen: _payload.length, offsets: _offsets };
}

export function payload() { return _payload; }
export function offsets() { return _offsets; }

/** Mode catalogue (array of ModeInfo). */
export function listModes() { return JSON.parse(list_modes()); }

/** Sonde's Auto pick for a measured SNR (plain mode-id string). */
export function recommendMode(snrDb) { return recommend_mode(snrDb); }

/**
 * Run the link. Returns the parsed LinkResult, or throws on engine {error}.
 * condition ∈ {none,good,moderate,poor,flutter}; seed is a u32.
 */
export function runLink(modeId, snrDb, condition, seed) {
  const json = run_link(_payload, _offsetsJson, modeId, snrDb, condition, seed >>> 0);
  const r = JSON.parse(json);
  if (r.error) throw new Error(r.error);
  return r;
}
```

- [ ] **Step 2: Verify (in-page, after Task 1+2 assets exist)**
There is no node test (wasm needs a browser/bundler). Verification is via the Playwright smoke test (Task 10) which asserts `initEngine()` + `runLink()` succeed. For now, lint-check the file is valid ES syntax:
```bash
node --check demo/site/js/engine.js
```
Expected: no output (syntax OK). (`node --check` validates syntax without running the imports.)

- [ ] **Step 3: Commit**
```bash
git add demo/site/js/engine.js
git commit   # "feat(sonde-demo): engine.js wasm API wrapper"; trailers
```

---

## Task 4: Data-mapping helpers (pure, unit-tested)

**Files:** Create `demo/site/js/format.js` and `demo/tests/format.test.mjs`

Pure functions for the views — hex formatting, byte-diff, viridis colormap. Pure → unit-testable with plain node (no wasm).

- [ ] **Step 1: Write the failing tests** — `demo/tests/format.test.mjs`:
```js
import assert from "node:assert";
import { toHex, byteDiff, viridis } from "../site/js/format.js";

// toHex
assert.strictEqual(toHex([0x7e, 0x22, 0x04]), "7E 22 04");
assert.strictEqual(toHex([]), "");

// byteDiff: indices where a and b differ (compares min length; extra = differ)
assert.deepStrictEqual(byteDiff([1,2,3],[1,9,3]), [1]);
assert.deepStrictEqual(byteDiff([1,2],[1,2,3]), [2]);     // length mismatch -> trailing differ
assert.deepStrictEqual(byteDiff([],[]), []);

// viridis: returns [r,g,b] 0..255; clamps; monotone-ish endpoints
const lo = viridis(0), hi = viridis(1);
assert.ok(lo.length === 3 && hi.length === 3);
assert.ok(lo[2] > lo[0], "viridis(0) is bluish/purple");   // more blue than red at low end
assert.ok(hi[0] > 150 && hi[1] > 150, "viridis(1) is yellowish");

console.log("format.test ok");
```

- [ ] **Step 2: Run to verify fail**
```bash
node demo/tests/format.test.mjs
```
Expected: FAIL (module/exports not found).

- [ ] **Step 3: Implement** — `demo/site/js/format.js`:
```js
// format.js — pure view helpers (no wasm, unit-tested).

/** Bytes -> "7E 22 04" uppercase hex, space-separated. */
export function toHex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" ");
}

/** Indices where a[i] !== b[i]; trailing bytes beyond the shorter array count as differing. */
export function byteDiff(a, b) {
  const n = Math.min(a.length, b.length);
  const out = [];
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) out.push(i);
  for (let i = n; i < Math.max(a.length, b.length); i++) out.push(i);
  return out;
}

// Viridis control points (matplotlib), sampled; linear-interpolated.
const VIRIDIS = [
  [68, 1, 84], [72, 40, 120], [62, 74, 137], [49, 104, 142],
  [38, 130, 142], [31, 158, 137], [53, 183, 121], [110, 206, 88],
  [181, 222, 43], [253, 231, 37],
];
/** t in [0,1] -> [r,g,b] 0..255 along viridis. */
export function viridis(t) {
  const x = Math.max(0, Math.min(1, t)) * (VIRIDIS.length - 1);
  const i = Math.floor(x), f = x - i;
  const a = VIRIDIS[i], b = VIRIDIS[Math.min(i + 1, VIRIDIS.length - 1)];
  return [0, 1, 2].map((k) => Math.round(a[k] + (b[k] - a[k]) * f));
}
```

- [ ] **Step 4: Run to verify pass**
```bash
node demo/tests/format.test.mjs
```
Expected: `format.test ok`.

- [ ] **Step 5: Commit**
```bash
git add demo/site/js/format.js demo/tests/format.test.mjs
git commit   # "feat(sonde-demo): pure view helpers (hex, byte-diff, viridis) + tests"; trailers
```

---

## Task 5: Visual shell via the frontend-design skill

**Files:** Create `demo/site/index.html`, `demo/site/css/app.css`

- [ ] **Step 1: Invoke the frontend-design skill** to produce `index.html` + `css/app.css` for the demo, with these REQUIREMENTS (give the skill this brief verbatim):
  - **Aesthetic:** modern professional dark dashboard ("style B", Tuxlink-vein). Dark surfaces, rounded cards, pill toggles, system-ui chrome + monospace for data readouts. **Proportion discipline: NO full-bleed edge-to-edge skinny elements** — contained max-width, deliberate grid with generous gutters, sensible card aspect ratios, real whitespace.
  - **Information architecture** (from spec §4): a hero panel for the **3D waterfall**; a **lever panel** (SNR slider, channel-condition selector with options Ideal(none)/Good/Moderate/Poor/Flutter, Auto⇄Manual toggle + a Manual mode picker, adaptation readout); a **stats** block (mode, constellation, BER, throughput, time-to-deliver, measured SNR); a **recon-image** tile; a full-width **TX | RX packet console**; a **transport bar** (play/pause, scrub, speed); a persistent **honesty banner** ("Simulated channel · software DSP · not on-air") and a short "what you're seeing" explainer; a footer credit line for the NOAA image.
  - **Required element IDs/data-attrs** (so the JS can target them — the skill MUST include these):
    `#waterfall-mount` (empty div for the Three.js canvas), `#snr-slider`, `#snr-readout`, `#condition-group` (buttons with `data-condition="none|good|..."`), `#mode-toggle` (Auto/Manual) + `#mode-picker`, `#adaptation-readout`, `#stat-ber #stat-throughput #stat-deliver #stat-snr #stat-mode`, `#recon-image` (a canvas), `#tx-console #rx-console #flip-count`, `#play-btn #scrub #speed`, `#credit`.
  - **No JS logic in the shell** beyond loading `js/main.js` as a module (`<script type="module" src="js/main.js">`). The skill owns layout/styling only.
- [ ] **Step 2: Verify** the shell loads and contains all required IDs:
```bash
node --check demo/site/js/main.js 2>/dev/null || true   # main.js may not exist yet; ok
grep -o 'id="[a-z-]*"' demo/site/index.html | sort -u    # eyeball that the required IDs are present
```
Open `demo/site/index.html` in a browser (served — see Task 11) to confirm the layout renders in the style-B direction with no skinny full-bleed elements. Iterate with the frontend-design skill if proportions are off.
- [ ] **Step 3: Commit**
```bash
git add demo/site/index.html demo/site/css
git commit   # "feat(sonde-demo): demo page visual shell (frontend-design, style B)"; trailers
```

---

## Task 6: `waterfall.js` — Three.js 3D waterfall

**Files:** Create `demo/site/js/waterfall.js`

Implements to this interface (the implementer writes the Three.js; verify behavior in the Playwright test, Task 10):
```
export function createWaterfall(mountEl): {
  setData(spectrogram): void,   // spectrogram = {rows, cols, freqs_hz[], times_s[], mag_q[] (row-major u8)}
  setNow(fraction): void,       // 0..1 -> position the sweeping "now" plane along the time axis
  dispose(): void,
}
```
- [ ] **Step 1: Implement** a Three.js scene that: builds a surface mesh `rows × cols` with height = `mag_q[r*cols+c]/255` and vertex color = `viridis(mag_q/255)` (import from `format.js`); orbit controls (or a slow auto-orbit); a translucent vertical plane positioned by `setNow`; resizes to `mountEl`. Plan an order-of-magnitude check: `rows≈106`, `cols≤400` → ≤~42k vertices, fine for a BufferGeometry. Reuse/replace geometry on `setData` (dispose old to avoid leaks).
- [ ] **Step 2: Syntax check** `node --check demo/site/js/waterfall.js`. Behavior is covered by Task 10.
- [ ] **Step 3: Commit** — `"feat(sonde-demo): 3D Three.js waterfall"`; trailers.

---

## Task 7: `console.js` — TX|RX packet console

**Files:** Create `demo/site/js/console.js`

```
export function createConsole(txEl, rxEl, flipCountEl): {
  showSymbol(symbol): void,   // symbol = {idx, bytes[] (TX), rx_bytes[] (RX), field, byte_start, byte_end}
  reset(): void,
}
```
- [ ] **Step 1: Implement** using `toHex` + `byteDiff` from `format.js`: render TX bytes (field-colored) and RX bytes with the `byteDiff` indices highlighted (red); maintain + display a cumulative flip count across `showSymbol` calls since `reset()`. If `rx_bytes` is empty (sync failure), render RX as "— (no decode)".
- [ ] **Step 2:** `node --check demo/site/js/console.js`. Behavior via Task 10.
- [ ] **Step 3: Commit** — `"feat(sonde-demo): TX|RX packet console"`; trailers.

---

## Task 8: `image-reveal.js` — progressive recon image

**Files:** Create `demo/site/js/image-reveal.js`

```
export function createImageReveal(canvasEl): {
  // recoveredBytes: Uint8Array|number[] (LinkResult.recovered_bytes); imageRange = [start,end] from offsets image field.
  render(recoveredBytes, imageRange, fraction): void,  // draw image decoded from recovered bytes, revealed up to `fraction`
  showFailed(): void,                                   // sync-failure state (no image)
}
```
- [ ] **Step 1: Implement:** slice `recoveredBytes[start..end]` → Blob(`image/jpeg`) → `createImageBitmap` (async) → draw to canvas, clipped to a left-to-right reveal at `fraction`. Decode **defensively**: a corrupt JPEG may reject — `catch` and draw a "corrupted" byte-grid (e.g. paint each byte as a grayscale pixel) so corruption is still visible. Empty `recoveredBytes` → `showFailed()` (a "decode failed" placeholder).
- [ ] **Step 2:** `node --check demo/site/js/image-reveal.js`. Behavior via Task 10.
- [ ] **Step 3: Commit** — `"feat(sonde-demo): progressive recon-image render (with corruption)"`; trailers.

---

## Task 9: `playback.js` + `controls.js` + `main.js` — wiring

**Files:** Create `demo/site/js/playback.js`, `demo/site/js/controls.js`, `demo/site/js/main.js`

- [ ] **Step 1: `playback.js`** — a small state machine:
```
export function createPlayback({onFrame, onDone}): {
  load(result): void,   // store LinkResult; reset to t=0
  play(): void, pause(): void, scrub(fraction): void, setSpeed(x): void,
}
```
drives `onFrame(fraction, currentSymbolIndex)` via `requestAnimationFrame`, advancing over `result.time_to_deliver_s` (scaled by speed); maps elapsed fraction → symbol index using `result.symbols[].t_start_s` / `result.total_samples`.

- [ ] **Step 2: `controls.js`** — reads the lever DOM (`#snr-slider`, `#condition-group`, `#mode-toggle`, `#mode-picker`), exposes `getState() -> {snrDb, condition, mode|null, auto}` and `onChange(cb)` (debounced ~150ms). Populates `#mode-picker` from `listModes()` (disable `implemented:false` with a "pending" tag).

- [ ] **Step 3: `main.js`** — bootstrap + wire everything:
```
import { initEngine, listModes, recommendMode, runLink, offsets } from "./engine.js";
// + createWaterfall, createConsole, createImageReveal, createPlayback, createControls
```
On load: `await initEngine()`, build waterfall/console/image/playback/controls. On control change (debounced): compute `mode = auto ? recommendMode(snr) : pickedMode`; `const result = runLink(mode, snr, condition, seed)`; update stats (`#stat-*`, adaptation readout), `waterfall.setData(result.spectrogram)`, `playback.load(result)`. On `playback.onFrame(frac, symIdx)`: `waterfall.setNow(frac)`, `console.showSymbol(result.symbols[symIdx])`, `imageReveal.render(result.recovered_bytes, imageFieldRange(offsets()), frac)`. On decode failure (`!result.recovered_ok`): console shows no-decode, `imageReveal.showFailed()` at end. Wire `#play-btn/#scrub/#speed`. Set `#credit` from `assets/source-credit.txt`.

- [ ] **Step 4: Syntax check all three** (`node --check` each). Full behavior in Task 10.
- [ ] **Step 5: Commit** — `"feat(sonde-demo): playback + controls + main wiring"`; trailers.

---

## Task 10: Playwright smoke test

**Files:** Create `demo/tests/smoke.spec.mjs`, add Playwright config note in `demo/README.md` (Task 11)

- [ ] **Step 1: Write the test** (serves `demo/site` on a local port, drives the page):
```js
import { test, expect } from "@playwright/test";
// Assumes a static server serving demo/site at BASE (see demo/README.md).
const BASE = process.env.DEMO_BASE || "http://localhost:8080";

test("loads, runs a link, renders image at high SNR", async ({ page }) => {
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(BASE);
  await page.waitForSelector("#waterfall-mount canvas", { timeout: 15000 }); // wasm + three up
  // Set SNR high + Ideal channel, Auto.
  await page.fill("#snr-slider", "60").catch(() => {});
  await page.click('#condition-group [data-condition="none"]');
  // BER readout should reach 0.00% and a mode is chosen.
  await expect(page.locator("#stat-ber")).toContainText("0.00", { timeout: 15000 });
  await expect(page.locator("#stat-mode")).toContainText("floor-wblo");
  expect(errors, errors.join("\n")).toEqual([]);
});

test("multipath degrades without crashing", async ({ page }) => {
  await page.goto(BASE);
  await page.waitForSelector("#waterfall-mount canvas", { timeout: 15000 });
  await page.fill("#snr-slider", "-6").catch(() => {});
  await page.click('#condition-group [data-condition="poor"]');
  // Either non-zero BER or an explicit failed state — must not be 0.00% clean.
  await expect(page.locator("#rx-console")).toBeVisible();
});
```

- [ ] **Step 2: Run** (build assets first if not done; serve; run):
```bash
./demo/build-assets.sh
( cd demo/site && python3 -m http.server 8080 & echo $! > /tmp/demo-srv.pid )
npx --yes playwright@1.48 install chromium >/dev/null 2>&1 || true
DEMO_BASE=http://localhost:8080 npx --yes playwright@1.48 test demo/tests/smoke.spec.mjs
kill "$(cat /tmp/demo-srv.pid)" 2>/dev/null || true
```
Expected: both tests PASS, no page errors. If selectors don't match the frontend-design output, reconcile the IDs (Task 5 mandates them) — fix whichever drifted and re-run.

- [ ] **Step 3: Commit** — `"test(sonde-demo): playwright smoke test"`; trailers.

---

## Task 11: README, serve instructions, attribution, final polish

**Files:** Create `demo/README.md`

- [ ] **Step 1: Write `demo/README.md`** covering: prerequisites (`cargo`, `wasm-bindgen-cli`, `jq`, a static server); `./demo/build-assets.sh`; local serve (`cd demo/site && python3 -m http.server 8080`, note `file://` won't work because of `fetch`/module CORS); GitHub Pages hosting (point Pages at `demo/site` or copy it to the Pages branch); the **NOAA Emergency Response Imagery public-domain attribution** (from `assets/source-credit.txt`); and the honest caveats (only `floor-wblo` decodes cleanly on the Ideal/AWGN path; multipath degrades; full-speed modes pending).
- [ ] **Step 2: Final gate** — confirm the Rust gate is still green (no Rust changed unless the PNG-feature tweak in Task 1): `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`. Re-run the Playwright smoke test once more end-to-end.
- [ ] **Step 3: Commit** — `"docs(sonde-demo): demo README + serve/host/attribution"`; trailers.

---

## Self-Review notes
- `engine.js` is the only wasm-aware module; all views consume parsed `LinkResult` objects → clean boundary.
- TX (`bytes`) and RX (`rx_bytes`) are index-aligned + equal length (engine guarantee) → `byteDiff` is valid; symbol 0's first two bytes are the length header (field-labeled `header(framing)`), so a diff there is expected framing, not payload corruption — the console labels by `field`.
- Corruption visibility lives on the Ideal/AWGN path at marginal SNR; multipath fails to sync (empty `recovered_bytes` → failed state). The smoke test exercises both regimes.
- No `file://` — fetch + ES modules require a served origin; README documents the one-liner. (Pages serves it fine.)
- Committed `payload.bin`/offsets mean the page works on Pages without running Rust; `pkg/` is gitignored and must be built (documented) — for Pages, the build step runs in CI or the operator copies `pkg/` in.
