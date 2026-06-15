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
    // Live on-air audio: base64 S16LE PCM (per direction), decoded to Float32.
    audio: (ev) => call("onAudio", { samples: b64ToFloat32(ev.pcm), rate: ev.rate, dir: ev.dir || "fwd" }),
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

/** Hex string → Uint8Array (the delivered image bytes from a result event). */
export function hexToBytes(hex) {
  const n = (hex || "").length >> 1;
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = parseInt(hex.substr(i * 2, 2), 16);
  return out;
}

/** base64 of little-endian S16 PCM → Float32Array in [-1,1] (one live audio chunk). */
function b64ToFloat32(b64) {
  const bin = atob(b64 || "");
  const n = bin.length >> 1;
  const out = new Float32Array(n);
  for (let i = 0; i < n; i++) {
    let v = (bin.charCodeAt(i * 2)) | (bin.charCodeAt(i * 2 + 1) << 8); // little-endian
    if (v >= 0x8000) v -= 0x10000; // signed
    out[i] = v / 32768;
  }
  return out;
}
