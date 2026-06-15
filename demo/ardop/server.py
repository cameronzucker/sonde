#!/usr/bin/env python3
"""ARDOP connected-mode demo backend (sonde-imh.2).

Serves the demo site AND drives ONE real ARDOP connected session live: two ardopcf
stations bridged through hf-channel-sim over snd-aloop loopback, a real CONNECT
handshake + bandwidth negotiation + adaptive-rate ARQ transfer. The browser opens
`/api/session` as a Server-Sent Events stream and watches it happen in real time —
protocol milestones AND the on-air audio, streamed as the modems transmit, so the
waterfall and sound are LIVE (no record-then-replay).

ardopcf is an external reference modem (clean-sheet, ADR 0014). Virtual audio only:
the stations have no PTT device, so nothing can key a real radio — no RF.

Run:  python3 demo/ardop/server.py [--port 8770]
Then: open http://<host>:<port>/

Endpoints:
  GET /                       → demo site (demo/site/index.html)
  GET /api/session?snr=&condition=&seed=&arqbw= → one REAL connected session as SSE:
        protocol events (handshake → negotiated BW → ARQ progress → rate adaptation
        → delivery) interleaved with live `audio` events (base64 S16LE on-air PCM).
"""
import argparse
import base64
import json
import os
import signal
import tempfile
import threading
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

from testbench import (run_session, SessionParams, ARQBW_VALUES, CONDITIONS,
                       RATE as SESSION_RATE)

_SITE_DIR = os.path.normpath(os.path.join(os.path.dirname(__file__), "../site"))
_AUDIO_DIR = tempfile.mkdtemp(prefix="ardop_audio_")  # holds the live on-air tap file

# Static assets the demo site serves. Anything else under _SITE_DIR is text/binary
# best-effort; we only need these to render.
_MIME = {
    ".html": "text/html", ".js": "text/javascript", ".mjs": "text/javascript",
    ".css": "text/css", ".json": "application/json", ".bin": "application/octet-stream",
    ".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
    ".svg": "image/svg+xml", ".txt": "text/plain", ".wasm": "application/wasm",
    ".map": "application/json",
}

# A connected session owns the two snd-aloop cards exclusively, so only ONE runs at a
# time. A new /api/session request signals the running one to release (set the abort
# Event), waits for the lock, then takes over — "latest lever wins".
_SESSION_LOCK = threading.Lock()
_SESSION_ABORT = threading.Event()

# ~120 ms of 12 kHz S16LE mono per audio event (even byte count → int16-aligned).
_AUDIO_CHUNK_BYTES = 2880


def _stream_tap_audio(taps, emit, stop, rate=SESSION_RATE, chunk=_AUDIO_CHUNK_BYTES):
    """Tail each on-air tap file and emit per-direction base64 PCM `audio` events LIVE.

    `taps` is [(dir, path), ...] — e.g. ("fwd", A->B data) and ("rev", B->A acks).
    Each direction's `hf-channel-pcm --tap` appends its impaired on-air PCM (what that
    station's listener hears), flushing every block. We tail each independently and
    tag every chunk with its direction, so the frontend can drive a separate waterfall
    for each station — you see the data sender and the receiver's ACK/NAK bursts
    alternating (half-duplex). Never touches the proven arecord|hf-channel-pcm|aplay
    OS pipes, so it can't perturb ARDOP's handshake timing."""
    for _ in range(200):  # wait up to ~4 s for the writers to create the files
        if all(os.path.exists(p) for _d, p in taps) or stop.is_set():
            break
        time.sleep(0.02)
    chans = []
    for d, path in taps:
        try:
            chans.append({"dir": d, "f": open(path, "rb"), "buf": bytearray()})
        except OSError:
            pass
    if not chans:
        return
    empties_after_stop = 0
    try:
        while True:
            got_any = False
            for c in chans:
                data = c["f"].read(65536)
                if data:
                    c["buf"] += data
                    got_any = True
                    while len(c["buf"]) >= chunk:
                        emit({"t": "audio", "dir": c["dir"], "rate": rate,
                              "pcm": base64.b64encode(bytes(c["buf"][:chunk])).decode("ascii")})
                        del c["buf"][:chunk]
            if not got_any:
                if stop.is_set():
                    empties_after_stop += 1
                    if empties_after_stop >= 3:  # drained ~75 ms past teardown
                        for c in chans:
                            n = len(c["buf"]) & ~1  # int16 align
                            if n:
                                emit({"t": "audio", "dir": c["dir"], "rate": rate,
                                      "pcm": base64.b64encode(bytes(c["buf"][:n])).decode("ascii")})
                        break
                time.sleep(0.025)
            else:
                empties_after_stop = 0
    finally:
        for c in chans:
            c["f"].close()


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

        # emit() runs from the session thread, the ardopcf host-reader threads, AND the
        # audio tailer thread, so serialize writes. A broken pipe (client closed the
        # tab) flips `gone`, which should_abort() polls — tearing ardopcf down instead
        # of leaking the loopback devices.
        wlock = threading.Lock()
        state = {"gone": False}

        def emit(ev):
            with wlock:
                if state["gone"]:
                    return
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
            # Tap BOTH directions: A->B (data) and B->A (acks), mixed into one
            # on-air stream so the receiver's ACK/NAK bursts are heard + seen too.
            tap_fwd = os.path.join(_AUDIO_DIR, "air_fwd.raw")
            tap_rev = os.path.join(_AUDIO_DIR, "air_rev.raw")
            for p in (tap_fwd, tap_rev):
                open(p, "wb").close()  # fresh; the tailer reads from byte 0
            params = SessionParams(
                snr=snr, condition=condition, seed=seed, arqbw=arqbw,
                tap=tap_fwd, tap_rev=tap_rev)
            stop = threading.Event()
            tailer = threading.Thread(
                target=_stream_tap_audio,
                args=([("fwd", tap_fwd), ("rev", tap_rev)], emit, stop), daemon=True)
            tailer.start()
            try:
                run_session(params, emit,
                            should_abort=lambda: state["gone"] or _SESSION_ABORT.is_set())
            except Exception as e:  # never 500 mid-stream — report as an SSE event
                traceback.print_exc()
                emit({"t": "error", "msg": str(e)})
                emit({"t": "done"})
            finally:
                stop.set()
                tailer.join(timeout=2)

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
    # Bind to all interfaces by default: the Pi serves the backend and a laptop on the
    # LAN renders the page (the Pi can't run a browser itself). /api/session shells out
    # to ardopcf, so every query param is validated/coerced (condition + arqbw
    # allowlists, numeric SNR/seed) — keep it to a trusted LAN, not the open internet.
    ap.add_argument("--host", default="0.0.0.0")
    args = ap.parse_args()

    # Clean shutdown: on Ctrl-C / kill, signal any in-flight session to abort so
    # run_session's teardown kills ardopcf/arecord/aplay. Without this, restarting
    # the server mid-session ORPHANS those procs, which keep holding the snd-aloop
    # loopback devices and poison every later session.
    def _shutdown(signum, frame):
        _SESSION_ABORT.set()
        time.sleep(2)  # grace for the session thread to tear its subprocesses down
        os._exit(0)
    signal.signal(signal.SIGINT, _shutdown)
    signal.signal(signal.SIGTERM, _shutdown)

    srv = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"ARDOP connected-mode demo on http://{args.host}:{args.port}/  (site + /api/session SSE)")
    srv.serve_forever()


if __name__ == "__main__":
    main()
