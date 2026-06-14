#!/usr/bin/env python3
"""Minimal ARDOP live-demo backend (sonde-imh.1).

Serves the proven file-based round-trip on demand: a lever change in the frontend
hits `/api/run`, which runs real ARDOP through hf-channel-sim and returns the
decode result as JSON. This is the MVP that takes the demo off static hosting and
onto a live, known-good-mode-anchored backend.

Run:  python3 demo/ardop/server.py [--port 8770]
Then: curl 'http://localhost:8770/api/run?frame=4PSK.500.100.E&snr=-6&condition=none'

No real radio: WAV files only (see ardop_channel.py). ardopcf is external/reference.
"""
import argparse
import json
import re
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

from ardop_channel import FRAMES, run_once

# This endpoint runs an external modem; do NOT open it to arbitrary web origins
# (a wildcard CORS + any query bug = a remote command path). Reflect only
# localhost origins, and validate inputs server-side regardless (see ardop_channel).
_ALLOWED_ORIGIN_RE = re.compile(r"https?://(localhost|127\.0\.0\.1)(:\d+)?")


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        # Reflect the Origin only for localhost (the demo's origin); never wildcard.
        origin = self.headers.get("Origin")
        if origin and _ALLOWED_ORIGIN_RE.fullmatch(origin):
            self.send_header("Access-Control-Allow-Origin", origin)
            self.send_header("Vary", "Origin")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/api/frames":
            return self._send(200, {"frames": FRAMES})
        if u.path == "/api/run":
            q = parse_qs(u.query)
            try:
                res = run_once(
                    frame=q.get("frame", ["4PSK.500.100.E"])[0],
                    snr_db=float(q.get("snr", ["6"])[0]),
                    condition=q.get("condition", ["none"])[0],
                    seed=int(q.get("seed", ["1"])[0]),
                )
                return self._send(200, res)
            except ValueError as e:  # rejected/invalid input → client error
                return self._send(400, {"error": str(e)})
            except Exception as e:  # surface failures as JSON, don't 500 silently
                traceback.print_exc()
                return self._send(500, {"error": str(e)})
        return self._send(404, {"error": "not found", "paths": ["/api/frames", "/api/run"]})

    def log_message(self, *args):
        pass  # quiet


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8770)
    args = ap.parse_args()
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"ARDOP demo backend on http://127.0.0.1:{args.port}  (/api/run, /api/frames)")
    srv.serve_forever()


if __name__ == "__main__":
    main()
