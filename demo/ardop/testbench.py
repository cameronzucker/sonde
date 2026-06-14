#!/usr/bin/env python3
"""Real ARDOP connected-mode testbench (sonde-imh.2).

Stands up TWO live ardopcf stations and bridges their audio through the channel
sim, then drives a real CONNECT over ardopcf's ARQ host protocol — the actual
ARDOP protocol (handshake + negotiated data mode + ARQ), not the one-way PHY demo.

Topology (two snd-aloop cards, bidirectional-per-card):
    Station A  <-> card aldA dev0          Station B  <-> card aldB dev0
    bridge uses aldA dev1 / aldB dev1
    A->B:  arecord aldA,1 | channel_filter | aplay aldB,1
    B->A:  arecord aldB,1 | channel_filter | aplay aldA,1

Prereq: snd-aloop loaded as two cards (the runner does this):
    sudo modprobe -r snd-aloop; sudo modprobe snd-aloop enable=1,1 \
        index=10,11 pcm_substreams=1 id=aldA,aldB

ardopcf is external/reference (clean-sheet, ADR 0014). Virtual audio only; the
stations have NO PTT device, so nothing can key a real radio — no RF.
"""
import argparse
import os
import signal
import socket
import subprocess
import threading
import time

ARDOPCF = os.environ.get("ARDOPCF", os.path.expanduser("~/Code/ardopcf-spike/build/linux/ardopcf"))
HERE = os.path.dirname(os.path.abspath(__file__))
FILTER = os.path.join(HERE, "channel_filter.py")
PAYLOAD = os.path.join(HERE, "../site/assets/payload.bin")
RATE = "12000"

# ardopcf data frame types (the modes it adapts among during a transfer), vs the
# robust connect frames (ConReq/ConAck) which we don't count as data adaptation.
import re as _re
_DATA_FRAME = _re.compile(r"Sending Frame Type ((?:4FSK|4PSK|8PSK|16QAM)\.\S+)")


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


def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


class Host:
    """ardopcf host command socket with a background line reader (keeps history)."""

    def __init__(self, name, port):
        self.name = name
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
                    log(f"  {self.name} >> {s}")

    def send(self, cmd):
        log(f"  {self.name} << {cmd}")
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


def launch_bridge(src_card, dst_card, snr, condition, seed):
    """arecord src dev1 | channel_filter | aplay dst dev1, one direction.

    Built as chained Popens (NO shell) so the SNR/condition values — which will
    later come from the UI via the backend — can't be shell-injected."""
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
        ["arecord", "-t", "raw", "-f", "S16_LE", "-r", RATE, "-c1", *lowlat,
         "-D", f"plughw:CARD={src_card},DEV=1"], stdout=subprocess.PIPE, stderr=dn)
    flt = subprocess.Popen(
        ["python3", "-u", FILTER, "--snr", str(snr), "--condition", condition, "--seed", str(seed)],
        stdin=rec.stdout, stdout=subprocess.PIPE, stderr=dn)
    rec.stdout.close()  # let arecord see SIGPIPE if the filter dies
    ply = subprocess.Popen(
        ["aplay", "-t", "raw", "-f", "S16_LE", "-r", RATE, "-c1", *lowlat,
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
    args = ap.parse_args()

    # Turn SIGTERM (e.g. `timeout`, or the backend killing us) into SystemExit so
    # the finally-block teardown runs — otherwise arecord/aplay leak and hold the
    # loopback devices, breaking the next run.
    signal.signal(signal.SIGTERM, lambda *a: (_ for _ in ()).throw(SystemExit(143)))

    procs, files = [], []
    hosts = []
    dataconns = []
    try:
        log(f"launching ardopcf A (aldA:8515) and B (aldB:8525), SNR={args.snr} dB")
        pa, fa = launch_ardopcf("aldA", 8515, "/tmp/tb_ardopA.log"); procs.append(pa); files.append(fa)
        pb, fb = launch_ardopcf("aldB", 8525, "/tmp/tb_ardopB.log"); procs.append(pb); files.append(fb)
        if not (wait_port(8515) and wait_port(8525)):
            log("FAIL: ardopcf host ports never opened"); return 1

        log("starting channel bridges (both directions)")
        procs += launch_bridge("aldA", "aldB", args.snr, args.condition, args.seed)
        procs += launch_bridge("aldB", "aldA", args.snr, args.condition, args.seed + 1)
        time.sleep(1.0)

        A = Host("A", 8515); B = Host("B", 8525); hosts = [A, B]

        for H, call in ((A, args.call_a), (B, args.call_b)):
            H.send("INITIALIZE")
            H.send(f"MYCALL {call}")
            H.send("PROTOCOLMODE ARQ")
            H.send(f"ARQBW {args.arqbw}")
            H.send("ARQTIMEOUT 90")
            H.send("CWID FALSE")
        time.sleep(0.5)

        log("B: LISTEN TRUE (answerer)")
        B.send("LISTEN TRUE")
        time.sleep(0.5)
        log(f"A: ARQCALL {args.call_b} (caller dials the remote station)")
        A.send(f"ARQCALL {args.call_b} 5")

        # Success = a CEMENTED connection: ardopcf emits "CONNECTED <call> <bw>"
        # only after the full ConReq→ConAck→ack handshake + bandwidth negotiation.
        ca = A.wait_for("CONNECTED", args.timeout)
        cb = B.wait_for("CONNECTED", 5)
        if ca:
            log(f"*** CONNECTED (handshake + bandwidth negotiated) ***  A: {ca!r}  B: {cb!r}")
            # ── Data transfer over the live ARQ link ──────────────────────────
            with open(args.payload, "rb") as f:
                payload = f.read()
            dA = DataConn("A", 8516); dB = DataConn("B", 8526)
            dataconns += [dA, dB]
            log(f"DATA: A loads {len(payload)} bytes ({os.path.basename(args.payload)}) into the ARQ buffer")
            dA.send_data(payload)
            end = time.time() + args.data_timeout
            while time.time() < end:
                if len(dB.received()) >= len(payload):
                    break
                time.sleep(0.3)
            got = dB.received()
            delivered = got[:len(payload)] == payload
            log(f"DATA: B received {len(got)}/{len(payload)} bytes  delivered_intact={delivered}")

            A.send("DISCONNECT")
            A.wait_for(["DISCONNECTED", "NEWSTATE DISC"], 15)
            modes = data_modes_from_log("/tmp/tb_ardopA.log")
            log(f"DATA: rate adaptation — A used data modes: {modes or '(none logged)'}")
            if delivered:
                log("RESULT: PASS — real ARDOP connect + ARQ data transfer through the channel sim")
                rc = 0
            else:
                log("RESULT: PARTIAL — connected but payload not fully delivered in time")
                rc = 3
        else:
            log("RESULT: FAIL — no CONNECT within timeout (see /tmp/tb_*.log)")
            rc = 2
        return rc
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


if __name__ == "__main__":
    raise SystemExit(main())
