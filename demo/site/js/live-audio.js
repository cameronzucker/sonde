// live-audio.js — streaming Web Audio playback of the on-air signal, LIVE.
//
// The backend tails the session's on-air tap and streams base64 PCM chunks over SSE
// as the modems transmit. This player schedules those chunks back-to-back on the
// AudioContext clock so they play continuously (~real time, with a small lead buffer
// to absorb network jitter), and exposes an AnalyserNode the waterfall taps for a
// genuinely live spectrogram. No record-then-replay: you hear + see it happen.

export function createLiveAudio({ gain = 0.32, leadSeconds = 0.4 } = {}) {
  let ctx = null;
  let gainNode = null;
  let analyser = null;        // tapped by the waterfall
  let nextStart = 0;          // AudioContext time the next chunk should start at
  let muted = false;
  let active = false;         // accepting + scheduling chunks this session
  let scheduled = 0;          // chunks scheduled but not yet finished

  function ensureCtx() {
    if (!ctx) {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return null;
      ctx = new AC();
      gainNode = ctx.createGain();
      gainNode.gain.value = muted ? 0 : gain;
      // Fixed dB window (no per-run normalization) so a noisy low-SNR signal really
      // reads as noisier on the waterfall — honest by construction.
      analyser = ctx.createAnalyser();
      analyser.fftSize = 1024;
      analyser.smoothingTimeConstant = 0.55;
      analyser.minDecibels = -95;
      analyser.maxDecibels = -20;
      analyser.connect(gainNode);
      gainNode.connect(ctx.destination);
    }
    if (ctx.state === "suspended") ctx.resume();
    return ctx;
  }

  /** Unlock audio on the user gesture (the Connect click) and reset the clock. */
  function start() {
    if (!ensureCtx()) return;
    active = true;
    nextStart = 0; // first enqueue() seeds it with a lead
  }

  /** Schedule one chunk of Float32 PCM for continuous playback. */
  function enqueue(samples, sampleRate) {
    if (!active || !ctx || !samples || !samples.length) return;
    const buf = ctx.createBuffer(1, samples.length, sampleRate || 12000);
    if (buf.copyToChannel) buf.copyToChannel(samples, 0);
    else buf.getChannelData(0).set(samples);
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(analyser); // analyser → gain → destination, so the waterfall sees it
    const now = ctx.currentTime;
    if (nextStart < now + 0.02) {
      // First chunk, or we underran (chunks arrived late) → re-seed with a lead so
      // playback is smooth rather than glitching on every late packet.
      nextStart = now + leadSeconds;
    }
    src.start(nextStart);
    nextStart += buf.duration;
    scheduled++;
    src.onended = () => { scheduled = Math.max(0, scheduled - 1); };
  }

  /** Stop accepting new chunks (session ended). Queued audio drains naturally. */
  function stop() {
    active = false;
  }

  /** True while audio is playing or buffered ahead — drives the live waterfall. */
  function isPlaying() {
    return active || scheduled > 0 || (ctx != null && nextStart > ctx.currentTime + 0.01);
  }

  function getAnalyser() { return analyser; }
  function setMuted(m) { muted = !!m; if (gainNode) gainNode.gain.value = muted ? 0 : gain; }
  function isMuted() { return muted; }

  return { start, enqueue, stop, isPlaying, getAnalyser, setMuted, isMuted };
}
