#!/usr/bin/env python3
"""ARDOP-through-hf-channel-sim SNR sweep (sonde-imh CLI).

Shares the round-trip with the live backend via `ardop_channel`. Sweeps SNR for
one frame and prints decode PASS/FAIL, BER, and RS corrections — a known-good
mode's decode-vs-SNR curve measured through our channel sim. WAV files only; no
real radio. ardopcf is external/reference (clean-sheet / ADR 0014).
"""
import os
import tempfile

from ardop_channel import apply_channel, decode_wav, tx_wav

FRAME = os.environ.get("FRAME", "4PSK.500.100.E")
CONDITION = os.environ.get("CONDITION", "none")
DATA = bytes(range(32))
SNRS = [10, 6, 3, 0, -3, -6, -9, -12, -15]


def main():
    tmp = tempfile.mkdtemp(prefix="ardop_sweep_")
    print(f"frame={FRAME}  condition={CONDITION}  payload={len(DATA)}B")
    tx = tx_wav(FRAME, DATA, tmp)
    print(f"TX WAV: {tx}\n")
    print(f"{'SNR(dB)':>8} | {'decode':>7} | {'BERmax':>7} | {'RSfixed':>8}")
    print("-" * 40)
    for snr in SNRS:
        rx = os.path.join(tmp, f"rx_{snr}.wav")
        apply_channel(tx, rx, snr, CONDITION)
        d = decode_wav(rx)
        ber = f"{d['ber_max']:.1f}%" if d["ber_max"] is not None else "-"
        rs = f"{d['rs_fixed']}/{d['rs_max']}" if d["rs_fixed"] is not None else "-"
        print(f"{snr:>8} | {('PASS' if d['decoded'] else 'FAIL'):>7} | {ber:>7} | {rs:>8}")


if __name__ == "__main__":
    main()
