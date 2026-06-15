// s-meter.js — live per-station S-meter (ES module).
//
// Reads RMS from one direction's AnalyserNode each frame and renders it as an
// amateur-radio S-meter: S-units (6 dB each, the conventional spacing) with over-S9
// shown as "S9+N dB". Honest: it's the ACTUAL on-air audio level for that station,
// so the A->B meter rises on data bursts and B->A on the receiver's ACK/NAK bursts.
//
// There is no calibrated RF reference here (it's audio through a simulated channel),
// so S9 is anchored at a strong on-air burst (~-5 dBFS) and the scale runs down from
// there at 6 dB/S-unit — a relative signal-strength display, not a calibrated dBm.

const S9_DBFS = -5; // a strong on-air burst reads ~S9
const DB_PER_S = 6; // conventional S-unit spacing

/** dBFS → { fill: 0..1 (S1..S9 across the bar), text: "S7" | "S9+12", over: bool } */
function readS(db) {
  if (db <= -120) return { fill: 0, text: "S—", over: false };
  if (db > S9_DBFS) {
    return { fill: 1, text: `S9+${Math.round(db - S9_DBFS)}`, over: true };
  }
  const s = 9 + (db - S9_DBFS) / DB_PER_S; // continuous S-units (≤ 9 here)
  const fill = Math.max(0, Math.min(1, (s - 1) / 8)); // S1→0, S9→1
  return { fill, text: `S${Math.max(0, Math.round(s))}`, over: false };
}

export function createSMeter(el, { getAnalyser } = {}) {
  const fillEl = el.querySelector(".vu__fill");
  const peakEl = el.querySelector(".vu__peak");
  const readEl = el.querySelector(".vu__read");
  let buf = null;
  let peak = 0;
  let raf = null;

  function frame() {
    raf = requestAnimationFrame(frame);
    const an = getAnalyser?.();
    if (!an || !an.getFloatTimeDomainData) return;
    if (!buf || buf.length !== an.fftSize) buf = new Float32Array(an.fftSize);
    an.getFloatTimeDomainData(buf);

    let sum = 0;
    for (let i = 0; i < buf.length; i++) sum += buf[i] * buf[i];
    const rms = Math.sqrt(sum / buf.length);
    const db = rms > 1e-5 ? 20 * Math.log10(rms) : -120;
    const { fill, text, over } = readS(db);

    peak = fill >= peak ? fill : Math.max(fill, peak * 0.94); // fast attack, slow fall
    if (fillEl) fillEl.style.width = `${(fill * 100).toFixed(1)}%`;
    if (peakEl) peakEl.style.left = `${(peak * 100).toFixed(1)}%`;
    if (readEl) {
      readEl.textContent = text;
      readEl.classList.toggle("vu__read--hot", over);
    }
  }
  raf = requestAnimationFrame(frame);

  return {
    reset() {
      peak = 0;
      if (fillEl) fillEl.style.width = "0%";
      if (peakEl) peakEl.style.left = "0%";
      if (readEl) {
        readEl.textContent = "S—";
        readEl.classList.remove("vu__read--hot");
      }
    },
    dispose() { if (raf) cancelAnimationFrame(raf); },
  };
}
