// main.js — bootstrap + wiring for the REAL ARDOP connected-mode link demo (ES module).
//
// session-engine.js is the only backend-aware module; it opens a Server-Sent Events
// stream to demo/ardop/server.py (/api/session), which runs two real ardopcf stations
// through a real CONNECT + bandwidth negotiation + adaptive-rate ARQ transfer over
// hf-channel-sim. This module turns that live event stream into the UI: a connection/
// ARQ timeline, live telemetry, the recon image as it's delivered, and the on-air
// waterfall. ardopcf is external/reference (clean-sheet, ADR 0014); the page keys nothing.

import { initSession, runSession, fetchAudio, hexToBytes, offsets } from "./session-engine.js";
import { createWaterfall } from "./waterfall.js";
import { createSessionLog } from "./session-log.js";
import { createImageReveal } from "./image-reveal.js";
import { createControls } from "./controls.js";
import { createAudioPlayer } from "./audio.js";

// ── DOM handles ─────────────────────────────────────────────────────────────
const el = (id) => document.getElementById(id);
const statMode = el("stat-mode");
const statConstellation = el("stat-constellation");
const statSnr = el("stat-snr");
const statBw = el("stat-bw");
const statThroughput = el("stat-throughput");
const statDeliver = el("stat-deliver");
const adaptation = el("adaptation-readout");
const playBtn = el("play-btn");
const scrub = el("scrub");
const muteBtn = el("mute-btn");
const muteGlyph = el("mute-glyph");

// ── Module instances + run state ─────────────────────────────────────────────
let waterfall, imageReveal, sessionLog, controls, audioPlayer;
let imageRange = [0, 0];     // [start,end] of the image field within the payload
let seed = 1;                // fresh channel realization per session
let session = null;          // current EventSource controller ({close})
let curSnr = 0;              // SNR this session was driven at (for telemetry labeling)

// ── Small helpers ─────────────────────────────────────────────────────────────
const CONSTELLATION = { "4FSK": "4-FSK", "4PSK": "QPSK", "8PSK": "8-PSK", "16QAM": "16-QAM" };

function constellationFor(modeId) {
  if (!modeId) return "—";
  return CONSTELLATION[modeId.split(".", 1)[0]] || "—";
}

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
  statSnr.textContent = "—";
  statBw.textContent = "—";
  statThroughput.textContent = "—";
  statDeliver.textContent = "—";
}

function setPlayGlyph(playing) {
  const glyph = playBtn.querySelector(".transport__glyph");
  const txt = playBtn.querySelector(".transport__txt");
  if (glyph) glyph.textContent = playing ? "❚❚" : "▶";
  if (txt) txt.textContent = playing ? "Pause" : "Play on-air audio";
}

// ── Run one connected session and stream it into the views ────────────────────
function runConnectedSession(state) {
  if (session) session.close();   // cancel any in-flight session (latest lever wins)
  curSnr = state.snrDb;
  const mySeed = seed++;

  // Reset views for a fresh session.
  controls.setRunning(true);
  sessionLog.reset();
  waterfall.reset();
  clearTelemetry();
  statSnr.textContent = `${state.snrDb} dB`;
  audioPlayer.stop();
  audioPlayer.load(null);
  playBtn.disabled = true;
  setPlayGlyph(false);
  scrub.value = "0";
  imageReveal.showFailed("CONNECTING…", `${state.condition} · ${state.arqbw.replace("MAX", " Hz max")}`);
  setStatus(`Dialing ${state.arqbw.replace("MAX", " Hz max")} at ${state.snrDb} dB / ${state.condition}…`);
  sessionLog.event("call", `ARQ CONNECT — SNR ${state.snrDb} dB · ${state.condition}`, state.arqbw);

  let lastModes = [];

  session = runSession(state, {
    onPhase: (ev) => setStatus(ev.msg),
    onStation: (ev) =>
      sessionLog.event("info", `${ev.station} = ${ev.call} (${ev.role})`),
    onConnected: (ev) => {
      statBw.textContent = ev.bandwidth ? `${ev.bandwidth} Hz` : "—";
      sessionLog.event("conn", `CONNECTED — ${ev.call_a} ⇄ ${ev.call_b}`,
        ev.bandwidth ? `${ev.bandwidth} Hz` : "");
      setStatus(`Connected · negotiated ${ev.bandwidth || "?"} Hz — transferring image over ARQ…`);
    },
    onDataStart: (ev) => {
      sessionLog.event("arq", `image handed to the ARQ buffer`, `${ev.bytes} B`);
      imageReveal.showFailed("RECEIVING…", `0 / ${ev.bytes} B`);
    },
    onProgress: (ev) => {
      sessionLog.setProgress(ev.received, ev.total);
      if (ev.received < ev.total) {
        imageReveal.showFailed("RECEIVING…", `${ev.received} / ${ev.total} B`);
      }
    },
    onMode: (ev) => {
      // Log only the newly-added modes (rate adaptation steps).
      const fresh = ev.modes.slice(lastModes.length);
      fresh.forEach((m) => sessionLog.event("rate", `data mode ${m}`, constellationFor(m)));
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
    onError: (ev) => {
      sessionLog.event("fail", ev.msg);
      setStatus(`Error: ${ev.msg}`);
    },
    onDone: () => {
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
  } else {
    const bps = ev.duration_s > 0 ? (ev.received * 8) / ev.duration_s : 0;
    statThroughput.textContent = fmtThroughput(bps);
    const bytes = hexToBytes(ev.image_hex);
    const fraction = ev.total > 0 ? ev.received / ev.total : 0;
    if (bytes.length) imageReveal.render(bytes, imageRange, fraction);
    else imageReveal.showFailed();
    setStatus(ev.outcome === "pass"
      ? `Delivered intact in ${ev.duration_s.toFixed(1)} s. Press play to hear the on-air signal.`
      : `Partial: ${ev.received}/${ev.total} B before the link dropped.`);
  }

  // Load the teed on-air audio for the waterfall (present even on a failed connect:
  // it captured the CONNECT attempts). Enable Play once it's decoded.
  if (ev.audio_url) {
    fetchAudio(ev.audio_url).then(({ samples, sampleRate }) => {
      audioPlayer.load(samples, sampleRate);
      playBtn.disabled = !(samples && samples.length);
    }).catch(() => { playBtn.disabled = true; });
  }
}

// ── Transport: play the captured on-air audio (drives the live waterfall) ──────
function wireTransport() {
  playBtn.addEventListener("click", () => {
    if (audioPlayer.isPlaying()) {
      audioPlayer.stop();
      setPlayGlyph(false);
    } else {
      const offset = Number(scrub.value) / 1000;
      audioPlayer.play(offset);
      setPlayGlyph(audioPlayer.isPlaying());
    }
  });
  audioPlayer.onEnded(() => setPlayGlyph(false));

  scrub.addEventListener("input", () => {
    audioPlayer.stop();   // resumes from the new position on next Play
    setPlayGlyph(false);
  });

  muteBtn.addEventListener("click", () => {
    const next = !audioPlayer.isMuted();
    audioPlayer.setMuted(next);
    muteBtn.setAttribute("aria-pressed", String(next));
    if (muteGlyph) muteGlyph.textContent = next ? "🔇" : "🔊";
  });
}

// ── Credit line ───────────────────────────────────────────────────────────────
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

// ── Bootstrap ─────────────────────────────────────────────────────────────────
async function boot() {
  await initSession();
  imageRange = imageFieldRange(offsets());

  audioPlayer = createAudioPlayer();
  waterfall = createWaterfall(el("waterfall-mount"), {
    getAnalyser: () => audioPlayer.getAnalyser(),
    isPlaying: () => audioPlayer.isPlaying(),
  });
  sessionLog = createSessionLog(el("session-log-stream"), el("session-progress"));
  imageReveal = createImageReveal(el("recon-image"));
  controls = createControls(runConnectedSession);

  wireTransport();
  loadCredit();
  setStatus("Set the channel, choose a bandwidth ceiling, then Run connected session.");
}

boot().catch((err) => {
  setStatus(`Failed to initialise: ${err.message}`);
  console.error("[ardop-demo] boot failed:", err);
});
