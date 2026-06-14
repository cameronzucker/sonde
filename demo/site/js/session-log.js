// session-log.js — connection / ARQ timeline for the connected-mode ARDOP demo.
//
// Replaces the PHY demo's per-frame PASS/FAIL log. Connected ARDOP has a link layer,
// so the honest story is the PROTOCOL timeline: the CONNECT handshake, the negotiated
// bandwidth, the data modes the modems adapt through, and ARQ delivery progress —
// each surfaced as a labelled row as the live session streams in.

const KIND = {
  call: { label: "CALL", cls: "slog__pill--call" },
  link: { label: "LINK", cls: "slog__pill--link" },
  conn: { label: "CONN", cls: "slog__pill--conn" },
  rate: { label: "RATE", cls: "slog__pill--rate" },
  arq: { label: "ARQ", cls: "slog__pill--arq" },
  ok: { label: "OK", cls: "slog__pill--ok" },
  fail: { label: "FAIL", cls: "slog__pill--fail" },
  info: { label: "·", cls: "slog__pill--info" },
};

/**
 * createSessionLog(streamEl, progressEl)
 *   streamEl   — scrollable container that receives one row per event.
 *   progressEl — small element showing the running "received / total B" tally.
 * Returns { event(kind, text, meta), setProgress(received, total), reset() }.
 */
export function createSessionLog(streamEl, progressEl) {
  function event(kind, text, meta) {
    const k = KIND[kind] || KIND.info;
    const row = document.createElement("div");
    row.className = "slog__row" + (kind === "fail" ? " slog__row--fail" : "");

    const pill = document.createElement("span");
    pill.className = "slog__pill " + k.cls;
    pill.textContent = k.label;
    row.appendChild(pill);

    const body = document.createElement("span");
    body.className = "slog__text";
    body.textContent = text;
    row.appendChild(body);

    if (meta != null && meta !== "") {
      const m = document.createElement("span");
      m.className = "slog__meta";
      m.textContent = meta;
      row.appendChild(m);
    }

    streamEl.appendChild(row);
    streamEl.scrollTop = streamEl.scrollHeight;
  }

  function setProgress(received, total) {
    if (progressEl) progressEl.textContent = `${received}/${total} B`;
  }

  function reset() {
    streamEl.innerHTML = "";
    if (progressEl) progressEl.textContent = "0/0 B";
  }

  return { event, setProgress, reset };
}
