// message.js — parse + render the delivered SNDM multipart message (ES module).
//
// The payload is a self-describing container (see demo/ardop/make_payload.py):
//   "SNDM" ver count  then per part: type(1) labelLen(1) label dataLen(4 BE) data
// type: 0 text/utf8, 1 image/jpeg, 2 image/png.
//
// This renders the delivered message — a text body + image attachment(s), like a
// Winlink/ICS-213 message — into the panel, and shows a per-part "receiving" view
// (driven by byte progress + the manifest) while the link delivers it.

const MIME = { 1: "image/jpeg", 2: "image/png" };

/** Parse SNDM bytes → { parts: [{type,label,data,complete}] } or null if not SNDM. */
export function parseMessage(bytes) {
  if (!bytes || bytes.length < 6) return null;
  if (!(bytes[0] === 0x53 && bytes[1] === 0x4e && bytes[2] === 0x44 && bytes[3] === 0x4d)) return null;
  const count = bytes[5];
  const dec = new TextDecoder();
  const parts = [];
  let off = 6;
  for (let i = 0; i < count; i++) {
    if (off + 2 > bytes.length) break;
    const type = bytes[off++];
    const llen = bytes[off++];
    if (off + llen + 4 > bytes.length) break;
    const label = dec.decode(bytes.subarray(off, off + llen));
    off += llen;
    const dlen = (bytes[off] * 0x1000000) + (bytes[off + 1] << 16) + (bytes[off + 2] << 8) + bytes[off + 3];
    off += 4;
    const end = off + dlen;
    const data = bytes.subarray(off, Math.min(end, bytes.length));
    parts.push({ type, label, data, complete: end <= bytes.length });
    off = end;
  }
  return { parts };
}

export function createMessageView(mountEl) {
  const objectUrls = [];
  function clearUrls() {
    objectUrls.forEach((u) => URL.revokeObjectURL(u));
    objectUrls.length = 0;
  }

  function reset() {
    clearUrls();
    mountEl.innerHTML = "";
  }

  /** Centered two-line placeholder (connecting / connect failed / no message). */
  function placeholder(line1, line2 = "") {
    reset();
    const box = document.createElement("div");
    box.className = "msg__placeholder";
    const a = document.createElement("div"); a.className = "msg__ph1"; a.textContent = line1;
    const b = document.createElement("div"); b.className = "msg__ph2"; b.textContent = line2;
    box.append(a, b);
    mountEl.appendChild(box);
  }

  /** While delivering: show the manifest's parts, ticking each off as its bytes arrive. */
  function receiving(received, total, manifest) {
    reset();
    const wrap = document.createElement("div");
    wrap.className = "msg__receiving";
    const head = document.createElement("div");
    head.className = "msg__rxhead";
    head.textContent = `Receiving message — ${received} / ${total} B`;
    wrap.appendChild(head);
    (manifest?.parts || []).forEach((p) => {
      const got = received >= p.offset + p.len;
      const row = document.createElement("div");
      row.className = "msg__rxrow" + (got ? " msg__rxrow--got" : "");
      row.textContent = `${got ? "✓" : "…"}  ${p.type === "text" ? "✉" : "🖼"} ${p.label} · ${p.len} B`;
      wrap.appendChild(row);
    });
    mountEl.appendChild(wrap);
  }

  /** Render the delivered message (text + image parts). */
  function render(bytes) {
    const msg = parseMessage(bytes);
    if (!msg || !msg.parts.length) {
      placeholder("MESSAGE UNREADABLE", "delivered bytes are not a valid message");
      return;
    }
    reset();
    for (const part of msg.parts) {
      const block = document.createElement("div");
      block.className = "msg__part";
      const label = document.createElement("div");
      label.className = "msg__label";
      label.textContent = part.label + (part.complete ? "" : " · partial");
      block.appendChild(label);

      if (part.type === 0) {
        const pre = document.createElement("pre");
        pre.className = "msg__text";
        pre.textContent = new TextDecoder().decode(part.data);
        block.appendChild(pre);
      } else if (MIME[part.type] && part.complete) {
        const url = URL.createObjectURL(new Blob([part.data], { type: MIME[part.type] }));
        objectUrls.push(url);
        const img = document.createElement("img");
        img.className = "msg__img";
        img.alt = part.label;
        img.src = url;
        block.appendChild(img);
      } else {
        const ph = document.createElement("div");
        ph.className = "msg__imgmiss";
        ph.textContent = part.complete ? "(unsupported attachment)" : "(attachment incomplete)";
        block.appendChild(ph);
      }
      mountEl.appendChild(block);
    }
  }

  return { render, receiving, placeholder, reset };
}
