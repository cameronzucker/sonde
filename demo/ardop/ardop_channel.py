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
import wave
from concurrent.futures import ThreadPoolExecutor

import numpy as np

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

# Net payload capacity (bytes) per frame type, measured empirically by sending an
# oversized payload and reading ardopcf's reported `frameLen` (sonde-imh.1 probe).
# A multi-frame transfer chunks the payload into ceil(len/capacity) frames.
FRAME_CAPACITY = {
    "4FSK.200.50S.E": 16,
    "4PSK.200.100.E": 64,
    "4PSK.500.100.E": 128,
    "4PSK.1000.100.E": 256,
    "16QAM.2000.100.E": 1024,
}

# A multi-frame transfer is bounded so a hostile/over-large payload can't fan out
# into unbounded ardopcf invocations (also keeps the demo's latency sane).
MAX_FRAMES = 64

# Audio band shown in the spectrogram (Hz). ARDOP SSB audio sits ~300–2700 Hz;
# cap the STFT a little above that so the occupied band fills the surface.
SPEC_FREQ_CAP_HZ = 3600.0

_PAYLOAD_PATH = os.path.join(os.path.dirname(__file__), "../site/assets/payload.bin")


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


# ─────────────────────── multi-frame image transfer ────────────────────────
# The live demo sends a real ~3 KB image (the committed payload.bin) across
# however many ARDOP frames the chosen mode needs, runs each through the channel
# sim independently, and reassembles what survived. There is no ARQ: a frame that
# fails to decode is a hole in the recovered image — an honest depiction of HF
# data transfer without a link layer (and a motivation for one).


def default_payload():
    """The committed demo payload (header + body + ~2.6 KB JPEG); bytes."""
    with open(_PAYLOAD_PATH, "rb") as f:
        return f.read()


def _read_wav_mono(path):
    """Read a mono PCM WAV → (float32 samples in [-1,1], sample_rate)."""
    with wave.open(path, "rb") as w:
        n, sr, sw, ch = w.getnframes(), w.getframerate(), w.getsampwidth(), w.getnchannels()
        raw = w.readframes(n)
    if sw == 2:
        a = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    elif sw == 1:
        a = (np.frombuffer(raw, dtype=np.uint8).astype(np.float32) - 128.0) / 128.0
    else:
        raise ValueError(f"unsupported WAV sample width {sw}")
    if ch > 1:
        a = a[::ch]  # take channel 0
    return a, sr


def _write_wav_mono(path, samples, sr):
    """Write float32 samples in [-1,1] to a 16-bit mono PCM WAV."""
    pcm = (np.clip(samples, -1.0, 1.0) * 32767.0).astype("<i2").tobytes()
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes(pcm)


def _trim_silence(samples, thresh=0.01, pad=600):
    """Trim leading/trailing near-silence (ardopcf pads each TX WAV) so concatenated
    frames play back-to-back. `pad` keeps a short cushion around the energy."""
    idx = np.where(np.abs(samples) > thresh)[0]
    if len(idx) == 0:
        return samples[:0]
    lo = max(0, idx[0] - pad)
    hi = min(len(samples), idx[-1] + pad)
    return samples[lo:hi]


def compute_spectrogram(samples, sr, rows=100, nfft=256):
    """STFT magnitude → {rows, cols, mag_q (0..255 row-major), freqs_hz, times_s}.

    Quantized to the 0..255 grid the demo waterfall consumes. Returns None for a
    signal too short to frame."""
    if len(samples) < nfft:
        return None
    hop = max(1, (len(samples) - nfft) // max(1, rows - 1))
    win = np.hanning(nfft).astype(np.float32)
    frames = []
    for r in range(rows):
        start = r * hop
        seg = samples[start:start + nfft]
        if len(seg) < nfft:
            seg = np.pad(seg, (0, nfft - len(seg)))
        frames.append(np.abs(np.fft.rfft(seg * win)))
    mag = np.array(frames)  # rows × (nfft/2 + 1)
    bin_hz = sr / nfft
    maxbin = min(mag.shape[1], int(SPEC_FREQ_CAP_HZ / bin_hz) + 1)
    mag = mag[:, :maxbin]
    mag_db = 20.0 * np.log10(mag + 1e-6)
    lo, hi = np.percentile(mag_db, 5.0), np.percentile(mag_db, 99.5)
    norm = np.clip((mag_db - lo) / max(1e-6, hi - lo), 0.0, 1.0)
    mag_q = (norm * 255.0).astype(np.uint8).flatten().tolist()
    n_rows, n_cols = mag.shape
    return {
        "rows": int(n_rows),
        "cols": int(n_cols),
        "mag_q": mag_q,
        "freqs_hz": [round(c * bin_hz, 1) for c in range(n_cols)],
        "times_s": [round(r * hop / sr, 3) for r in range(n_rows)],
    }


def _run_one_frame(frame, chunk, snr_db, condition, seed, idx):
    """TX one chunk → channel → decode. Returns (decode_dict, rx_samples, sr)."""
    tmp = tempfile.mkdtemp(prefix=f"ardop_f{idx}_")
    tx = tx_wav(frame, chunk, tmp)
    rx = os.path.join(tmp, "rx.wav")
    apply_channel(tx, rx, snr_db, condition, seed)
    dec = decode_wav(rx)
    samples, sr = _read_wav_mono(rx)
    return dec, _trim_silence(samples), sr


def run_transfer(frame="4PSK.500.100.E", snr_db=6.0, condition="none", seed=1,
                 payload=None, max_workers=8):
    """Send `payload` across as many `frame`s as its capacity requires; reassemble.

    Returns a JSON-serializable dict the demo frontend consumes directly:
      frame, snr_db, condition, seed, capacity, payload_len,
      frames[]      per-frame {seq, decoded, ber_max, rs_fixed, rs_max, quality,
                               byte_start, byte_end, t_start_s},
      recovered_hex reassembled payload (PASS frames = ground truth, FAIL = zeros),
      summary       {frames_total, frames_decoded, bytes_recovered, frame_loss},
      audio         {wav: <float32 list is NOT inlined — see server /api/audio>},
      spectrogram   {rows, cols, mag_q, freqs_hz, times_s},
      duration_s    concatenated transmission length.
    The reassembly trusts ARDOP's CRC: a decode PASS means the recovered bytes
    equal the transmitted bytes, so PASS chunks are filled from ground truth and
    FAIL chunks are left as holes (no ARQ retransmit)."""
    if frame not in FRAMES:
        raise ValueError(f"unknown frame {frame!r} (not in allowlist)")
    if condition not in CONDITIONS:
        raise ValueError(f"unknown condition {condition!r} (not in allowlist)")
    snr_db = float(snr_db)
    seed = int(seed)
    if payload is None:
        payload = default_payload()
    cap = FRAME_CAPACITY[frame]
    chunks = [payload[i:i + cap] for i in range(0, len(payload), cap)]
    if len(chunks) > MAX_FRAMES:
        raise ValueError(f"payload needs {len(chunks)} frames > MAX_FRAMES {MAX_FRAMES}")

    # Frames are independent (no ARQ ordering) → decode them in parallel. Each
    # frame gets seed+idx so it sees its own channel realization.
    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        outs = list(pool.map(
            lambda t: _run_one_frame(frame, t[1], snr_db, condition, seed + t[0], t[0]),
            list(enumerate(chunks)),
        ))

    recovered = bytearray(len(payload))
    frame_rows, audio_parts = [], []
    sr = 12000
    cursor = 0.0
    decoded_count = 0
    for i, (dec, samples, frame_sr) in enumerate(outs):
        sr = frame_sr
        start = i * cap
        end = start + len(chunks[i])
        if dec["decoded"]:
            recovered[start:end] = chunks[i]  # CRC-validated → equals ground truth
            decoded_count += 1
        frame_rows.append({
            "seq": i,
            "decoded": dec["decoded"],
            "ber_max": dec["ber_max"],
            "rs_fixed": dec["rs_fixed"],
            "rs_max": dec["rs_max"],
            "quality": dec["quality"],
            "byte_start": start,
            "byte_end": end,
            "t_start_s": round(cursor, 3),
        })
        audio_parts.append(samples)
        cursor += len(samples) / frame_sr

    audio = np.concatenate(audio_parts) if audio_parts else np.zeros(1, dtype=np.float32)
    duration_s = len(audio) / sr
    spec = compute_spectrogram(audio, sr)

    return {
        "frame": frame,
        "snr_db": snr_db,
        "condition": condition,
        "seed": seed,
        "capacity": cap,
        "payload_len": len(payload),
        "frames": frame_rows,
        "recovered_hex": bytes(recovered).hex(),
        "summary": {
            "frames_total": len(chunks),
            "frames_decoded": decoded_count,
            "bytes_recovered": decoded_count * cap if decoded_count else 0,
            "frame_loss": round(1.0 - decoded_count / len(chunks), 3) if chunks else 1.0,
        },
        "spectrogram": spec,
        "duration_s": round(duration_s, 3),
        "sample_rate": sr,
        "_audio_samples": audio,  # consumed by the server to serve /api/audio; not JSON
    }
