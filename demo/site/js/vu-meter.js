// vu-meter.js — live per-station audio-level (V/U) meter (ES module).
//
// Reads RMS from one direction's AnalyserNode each frame and drives a fill bar +
// peak-hold marker + dB readout. Honest: it's the ACTUAL on-air audio level for that
// station, so the A->B meter pulses on data bursts and the B->A meter pulses on the
// receiver's ACK/NAK bursts — the half-duplex turn-taking, as levels.

const DB_FLOOR = -48; // bottom of the meter scale (dBFS)

export function createMeter(vuEl, { getAnalyser } = {}) {
  const fill = vuEl.querySelector(".vu__fill");
  const peakEl = vuEl.querySelector(".vu__peak");
  const dbEl = vuEl.querySelector(".vu__db");
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
    const level = Math.max(0, Math.min(1, (db - DB_FLOOR) / -DB_FLOOR));

    peak = level >= peak ? level : Math.max(level, peak * 0.94); // fast attack, slow fall
    if (fill) fill.style.width = `${(level * 100).toFixed(1)}%`;
    if (peakEl) peakEl.style.left = `${(peak * 100).toFixed(1)}%`;
    if (dbEl) dbEl.textContent = db <= -120 ? "—" : `${Math.round(db)} dB`;
  }
  raf = requestAnimationFrame(frame);

  return {
    reset() {
      peak = 0;
      if (fill) fill.style.width = "0%";
      if (peakEl) peakEl.style.left = "0%";
      if (dbEl) dbEl.textContent = "—";
    },
    dispose() { if (raf) cancelAnimationFrame(raf); },
  };
}
