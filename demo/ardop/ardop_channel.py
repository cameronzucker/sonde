"""Reusable ARDOP-through-hf-channel-sim round-trip runner (sonde-imh.1).

Wraps the proven spike pipeline as a callable so both the CLI sweep
(`spike_roundtrip.py`) and the live demo backend (`server.py`) share one code
path:

    ardopcf TXFRAME --writetxwav -> hf-channel-sim wav_channel -> ardopcf --decodewav

No real radio: WAV files / virtual audio only. ardopcf is an external reference
binary (built separately; never reimplemented — clean-sheet / ADR 0014).

Env overrides:
    ARDOPCF  path to the built ardopcf binary
    WAVCHAN  path to the hf-channel-sim `wav_channel` example binary
"""
import os
import re
import subprocess
import tempfile

# Allowlists / patterns — these values are embedded in ardopcf's `--hostcommands`
# string (which ardopcf parses as `;`-separated commands) and in subprocess args,
# so every externally-influenced value is validated before use. ardopcf host
# commands can drive PTT/CAT, so an unvalidated `frame` is a real injection path.
CONDITIONS = {"none", "good", "moderate", "poor", "flutter"}
_SESSIONID_RE = re.compile(r"[0-9a-fA-F]{1,2}")
_DATA_RE = re.compile(r"[0-9a-fA-F]*")

ARDOPCF = os.environ.get(
    "ARDOPCF", os.path.expanduser("~/Code/ardopcf-spike/build/linux/ardopcf")
)
WAVCHAN = os.environ.get(
    "WAVCHAN",
    os.path.join(os.path.dirname(__file__), "../../target/debug/examples/wav_channel"),
)

# ARDOP data frame types (name, carriers, data-bytes-per-carrier) — robust first.
FRAMES = [
    "4FSK.200.50S.E",
    "4PSK.200.100.E",
    "4PSK.500.100.E",
    "4PSK.1000.100.E",
    "16QAM.2000.100.E",
]


def tx_wav(frame, data, tmp, sessionid="ff", drivelevel=80):
    """Encode `data` (bytes) as one ARDOP `frame`; return the TX WAV path."""
    # Validate everything that lands in the `--hostcommands` string (injection guard).
    if frame not in FRAMES:
        raise ValueError(f"unknown frame {frame!r} (not in allowlist)")
    if not _SESSIONID_RE.fullmatch(sessionid):
        raise ValueError(f"bad sessionid {sessionid!r} (expect 1-2 hex digits)")
    if not (isinstance(drivelevel, int) and 1 <= drivelevel <= 100):
        raise ValueError(f"bad drivelevel {drivelevel!r} (expect int 1..100)")
    datahex = data.hex()
    if not _DATA_RE.fullmatch(datahex):  # bytes.hex() is always hex, belt-and-suspenders
        raise ValueError("payload is not clean hex")
    r = subprocess.run(
        [ARDOPCF, "--nologfile", "--logdir", tmp, "--writetxwav", "-i", "-1", "-o", "-1",
         "--hostcommands",
         f"CONSOLELOG 2;MYCALL N0CALL;DRIVELEVEL {drivelevel};"
         f"TXFRAME {frame} {datahex} 0x{sessionid};CLOSE"],
        capture_output=True, check=True)
    m = re.search(r"Opening WAV file for writing: (\S+)", r.stdout.decode("iso-8859-1"))
    if not m:
        raise RuntimeError("ardopcf produced no TX WAV (check MYCALL/frame/args)")
    return m.group(1)


def apply_channel(src, dst, snr_db, condition="none", seed=1):
    """Run the TX WAV through hf-channel-sim (AWGN [+ Watterson]) -> impaired WAV."""
    if condition not in CONDITIONS:
        raise ValueError(f"unknown condition {condition!r} (not in allowlist)")
    # Coerce numerics so a hostile string can't reach the subprocess arg.
    snr_db = float(snr_db)
    seed = int(seed)
    subprocess.run(
        [WAVCHAN, "--input", src, "--output", dst,
         f"--snr-db={snr_db}", f"--condition={condition}", f"--seed={seed}"],
        capture_output=True, check=True)


def decode_wav(wav):
    """Decode an impaired WAV; return {decoded, ber_max, rs_fixed, rs_max, quality}."""
    r = subprocess.run(
        [ARDOPCF, "--nologfile", "--decodewav", wav, "-y", "--hostcommands", "CONSOLELOG 1"],
        capture_output=True, check=True)
    out = r.stdout.decode("iso-8859-1")
    pas = re.search(r"\[DecodeFrame\] Frame: \S+ Decode (PASS|FAIL)", out)
    ber = [float(b) for b in re.findall(r"BER=([\d.]+)%", out)]
    rs = re.search(r"RS fixed (\d+) \(of (\d+) max\)", out)
    q = re.search(r"Quality= ?(\d+)", out)
    return {
        "decoded": bool(pas) and pas.group(1) == "PASS",
        "ber_max": max(ber) if ber else None,
        "rs_fixed": int(rs.group(1)) if rs else None,
        "rs_max": int(rs.group(2)) if rs else None,
        "quality": int(q.group(1)) if q else None,
    }


def run_once(frame="4PSK.500.100.E", snr_db=6.0, condition="none", data=None, seed=1):
    """One end-to-end round-trip. Returns a JSON-serializable result dict."""
    if data is None:
        data = bytes(range(32))
    tmp = tempfile.mkdtemp(prefix="ardop_run_")
    tx = tx_wav(frame, data, tmp)
    rx = os.path.join(tmp, "rx.wav")
    apply_channel(tx, rx, snr_db, condition, seed)
    res = decode_wav(rx)
    res.update({
        "frame": frame,
        "snr_db": snr_db,
        "condition": condition,
        "payload_len": len(data),
        "tx_wav": tx,
        "rx_wav": rx,
    })
    return res
