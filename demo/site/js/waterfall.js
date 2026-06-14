// waterfall.js — Three.js 3D spectrogram surface (ES module).
// Exports: createWaterfall(mountEl) -> { setData, setNow, dispose }
//
// Three.js core only (no OrbitControls). Camera auto-orbits slowly each frame.
// Surface: one BufferGeometry, rows×cols vertices, vertex-colored via viridis.
// Sweep plane: translucent vertical plane repositioned by setNow(fraction).

import * as THREE from "../vendor/three.module.js";
import { viridis } from "./format.js";

// World-space extents for the surface mesh.
const HALF_X = 4.0; // cols mapped to [-HALF_X, +HALF_X]
const HALF_Z = 2.5; // rows  mapped to [-HALF_Z, +HALF_Z]
const Y_SCALE = 1.5; // vertical exaggeration

// Camera parameters. The camera no longer auto-orbits — a moving surface reads
// as a spinning toy and can't be studied. It holds a fixed 3/4 view; the
// operator drags to orbit/tilt so the occupied band can actually be inspected.
const CAM_RADIUS = 7.5;
const CAM_Y_MIN = 1.2;
const CAM_Y_MAX = 7.5;
const Y_GAMMA = 0.75; // <1 lifts low/mid magnitudes so structure is visible

export function createWaterfall(mountEl) {
  // ── Renderer ──────────────────────────────────────────────────────────────
  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2));
  renderer.setClearColor(0x000000, 0); // transparent; panel provides bg
  mountEl.appendChild(renderer.domElement);

  // ── Scene & Lights ────────────────────────────────────────────────────────
  const scene = new THREE.Scene();

  const ambient = new THREE.AmbientLight(0xffffff, 0.6);
  scene.add(ambient);

  const dirLight = new THREE.DirectionalLight(0xffffff, 0.9);
  dirLight.position.set(5, 10, 5);
  scene.add(dirLight);

  // ── Camera ────────────────────────────────────────────────────────────────
  const camera = new THREE.PerspectiveCamera(
    45,
    mountEl.clientWidth / Math.max(mountEl.clientHeight, 1),
    0.1,
    100,
  );
  // Fixed 3/4 starting view; operator drags to orbit (azimuth) and tilt (height).
  let orbitAngle = Math.PI / 6;
  let camY = 4.5;
  const orbitCenter = new THREE.Vector3(0, 0, 0);
  updateCamera();

  function updateCamera() {
    camera.position.set(
      CAM_RADIUS * Math.cos(orbitAngle),
      camY,
      CAM_RADIUS * Math.sin(orbitAngle),
    );
    camera.lookAt(orbitCenter);
  }

  // ── Manual orbit: pointer drag (mouse + touch via pointer events) ───────────
  let dragging = false;
  let lastPX = 0;
  let lastPY = 0;
  renderer.domElement.style.touchAction = "none";
  renderer.domElement.style.cursor = "grab";
  renderer.domElement.addEventListener("pointerdown", (e) => {
    dragging = true;
    lastPX = e.clientX;
    lastPY = e.clientY;
    renderer.domElement.style.cursor = "grabbing";
    renderer.domElement.setPointerCapture?.(e.pointerId);
  });
  renderer.domElement.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    orbitAngle -= (e.clientX - lastPX) * 0.01; // horizontal drag → orbit
    camY = Math.max(CAM_Y_MIN, Math.min(CAM_Y_MAX, camY + (e.clientY - lastPY) * 0.03));
    lastPX = e.clientX;
    lastPY = e.clientY;
    updateCamera();
  });
  const endDrag = (e) => {
    dragging = false;
    renderer.domElement.style.cursor = "grab";
    if (e?.pointerId !== undefined) renderer.domElement.releasePointerCapture?.(e.pointerId);
  };
  renderer.domElement.addEventListener("pointerup", endDrag);
  renderer.domElement.addEventListener("pointercancel", endDrag);
  renderer.domElement.addEventListener("pointerleave", endDrag);

  // ── Surface mesh (placeholder, rebuilt in setData) ────────────────────────
  // material is shared and reused; only geometry is replaced on setData.
  const surfaceMaterial = new THREE.MeshLambertMaterial({
    vertexColors: true,
    side: THREE.DoubleSide,
  });
  let surfaceMesh = null;
  let currentGeo = null;

  // ── Sweep plane ───────────────────────────────────────────────────────────
  // A thin translucent vertical plane swept along the Z (time) axis.
  const sweepGeo = new THREE.PlaneGeometry(HALF_X * 2 + 0.2, Y_SCALE + 0.4);
  const sweepMat = new THREE.MeshBasicMaterial({
    color: 0x2fd4c4,
    transparent: true,
    opacity: 0.25,
    side: THREE.DoubleSide,
    depthWrite: false,
  });
  const sweepPlane = new THREE.Mesh(sweepGeo, sweepMat);
  // Plane geometry faces +Z by default; rotate 90° around X so it stands vertical.
  sweepPlane.rotation.x = Math.PI / 2;
  // Position it slightly above the surface center vertically.
  sweepPlane.position.set(0, (Y_SCALE + 0.4) / 2, -HALF_Z);
  sweepPlane.visible = false; // hidden until first setData + setNow
  scene.add(sweepPlane);

  // ── Resize handling ───────────────────────────────────────────────────────
  function onResize() {
    const w = mountEl.clientWidth;
    const h = mountEl.clientHeight;
    if (w === 0 || h === 0) return;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }

  let resizeObserver = null;
  if (typeof ResizeObserver !== "undefined") {
    resizeObserver = new ResizeObserver(onResize);
    resizeObserver.observe(mountEl);
  } else {
    window.addEventListener("resize", onResize);
  }
  onResize();

  // ── Animation loop ────────────────────────────────────────────────────────
  let rafId = null;

  function animate() {
    rafId = requestAnimationFrame(animate);
    // No auto-orbit: the camera moves only on operator drag (updateCamera()).
    // The loop still runs so the sweep plane animates during playback.
    renderer.render(scene, camera);
  }

  rafId = requestAnimationFrame(animate);

  // ── Build surface geometry from spectrogram data ───────────────────────────
  function buildGeometry(rows, cols, mag_q) {
    const vertexCount = rows * cols;
    const positions = new Float32Array(vertexCount * 3);
    const colors = new Float32Array(vertexCount * 3);

    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        const idx = r * cols + c;
        const t = (mag_q[idx] ?? 0) / 255;
        // Gamma-lift height + color so the occupied band stands out against the
        // noise floor instead of reading as a flat slab (legibility, not data
        // change — t is still the real quantized magnitude).
        const ts = Math.pow(t, Y_GAMMA);
        const x = cols <= 1 ? 0 : (c / (cols - 1)) * (HALF_X * 2) - HALF_X;
        const z = rows <= 1 ? 0 : (r / (rows - 1)) * (HALF_Z * 2) - HALF_Z;
        const y = ts * Y_SCALE;

        const vi = idx * 3;
        positions[vi] = x;
        positions[vi + 1] = y;
        positions[vi + 2] = z;

        const [cr, cg, cb] = viridis(ts);
        colors[vi] = cr / 255;
        colors[vi + 1] = cg / 255;
        colors[vi + 2] = cb / 255;
      }
    }

    // Index buffer: two triangles per quad cell.
    // (rows-1)*(cols-1) quads, 6 indices each.
    const quadRows = rows - 1;
    const quadCols = cols - 1;
    const indexCount = quadRows * quadCols * 6;
    // Use Uint32Array to safely handle > 65535 vertices.
    const indices = new Uint32Array(indexCount);
    let ii = 0;
    for (let r = 0; r < quadRows; r++) {
      for (let c = 0; c < quadCols; c++) {
        const tl = r * cols + c;
        const tr = tl + 1;
        const bl = tl + cols;
        const br = bl + 1;
        // Triangle 1: tl, bl, tr
        indices[ii++] = tl;
        indices[ii++] = bl;
        indices[ii++] = tr;
        // Triangle 2: tr, bl, br
        indices[ii++] = tr;
        indices[ii++] = bl;
        indices[ii++] = br;
      }
    }

    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geo.setAttribute("color", new THREE.BufferAttribute(colors, 3));
    geo.setIndex(new THREE.BufferAttribute(indices, 1));
    geo.computeVertexNormals();
    return geo;
  }

  // ── Public API ─────────────────────────────────────────────────────────────

  /**
   * setData({ rows, cols, freqs_hz, times_s, mag_q })
   * Rebuilds the surface mesh. Disposes the previous geometry to avoid GPU leaks.
   */
  function setData(spectrogram) {
    const { rows, cols, mag_q } = spectrogram;

    // Guard: degenerate input.
    if (!rows || !cols || rows < 2 || cols < 2) return;
    if (!mag_q || mag_q.length < rows * cols) return;

    // Dispose previous geometry.
    if (currentGeo) {
      currentGeo.dispose();
      currentGeo = null;
    }
    if (surfaceMesh) {
      scene.remove(surfaceMesh);
      surfaceMesh = null;
    }

    const geo = buildGeometry(rows, cols, mag_q);
    currentGeo = geo;
    surfaceMesh = new THREE.Mesh(geo, surfaceMaterial);
    scene.add(surfaceMesh);

    // Show the sweep plane; reset to fraction 0 (leading edge).
    sweepPlane.visible = true;
    setNow(0);

    // Signal readiness: remove the boot overlay by adding is-ready to mountEl.
    mountEl.classList.add("is-ready");
  }

  /**
   * setNow(fraction) — 0..1 — positions the translucent sweep plane along
   * the time (Z) axis of the surface.
   */
  function setNow(fraction) {
    const f = Math.max(0, Math.min(1, fraction));
    // Map fraction to Z: 0 → -HALF_Z (start), 1 → +HALF_Z (end).
    const z = f * (HALF_Z * 2) - HALF_Z;
    sweepPlane.position.z = z;
  }

  /**
   * dispose() — stop the render loop, remove listeners, free GPU resources.
   */
  function dispose() {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }

    if (resizeObserver) {
      resizeObserver.disconnect();
      resizeObserver = null;
    } else {
      window.removeEventListener("resize", onResize);
    }

    if (surfaceMesh) {
      scene.remove(surfaceMesh);
      surfaceMesh = null;
    }
    if (currentGeo) {
      currentGeo.dispose();
      currentGeo = null;
    }
    surfaceMaterial.dispose();
    sweepGeo.dispose();
    sweepMat.dispose();

    renderer.dispose();

    if (renderer.domElement.parentNode === mountEl) {
      mountEl.removeChild(renderer.domElement);
    }
  }

  return { setData, setNow, dispose };
}
