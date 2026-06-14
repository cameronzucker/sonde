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
