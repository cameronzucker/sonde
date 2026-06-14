// audio.js — Web Audio playback of the modulated link waveform (ES module).
// Lets the operator *hear* the real channel-impaired signal as a sniff test:
// does it sound like a plausible HF data signal? The AudioContext is created
// lazily on the first play() call (a user gesture), per browser autoplay policy.

export function createAudioPlayer({ gain = 0.3 } = {}) {
  let ctx = null;
  let gainNode = null;
  let buffer = null; // AudioBuffer, built lazily once a ctx exists
  let pendingSamples = null; // Float32Array awaiting a ctx
  let sampleRate = 48000;
  let source = null;
  let muted = false;
  let onEndedCb = null;

  function ensureCtx() {
    if (!ctx) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return null;
      ctx = new AC();
      gainNode = ctx.createGain();
      gainNode.gain.value = muted ? 0 : gain;
      gainNode.connect(ctx.destination);
    }
    if (ctx.state === "suspended") ctx.resume();
    return ctx;
  }

  function buildBuffer() {
    if (buffer || !pendingSamples || !ctx) return;
    const buf = ctx.createBuffer(1, pendingSamples.length, sampleRate);
    if (buf.copyToChannel) buf.copyToChannel(pendingSamples, 0);
    else buf.getChannelData(0).set(pendingSamples);
    buffer = buf;
  }

  /** Load fresh waveform samples (Float32Array). Stops any current playback. */
  function load(samples, sr) {
    stop();
    sampleRate = sr || 48000;
    buffer = null;
    pendingSamples = samples && samples.length ? samples : null;
  }

  function stop() {
    if (source) {
      try {
        source.onended = null;
        source.stop();
      } catch {
        /* already stopped */
      }
      source = null;
    }
  }

  /** Play from offsetFraction (0..1) of the waveform. No-op if nothing loaded. */
  function play(offsetFraction = 0) {
    if (!pendingSamples || !ensureCtx()) return;
    buildBuffer();
    if (!buffer) return;
    stop();
    source = ctx.createBufferSource();
    source.buffer = buffer;
    source.connect(gainNode);
    source.onended = () => {
      source = null;
      if (onEndedCb) onEndedCb();
    };
    const off = Math.max(0, Math.min(0.999, offsetFraction)) * buffer.duration;
    source.start(0, off);
  }

  function setMuted(m) {
    muted = !!m;
    if (gainNode) gainNode.gain.value = muted ? 0 : gain;
  }
  function isMuted() {
    return muted;
  }
  function isPlaying() {
    return source !== null;
  }
  function onEnded(cb) {
    onEndedCb = cb;
  }

  return { load, play, stop, setMuted, isMuted, isPlaying, onEnded };
}
