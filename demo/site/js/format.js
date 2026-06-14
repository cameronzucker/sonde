// format.js — pure view helpers (no wasm, unit-tested).

/** Bytes -> "7E 22 04" uppercase hex, space-separated. */
export function toHex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" ");
}

/** Indices where a[i] !== b[i]; trailing bytes beyond the shorter array count as differing. */
export function byteDiff(a, b) {
  const n = Math.min(a.length, b.length);
  const out = [];
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) out.push(i);
  for (let i = n; i < Math.max(a.length, b.length); i++) out.push(i);
  return out;
}

// Viridis control points (matplotlib), sampled; linear-interpolated.
const VIRIDIS = [
  [68, 1, 84], [72, 40, 120], [62, 74, 137], [49, 104, 142],
  [38, 130, 142], [31, 158, 137], [53, 183, 121], [110, 206, 88],
  [181, 222, 43], [253, 231, 37],
];
/** t in [0,1] -> [r,g,b] 0..255 along viridis. */
export function viridis(t) {
  const x = Math.max(0, Math.min(1, t)) * (VIRIDIS.length - 1);
  const i = Math.floor(x), f = x - i;
  const a = VIRIDIS[i], b = VIRIDIS[Math.min(i + 1, VIRIDIS.length - 1)];
  return [0, 1, 2].map((k) => Math.round(a[k] + (b[k] - a[k]) * f));
}
