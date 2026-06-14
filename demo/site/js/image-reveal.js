// image-reveal.js — progressive recon-image render for the Sonde demo.
// ES module; no external imports; pure Canvas 2D API + browser createImageBitmap.

/**
 * Create an image-reveal controller bound to a canvas element.
 *
 * @param {HTMLCanvasElement} canvasEl
 * @returns {{ render(recoveredBytes, imageRange, fraction): void, showFailed(): void }}
 */
export function createImageReveal(canvasEl) {
  const ctx = canvasEl.getContext("2d");

  // ── cache: keyed by a cheap signature so we decode once per distinct image ──
  // signature = "length:b0:b127:blast" (sampled bytes, cheap, not cryptographic)
  let _cachedSig = null;
  let _cachedBitmap = null;   // ImageBitmap | "corrupt" | null
  let _cachedPixelData = null; // ImageData for the byte-grid (corrupt path)
  let _cachedImgBytes = null; // Uint8Array used to build byte-grid

  // ── race guard: monotonically increasing generation token ──
  let _generation = 0;
  // Latest fraction passed to render(), so an async decode that resolves a few
  // frames late draws at the CURRENT playback position, not a stale one.
  let _lastFraction = 0;

  // ── reusable offscreen canvas for the corrupt-JPEG byte-grid composite ──
  let _offscreen = null;
  let _offctx = null;

  // ── dark background color ──
  const BG = "#0b0f14";

  // ─────────────────────────── helpers ───────────────────────────

  function byteSig(bytes) {
    const n = bytes.length;
    if (n === 0) return "empty";
    const last = bytes[n - 1];
    const mid = bytes[Math.min(127, n - 1)];
    return `${n}:${bytes[0]}:${mid}:${last}`;
  }

  /** Fill the canvas with the dark BG color. */
  function paintBackground() {
    ctx.fillStyle = BG;
    ctx.fillRect(0, 0, canvasEl.width, canvasEl.height);
  }

  /**
   * Draw src (ImageBitmap or null-for-grid) clipped to fraction of canvas width.
   * @param {ImageBitmap|null} bitmap  — pass null to use byte-grid path instead
   * @param {number} fraction  0..1
   */
  function drawWithClip(bitmap, fraction) {
    const W = canvasEl.width;
    const H = canvasEl.height;
    const revealW = Math.round(W * Math.max(0, Math.min(1, fraction)));

    // Paint dark BG over the whole canvas first (clears stale pixels in unrevealed region).
    paintBackground();

    if (revealW <= 0) return;

    ctx.save();
    ctx.beginPath();
    ctx.rect(0, 0, revealW, H);
    ctx.clip();

    if (bitmap) {
      // Scale-to-fit with letterboxing.
      const scale = Math.min(W / bitmap.width, H / bitmap.height);
      const dw = bitmap.width * scale;
      const dh = bitmap.height * scale;
      const dx = (W - dw) / 2;
      const dy = (H - dh) / 2;
      ctx.drawImage(bitmap, dx, dy, dw, dh);
    } else if (_cachedPixelData) {
      // Byte-grid: putImageData ignores the clip region, so we composite via
      // a reused offscreen canvas (avoids allocating one per frame).
      if (!_offscreen || _offscreen.width !== W || _offscreen.height !== H) {
        _offscreen = new OffscreenCanvas(W, H);
        _offctx = _offscreen.getContext("2d");
      }
      _offctx.putImageData(_cachedPixelData, 0, 0);
      ctx.drawImage(_offscreen, 0, 0);
    }

    ctx.restore();
  }

  /**
   * Build an ImageData that renders each byte of imgBytes as a grayscale pixel,
   * tiled across the canvas. Stored in _cachedPixelData.
   */
  function buildByteGrid(imgBytes) {
    const W = canvasEl.width;
    const H = canvasEl.height;
    const n = imgBytes.length;

    // Choose a grid layout: cols ≈ sqrt(n * (W/H)), rows = ceil(n/cols).
    const aspect = W / H;
    const cols = Math.max(1, Math.round(Math.sqrt(n * aspect)));
    const rows = Math.max(1, Math.ceil(n / cols));

    // Cell size in pixels.
    const cw = W / cols;
    const ch = H / rows;

    const data = new ImageData(W, H);
    const buf = data.data; // Uint8ClampedArray, RGBA

    // Initialise to BG (#0b0f14 = 11, 15, 20).
    for (let i = 0; i < buf.length; i += 4) {
      buf[i] = 11; buf[i + 1] = 15; buf[i + 2] = 20; buf[i + 3] = 255;
    }

    for (let idx = 0; idx < n; idx++) {
      const row = Math.floor(idx / cols);
      const col = idx % cols;
      const v = imgBytes[idx]; // grayscale value

      const x0 = Math.round(col * cw);
      const y0 = Math.round(row * ch);
      const x1 = Math.min(W, Math.round((col + 1) * cw));
      const y1 = Math.min(H, Math.round((row + 1) * ch));

      for (let py = y0; py < y1; py++) {
        for (let px = x0; px < x1; px++) {
          const off = (py * W + px) * 4;
          buf[off] = v; buf[off + 1] = v; buf[off + 2] = v; buf[off + 3] = 255;
        }
      }
    }

    _cachedPixelData = data;
    _cachedImgBytes = imgBytes;
  }

  // ─────────────────────────── public API ────────────────────────────

  /**
   * Render the image (or byte-grid for corrupt JPEGs) clipped to [0, fraction*W].
   * Safe to call on every animation frame; decoding is cached and race-guarded.
   *
   * @param {Uint8Array|number[]} recoveredBytes  Full link payload (LinkResult.recovered_bytes)
   * @param {[number, number]} imageRange  [start, end] byte offsets of the JPEG field
   * @param {number} fraction  0..1 reveal progress
   */
  function render(recoveredBytes, imageRange, fraction) {
    _lastFraction = fraction;
    // Guard: empty payload or empty image slice → showFailed.
    if (!recoveredBytes || recoveredBytes.length === 0) {
      showFailed();
      return;
    }
    const [start, end] = imageRange;
    const imgBytes = recoveredBytes.slice(start, end);
    if (imgBytes.length === 0) {
      showFailed();
      return;
    }

    const sig = byteSig(imgBytes);

    if (sig === _cachedSig) {
      // Already decoded (success or corrupt): just re-draw the clip.
      if (_cachedBitmap === "corrupt") {
        drawWithClip(null, fraction);
      } else if (_cachedBitmap) {
        drawWithClip(_cachedBitmap, fraction);
      }
      // If _cachedBitmap is still null, a decode is in-flight; ignore (it will re-draw on completion).
      return;
    }

    // New image bytes: start a fresh decode, bump generation to cancel any stale in-flight.
    _cachedSig = sig;
    _cachedBitmap = null;
    _cachedPixelData = null;
    _cachedImgBytes = null;

    const myGen = ++_generation;

    // Draw the dark BG immediately so the canvas isn't stale while we decode.
    paintBackground();

    const bytes = imgBytes instanceof Uint8Array ? imgBytes : Uint8Array.from(imgBytes);
    const blob = new Blob([bytes], { type: "image/jpeg" });

    createImageBitmap(blob).then((bitmap) => {
      if (_generation !== myGen) return; // stale — a newer render() superseded this

      _cachedBitmap = bitmap;
      drawWithClip(bitmap, _lastFraction); // draw at the current position, not the stale one
    }).catch(() => {
      if (_generation !== myGen) return;

      // Corrupt JPEG: render byte-grid.
      _cachedBitmap = "corrupt";
      buildByteGrid(bytes);
      drawWithClip(null, _lastFraction);
    });
  }

  /**
   * Show a "DECODE FAILED — frame did not sync" placeholder.
   * Clears any cached state so the next render() re-evaluates.
   */
  function showFailed() {
    _cachedSig = null;
    _cachedBitmap = null;
    _cachedPixelData = null;
    _cachedImgBytes = null;
    ++_generation; // invalidate any in-flight decode

    const W = canvasEl.width;
    const H = canvasEl.height;

    ctx.fillStyle = BG;
    ctx.fillRect(0, 0, W, H);

    // Centered muted message.
    const line1 = "DECODE FAILED";
    const line2 = "frame did not sync";
    const fontSize = Math.round(W / 22); // scales with canvas width (~14px at 320px)

    ctx.font = `${fontSize}px "IBM Plex Mono", "Courier New", monospace`;
    ctx.fillStyle = "#5d7088";
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";

    ctx.fillText(line1, W / 2, H / 2 - fontSize * 0.9);
    ctx.fillText(line2, W / 2, H / 2 + fontSize * 0.9);

    // Reset text state so future draws are clean.
    ctx.textAlign = "start";
    ctx.textBaseline = "alphabetic";
  }

  return { render, showFailed };
}
