// waterfall.js — live, real-time 3D scrolling spectrogram (ES module).
// Exports: createWaterfall(mountEl, { getAnalyser, isPlaying }) -> { reset, dispose }
//
// This is a GENUINE live spectrogram, not a static render with a moving plane: each
// animation frame it pulls a fresh FFT slice from the Web Audio AnalyserNode tapping
// the playing transmission audio, pushes it on as the newest time-row at the front,
// and scrolls older rows back — a 3D Three.js surface that flows while audio plays.
//
// It is honest about SNR BY CONSTRUCTION: the magnitudes are the real playing audio
// on a fixed dB scale (analyser.min/maxDecibels), so a noisy low-SNR transmission
// reads as a noisy surface and a clean high-SNR one as a clean band over a dark
// floor — no per-run normalization. When audio isn't playing, the surface holds.

import * as THREE from "../vendor/three.module.js";
import { viridis } from "./format.js";

// Surface extents / look.
const HALF_X = 4.0;   // frequency axis → [-HALF_X, +HALF_X]
const HALF_Z = 2.6;   // time axis → [-HALF_Z (newest/front), +HALF_Z (oldest/back)]
const Y_SCALE = 2.3;  // vertical exaggeration
const HEIGHT_GAMMA = 1.25; // >1 tames the floor so the band stands out as relief

// Grid. COLS = frequency bins across the SSB passband; ROWS = time history depth.
const COLS = 56;
const ROWS = 110;
const BAND_LO_HZ = 200;
const BAND_HI_HZ = 2800;

// Camera.
const CAM_RADIUS = 7.5;
const CAM_Y_MIN = 1.2;
const CAM_Y_MAX = 7.5;

export function createWaterfall(mountEl, { getAnalyser, isPlaying } = {}) {
  // ── Renderer / scene / lights ──────────────────────────────────────────────
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
  renderer.setClearColor(0x000000, 0);
  mountEl.appendChild(renderer.domElement);

  const scene = new THREE.Scene();
  scene.add(new THREE.AmbientLight(0xffffff, 0.65));
  const dirLight = new THREE.DirectionalLight(0xffffff, 0.85);
  dirLight.position.set(4, 9, 6);
  scene.add(dirLight);

  // ── Camera + manual orbit (drag) ─────────────────────────────────────────--
  const camera = new THREE.PerspectiveCamera(
    45, mountEl.clientWidth / Math.max(mountEl.clientHeight, 1), 0.1, 100);
  let orbitAngle = Math.PI / 6;
  let camY = 4.2;
  const orbitCenter = new THREE.Vector3(0, 0.3, 0);
  function updateCamera() {
    camera.position.set(CAM_RADIUS * Math.cos(orbitAngle), camY, CAM_RADIUS * Math.sin(orbitAngle));
    camera.lookAt(orbitCenter);
  }
  updateCamera();

  let dragging = false, lastPX = 0, lastPY = 0;
  renderer.domElement.style.touchAction = "none";
  renderer.domElement.style.cursor = "grab";
  renderer.domElement.addEventListener("pointerdown", (e) => {
    dragging = true; lastPX = e.clientX; lastPY = e.clientY;
    renderer.domElement.style.cursor = "grabbing";
    renderer.domElement.setPointerCapture?.(e.pointerId);
  });
  renderer.domElement.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    orbitAngle -= (e.clientX - lastPX) * 0.01;
    camY = Math.max(CAM_Y_MIN, Math.min(CAM_Y_MAX, camY + (e.clientY - lastPY) * 0.03));
    lastPX = e.clientX; lastPY = e.clientY;
    updateCamera();
  });
  const endDrag = (e) => {
    dragging = false; renderer.domElement.style.cursor = "grab";
    if (e?.pointerId !== undefined) renderer.domElement.releasePointerCapture?.(e.pointerId);
  };
  renderer.domElement.addEventListener("pointerup", endDrag);
  renderer.domElement.addEventListener("pointercancel", endDrag);
  renderer.domElement.addEventListener("pointerleave", endDrag);

  // ── Surface geometry: fixed x/z topology, per-frame y + colour ─────────────
  const vtxCount = ROWS * COLS;
  const positions = new Float32Array(vtxCount * 3);
  const colors = new Float32Array(vtxCount * 3);
  for (let r = 0; r < ROWS; r++) {
    for (let c = 0; c < COLS; c++) {
      const i = (r * COLS + c) * 3;
      positions[i] = (c / (COLS - 1)) * (HALF_X * 2) - HALF_X;     // freq
      positions[i + 1] = 0;                                         // magnitude (updated)
      positions[i + 2] = (r / (ROWS - 1)) * (HALF_Z * 2) - HALF_Z; // time (r=0 → front)
      colors[i] = 0.04; colors[i + 1] = 0.0; colors[i + 2] = 0.08; // viridis(0)-ish
    }
  }
  const indices = new Uint32Array((ROWS - 1) * (COLS - 1) * 6);
  let ii = 0;
  for (let r = 0; r < ROWS - 1; r++) {
    for (let c = 0; c < COLS - 1; c++) {
      const tl = r * COLS + c, tr = tl + 1, bl = tl + COLS, br = bl + 1;
      indices[ii++] = tl; indices[ii++] = bl; indices[ii++] = tr;
      indices[ii++] = tr; indices[ii++] = bl; indices[ii++] = br;
    }
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  geo.setAttribute("color", new THREE.BufferAttribute(colors, 3));
  geo.setIndex(new THREE.BufferAttribute(indices, 1));
  geo.computeVertexNormals();
  const material = new THREE.MeshLambertMaterial({ vertexColors: true, side: THREE.DoubleSide });
  const mesh = new THREE.Mesh(geo, material);
  scene.add(mesh);

  // ── Reference grid + axis labels (X = frequency, Z = time) ─────────────────
  // A static floor grid under the surface so you can read the spectrum: vertical
  // lines at frequency ticks, horizontals marking time, and Hz labels along the
  // camera-facing edge. Added to the scene (not the mesh) so it doesn't scroll.
  const FREQ_TICKS = [500, 1000, 1500, 2000, 2500];
  const freqToX = (f) => ((f - BAND_LO_HZ) / (BAND_HI_HZ - BAND_LO_HZ)) * (HALF_X * 2) - HALF_X;
  (function buildAxes() {
    const y0 = -0.03;
    const seg = [];
    for (const f of FREQ_TICKS) { const x = freqToX(f); seg.push(x, y0, -HALF_Z, x, y0, HALF_Z); }
    const TROWS = 4;
    for (let i = 0; i <= TROWS; i++) {
      const z = -HALF_Z + (i / TROWS) * (HALF_Z * 2);
      seg.push(-HALF_X, y0, z, HALF_X, y0, z);
    }
    const gGeo = new THREE.BufferGeometry();
    gGeo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(seg), 3));
    scene.add(new THREE.LineSegments(gGeo,
      new THREE.LineBasicMaterial({ color: 0x2f4a58, transparent: true, opacity: 0.5 })));

    const labels = [];
    function makeLabel(text, scaleX = 1.1) {
      const cv = document.createElement("canvas"); cv.width = 256; cv.height = 64;
      const cx = cv.getContext("2d");
      cx.fillStyle = "#9fc0d4"; cx.font = "bold 34px 'IBM Plex Mono', monospace";
      cx.textAlign = "center"; cx.textBaseline = "middle"; cx.fillText(text, 128, 34);
      const tex = new THREE.CanvasTexture(cv);
      const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, transparent: true, depthTest: false }));
      sp.scale.set(scaleX, scaleX * 0.25, 1);
      labels.push({ sp, tex });
      return sp;
    }
    for (const f of FREQ_TICKS) {
      const sp = makeLabel(String(f));
      sp.position.set(freqToX(f), y0 + 0.05, HALF_Z + 0.38);
      scene.add(sp);
    }
    const cap = makeLabel("FREQ (Hz)", 1.5);
    cap.position.set(0, y0 + 0.05, HALF_Z + 1.0);
    scene.add(cap);
    buildAxes.labels = labels;
  })();

  // ── Rolling spectral history (ring buffer of normalized rows) ──────────────
  const hist = new Float32Array(ROWS * COLS); // 0..1
  let writeRow = 0;
  let freqBins = null; // Uint8Array sized to analyser.frequencyBinCount
  let lobin = 0, hibin = 0;

  function ensureBins(analyser) {
    if (freqBins && freqBins.length === analyser.frequencyBinCount) return;
    freqBins = new Uint8Array(analyser.frequencyBinCount);
    const binHz = analyser.context.sampleRate / analyser.fftSize;
    lobin = Math.max(0, Math.floor(BAND_LO_HZ / binHz));
    hibin = Math.min(analyser.frequencyBinCount - 1, Math.ceil(BAND_HI_HZ / binHz));
  }

  // Pull one FFT slice → newest row (averaged into COLS, light 3-tap freq smooth).
  function pushRow(analyser) {
    ensureBins(analyser);
    analyser.getByteFrequencyData(freqBins);
    const span = hibin - lobin;
    writeRow = (writeRow + 1) % ROWS;
    const base = writeRow * COLS;
    for (let c = 0; c < COLS; c++) {
      // Average the analyser bins falling in this column.
      const b0 = lobin + Math.floor((c / COLS) * span);
      const b1 = lobin + Math.floor(((c + 1) / COLS) * span);
      let sum = 0, n = 0;
      for (let b = b0; b <= b1; b++) { sum += freqBins[b]; n++; }
      hist[base + c] = n ? sum / n / 255 : 0;
    }
    // Light freq smoothing to reduce single-bin spikes.
    for (let c = 1; c < COLS - 1; c++) {
      hist[base + c] = 0.25 * hist[base + c - 1] + 0.5 * hist[base + c] + 0.25 * hist[base + c + 1];
    }
  }

  // Rewrite y + colour from the ring buffer (row 0 = newest at the front).
  function refreshGeometry() {
    const pos = geo.attributes.position.array;
    const col = geo.attributes.color.array;
    for (let r = 0; r < ROWS; r++) {
      const bufRow = ((writeRow - r) % ROWS + ROWS) % ROWS;
      for (let c = 0; c < COLS; c++) {
        const v = hist[bufRow * COLS + c];
        const i = (r * COLS + c) * 3;
        pos[i + 1] = Math.pow(v, HEIGHT_GAMMA) * Y_SCALE;
        const [cr, cg, cb] = viridis(v);
        col[i] = cr / 255; col[i + 1] = cg / 255; col[i + 2] = cb / 255;
      }
    }
    geo.attributes.position.needsUpdate = true;
    geo.attributes.color.needsUpdate = true;
    geo.computeVertexNormals();
  }

  // ── Resize ─────────────────────────────────────────────────────────────────
  function onResize() {
    const w = mountEl.clientWidth, h = mountEl.clientHeight;
    if (!w || !h) return;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }
  let ro = null;
  if (typeof ResizeObserver !== "undefined") { ro = new ResizeObserver(onResize); ro.observe(mountEl); }
  else window.addEventListener("resize", onResize);
  onResize();

  // ── Animation loop ───────────────────────────────────────────────────────--
  let rafId = null;
  function animate() {
    rafId = requestAnimationFrame(animate);
    mountEl.classList.add("is-ready"); // clear the boot overlay once we're rendering
    const analyser = getAnalyser?.();
    if (analyser && isPlaying?.()) {
      pushRow(analyser);
      refreshGeometry();
    }
    renderer.render(scene, camera);
  }
  rafId = requestAnimationFrame(animate);

  // ── Public API ───────────────────────────────────────────────────────────--
  /** Clear the spectral history (e.g. on a new run). */
  function reset() {
    hist.fill(0);
    writeRow = 0;
    refreshGeometry();
  }

  function dispose() {
    if (rafId !== null) cancelAnimationFrame(rafId);
    if (ro) ro.disconnect(); else window.removeEventListener("resize", onResize);
    geo.dispose(); material.dispose(); renderer.dispose();
    if (renderer.domElement.parentNode === mountEl) mountEl.removeChild(renderer.domElement);
  }

  return { reset, dispose };
}
