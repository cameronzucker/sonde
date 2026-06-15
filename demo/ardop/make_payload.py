#!/usr/bin/env python3
"""Build the demo transport payload as a real multipart MESSAGE (sonde-imh).

The demo sends this payload over the real link (ARDOP today, Sonde later); the
frontend rebuilds it from what decoded. Instead of a single contrived image, the
payload is a small self-describing **message container** — text + image
attachment(s), Winlink/ICS-213-flavoured — so a demo shows a believable EmComm
payload, not a synthetic blob.

Container format `SNDM` v1 (the frontend parses this from the delivered bytes):

    "SNDM"            4 bytes magic
    version           1 byte  (= 1)
    part_count        1 byte
    per part:
      type            1 byte  (0 = text/utf8, 1 = image/jpeg, 2 = image/png)
      label_len       1 byte
      label           label_len bytes, utf8
      data_len        4 bytes, big-endian
      data            data_len bytes

Default (no args): a realistic SITREP text + a synthetic dusk antenna-tower JPEG.
Custom: compose any text + image(s):

    python3 make_payload.py --text "SITREP ..." --image photo.jpg --label "Damage"
    python3 make_payload.py --text-file msg.txt --image a.jpg --image b.png

Keep total size modest (a few KB): every byte is real airtime over the link.

Writes  demo/site/assets/{payload.bin, payload.offsets.json, source-credit.txt}.
Requires Pillow. Re-runnable; deterministic for the default.
"""
import argparse
import io
import json
import os
import struct

from PIL import Image, ImageDraw

W, H = 256, 192
OUT = os.path.join(os.path.dirname(__file__), "../site/assets")

TYPE_TEXT, TYPE_JPEG, TYPE_PNG = 0, 1, 2

DEFAULT_SITREP = """\
SITREP / ICS-213
DE: K7XYZ  Net Control, Cascade ARES
TO: County EOC, Operations
TIME: 2026-06-15 1432Z   PREC: PRIORITY

1. Repeater site WX7RPT on generator power; fuel ~18 h remaining.
2. Tower base flooding receding, antenna intact (photo attached).
3. HF NVIS net 3.815 MHz operational; 6 stations checked in.
4. Request fuel resupply + damage-assessment team by 1800Z.

BT  K7XYZ  AR
"""


def _lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def render_tower():
    """Synthetic dusk antenna-tower scene — on-theme, deterministic, JPEG-friendly."""
    im = Image.new("RGB", (W, H))
    px = im.load()
    sky_top, sky_horizon = (24, 26, 64), (250, 150, 70)
    horizon_y = int(H * 0.66)
    for y in range(horizon_y):
        col = _lerp(sky_top, sky_horizon, (y / horizon_y) ** 1.6)
        for x in range(W):
            px[x, y] = col
    d = ImageDraw.Draw(im)
    sx, sy, sr = int(W * 0.30), int(horizon_y * 0.82), 22
    for r in range(sr, 0, -1):
        d.ellipse([sx - r, sy - r, sx + r, sy + r], fill=_lerp((255, 240, 200), sky_horizon, r / sr))
    d.polygon([(0, horizon_y), (W * 0.25, horizon_y - 26), (W * 0.5, horizon_y - 6),
               (W * 0.78, horizon_y - 30), (W, horizon_y - 10), (W, H), (0, H)], fill=(40, 36, 52))
    d.rectangle([0, horizon_y + 14, W, H], fill=(20, 18, 28))
    tx, base_y, top_y = int(W * 0.70), horizon_y + 8, int(H * 0.10)
    half_b, half_t, tower = 16, 3, (12, 12, 18)
    d.polygon([(tx - half_b, base_y), (tx - half_t, top_y), (tx + half_t, top_y),
               (tx + half_b, base_y)], fill=tower)
    rungs = 9
    for i in range(rungs + 1):
        t = i / rungs
        y = base_y + (top_y - base_y) * t
        hw = half_b + (half_t - half_b) * t
        d.line([(tx - hw, y), (tx + hw, y)], fill=tower, width=1)
    for i in range(rungs):
        t0, t1 = i / rungs, (i + 1) / rungs
        y0, y1 = base_y + (top_y - base_y) * t0, base_y + (top_y - base_y) * t1
        hw0 = half_b + (half_t - half_b) * t0
        hw1 = half_b + (half_t - half_b) * t1
        s = -1 if i % 2 else 1
        d.line([(tx - s * hw0, y0), (tx + s * hw1, y1)], fill=tower, width=1)
    d.line([(tx, top_y + 6), (tx - 52, base_y)], fill=(60, 56, 70), width=1)
    d.line([(tx, top_y + 6), (tx + 52, base_y)], fill=(60, 56, 70), width=1)
    d.line([(tx, top_y), (tx, top_y - 10)], fill=(120, 120, 140), width=1)
    d.ellipse([tx - 3, top_y - 14, tx + 3, top_y - 8], fill=(255, 80, 80))
    buf = io.BytesIO()
    im.save(buf, format="JPEG", quality=62, optimize=True)
    return buf.getvalue()


def load_image(path):
    """Load an image file → (type, jpeg/png bytes). JPEG is downscaled + re-encoded
    modestly to keep airtime sane (a phone photo is otherwise minutes over the link).
    Small PNGs pass through."""
    if os.path.splitext(path)[1].lower() == ".png":
        with open(path, "rb") as f:
            return TYPE_PNG, f.read()
    im = Image.open(path).convert("RGB")
    im.thumbnail((320, 320))
    buf = io.BytesIO()
    im.save(buf, format="JPEG", quality=62, optimize=True)
    return TYPE_JPEG, buf.getvalue()


def encode_message(parts):
    """parts = [(type:int, label:str, data:bytes), ...] → SNDM v1 container bytes."""
    out = bytearray(b"SNDM")
    out.append(1)             # version
    out.append(len(parts))    # part count
    manifest = []
    kind = {TYPE_TEXT: "text", TYPE_JPEG: "image/jpeg", TYPE_PNG: "image/png"}
    for ptype, label, data in parts:
        lbl = label.encode("utf-8")[:255]
        out.append(ptype)
        out.append(len(lbl))
        out += lbl
        out += struct.pack(">I", len(data))
        data_off = len(out)
        out += data
        manifest.append({"type": kind[ptype], "label": label, "offset": data_off, "len": len(data)})
    return bytes(out), manifest


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--text", help="message body text (overrides the default SITREP)")
    ap.add_argument("--text-file", help="read the message body from this file")
    ap.add_argument("--text-label", default="SITREP", help="label for the text part")
    ap.add_argument("--image", action="append", default=[], metavar="PATH",
                    help="attach an image (repeatable). Default: a synthetic tower scene.")
    ap.add_argument("--label", action="append", default=[], metavar="NAME",
                    help="label for the Nth --image (repeatable; defaults to the filename)")
    args = ap.parse_args()

    parts = []
    if args.text_file:
        with open(args.text_file, encoding="utf-8") as f:
            body = f.read()
    elif args.text is not None:
        body = args.text
    else:
        body = DEFAULT_SITREP
    if body.strip():
        parts.append((TYPE_TEXT, args.text_label, body.encode("utf-8")))

    if args.image:
        for i, path in enumerate(args.image):
            ptype, data = load_image(path)
            label = args.label[i] if i < len(args.label) else os.path.basename(path)
            parts.append((ptype, label, data))
    else:
        parts.append((TYPE_JPEG, "Tower (site)", render_tower()))

    payload, manifest = encode_message(parts)
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, "payload.bin"), "wb") as f:
        f.write(payload)
    with open(os.path.join(OUT, "payload.offsets.json"), "w") as f:
        json.dump({"format": "sndmsg-v1", "total_len": len(payload), "parts": manifest}, f, indent=2)
    with open(os.path.join(OUT, "source-credit.txt"), "w") as f:
        f.write("Demo message: synthetic SITREP + dusk antenna-tower scene, generated by "
                "demo/ardop/make_payload.py (CC0 / public domain).\n")
    kinds = ", ".join(f"{p['type']} '{p['label']}' {p['len']}B" for p in manifest)
    print(f"payload.bin: {len(payload)} bytes — {len(parts)} part(s): {kinds}")


if __name__ == "__main__":
    main()
