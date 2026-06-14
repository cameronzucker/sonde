// frame-log.js — per-frame decode log for the ARDOP-anchored demo (ES module).
//
// Replaces the WASM demo's per-symbol TX|RX byte console: ARDOP is an external
// reference modem and exposes no per-symbol I/Q, but it DOES report, per frame,
// whether the frame decoded, how many Reed–Solomon byte-errors it corrected, and
// the residual BER. That's the honest per-frame story this panel shows.

/**
 * createFrameLog(streamEl, statEl)
 *   streamEl — scrollable container that receives one row per frame.
 *   statEl   — small element showing the running "decoded / total" tally.
 * Returns { showFrame(frame), reset() }.
 */
export function createFrameLog(streamEl, statEl) {
  let total = 0;
  let decoded = 0;

  function pill(text, cls) {
    const span = document.createElement("span");
    span.className = `flog__pill ${cls}`;
    span.textContent = text;
    return span;
  }

  function showFrame(frame) {
    const row = document.createElement("div");
    row.className = "flog__row" + (frame.decoded ? "" : " flog__row--fail");

    const idx = document.createElement("span");
    idx.className = "flog__idx";
    idx.textContent = "#" + String(frame.seq).padStart(2, "0");
    row.appendChild(idx);

    row.appendChild(pill(frame.decoded ? "PASS" : "FAIL",
      frame.decoded ? "flog__pill--pass" : "flog__pill--fail"));

    // RS corrections — how hard the FEC worked (only meaningful on a decode).
    if (frame.decoded && frame.rs_fixed != null && frame.rs_max != null) {
      const rs = document.createElement("span");
      rs.className = "flog__meta";
      rs.textContent = `RS ${frame.rs_fixed}/${frame.rs_max}`;
      row.appendChild(rs);
    }
    // Residual BER after correction.
    if (frame.decoded && frame.ber_max != null) {
      const ber = document.createElement("span");
      ber.className = "flog__meta";
      ber.textContent = `BER ${frame.ber_max.toFixed(1)}%`;
      row.appendChild(ber);
    }
    if (!frame.decoded) {
      const lost = document.createElement("span");
      lost.className = "flog__meta flog__meta--lost";
      lost.textContent = "— lost (no ARQ)";
      row.appendChild(lost);
    }

    streamEl.appendChild(row);
    streamEl.scrollTop = streamEl.scrollHeight;

    total += 1;
    if (frame.decoded) decoded += 1;
    if (statEl) statEl.textContent = `${decoded}/${total}`;
  }

  function reset() {
    streamEl.innerHTML = "";
    total = 0;
    decoded = 0;
    if (statEl) statEl.textContent = "0/0";
  }

  return { showFrame, reset };
}
