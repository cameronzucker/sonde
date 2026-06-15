// main.js — bootstrap + wiring for the LIVE ARDOP connected-mode demo (ES module).
//
// One action (Connect) starts a real connected session on the backend and the whole
// page animates LIVE as it happens: the on-air audio streams from the Pi and plays
// continuously (driving the waterfall), the connection/ARQ log + telemetry stream in,
// and the recon image lands as ARQ delivers it. No record-then-replay. ardopcf is
// external/reference (clean-sheet, ADR 0014); the page keys nothing.

import { initSession, runSession, hexToBytes, offsets } from "./session-engine.js";
import { createWaterfall } from "./waterfall.js";
import { createSessionLog } from "./session-log.js";
import { createImageReveal } from "./image-reveal.js";
import { createControls } from "./controls.js";
import { createLiveAudio } from "./live-audio.js";
import { createSMeter } from "./s-meter.js";

// ── DOM handles ─────────────────────────────────────────────────────────────
const el = (id) => document.getElementById(id);
const statMode = el("stat-mode");
const statConstellation = el("stat-constellation");
const statSnr = el("stat-snr");
const statBw = el("stat-bw");
const statThroughput = el("stat-throughput");
const statDeliver = el("stat-deliver");
const adaptation = el("adaptation-readout");
const muteBtn = el("mute-btn");
const muteGlyph = el("mute-glyph");

// ── Module instances + run state ─────────────────────────────────────────────
let waterfallFwd, waterfallRev, meterFwd, meterRev, imageReveal, sessionLog, controls, liveAudio;
let imageRange = [0, 0];
let seed = 1;
let session = null;   // current EventSource controller ({close})

// ── Helpers ───────────────────────────────────────────────────────────────────
const CONSTELLATION = { "4FSK": "4-FSK", "4PSK": "QPSK", "8PSK": "8-PSK", "16QAM": "16-QAM" };
const constellationFor = (m) => (m ? CONSTELLATION[m.split(".", 1)[0]] || "—" : "—");

function fmtThroughput(bps) {
  if (!isFinite(bps) || bps <= 0) return "—";
  return bps >= 1000 ? `${(bps / 1000).toFixed(2)} kbps` : `${bps.toFixed(0)} bps`;
}

function setStatus(text) {
  const t = adaptation.querySelector(".adaptation__text");
  if (t) t.textContent = text;
}

function imageFieldRange(off) {
  const f = off?.fields?.find((x) => x.label === "image");
  return f ? [f.start, f.end] : [0, 0];
}

function clearTelemetry() {
  statMode.textContent = "—";
  statConstellation.textContent = "—";
  statBw.textContent = "—";
  statThroughput.textContent = "—";
  statDeliver.textContent = "—";
}

// ── Run one LIVE connected session ────────────────────────────────────────────
function runConnectedSession(state) {
  if (session) session.close();   // cancel any in-flight session (latest lever wins)
  const mySeed = seed++;

  // The Connect click is the user gesture that unlocks audio — start the player now.
  liveAudio.start();

  controls.setRunning(true);
  sessionLog.reset();
  waterfallFwd.reset();
  waterfallRev.reset();
  meterFwd.reset();
  meterRev.reset();
  clearTelemetry();
  statSnr.textContent = `${state.snrDb} dB`;
  imageReveal.showFailed("CONNECTING…", `${state.condition} · ${state.arqbw.replace("MAX", " Hz max")}`);
  setStatus(`Dialing ${state.arqbw.replace("MAX", " Hz max")} at ${state.snrDb} dB / ${state.condition}…`);
  sessionLog.event("call", `ARQ CONNECT — SNR ${state.snrDb} dB · ${state.condition}`, state.arqbw);

  let lastModes = [];

  session = runSession({ ...state, seed: mySeed }, {
    onPhase: (ev) => setStatus(ev.msg),
    onStation: (ev) => sessionLog.event("info", `${ev.station} = ${ev.call} (${ev.role})`),
    onAudio: (ev) => liveAudio.enqueue(ev.samples, ev.rate, ev.dir),  // per-direction → its waterfall
    onConnected: (ev) => {
      statBw.textContent = ev.bandwidth ? `${ev.bandwidth} Hz` : "—";
      sessionLog.event("conn", `CONNECTED — ${ev.call_a} ⇄ ${ev.call_b}`,
        ev.bandwidth ? `${ev.bandwidth} Hz` : "");
      setStatus(`Connected · negotiated ${ev.bandwidth || "?"} Hz — transferring image over ARQ…`);
    },
    onDataStart: (ev) => {
      sessionLog.event("arq", "image handed to the ARQ buffer", `${ev.bytes} B`);
      imageReveal.showFailed("RECEIVING…", `0 / ${ev.bytes} B`);
    },
    onProgress: (ev) => {
      sessionLog.setProgress(ev.received, ev.total);
      if (ev.received < ev.total) imageReveal.showFailed("RECEIVING…", `${ev.received} / ${ev.total} B`);
    },
    onMode: (ev) => {
      ev.modes.slice(lastModes.length).forEach((m) =>
        sessionLog.event("rate", `data mode ${m}`, constellationFor(m)));
      lastModes = ev.modes;
      statMode.textContent = ev.current || "—";
      statConstellation.textContent = constellationFor(ev.current);
    },
    onDelivered: (ev) => {
      sessionLog.setProgress(ev.received, ev.total);
      sessionLog.event(ev.intact ? "ok" : "fail",
        ev.intact ? "image delivered intact" : "transfer incomplete",
        `${ev.received}/${ev.total} B`);
    },
    onResult: (ev) => finishSession(ev),
    onError: (ev) => { sessionLog.event("fail", ev.msg); setStatus(`Error: ${ev.msg}`); },
    onDone: () => {
      liveAudio.stop();   // stop accepting chunks; queued audio drains, waterfall idles
      controls.setRunning(false);
      session = null;
    },
  });
}

// ── Apply the terminal result event ───────────────────────────────────────────
function finishSession(ev) {
  statDeliver.textContent = `${ev.duration_s.toFixed(1)} s`;
  if (ev.modes && ev.modes.length) {
    statMode.textContent = ev.modes[ev.modes.length - 1];
    statConstellation.textContent = constellationFor(ev.modes[ev.modes.length - 1]);
  }
  statBw.textContent = ev.bandwidth ? `${ev.bandwidth} Hz` : "—";

  if (ev.outcome === "fail") {
    setStatus("CONNECT FAILED — the link can't close at this SNR; nothing was delivered.");
    imageReveal.showFailed("CONNECT FAILED", "link could not close");
    statThroughput.textContent = "—";
    return;
  }
  const bps = ev.duration_s > 0 ? (ev.received * 8) / ev.duration_s : 0;
  statThroughput.textContent = fmtThroughput(bps);
  const bytes = hexToBytes(ev.image_hex);
  const fraction = ev.total > 0 ? ev.received / ev.total : 0;
  if (bytes.length) imageReveal.render(bytes, imageRange, fraction);
  else imageReveal.showFailed();
  setStatus(ev.outcome === "pass"
    ? `Delivered intact in ${ev.duration_s.toFixed(1)} s.`
    : `Partial: ${ev.received}/${ev.total} B before the link dropped.`);
}

// ── Audio mute ────────────────────────────────────────────────────────────────
function wireMute() {
  muteBtn.addEventListener("click", () => {
    const next = !liveAudio.isMuted();
    liveAudio.setMuted(next);
    muteBtn.setAttribute("aria-pressed", String(next));
    if (muteGlyph) muteGlyph.textContent = next ? "🔇" : "🔊";
  });
}

// ── Credit line ───────────────────────────────────────────────────────────────
async function loadCredit() {
  try {
    const resp = await fetch("./assets/source-credit.txt");
    if (resp.ok) el("credit").textContent = (await resp.text()).trim().replace(/\n+/g, " · ");
  } catch { /* leave the placeholder */ }
}

// ── Bootstrap ─────────────────────────────────────────────────────────────────
async function boot() {
  await initSession();
  imageRange = imageFieldRange(offsets());

  liveAudio = createLiveAudio();
  // Two waterfalls: A->B (data sender) and B->A (receiver's ACK/NAK bursts), each
  // tapping its own direction's analyser so the half-duplex turn-taking is visible.
  waterfallFwd = createWaterfall(el("waterfall-fwd"), {
    getAnalyser: () => liveAudio.getAnalyser("fwd"),
    isPlaying: () => liveAudio.isPlaying("fwd"),
  });
  waterfallRev = createWaterfall(el("waterfall-rev"), {
    getAnalyser: () => liveAudio.getAnalyser("rev"),
    isPlaying: () => liveAudio.isPlaying("rev"),
  });
  // Per-station S-meters off the same per-direction analysers (real on-air level).
  meterFwd = createSMeter(el("vu-fwd"), { getAnalyser: () => liveAudio.getAnalyser("fwd") });
  meterRev = createSMeter(el("vu-rev"), { getAnalyser: () => liveAudio.getAnalyser("rev") });
  sessionLog = createSessionLog(el("session-log-stream"), el("session-progress"));
  imageReveal = createImageReveal(el("recon-image"));
  controls = createControls(runConnectedSession);

  wireMute();
  loadCredit();
  setStatus("Set the channel + bandwidth ceiling, then press Connect to run a live session.");
}

boot().catch((err) => {
  setStatus(`Failed to initialise: ${err.message}`);
  console.error("[ardop-demo] boot failed:", err);
});
