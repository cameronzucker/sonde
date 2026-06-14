// main.js — bootstrap + wiring for the Sonde adaptive-link demo (ES module).
// engine.js is the only wasm-aware module; every view consumes parsed LinkResult objects.

import { initEngine, listModes, recommendMode, runLink, offsets } from "./engine.js";
import { createWaterfall } from "./waterfall.js";
import { createConsole } from "./console.js";
import { createImageReveal } from "./image-reveal.js";
import { createPlayback } from "./playback.js";
import { createControls } from "./controls.js";

// ── DOM handles ─────────────────────────────────────────────────────────────
const el = (id) => document.getElementById(id);
const statMode = el("stat-mode");
const statConstellation = el("stat-constellation");
const statSnr = el("stat-snr");
const statBer = el("stat-ber");
const statThroughput = el("stat-throughput");
const statDeliver = el("stat-deliver");
const adaptation = el("adaptation-readout");
const playBtn = el("play-btn");
const scrub = el("scrub");
const speedSel = el("speed");

// ── Module instances + run state ─────────────────────────────────────────────
let waterfall, packetConsole, imageReveal, playback, controls;
let modes = [];
let result = null;          // current LinkResult
let imageRange = [0, 0];    // [start,end] of the image field within the payload
let renderedSymbols = 0;    // how many symbols the console has shown this run
let scrubbing = false;      // suppress scrub feedback while the user drags
let seed = 1;               // fresh channel realization per run

/** Find the [start,end] byte range of the "image" field in the offsets map. */
function imageFieldRange(off) {
  const f = off?.fields?.find((x) => x.label === "image");
  return f ? [f.start, f.end] : [0, 0];
}

function constellationFor(modeId) {
  const m = modes.find((x) => x.id === modeId);
  return m ? m.constellation : "—";
}

function fmtThroughput(bps) {
  if (!isFinite(bps) || bps <= 0) return "0 bps";
  return bps >= 1000 ? `${(bps / 1000).toFixed(2)} kbps` : `${bps.toFixed(0)} bps`;
}

function setAdaptation(text) {
  const t = adaptation.querySelector(".adaptation__text");
  if (t) t.textContent = text;
}

// ── Run the link for the current control state and load it for playback ──────
function runAndLoad() {
  const state = controls.getState();
  const mode = state.auto ? recommendMode(state.snrDb) : (state.mode || recommendMode(state.snrDb));

  let r;
  try {
    r = runLink(mode, state.snrDb, state.condition, seed++);
  } catch (err) {
    // Engine refused this mode/condition — surface it without crashing the page.
    statMode.textContent = mode;
    statConstellation.textContent = constellationFor(mode);
    statSnr.textContent = "—";
    statBer.textContent = "—";
    statThroughput.textContent = "—";
    statDeliver.textContent = "—";
    setAdaptation(`Engine error: ${err.message}`);
    imageReveal.showFailed();
    playback.load(null); // stop any animation from the previous result
    setPlayGlyph(false);
    return;
  }

  result = r;
  imageRange = imageFieldRange(offsets());
  renderedSymbols = 0;

  // Telemetry.
  statMode.textContent = r.mode_id;
  statConstellation.textContent = constellationFor(r.mode_id);
  statSnr.textContent = `${r.measured_snr_db.toFixed(1)} dB`;
  statBer.textContent = `${(r.ber * 100).toFixed(2)}%`;
  statThroughput.textContent = fmtThroughput(r.throughput_bps);
  statDeliver.textContent = `${r.time_to_deliver_s.toFixed(1)} s`;

  // Adaptation narrative.
  // Three outcome states: clean recovery; synced-but-corrupted (frame decoded
  // with bit errors — recovered_bytes present but != payload); failed sync
  // (no recovered_bytes). The engine reports recovered_ok=false for BOTH of the
  // latter two, so the image/narrative key off recovered_bytes PRESENCE, not
  // recovered_ok — otherwise the corrupting-image showcase reads "DECODE FAILED".
  const synced = Array.isArray(r.recovered_bytes) && r.recovered_bytes.length > 0;
  const pick = state.auto ? `Auto selected ${r.mode_id}` : `Manual: ${r.mode_id}`;
  const outcome = r.recovered_ok
    ? "link closes — payload recovered."
    : synced
      ? `link syncs but bit errors corrupt the payload (BER ${(r.ber * 100).toFixed(1)}%) — watch the recon image degrade.`
      : (state.condition === "none"
          ? "SNR too low — the floor waveform can't close the link."
          : "link fails to sync — multipath beyond the floor's reach.");
  setAdaptation(`${pick} for ${state.snrDb} dB / ${state.condition}. ${outcome}`);

  // Views.
  waterfall.setData(r.spectrogram);
  packetConsole.reset();
  if (synced) imageReveal.render(r.recovered_bytes, imageRange, 0);
  else imageReveal.showFailed();

  playback.load(r);
  scrub.value = "0";
  setPlayGlyph(false);
}

// ── Playback frame handler ───────────────────────────────────────────────────
function onFrame(fraction, symbolIndex) {
  waterfall.setNow(fraction);

  // Render whenever bytes were recovered (clean OR corrupted) — not only on
  // recovered_ok, so the corrupting reveal works in the marginal regime.
  if (result && Array.isArray(result.recovered_bytes) && result.recovered_bytes.length > 0) {
    imageReveal.render(result.recovered_bytes, imageRange, fraction);
  }

  // Show every symbol up to symbolIndex; on a backward scrub, replay from scratch.
  const target = symbolIndex + 1;
  if (target < renderedSymbols) {
    packetConsole.reset();
    renderedSymbols = 0;
  }
  while (renderedSymbols < target && renderedSymbols < (result?.symbols.length || 0)) {
    packetConsole.showSymbol(result.symbols[renderedSymbols]);
    renderedSymbols++;
  }

  if (!scrubbing) scrub.value = String(Math.round(fraction * 1000));
}

function onDone() {
  setPlayGlyph(false);
  // Only show the failed placeholder for a true sync failure (no bytes at all);
  // a synced-but-corrupted decode keeps its (corrupted) recovered image.
  const synced = result && Array.isArray(result.recovered_bytes) && result.recovered_bytes.length > 0;
  if (result && !synced) imageReveal.showFailed();
}

// ── Transport controls ───────────────────────────────────────────────────────
function setPlayGlyph(playing) {
  const glyph = playBtn.querySelector(".transport__glyph");
  const txt = playBtn.querySelector(".transport__txt");
  if (glyph) glyph.textContent = playing ? "❚❚" : "▶";
  if (txt) txt.textContent = playing ? "Pause" : "Play link";
}

function wireTransport() {
  playBtn.addEventListener("click", () => {
    if (playback.isPlaying()) {
      playback.pause();
      setPlayGlyph(false);
    } else {
      playback.play();
      setPlayGlyph(playback.isPlaying());
    }
  });

  scrub.addEventListener("input", () => {
    scrubbing = true;
    playback.scrub(Number(scrub.value) / 1000);
    setPlayGlyph(false);
  });
  scrub.addEventListener("change", () => { scrubbing = false; });

  speedSel.addEventListener("change", () => playback.setSpeed(Number(speedSel.value)));
}

// ── Credit line ────────────────────────────────────────────────────────────--
async function loadCredit() {
  try {
    const resp = await fetch("./assets/source-credit.txt");
    if (resp.ok) {
      const text = (await resp.text()).trim().replace(/\n+/g, " · ");
      el("credit").textContent = text;
    }
  } catch {
    /* leave the placeholder credit text */
  }
}

// ── Bootstrap ────────────────────────────────────────────────────────────────
async function boot() {
  await initEngine();
  modes = listModes();

  waterfall = createWaterfall(el("waterfall-mount"));
  packetConsole = createConsole(el("tx-console"), el("rx-console"), el("flip-count"));
  imageReveal = createImageReveal(el("recon-image"));
  playback = createPlayback({ onFrame, onDone });
  controls = createControls(modes, runAndLoad);

  wireTransport();
  loadCredit();

  runAndLoad(); // initial run with default levers
}

boot().catch((err) => {
  setAdaptation(`Failed to initialise: ${err.message}`);
  // Surface to console for debugging the wasm/asset load path.
  console.error("[sonde-demo] boot failed:", err);
});
