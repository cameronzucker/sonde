# ARDOP feasibility spike (sonde-imh)

**Question:** can we drive a real, known-good HF mode (ARDOP) *through our
`hf-channel-sim`* and measure its behaviour — the foundation for re-anchoring the
demo on ARDOP, then adding Sonde as a comparison?

**Answer: yes, conclusively.** No real radio involved — WAV files only.

## What was proven

`ardopcf` (the maintained C ARDOP, built from source) runs **headless** with no
sound hardware (`-i -1 -o -1`), emits a WAV of its modulated TX audio
(`--writetxwav` + `TXFRAME`), and decodes a WAV instead of listening
(`--decodewav`), reporting `Decode PASS/FAIL`, `Quality`, per-carrier `BER`, and
Reed-Solomon corrections used. We splice our channel in between with a new
`hf-channel-sim` example, `wav_channel` (WAV in → AWGN [+ Watterson] → WAV out).

Pipeline: `ardopcf TXFRAME --writetxwav` → `wav_channel` (our sim) → `ardopcf --decodewav`.

### Result — `4PSK.500.100.E`, 32 B payload, AWGN, through `hf-channel-sim`

| SNR (dB)\* | decode | BER  | RS fixed |
|---:|:---:|---:|:---:|
| 0  | PASS | 0.0% | 0/32 |
| −3 | PASS | 0.3% | 1/32 |
| −6 | PASS | 1.9% | 13/32 |
| −9 | FAIL | – | – |

Monotonic, physically sensible: error-correction load ramps as SNR drops, then a
sharp decode cliff between −6 and −9 dB. That's a real modem's behaviour, measured
through our sim.

\* **SNR units are the sim's current ad-hoc full-band ratio** (the one the modem
audit flags as non-standard). The absolute dB are *not* a real-HF reference yet —
but ARDOP and Sonde measured in the **same sim with the same axis are directly
comparable**, which is the whole point. Better: ARDOP's *documented* real-world
sensitivity gives a way to **calibrate the axis** (map "−6 dB here" → real dB),
which also feeds the P0 SNR-methodology fix in the modem-direction doc.

## Why this matters

- The demo can be re-anchored on a mode whose real behaviour is known, so the
  demo itself becomes auditable. Sonde added later sits **beside** ARDOP — any
  Sonde wrongness shows up by comparison (the sniff test the operator needs).
- `wav_channel` is the reusable bridge between any WAV-capable modem and our sim.

## Path to "ARDOP live in the demo" (the chosen architecture)

This spike is file-based (offline). The live demo needs a backend harness:
1. `ardopcf` running as a TNC (TCP host control); **or** keep the file-based
   round-trip but drive it on demand from the backend (simpler, already proven).
2. The backend serves results (decode/BER/RS vs SNR, and the audio/spectrogram)
   to the frontend — the demo leaves static GitHub Pages.
3. `snd-aloop` is present on this host if true real-time audio streaming is later
   wanted, but the file-based path already gives everything the demo needs and is
   far simpler — **recommend starting the live demo on the file-based round-trip.**

## Reproduce

```bash
# 1. Build ardopcf (external; github.com/Dinsmoor/ardopcf), needs libasound2-dev:
#    git clone https://github.com/Dinsmoor/ardopcf && cd ardopcf && make
# 2. Build the channel bridge:
cargo build -p hf-channel-sim --example wav_channel
# 3. Run the sweep (set ARDOPCF / WAVCHAN env if paths differ):
python3 demo/ardop/spike_roundtrip.py
```

`ardopcf` is **not** vendored here — it's external, used as a reference only
(clean-sheet / ADR 0014 intact: we never examine or reimplement its internals).

**Safety:** the spike runs ardopcf only against WAV files / virtual audio — never
a real sound card or PTT, no RF.
