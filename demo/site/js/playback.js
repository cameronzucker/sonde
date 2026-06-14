// playback.js — timeline / now-marker state machine for a LinkResult (ES module).
// Drives onFrame(fraction, currentSymbolIndex) via requestAnimationFrame, advancing
// over result.time_to_deliver_s scaled by playback speed.

/**
 * createPlayback({ onFrame, onDone })
 *   onFrame(fraction, symbolIndex) — called each animation frame (and on scrub).
 *   onDone()                       — called once when playback reaches the end.
 * Returns { load, play, pause, scrub, setSpeed }.
 */
export function createPlayback({ onFrame, onDone }) {
  let result = null;
  let durationS = 0;       // wall-clock length of the link (seconds)
  let elapsedS = 0;        // current playback position (seconds)
  let speed = 1;
  let rafId = null;
  let lastNow = null;

  // Symbol t_start_s values, ascending, for fraction→index mapping.
  let symbolStarts = [];

  /** Largest symbol index whose t_start_s <= elapsed seconds (or -1 before the first). */
  function symbolIndexAt(seconds) {
    let idx = -1;
    for (let i = 0; i < symbolStarts.length; i++) {
      if (symbolStarts[i] <= seconds) idx = i;
      else break;
    }
    return idx;
  }

  function emit() {
    const frac = durationS > 0 ? Math.max(0, Math.min(1, elapsedS / durationS)) : 1;
    onFrame(frac, symbolIndexAt(elapsedS));
  }

  function stopLoop() {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    lastNow = null;
  }

  function tick(now) {
    const dt = lastNow === null ? 0 : (now - lastNow) / 1000; // seconds
    lastNow = now;
    elapsedS += dt * speed;

    if (elapsedS >= durationS) {
      elapsedS = durationS;
      emit();
      stopLoop();
      if (onDone) onDone();
      return;
    }
    emit();
    rafId = requestAnimationFrame(tick);
  }

  /** Store a fresh LinkResult and reset to the start. Does not auto-play. */
  function load(r) {
    stopLoop();
    result = r;
    durationS = Math.max(0, Number(r?.time_to_deliver_s) || 0);
    symbolStarts = Array.isArray(r?.symbols) ? r.symbols.map((s) => Number(s.t_start_s) || 0) : [];
    elapsedS = 0;
    if (r) emit(); // load(null) just resets state (e.g. on engine error) — no frame
  }

  function play() {
    if (!result) return;
    if (rafId !== null) return; // already playing
    if (elapsedS >= durationS) elapsedS = 0; // replay from start if at the end
    lastNow = null;
    rafId = requestAnimationFrame(tick);
  }

  function pause() {
    stopLoop();
  }

  /** Jump to a position (0..1) and pause there. */
  function scrub(fraction) {
    if (!result) return;
    stopLoop();
    const f = Math.max(0, Math.min(1, fraction));
    elapsedS = f * durationS;
    emit();
  }

  function setSpeed(x) {
    const v = Number(x);
    if (v > 0) speed = v;
  }

  function isPlaying() {
    return rafId !== null;
  }

  return { load, play, pause, scrub, setSpeed, isPlaying };
}
