// live-audio.js — streaming Web Audio playback of the on-air signal, LIVE.
//
// ARDOP is half-duplex single-frequency, so the backend streams BOTH directions
// (fwd = A->B data, rev = B->A acks) tagged by direction. This player keeps a
// separate scheduling queue + AnalyserNode per direction (so each can drive its own
// waterfall and you SEE the data sender and the receiver's ACK bursts alternating),
// but mixes both into the one shared output so you HEAR the whole conversation.

export function createLiveAudio({ gain = 0.32, leadSeconds = 0.4 } = {}) {
  let ctx = null;
  let gainNode = null;
  const chans = {};          // dir -> { analyser, nextStart, scheduled }
  let muted = false;
  let active = false;        // accepting + scheduling chunks this session

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

  function ensureChan(dir) {
    if (chans[dir]) return chans[dir];
    // Fixed dB window (no per-run normalization) so a noisy direction reads noisy.
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 1024;
    analyser.smoothingTimeConstant = 0.55;
    analyser.minDecibels = -95;
    analyser.maxDecibels = -20;
    analyser.connect(gainNode); // each direction -> shared gain -> destination
    chans[dir] = { analyser, nextStart: 0, scheduled: 0 };
    return chans[dir];
  }

  /** Unlock audio on the user gesture (the Connect click); reset both clocks. */
  function start() {
    if (!ensureCtx()) return;
    ensureChan("fwd");
    ensureChan("rev");
    active = true;
    for (const d in chans) chans[d].nextStart = 0;
  }

  /** Schedule one chunk of Float32 PCM for direction `dir`'s continuous playback. */
  function enqueue(samples, sampleRate, dir = "fwd") {
    if (!active || !ctx || !samples || !samples.length) return;
    const ch = ensureChan(dir);
    const buf = ctx.createBuffer(1, samples.length, sampleRate || 12000);
    if (buf.copyToChannel) buf.copyToChannel(samples, 0);
    else buf.getChannelData(0).set(samples);
    const src = ctx.createBufferSource();
    src.buffer = buf;
    src.connect(ch.analyser);
    const now = ctx.currentTime;
    if (ch.nextStart < now + 0.02) ch.nextStart = now + leadSeconds; // (re)seed lead
    src.start(ch.nextStart);
    ch.nextStart += buf.duration;
    ch.scheduled++;
    src.onended = () => { ch.scheduled = Math.max(0, ch.scheduled - 1); };
  }

  /** Stop accepting new chunks (session ended). Queued audio drains naturally. */
  function stop() {
    active = false;
  }

  /** True while direction `dir` is playing/buffered — drives that waterfall. */
  function isPlaying(dir = "fwd") {
    const ch = chans[dir];
    if (!ch) return false;
    return active || ch.scheduled > 0 || (ctx != null && ch.nextStart > ctx.currentTime + 0.01);
  }

  function getAnalyser(dir = "fwd") { return chans[dir] ? chans[dir].analyser : null; }
  function setMuted(m) { muted = !!m; if (gainNode) gainNode.gain.value = muted ? 0 : gain; }
  function isMuted() { return muted; }

  return { start, enqueue, stop, isPlaying, getAnalyser, setMuted, isMuted };
}
