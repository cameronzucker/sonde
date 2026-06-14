#!/usr/bin/env python3
"""ARDOP live-demo backend (sonde-imh.1).

Serves the demo site AND the live ARDOP-through-hf-channel-sim round-trip. A lever
change in the frontend hits `/api/run`, which sends the demo payload across as many
real ARDOP frames as the chosen mode needs, runs each through the channel sim, and
returns per-frame decode results + reassembled image bytes + a spectrogram as JSON,
plus an `/api/audio` URL for the concatenated transmission.

This re-anchors the demo on a real, known-good reference modem (ardopcf, external /
clean-sheet — never reimplemented, ADR 0014) instead of the Sonde WASM engine.

Run:  python3 demo/ardop/server.py [--port 8770]
Then: open http://localhost:8770/  (or curl the endpoints below)

Endpoints:
  GET /                      → demo site (demo/site/index.html)
  GET /api/modes             → mode catalogue + the provisional Auto ladder
  GET /api/run?frame=&snr=&condition=&seed=   → one multi-frame transfer (JSON)
  GET /api/audio?id=<token>  → the concatenated transmission WAV for a run

No real radio: WAV files only (see ardop_channel.py). ardopcf is external/reference.
"""
import argparse
import json
import os
import re
import tempfile
import threading
import traceback
import uuid
from collections import OrderedDict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

from ardop_channel import FRAMES, FRAME_CAPACITY, run_transfer, run_once, _write_wav_mono

_SITE_DIR = os.path.normpath(os.path.join(os.path.dirname(__file__), "../site"))
_AUDIO_DIR = tempfile.mkdtemp(prefix="ardop_audio_")

# Human-facing constellation label per modulation prefix (for the mode picker).
_CONSTELLATION = {"4FSK": "4-FSK", "4PSK": "QPSK", "16QAM": "16-QAM"}

# Provisional Auto ladder: pick the highest-rate frame expected to close the link
# at a given SNR. Thresholds are PLACEHOLDERS pending the real SNR-axis calibration
# against ARDOP's documented sensitivity (sonde-imh.1 follow-up; feeds P0). Ordered
# high-SNR → low-SNR; first threshold the SNR clears wins.
#
# Floored at 4PSK.500 (128 B/frame → 24 frames for the ~3 KB demo payload): the
# slower 4PSK.200 / 4FSK.200 modes would need 48 / 190 frames to carry it, which is
# impractical airtime for a demo. At very low SNR Auto stays on 4PSK.500 and the
# frames simply fail to decode — the honest "link can't close, image full of holes"
# story, rather than a too-many-frames error. The slow modes remain Manual-only.
_AUTO_LADDER = [
    (15.0, "16QAM.2000.100.E"),
    (6.0, "4PSK.1000.100.E"),
    (-1e9, "4PSK.500.100.E"),
]

# Static assets the demo site serves. Anything else under _SITE_DIR is text/binary
# best-effort; we only need these to render.
_MIME = {
    ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript",
    ".css": "text/css", ".json": "application/json", ".bin": "application/octet-stream",
    ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".svg": "image/svg+xml", ".txt": "text/plain", ".wasm": "application/wasm",
    ".map": "application/json",
}


def _constellation(frame):
    return _CONSTELLATION.get(frame.split(".", 1)[0], "—")


def auto_frame(snr_db):
    """The Auto ladder's pick for a measured SNR (provisional; see _AUTO_LADDER)."""
    for thresh, frame in _AUTO_LADDER:
        if snr_db >= thresh:
            return frame
    return _AUTO_LADDER[-1][1]


def mode_catalogue():
    return [
        {"id": f, "implemented": True, "constellation": _constellation(f),
         "capacity_bytes": FRAME_CAPACITY[f]}
        for f in FRAMES
    ]


class _AudioCache:
    """Bounded id→WAV-path store so /api/audio can serve a recent run's audio."""

    def __init__(self, cap=32):
        self._cap = cap
        self._lock = threading.Lock()
        self._items = OrderedDict()

    def put(self, samples, sr):
        token = uuid.uuid4().hex
        path = os.path.join(_AUDIO_DIR, f"{token}.wav")
        _write_wav_mono(path, samples, sr)
        with self._lock:
            self._items[token] = path
            while len(self._items) > self._cap:
                _, old = self._items.popitem(last=False)
                try:
                    os.remove(old)
                except OSError:
                    pass
        return token

    def path(self, token):
        with self._lock:
            return self._items.get(token)


_AUDIO = _AudioCache()


class Handler(BaseHTTPRequestHandler):
    def _send_json(self, code, obj):
        self._send(code, "application/json", json.dumps(obj).encode())

    def _send(self, code, ctype, body):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    # ── API ──────────────────────────────────────────────────────────────────
    def _api_run(self, q):
        frame_arg = q.get("frame", ["auto"])[0]
        snr = float(q.get("snr", ["6"])[0])
        condition = q.get("condition", ["none"])[0]
        seed = int(q.get("seed", ["1"])[0])
        auto = frame_arg in ("", "auto")
        frame = auto_frame(snr) if auto else frame_arg
        res = run_transfer(frame=frame, snr_db=snr, condition=condition, seed=seed)
        samples = res.pop("_audio_samples")
        token = _AUDIO.put(samples, res["sample_rate"])
        res["audio_url"] = f"/api/audio?id={token}"
        res["auto"] = auto
        res["constellation"] = _constellation(frame)
        return res

    def _api_audio(self, q):
        token = q.get("id", [""])[0]
        if not re.fullmatch(r"[0-9a-f]{32}", token):
            return self._send_json(400, {"error": "bad id"})
        path = _AUDIO.path(token)
        if not path or not os.path.exists(path):
            return self._send_json(404, {"error": "audio expired or unknown"})
        with open(path, "rb") as f:
            self._send(200, "audio/wav", f.read())

    # ── static site ────────────────────────────────────────────────────────--
    def _serve_static(self, path):
        rel = path.lstrip("/") or "index.html"
        full = os.path.normpath(os.path.join(_SITE_DIR, rel))
        # Path-traversal guard: resolved path must stay under the site dir.
        if not (full == _SITE_DIR or full.startswith(_SITE_DIR + os.sep)):
            return self._send_json(403, {"error": "forbidden"})
        if os.path.isdir(full):
            full = os.path.join(full, "index.html")
        if not os.path.isfile(full):
            return self._send_json(404, {"error": "not found", "path": path})
        ctype = _MIME.get(os.path.splitext(full)[1].lower(), "application/octet-stream")
        with open(full, "rb") as f:
            self._send(200, ctype, f.read())

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        try:
            if u.path == "/api/modes":
                return self._send_json(200, {"modes": mode_catalogue(), "auto_ladder": _AUTO_LADDER})
            if u.path == "/api/run":
                return self._send_json(200, self._api_run(q))
            if u.path == "/api/audio":
                return self._api_audio(q)
            if u.path == "/api/frames":  # back-compat with the MVP
                return self._send_json(200, {"frames": FRAMES})
            if u.path == "/api/run_once":  # single-frame MVP, kept for debugging
                return self._send_json(200, run_once(
                    frame=q.get("frame", ["4PSK.500.100.E"])[0],
                    snr_db=float(q.get("snr", ["6"])[0]),
                    condition=q.get("condition", ["none"])[0],
                    seed=int(q.get("seed", ["1"])[0])))
            return self._serve_static(u.path)
        except ValueError as e:  # rejected/invalid input → client error
            return self._send_json(400, {"error": str(e)})
        except Exception as e:  # surface failures as JSON, don't 500 silently
            traceback.print_exc()
            return self._send_json(500, {"error": str(e)})

    do_HEAD = do_GET

    def log_message(self, *args):
        pass  # quiet


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8770)
    args = ap.parse_args()
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"ARDOP demo on http://127.0.0.1:{args.port}/  (site + /api/run, /api/modes, /api/audio)")
    srv.serve_forever()


if __name__ == "__main__":
    main()
