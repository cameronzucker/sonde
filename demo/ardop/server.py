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
  GET /api/session?snr=&condition=&seed=&arqbw= → one REAL ARDOP connected session,
                               streamed as Server-Sent Events (handshake → negotiated
                               BW → ARQ progress → rate adaptation → delivery), plus a
                               final audio_url for the teed on-air waterfall.
  GET /api/audio?id=<token>  → the WAV for a run/session (PHY concat, or session air)

No real radio: WAV files only. ardopcf is external/reference. The connected-mode
session (testbench.run_session) runs two ardopcf instances over snd-aloop loopback —
no PTT device, so nothing can key a radio.
"""
import argparse
import json
import os
import re
import tempfile
import threading
import traceback
import uuid
import wave
from collections import OrderedDict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

import numpy as np

from testbench import (run_session, SessionParams, ARQBW_VALUES, CONDITIONS,
                       RATE as SESSION_RATE)

_SITE_DIR = os.path.normpath(os.path.join(os.path.dirname(__file__), "../site"))
_AUDIO_DIR = tempfile.mkdtemp(prefix="ardop_audio_")

# Static assets the demo site serves. Anything else under _SITE_DIR is text/binary
# best-effort; we only need these to render.
_MIME = {
    ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript",
    ".css": "text/css", ".json": "application/json", ".bin": "application/octet-stream",
    ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".svg": "image/svg+xml", ".txt": "text/plain", ".wasm": "application/wasm",
    ".map": "application/json",
}


def _write_wav_mono(path, samples, sr):
    """Write float32 samples in [-1,1] to a 16-bit mono PCM WAV (for /api/audio)."""
    pcm = (np.clip(samples, -1.0, 1.0) * 32767.0).astype("<i2").tobytes()
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes(pcm)


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

# A connected session owns the two snd-aloop cards exclusively, so only ONE can run
# at a time. A new /api/session request signals the running one to release (set the
# abort Event), waits for the lock, then takes over — "latest lever wins", matching
# the frontend's discard-stale-run pattern.
_SESSION_LOCK = threading.Lock()
_SESSION_ABORT = threading.Event()


def _tap_to_audio_url(tap_path, rate):
    """Convert the session's raw S16LE on-air tap → a cached WAV; return its URL.

    The tap is what the *receiver* heard at the chosen SNR, so the waterfall plays
    the genuine on-air spectrum. Returns None if the tap is empty/missing."""
    try:
        with open(tap_path, "rb") as f:
            raw = f.read()
    except OSError:
        return None
    if len(raw) < 2:
        return None
    samples = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    token = _AUDIO.put(samples, rate)
    return f"/api/audio?id={token}"


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
    def _api_session(self, q):
        """Stream ONE real ARDOP connected session as Server-Sent Events.

        Param validation raises ValueError BEFORE any SSE byte is written, so bad
        input still gets a clean 400 JSON from do_GET. Once the event-stream headers
        are sent, every failure is surfaced as an SSE `error` event instead."""
        snr = float(q.get("snr", ["20"])[0])
        seed = int(q.get("seed", ["1"])[0]) & 0xFFFFFFFF
        condition = q.get("condition", ["none"])[0]
        arqbw = q.get("arqbw", ["2000MAX"])[0]
        if condition not in CONDITIONS:
            raise ValueError(f"bad condition {condition!r}")
        if arqbw not in ARQBW_VALUES:
            raise ValueError(f"bad arqbw {arqbw!r}")

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.send_header("X-Accel-Buffering", "no")  # don't let a proxy buffer the stream
        self.end_headers()

        # emit() runs from the session thread AND ardopcf host-reader threads, so
        # serialize writes. A broken pipe (client closed the tab) flips `gone`, which
        # the session's should_abort() polls — tearing down ardopcf instead of leaking.
        wlock = threading.Lock()
        state = {"gone": False}

        def emit(ev):
            with wlock:
                if state["gone"]:
                    return
                if ev.get("t") == "result" and ev.get("tap_path"):
                    url = _tap_to_audio_url(ev["tap_path"], ev.get("tap_rate", SESSION_RATE))
                    if url:
                        ev = {**ev, "audio_url": url}
                try:
                    self.wfile.write(f"data: {json.dumps(ev)}\n\n".encode())
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError, OSError):
                    state["gone"] = True
                    _SESSION_ABORT.set()

        # Take the loopback hardware: signal any running session to release, then wait.
        _SESSION_ABORT.set()
        with _SESSION_LOCK:
            _SESSION_ABORT.clear()
            params = SessionParams(
                snr=snr, condition=condition, seed=seed, arqbw=arqbw,
                tap=os.path.join(_AUDIO_DIR, "session_air.raw"))
            try:
                run_session(params, emit,
                            should_abort=lambda: state["gone"] or _SESSION_ABORT.is_set())
            except Exception as e:  # never 500 mid-stream — report as an SSE event
                traceback.print_exc()
                emit({"t": "error", "msg": str(e)})
                emit({"t": "done"})

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
            if u.path == "/api/session":
                return self._api_session(q)
            if u.path == "/api/audio":
                return self._api_audio(q)
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
    # Bind to all interfaces by default: the Pi serves the backend and a laptop on
    # the LAN renders the page (the Pi can't run a browser itself). /api/run shells
    # out to ardopcf, so every query param is validated/coerced (frame + condition
    # allowlists, numeric SNR/seed) — keep it to a trusted LAN, not the open internet.
    ap.add_argument("--host", default="0.0.0.0")
    args = ap.parse_args()
    srv = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"ARDOP demo on http://{args.host}:{args.port}/  (site + /api/run, /api/modes, /api/audio)")
    srv.serve_forever()


if __name__ == "__main__":
    main()
