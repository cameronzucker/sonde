#!/usr/bin/env python3
"""Real-time channel filter for the ARDOP connected-mode bridge (sonde-imh.2).

Reads raw mono S16LE PCM from stdin (one station's TX, off an snd-aloop loopback),
applies the HF channel (AWGN now; Watterson fading is a follow-up), and writes the
impaired PCM to stdout (into the other station's RX loopback). Sits in the pipe:

    arecord -t raw -f S16_LE -r 12000 -c1 -D <txloop> \
      | channel_filter.py --snr <dB> --condition <c> \
      | aplay -t raw -f S16_LE -r 12000 -c1 -D <rxloop>

Honest noise model: a CONSTANT noise floor is added always (like a real receiver
hearing band noise), with power set so the target SNR holds against a fixed
reference signal level (ARDOP's TX drive is ~constant during a burst). So dead air
carries noise and a weak signal is genuinely buried — the SNR lever is real.

Deterministic-ish: seed is fixed per process; vary --seed across runs.
"""
import argparse
import sys

import numpy as np

# Reference TX RMS in int16 units, MEASURED at ardopcf's output through the snd-aloop
# loopback (active-burst RMS ≈ 17900, peak ≈ 27700; sonde-imh.2 calibration). Noise
# power is set relative to THIS, not the instantaneous signal, so silence still
# carries the noise floor and the labeled SNR is honest during a transmission.
REF_RMS = 17900.0
# Small block = low added latency. ARDOP's half-duplex turnaround (ConReq→ConAck)
# misses its RX window if the bridge buffers too much, so keep this tight (~21 ms
# at 12 kHz). The per-block numpy overhead is negligible.
BLOCK = 256


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--snr", type=float, required=True, help="target SNR in dB")
    ap.add_argument("--condition", default="none")  # reserved for Watterson taps
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--ref-rms", type=float, default=REF_RMS)
    ap.add_argument("--tap", default=None,
                    help="also write the impaired (on-air) PCM to this file, for the "
                         "live waterfall. Raw S16LE, same rate as the stream.")
    args = ap.parse_args()

    rng = np.random.default_rng(args.seed)
    noise_std = args.ref_rms / (10.0 ** (args.snr / 20.0))

    rin = sys.stdin.buffer
    wout = sys.stdout.buffer
    # Optional tap: capture exactly what the *receiver* hears (signal + the same
    # noise we add to the link), so the waterfall is the honest on-air spectrum at
    # the chosen SNR — not a separate, prettier rendering. Append-mode so a caller
    # can truncate it once at session start and we never clobber a fresh path.
    tap = open(args.tap, "wb") if args.tap else None
    nbytes = BLOCK * 2
    try:
        while True:
            chunk = rin.read(nbytes)
            if not chunk:
                break
            sig = np.frombuffer(chunk, dtype="<i2").astype(np.float32)
            noise = rng.normal(0.0, noise_std, size=sig.shape).astype(np.float32)
            out = np.clip(sig + noise, -32768, 32767).astype("<i2")
            buf = out.tobytes()
            wout.write(buf)
            wout.flush()
            if tap is not None:
                tap.write(buf)
                tap.flush()
    finally:
        if tap is not None:
            tap.close()


if __name__ == "__main__":
    main()
