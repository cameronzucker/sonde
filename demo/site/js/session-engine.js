// session-engine.js — the demo's data source for REAL ARDOP connected mode.
//
// Where ardop-engine.js ran a one-way PHY transfer (request → one JSON response),
// this drives a full connected SESSION: it opens a Server-Sent Events stream to the
// backend (`/api/session`), which stands up two ardopcf stations, runs a real
// CONNECT handshake + bandwidth negotiation + adaptive-rate ARQ data transfer
// through hf-channel-sim, and emits a live timeline of milestone events. ardopcf is
// an external reference modem (clean-sheet / ADR 0014) — never reimplemented here.
//
// Event vocabulary (each SSE `data:` line is one JSON object with a "t" type):
//   phase      {phase, msg}                     coarse lifecycle step
//   station    {station, call, role}            A=caller / B=answerer identities
//   host       {station, line}                  raw ardopcf protocol line
//   connected  {call_a, call_b, bandwidth, ...} cemented connection + negotiated BW
//   data_start {bytes, name}                    payload handed to the ARQ buffer
//   progress   {received, total}                ARQ bytes delivered so far
//   mode       {modes[], current}               data modes used (rate adaptation)
//   delivered  {received, total, intact}        transfer finished
//   result     {outcome, bandwidth, modes[], received, total, duration_s,
//               image_hex, audio_url}           terminal summary (+ on-air audio)
//   error      {msg}                            failure (stream still ends with done)
//   done       {}                               stream terminator — engine closes here

export const SAMPLE_RATE_HZ = 12000;

let _offsets = null; // {total_len, fields[], image_byte_len}

/** Load the payload field offsets (for the recon-image reveal). Call once at startup. */
export async function initSession(base = ".") {
  const resp = await fetch(`${base}/assets/payload.offsets.json`);
  if (!resp.ok) throw new Error(`payload.offsets.json ${resp.status}`);
  _offsets = await resp.json();
  return { offsets: _offsets };
}

export function offsets() { return _offsets; }

/**
 * Run ONE connected session, streaming milestones to `handlers`.
 *
 * @param {{snrDb:number, condition:string, seed:number, arqbw:string}} params
 * @param {object} handlers  any subset of {onPhase,onStation,onHost,onConnected,
 *        onDataStart,onProgress,onMode,onDelivered,onResult,onError,onDone}
 * @returns {{close():void}}  call close() to cancel the session (tears down the
 *          backend's ardopcf stations via the broken-pipe → abort path).
 */
export function runSession(params, handlers = {}) {
  const q = new URLSearchParams({
    snr: String(params.snrDb),
    condition: params.condition,
    seed: String(params.seed >>> 0),
    arqbw: params.arqbw,
  });
  const es = new EventSource(`./api/session?${q.toString()}`);
  let finished = false;

  const call = (name, ev) => { try { handlers[name] && handlers[name](ev); } catch (e) { console.error(e); } };

  const DISPATCH = {
    phase: (ev) => call("onPhase", ev),
    station: (ev) => call("onStation", ev),
    host: (ev) => call("onHost", ev),
    connected: (ev) => call("onConnected", ev),
    data_start: (ev) => call("onDataStart", ev),
    progress: (ev) => call("onProgress", ev),
    mode: (ev) => call("onMode", ev),
    delivered: (ev) => call("onDelivered", ev),
    result: (ev) => call("onResult", ev),
    error: (ev) => call("onError", ev),
  };

  es.onmessage = (e) => {
    let ev;
    try { ev = JSON.parse(e.data); } catch { return; }
    if (ev.t === "done") {
      finished = true;
      es.close();          // stop EventSource auto-reconnect (would start a NEW session)
      call("onDone", ev);
      return;
    }
    const fn = DISPATCH[ev.t];
    if (fn) fn(ev);
  };

  es.onerror = () => {
    // EventSource fires onerror on normal close too; only surface a real drop.
    if (finished) return;
    finished = true;
    es.close();
    call("onError", { t: "error", msg: "session stream lost (backend closed or unreachable)" });
    call("onDone", { t: "done" });
  };

  return {
    close() {
      if (finished) return;
      finished = true;
      es.close();
    },
  };
}

/** Fetch + decode the session's on-air WAV → Float32 samples for the waterfall. */
export async function fetchAudio(audioUrl) {
  const resp = await fetch(`.${audioUrl}`);
  if (!resp.ok) return { samples: null, sampleRate: SAMPLE_RATE_HZ };
  return parseWav(await resp.arrayBuffer());
}

/** Hex string → Uint8Array (the delivered image bytes from a result event). */
export function hexToBytes(hex) {
  const n = (hex || "").length >> 1;
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}

/**
 * Minimal RIFF/WAVE parser for 8/16-bit PCM mono → {samples: Float32Array, sampleRate}.
 * Walks the chunk list (don't assume a fixed 44-byte header).
 */
function parseWav(buf) {
  const dv = new DataView(buf);
  if (dv.getUint32(0, false) !== 0x52494646 /* "RIFF" */) throw new Error("not a RIFF file");
  let sampleRate = SAMPLE_RATE_HZ;
  let bits = 16;
  let off = 12;
  let dataOff = -1;
  let dataLen = 0;
  while (off + 8 <= dv.byteLength) {
    const id = dv.getUint32(off, false);
    const size = dv.getUint32(off + 4, true);
    const body = off + 8;
    if (id === 0x666d7420 /* "fmt " */) {
      sampleRate = dv.getUint32(body + 4, true);
      bits = dv.getUint16(body + 14, true);
    } else if (id === 0x64617461 /* "data" */) {
      dataOff = body;
      dataLen = size;
    }
    off = body + size + (size & 1);
  }
  if (dataOff < 0) throw new Error("no data chunk in WAV");
  if (bits === 16) {
    const n = dataLen >> 1;
    const out = new Float32Array(n);
    for (let i = 0; i < n; i++) out[i] = dv.getInt16(dataOff + i * 2, true) / 32768;
    return { samples: out, sampleRate };
  }
  if (bits === 8) {
    const out = new Float32Array(dataLen);
    for (let i = 0; i < dataLen; i++) out[i] = (dv.getUint8(dataOff + i) - 128) / 128;
    return { samples: out, sampleRate };
  }
  throw new Error(`unsupported WAV bit depth ${bits}`);
}
