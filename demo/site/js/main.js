// main.js — bootstrap + wiring for the ARDOP-anchored HF link demo (ES module).
//
// Re-anchored on a real reference modem: ardop-engine.js is the only backend-aware
// module; it hits demo/ardop/server.py, which runs real ardopcf frames through
// hf-channel-sim. Every view consumes the parsed result objects unchanged. ardopcf
// is external/reference (clean-sheet, ADR 0014); the page transmits nothing.

import { initEngine, listModes, recommendMode, runLink, fetchAudio, offsets } from "./ardop-engine.js";
import { createWaterfall } from "./waterfall.js";
import { createFrameLog } from "./frame-log.js";
import { createImageReveal } from "./image-reveal.js";
import { createPlayback } from "./playback.js";
import { createControls } from "./controls.js";
import { createAudioPlayer } from "./audio.js";

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
const muteBtn = el("mute-btn");
const muteGlyph = el("mute-glyph");

// ── Module instances + run state ─────────────────────────────────────────────
let waterfall, frameLog, imageReveal, playback, controls, audioPlayer;
let modes = [];
let result = null;          // current run result
let imageRange = [0, 0];    // [start,end] of the image field within the payload
let renderedFrames = 0;     // how many frame-log rows shown this run
let scrubbing = false;      // suppress scrub feedback while the user drags
let seed = 1;               // fresh channel realization per run
let runToken = 0;           // guards against stale async runs/audio applying late

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

function clearStats(mode, msg) {
  statMode.textContent = mode;
  statConstellation.textContent = constellationFor(mode);
  statSnr.textContent = "—";
  statBer.textContent = "—";
  statThroughput.textContent = "—";
  statDeliver.textContent = "—";
  setAdaptation(msg);
}

// ── Run the link for the current control state and load it for playback ──────
async function runAndLoad() {
  const state = controls.getState();
  const requested = state.auto ? "auto" : (state.mode || "auto");
  const myToken = ++runToken;
  const runSeed = seed++; // capture so the audio uses the SAME channel realization

  setAdaptation("Running real ARDOP through the channel sim…");

  let r;
  try {
    r = await runLink(requested, state.snrDb, state.condition, runSeed);
  } catch (err) {
    if (myToken !== runToken) return; // a newer run superseded this one
    clearStats(state.auto ? "auto" : (state.mode || "—"), `Backend error: ${err.message}`);
    imageReveal.showFailed();
    frameLog.reset();
    audioPlayer.load(null);
    playback.load(null);
    setPlayGlyph(false);
    return;
  }
  if (myToken !== runToken) return; // stale — discard

  result = r;
  imageRange = imageFieldRange(offsets());
  renderedFrames = 0;

  // Telemetry. ARDOP reports link quality, not an SNR estimate — show the channel
  // SNR we drove the sim with (honest labeling).
  statMode.textContent = r.mode_id;
  statConstellation.textContent = r.constellation || constellationFor(r.mode_id);
  statSnr.textContent = `${r.set_snr_db.toFixed(0)} dB set`;
  statBer.textContent = isFinite(r.ber) ? `${(r.ber * 100).toFixed(2)}%` : "—";
  statThroughput.textContent = fmtThroughput(r.throughput_bps);
  statDeliver.textContent = `${r.time_to_deliver_s.toFixed(1)} s`;

  // Adaptation narrative — three outcomes keyed on frame loss.
  const s = r.summary;
  const pick = r.auto ? `Auto selected ${r.mode_id}` : `Manual: ${r.mode_id}`;
  let outcome;
  if (s.frames_decoded === s.frames_total) {
    outcome = `all ${s.frames_total} frames decoded — image recovered intact.`;
  } else if (s.frames_decoded === 0) {
    outcome = `0 / ${s.frames_total} frames decoded — link can't close, nothing recovered.`;
  } else {
    outcome = `${s.frames_decoded} / ${s.frames_total} frames decoded — ${s.frames_total - s.frames_decoded} lost to the channel (no ARQ), leaving holes in the image.`;
  }
  setAdaptation(`${pick} for ${state.snrDb} dB / ${state.condition}. ${outcome}`);

  // Views. The waterfall is a LIVE spectrogram driven by the playing audio's
  // analyser (see boot()); just clear its history so this transmission scrolls in
  // fresh. (The backend's static spectrogram field is intentionally unused.)
  waterfall.reset();
  frameLog.reset();
  if (s.frames_decoded === 0) imageReveal.showFailed();

  // Audio: the concatenated channel-impaired transmission for this exact run. Fetch
  // is async; guard against a stale run applying its audio over a newer one.
  fetchAudio(r.audio_url).then(({ samples, sampleRate }) => {
    if (myToken !== runToken) return;
    audioPlayer.load(samples, sampleRate);
  }).catch(() => { if (myToken === runToken) audioPlayer.load(null); });

  playback.load(r);
  // Land on the fully-delivered frame so the recon image + frame log are populated
  // at rest; Play/scrub replays the progressive fill-in on demand.
  playback.scrub(1);
  setPlayGlyph(false);
}

// ── Playback frame handler ───────────────────────────────────────────────────
function onFrame(fraction, frameIndex) {
  // Reveal the recovered image (clipped to fraction). Holes from lost frames show
  // as corruption in the byte-grid fallback when the JPEG won't decode.
  if (result && result.recovered_bytes && result.recovered_bytes.length > 0) {
    imageReveal.render(result.recovered_bytes, imageRange, fraction);
  }

  // Append frame-log rows up to frameIndex; on a backward scrub, replay from scratch.
  const target = frameIndex + 1;
  if (target < renderedFrames) {
    frameLog.reset();
    renderedFrames = 0;
  }
  while (renderedFrames < target && renderedFrames < (result?.frames.length || 0)) {
    frameLog.showFrame(result.frames[renderedFrames]);
    renderedFrames++;
  }

  if (!scrubbing) scrub.value = String(Math.round(fraction * 1000));
}

function onDone() {
  setPlayGlyph(false);
  if (result && result.summary.frames_decoded === 0) imageReveal.showFailed();
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
      audioPlayer.stop();
      setPlayGlyph(false);
    } else {
      const offset = Number(scrub.value) >= 1000 ? 0 : Number(scrub.value) / 1000;
      playback.play();
      audioPlayer.play(offset);
      setPlayGlyph(playback.isPlaying());
    }
  });

  scrub.addEventListener("input", () => {
    scrubbing = true;
    playback.scrub(Number(scrub.value) / 1000);
    audioPlayer.stop(); // audio resumes from the new position on next Play
    setPlayGlyph(false);
  });
  scrub.addEventListener("change", () => { scrubbing = false; });

  speedSel.addEventListener("change", () => playback.setSpeed(Number(speedSel.value)));

  muteBtn.addEventListener("click", () => {
    const next = !audioPlayer.isMuted();
    audioPlayer.setMuted(next);
    muteBtn.setAttribute("aria-pressed", String(next));
    if (muteGlyph) muteGlyph.textContent = next ? "🔇" : "🔊";
  });
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

  audioPlayer = createAudioPlayer();
  // The waterfall is a live spectrogram: it taps the audio player's AnalyserNode
  // each frame while audio is playing, so it animates in real time and honestly
  // reflects the channel's noise at the current SNR.
  waterfall = createWaterfall(el("waterfall-mount"), {
    getAnalyser: () => audioPlayer.getAnalyser(),
    isPlaying: () => audioPlayer.isPlaying(),
  });
  frameLog = createFrameLog(el("frame-log-stream"), el("frame-log-stat"));
  imageReveal = createImageReveal(el("recon-image"));
  playback = createPlayback({ onFrame, onDone });
  controls = createControls(modes, runAndLoad);

  wireTransport();
  loadCredit();

  runAndLoad(); // initial run with default levers
}

boot().catch((err) => {
  setAdaptation(`Failed to initialise: ${err.message}`);
  console.error("[ardop-demo] boot failed:", err);
});
