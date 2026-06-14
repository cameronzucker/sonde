// ardop-engine.js — the demo's data source, re-anchored on the live ARDOP backend.
//
// Replaces the in-browser Sonde WASM engine: instead of modulating/demodulating in
// the page, every run hits the backend (`demo/ardop/server.py`), which sends a real
// ~3 KB image across real ardopcf frames through hf-channel-sim and returns the
// per-frame decode result + reassembled bytes + spectrogram + an audio URL. ardopcf
// is an external reference modem (clean-sheet / ADR 0014) — never reimplemented here.
//
// Same shape of contract the views already expect (spectrogram for the waterfall,
// recovered_bytes for the image reveal, a per-frame timeline for playback), so the
// view modules are reused unchanged.

/** Sample rate is whatever the backend reports per run (ardopcf WAVs are 12 kHz). */
export const SAMPLE_RATE_HZ = 12000;

let _modes = [];          // [{id, implemented, constellation, capacity_bytes}]
let _autoLadder = [];     // [[snr_threshold, frame_id], ...] high→low
let _offsets = null;      // {total_len, fields[], image_byte_len}

/** Load the mode catalogue + payload field offsets. Call once at startup. */
export async function initEngine(base = ".") {
  const [modesResp, offResp] = await Promise.all([
    fetch(`${base}/api/modes`),
    fetch(`${base}/assets/payload.offsets.json`),
  ]);
  if (!modesResp.ok) throw new Error(`backend /api/modes ${modesResp.status} — is the server running? (python3 demo/ardop/server.py)`);
  if (!offResp.ok) throw new Error(`payload.offsets.json ${offResp.status}`);
  const cat = await modesResp.json();
  _modes = cat.modes || [];
  _autoLadder = cat.auto_ladder || [];
  _offsets = await offResp.json();
  return { modes: _modes, offsets: _offsets };
}

export function offsets() { return _offsets; }

/** Mode catalogue for the picker (already shaped {id, implemented, constellation}). */
export function listModes() { return _modes; }

/** The Auto ladder's pick for a measured SNR (mirrors the server; for display). */
export function recommendMode(snrDb) {
  for (const [thresh, frame] of _autoLadder) {
    if (snrDb >= thresh) return frame;
  }
  return _modes.length ? _modes[0].id : "4PSK.500.100.E";
}

/** Mean post-decode BER over frames that decoded (fraction 0..1), or NaN if none. */
function overallBer(frames) {
  const decoded = frames.filter((f) => f.decoded && typeof f.ber_max === "number");
  if (!decoded.length) return NaN;
  return decoded.reduce((a, f) => a + f.ber_max, 0) / decoded.length / 100;
}

/**
 * Run one multi-frame transfer on the backend. ASYNC (network round-trip + real
 * modem runs). Pass frame="auto" (or null) to let the backend's Auto ladder pick.
 * Returns a view-friendly result, or throws on a backend {error}.
 */
export async function runLink(frame, snrDb, condition, seed) {
  const f = frame || "auto";
  const url = `./api/run?frame=${encodeURIComponent(f)}&snr=${snrDb}&condition=${encodeURIComponent(condition)}&seed=${seed >>> 0}`;
  const resp = await fetch(url);
  const r = await resp.json();
  if (!resp.ok || r.error) throw new Error(r.error || `backend ${resp.status}`);

  const recovered = hexToBytes(r.recovered_hex);
  const deliveredBytes = r.summary.frames_decoded * r.capacity;
  const throughputBps = r.duration_s > 0 ? (deliveredBytes * 8) / r.duration_s : 0;

  return {
    mode_id: r.frame,
    constellation: r.constellation,
    auto: r.auto,
    // ARDOP reports a link quality score, not an SNR estimate. The "measured" SNR
    // here is the channel SNR we drove the sim with — labeled as such in the UI.
    set_snr_db: r.snr_db,
    ber: overallBer(r.frames),
    throughput_bps: throughputBps,
    time_to_deliver_s: r.duration_s,
    recovered_ok: r.summary.frames_decoded === r.summary.frames_total,
    recovered_bytes: recovered,
    spectrogram: r.spectrogram,
    frames: r.frames,
    // Playback indexes a fraction → element via t_start_s; one entry per frame so
    // the timeline advances the image reveal + frame log frame-by-frame.
    symbols: r.frames.map((fr) => ({ t_start_s: fr.t_start_s })),
    summary: r.summary,
    audio_url: r.audio_url,
  };
}

/** Fetch + decode the transmission WAV → Float32 samples for the audio player. */
export async function fetchAudio(audioUrl) {
  const resp = await fetch(`.${audioUrl}`);
  if (!resp.ok) return { samples: null, sampleRate: SAMPLE_RATE_HZ };
  return parseWav(await resp.arrayBuffer());
}

// ── helpers ──────────────────────────────────────────────────────────────────

function hexToBytes(hex) {
  const n = hex.length >> 1;
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
  let off = 12; // past RIFF + size + "WAVE"
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
    off = body + size + (size & 1); // chunks are word-aligned
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
