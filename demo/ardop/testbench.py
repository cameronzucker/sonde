#!/usr/bin/env python3
"""Real ARDOP connected-mode testbench (sonde-imh.2).

Stands up TWO live ardopcf stations and bridges their audio through the channel
sim, then drives a real CONNECT over ardopcf's ARQ host protocol — the actual
ARDOP protocol (handshake + negotiated data mode + ARQ), not the one-way PHY demo.

Topology (two snd-aloop cards, bidirectional-per-card):
    Station A  <-> card aldA dev0          Station B  <-> card aldB dev0
    bridge uses aldA dev1 / aldB dev1
    A->B:  arecord aldA,1 | hf-channel-pcm | aplay aldB,1
    B->A:  arecord aldB,1 | hf-channel-pcm | aplay aldA,1
    (hf-channel-pcm = the real ITU-R F.520 Watterson channel binary from hf-channel-sim)

Prereq: snd-aloop loaded as two cards (the runner does this):
    sudo modprobe -r snd-aloop; sudo modprobe snd-aloop enable=1,1 \
        index=10,11 pcm_substreams=1 id=aldA,aldB

ardopcf is external/reference (clean-sheet, ADR 0014). Virtual audio only; the
stations have NO PTT device, so nothing can key a real radio — no RF.

Two entry points share the same orchestration:
  * `run_session(params, emit, should_abort)` — drives ONE connected session and
    calls `emit(event)` at each milestone (handshake, negotiated BW, ARQ progress,
    rate adaptation, delivery). Used by the demo backend to stream a live timeline.
  * `main()` — a thin CLI wrapper whose `emit` pretty-prints the same events.
"""
import argparse
import os
import re
import signal
import socket
import subprocess
import threading
import time

ARDOPCF = os.environ.get("ARDOPCF", os.path.expanduser("~/Code/ardopcf-spike/build/linux/ardopcf"))
HERE = os.path.dirname(os.path.abspath(__file__))
# Real ITU-R F.520 channel: the hf-channel-sim streaming binary (Watterson multipath
# + fixed-floor AWGN). Built with `cargo build --release -p hf-channel-sim`. Override
# with $HF_CHANNEL_PCM. (Supersedes the AWGN-only channel_filter.py stand-in.)
CHANNEL_BIN = os.environ.get(
    "HF_CHANNEL_PCM", os.path.normpath(os.path.join(HERE, "../../target/release/hf-channel-pcm")))
PAYLOAD = os.path.join(HERE, "../site/assets/payload.bin")
RATE = 12000  # Hz, the loopback + on-air sample rate

# ardopcf data frame types (the modes it adapts among during a transfer), vs the
# robust connect frames (ConReq/ConAck) which we don't count as data adaptation.
_DATA_FRAME = re.compile(r"Sending Frame Type ((?:4FSK|4PSK|8PSK|16QAM)\.\S+)")
# A cemented connection reports the negotiated bandwidth: "CONNECTED <call> <bw>".
_CONNECTED = re.compile(r"CONNECTED\s+(\S+)\s+(\d+)")


def data_modes_from_log(path):
    """Distinct data frame types ardopcf used, in order — shows rate adaptation."""
    seen, out = set(), []
    try:
        with open(path, encoding="iso-8859-1") as f:
            for line in f:
                m = _DATA_FRAME.search(line)
                if m and m.group(1) not in seen:
                    seen.add(m.group(1)); out.append(m.group(1))
    except OSError:
        pass
    return out


def parse_connected(line):
    """(remote_call, bandwidth_hz) from a 'CONNECTED <call> <bw>' line, or (None, None)."""
    m = _CONNECTED.search(line.upper())
    if not m:
        return None, None
    return m.group(1), int(m.group(2))


class Host:
    """ardopcf host command socket with a background line reader (keeps history).

    Each received protocol line is forwarded to the optional `on_line(name, line)`
    callback so a caller can stream the live handshake without polling history."""

    def __init__(self, name, port, on_line=None):
        self.name = name
        self.on_line = on_line
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=10)
        self.sock.settimeout(0.3)
        self.lines = []
        self.lock = threading.Lock()
        self.alive = True
        self.t = threading.Thread(target=self._read, daemon=True)
        self.t.start()

    def _read(self):
        buf = b""
        while self.alive:
            try:
                d = self.sock.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                break
            if not d:
                break
            buf += d
            while b"\r" in buf:
                line, buf = buf.split(b"\r", 1)
                s = line.decode("iso-8859-1").strip()
                if s:
                    with self.lock:
                        self.lines.append(s)
                    if self.on_line:
                        try:
                            self.on_line(self.name, s)
                        except Exception:
                            pass

    def send(self, cmd):
        self.sock.sendall((cmd + "\r").encode())

    def wait_for(self, substrs, timeout):
        """Wait until any host line contains any of substrs; return the line or None."""
        if isinstance(substrs, str):
            substrs = [substrs]
        end = time.time() + timeout
        seen = 0
        while time.time() < end:
            with self.lock:
                cur = self.lines[seen:]
                seen = len(self.lines)
            for ln in cur:
                if any(x in ln.upper() for x in [s.upper() for s in substrs]):
                    return ln
            time.sleep(0.1)
        return None

    def close(self):
        self.alive = False
        try:
            self.sock.close()
        except OSError:
            pass


class DataConn:
    """ardopcf data socket (host_port+1). Outbound frames are <2B BE len><data>;
    inbound are <2B BE len><3B tag 'ARQ'/'FEC'/'ERR'><data>."""

    def __init__(self, name, port):
        self.name = name
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=10)
        self.sock.settimeout(0.3)
        self.recv_bytes = bytearray()
        self.tags = []
        self.lock = threading.Lock()
        self.alive = True
        self._buf = b""
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        while self.alive:
            try:
                d = self.sock.recv(8192)
            except socket.timeout:
                continue
            except OSError:
                break
            if not d:
                break
            self._buf += d
            while len(self._buf) >= 2:
                ln = (self._buf[0] << 8) | self._buf[1]
                if len(self._buf) < 2 + ln:
                    break
                msg = self._buf[2:2 + ln]
                self._buf = self._buf[2 + ln:]
                tag, data = msg[:3].decode("ascii", "replace"), bytes(msg[3:])
                with self.lock:
                    self.tags.append(tag)
                    if tag == "ARQ":
                        self.recv_bytes += data

    def send_data(self, data):
        frame = len(data).to_bytes(2, "big") + data
        self.sock.sendall(frame)

    def received(self):
        with self.lock:
            return bytes(self.recv_bytes)

    def close(self):
        self.alive = False
        try:
            self.sock.close()
        except OSError:
            pass


def launch_ardopcf(card, port, logpath):
    dev = f"plughw:CARD={card},DEV=0"
    f = open(logpath, "w")
    p = subprocess.Popen(
        [ARDOPCF, "--nologfile", "-i", dev, "-o", dev, str(port)],
        stdout=f, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
    return p, f


CONDITIONS = {"none", "good", "moderate", "poor", "flutter"}


def launch_bridge(src_card, dst_card, snr, condition, seed, tap=None):
    """arecord src dev1 | hf-channel-pcm | aplay dst dev1, one direction.

    Built as chained Popens (NO shell) so the SNR/condition values — which come
    from the UI via the backend — can't be shell-injected. `tap`, when set, asks
    hf-channel-pcm to also write the impaired on-air PCM to that file (per direction,
    to feed that station's live waterfall)."""
    if condition not in CONDITIONS:
        raise ValueError(f"bad condition {condition!r}")
    snr = float(snr)
    seed = int(seed)
    dn = subprocess.DEVNULL
    # Balance latency vs xruns: ~100 ms buffer / 20 ms period. Tighter (40 ms)
    # under-runs on the Pi ("sync_ptr1: Broken pipe") and breaks the handshake;
    # the default (seconds) blows the ARQ turnaround budget. 100 ms is stable and
    # still within ARDOP's half-duplex timing.
    lowlat = ["--buffer-time=100000", "--period-time=20000"]
    rec = subprocess.Popen(
        ["arecord", "-t", "raw", "-f", "S16_LE", "-r", str(RATE), "-c1", *lowlat,
         "-D", f"plughw:CARD={src_card},DEV=1"], stdout=subprocess.PIPE, stderr=dn)
    flt_cmd = [CHANNEL_BIN, "--snr-db", str(snr), "--sample-rate", str(RATE),
               "--condition", condition, "--seed", str(seed)]
    if tap:
        flt_cmd += ["--tap", tap]
    flt = subprocess.Popen(
        flt_cmd, stdin=rec.stdout, stdout=subprocess.PIPE, stderr=dn)
    rec.stdout.close()  # let arecord see SIGPIPE if the filter dies
    ply = subprocess.Popen(
        ["aplay", "-t", "raw", "-f", "S16_LE", "-r", str(RATE), "-c1", *lowlat,
         "-D", f"plughw:CARD={dst_card},DEV=1"], stdin=flt.stdout, stderr=dn)
    flt.stdout.close()
    return [rec, flt, ply]


def wait_port(port, timeout=15):
    end = time.time() + timeout
    while time.time() < end:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return True
        except OSError:
            time.sleep(0.3)
    return False


def aloop_ready():
    """True iff the two snd-aloop cards (aldA/aldB) the bridge needs are present."""
    try:
        out = subprocess.run(["aplay", "-l"], capture_output=True, text=True, timeout=5).stdout
    except (OSError, subprocess.SubprocessError):
        return False
    return "aldA" in out and "aldB" in out


class SessionParams:
    """Tunables for one connected session (defaults match the proven CLI run)."""

    def __init__(self, snr=20.0, condition="none", seed=1, arqbw="2000MAX",
                 call_a="N0AAA", call_b="N0BBB", timeout=60.0, payload=PAYLOAD,
                 data_timeout=90.0, tap=None, tap_rev=None):
        self.snr = float(snr)
        self.condition = condition
        self.seed = int(seed)
        self.arqbw = arqbw
        self.call_a = call_a
        self.call_b = call_b
        self.timeout = float(timeout)
        self.payload = payload
        self.data_timeout = float(data_timeout)
        self.tap = tap          # A->B (data) on-air PCM tap path, or None
        self.tap_rev = tap_rev  # B->A (acks) on-air PCM tap path, or None


# Allowlist of ARQBW values ardopcf accepts (operator-facing bandwidth ceiling).
ARQBW_VALUES = {"200MAX", "500MAX", "1000MAX", "2000MAX",
                "200FORCED", "500FORCED", "1000FORCED", "2000FORCED"}


def run_session(params, emit, should_abort=None):
    """Drive ONE real ARDOP connected session, streaming milestones to `emit`.

    `emit(event)` receives dicts with a "t" (type) key — see the module docstring
    and the demo backend for the vocabulary. `should_abort()` (optional) is polled
    in the long waits; return True to tear the session down early (e.g. the web
    client disconnected). Returns a process exit code (0 pass / 2 no-connect /
    3 partial / 4 environment / 143 aborted)."""
    if params.condition not in CONDITIONS:
        raise ValueError(f"bad condition {params.condition!r}")
    if params.arqbw not in ARQBW_VALUES:
        raise ValueError(f"bad arqbw {params.arqbw!r}")
    abort = should_abort or (lambda: False)

    if not aloop_ready():
        emit({"t": "error", "msg": "snd-aloop cards (aldA/aldB) not loaded — run the "
              "modprobe from the handoff before starting a session."})
        emit({"t": "done"})
        return 4
    if not os.path.exists(CHANNEL_BIN):
        emit({"t": "error", "msg": f"channel binary not built ({CHANNEL_BIN}) — run "
              "`cargo build --release -p hf-channel-sim`."})
        emit({"t": "done"})
        return 4

    procs, files, hosts, dataconns = [], [], [], []
    # Truncate the tap up front so a fresh session's waterfall never shows stale air.
    if params.tap:
        open(params.tap, "wb").close()
    started = time.time()
    try:
        emit({"t": "phase", "phase": "init",
              "msg": f"launching two ardopcf stations · SNR {params.snr:g} dB · {params.condition}"})
        pa, fa = launch_ardopcf("aldA", 8515, "/tmp/tb_ardopA.log"); procs.append(pa); files.append(fa)
        pb, fb = launch_ardopcf("aldB", 8525, "/tmp/tb_ardopB.log"); procs.append(pb); files.append(fb)
        if not (wait_port(8515) and wait_port(8525)):
            emit({"t": "error", "msg": "ardopcf host ports never opened"})
            return 4

        emit({"t": "phase", "phase": "bridge", "msg": "channel bridges up (both directions)"})
        # Tap both directions: A->B (data) and B->A (acks), each to its own file,
        # so the frontend can show a waterfall per station (half-duplex turn-taking).
        procs += launch_bridge("aldA", "aldB", params.snr, params.condition, params.seed, tap=params.tap)
        procs += launch_bridge("aldB", "aldA", params.snr, params.condition, params.seed + 1, tap=params.tap_rev)
        time.sleep(1.0)

        on_line = lambda name, line: emit({"t": "host", "station": name, "line": line})
        A = Host("A", 8515, on_line=on_line); B = Host("B", 8525, on_line=on_line)
        hosts = [A, B]
        emit({"t": "station", "station": "A", "call": params.call_a, "role": "caller"})
        emit({"t": "station", "station": "B", "call": params.call_b, "role": "answerer"})

        for H, call in ((A, params.call_a), (B, params.call_b)):
            H.send("INITIALIZE")
            H.send(f"MYCALL {call}")
            H.send("PROTOCOLMODE ARQ")
            H.send(f"ARQBW {params.arqbw}")
            H.send("ARQTIMEOUT 90")
            H.send("CWID FALSE")
        time.sleep(0.5)

        emit({"t": "phase", "phase": "listen", "msg": f"{params.call_b} listening (answerer)"})
        B.send("LISTEN TRUE")
        time.sleep(0.5)
        emit({"t": "phase", "phase": "call",
              "msg": f"{params.call_a} dials {params.call_b} — ARQ CONNECT request"})
        A.send(f"ARQCALL {params.call_b} 5")

        # Success = a CEMENTED connection: ardopcf emits "CONNECTED <call> <bw>"
        # only after the full ConReq->ConAck->ack handshake + bandwidth negotiation.
        ca = A.wait_for("CONNECTED", params.timeout)
        cb = B.wait_for("CONNECTED", 5)
        if abort():
            emit({"t": "error", "msg": "aborted"}); return 143
        if not ca:
            emit({"t": "result", "outcome": "fail", "bandwidth": None, "modes": [],
                  "received": 0, "total": 0, "duration_s": time.time() - started,
                  "image_hex": "", "tap_path": params.tap, "tap_rate": RATE,
                  "msg": "no CONNECT within timeout — link can't close at this SNR"})
            return 2

        _, bw = parse_connected(ca)
        emit({"t": "connected", "call_a": params.call_a, "call_b": params.call_b,
              "bandwidth": bw, "raw_a": ca, "raw_b": cb})

        # ── Data transfer over the live ARQ link ──────────────────────────────
        with open(params.payload, "rb") as f:
            payload = f.read()
        total = len(payload)
        dA = DataConn("A", 8516); dB = DataConn("B", 8526); dataconns += [dA, dB]
        emit({"t": "data_start", "bytes": total, "name": os.path.basename(params.payload)})
        dA.send_data(payload)

        seen_modes, last_recv = [], -1
        end = time.time() + params.data_timeout
        while time.time() < end:
            if abort():
                emit({"t": "error", "msg": "aborted"}); return 143
            got = len(dB.received())
            if got != last_recv:
                emit({"t": "progress", "received": got, "total": total})
                last_recv = got
            modes = data_modes_from_log("/tmp/tb_ardopA.log")
            if modes != seen_modes:
                seen_modes = modes
                emit({"t": "mode", "modes": modes, "current": modes[-1] if modes else None})
            if got >= total:
                break
            time.sleep(0.3)

        got = dB.received()
        intact = got[:total] == payload
        emit({"t": "delivered", "received": len(got), "total": total, "intact": intact})

        A.send("DISCONNECT")
        A.wait_for(["DISCONNECTED", "NEWSTATE DISC"], 15)
        modes = data_modes_from_log("/tmp/tb_ardopA.log")
        outcome = "pass" if intact else "partial"
        emit({"t": "result", "outcome": outcome, "bandwidth": bw, "modes": modes,
              "received": len(got), "total": total, "duration_s": time.time() - started,
              "image_hex": got.hex(), "tap_path": params.tap, "tap_rate": RATE})
        return 0 if intact else 3
    finally:
        for D in dataconns:
            D.close()
        for H in hosts:
            H.close()
        for p in procs:
            try:
                p.terminate()
            except (ProcessLookupError, OSError):
                pass
        time.sleep(0.5)
        for p in procs:
            try:
                p.kill()
            except (ProcessLookupError, OSError):
                pass
        for f in files:
            try:
                f.close()
            except OSError:
                pass
        emit({"t": "done"})


def _cli_emit(ev):
    """Pretty-print one session event to stdout (the CLI's view of the stream)."""
    t = ev.get("t")
    ts = time.strftime("%H:%M:%S")
    if t == "host":
        print(f"[{ts}]   {ev['station']} >> {ev['line']}", flush=True)
    elif t == "phase":
        print(f"[{ts}] {ev['msg']}", flush=True)
    elif t == "station":
        print(f"[{ts}] station {ev['station']} = {ev['call']} ({ev['role']})", flush=True)
    elif t == "connected":
        print(f"[{ts}] *** CONNECTED — negotiated BW {ev['bandwidth']} Hz ***  "
              f"A:{ev['raw_a']!r} B:{ev['raw_b']!r}", flush=True)
    elif t == "data_start":
        print(f"[{ts}] DATA: loading {ev['bytes']} B ({ev['name']}) into the ARQ buffer", flush=True)
    elif t == "progress":
        print(f"[{ts}] DATA: {ev['received']}/{ev['total']} B delivered", flush=True)
    elif t == "mode":
        print(f"[{ts}] RATE: data modes used → {ev['modes'] or '(none yet)'}", flush=True)
    elif t == "delivered":
        print(f"[{ts}] DATA: {ev['received']}/{ev['total']} B  intact={ev['intact']}", flush=True)
    elif t == "result":
        print(f"[{ts}] RESULT: {ev['outcome'].upper()} — {ev.get('msg', '')}".rstrip(" —"), flush=True)
    elif t == "error":
        print(f"[{ts}] ERROR: {ev['msg']}", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--snr", type=float, default=20)
    ap.add_argument("--condition", default="none")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--arqbw", default="2000MAX")
    ap.add_argument("--call-a", default="N0AAA")
    ap.add_argument("--call-b", default="N0BBB")
    ap.add_argument("--timeout", type=float, default=60)
    ap.add_argument("--payload", default=PAYLOAD, help="file to transfer over the link")
    ap.add_argument("--data-timeout", type=float, default=90)
    ap.add_argument("--tap", default=None, help="capture data-direction on-air PCM here")
    args = ap.parse_args()

    # Turn SIGTERM (e.g. `timeout`, or the backend killing us) into SystemExit so
    # the finally-block teardown runs — otherwise arecord/aplay leak and hold the
    # loopback devices, breaking the next run.
    signal.signal(signal.SIGTERM, lambda *a: (_ for _ in ()).throw(SystemExit(143)))

    params = SessionParams(
        snr=args.snr, condition=args.condition, seed=args.seed, arqbw=args.arqbw,
        call_a=args.call_a, call_b=args.call_b, timeout=args.timeout,
        payload=args.payload, data_timeout=args.data_timeout, tap=args.tap)
    return run_session(params, _cli_emit)


if __name__ == "__main__":
    raise SystemExit(main())
