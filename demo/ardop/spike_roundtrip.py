#!/usr/bin/env python3
"""sonde-imh feasibility spike: round-trip a real ARDOP data frame through hf-channel-sim.

Pipeline (no real radio — WAV files only, per the spike safety boundary):
    ardopcf TXFRAME --writetxwav        -> tx.wav   (real ARDOP modulated audio)
    hf-channel-sim `wav_channel` example -> rx.wav   (our channel: AWGN [+ Watterson])
    ardopcf --decodewav rx.wav          -> decode    (PASS/FAIL + BER + RS corrections)

Sweeping SNR shows a known-good mode's real decode behavior *measured through our
channel sim* — the foundation for auditing the demo against ARDOP, then adding Sonde
as a comparison.

Env overrides:
    ARDOPCF  path to the built ardopcf binary (built separately; external)
    WAVCHAN  path to the hf-channel-sim wav_channel example binary
"""
import os
import re
import subprocess
import tempfile

ARDOPCF = os.environ.get("ARDOPCF", os.path.expanduser("~/Code/ardopcf-spike/build/linux/ardopcf"))
WAVCHAN = os.environ.get(
    "WAVCHAN",
    os.path.join(os.path.dirname(__file__), "../../target/debug/examples/wav_channel"),
)
FRAME = os.environ.get("FRAME", "4PSK.500.100.E")
DATA = bytes(range(32))           # 32 known payload bytes
SNRS = [10, 6, 3, 0, -3, -6, -9, -12, -15]  # dB, AWGN ("none")
CONDITION = os.environ.get("CONDITION", "none")


def tx_wav(tmp):
    r = subprocess.run(
        [ARDOPCF, "--nologfile", "--logdir", tmp, "--writetxwav", "-i", "-1", "-o", "-1",
         "--hostcommands",
         f"CONSOLELOG 2;MYCALL N0CALL;DRIVELEVEL 80;TXFRAME {FRAME} {DATA.hex()} 0xff;CLOSE"],
        capture_output=True, check=True)
    m = re.search(r"Opening WAV file for writing: (\S+)", r.stdout.decode("iso-8859-1"))
    if not m:
        raise SystemExit("could not find TX WAV path in ardopcf output")
    return m.group(1)


def channel(src, dst, snr_db):
    # `--snr-db=-3` (equals form) so clap doesn't read a negative value as a flag.
    subprocess.run([WAVCHAN, "--input", src, "--output", dst,
                    f"--snr-db={snr_db}", f"--condition={CONDITION}"],
                   capture_output=True, check=True)


def decode(wav):
    r = subprocess.run([ARDOPCF, "--nologfile", "--decodewav", wav, "-y",
                        "--hostcommands", "CONSOLELOG 1"], capture_output=True, check=True)
    out = r.stdout.decode("iso-8859-1")
    pas = re.search(r"\[DecodeFrame\] Frame: \S+ Decode (PASS|FAIL)", out)
    ber = [float(b) for b in re.findall(r"BER=([\d.]+)%", out)]
    rs = re.search(r"RS fixed (\d+) \(of (\d+) max\)", out)
    return {
        "pass": pas.group(1) if pas else "NONE",
        "ber_max": max(ber) if ber else None,
        "rs": f"{rs.group(1)}/{rs.group(2)}" if rs else "-",
    }


def main():
    tmp = tempfile.mkdtemp(prefix="ardop_spike_")
    print(f"frame={FRAME}  condition={CONDITION}  payload={len(DATA)}B  tmp={tmp}")
    tx = tx_wav(tmp)
    print(f"TX WAV: {tx}\n")
    print(f"{'SNR(dB)':>8} | {'decode':>7} | {'BERmax':>7} | {'RSfixed':>8}")
    print("-" * 40)
    for snr in SNRS:
        rx = os.path.join(tmp, f"rx_{snr}.wav")
        channel(tx, rx, snr)
        d = decode(rx)
        ber = f"{d['ber_max']:.1f}%" if d["ber_max"] is not None else "-"
        print(f"{snr:>8} | {d['pass']:>7} | {ber:>7} | {d['rs']:>8}")


if __name__ == "__main__":
    main()
