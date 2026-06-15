# Converge: Step 3 validation gate — coded mode over realistic fading (sonde-xhw.4)

Context: clean-sheet HF OFDM modem. Step 0 (Eb/N0 methodology), Step 1
(transmittable waveform), Step 2 (real Schmidl-Cox sync in the loop), and the n0
estimator (sonde-gtg: per-bin effective noise) are all DONE + merged. Step 3
composes them and VALIDATES the coded floor over realistic fading.

Acceptance criteria (operator-set): "Stated success-rate over Watterson
Good/Moderate/Poor at stated Eb/N0, AND a coded-vs-uncoded BER curve showing the
expected LDPC coding gain in dB."

## What's already true (measured this session, 8 seeds, production sync path)
Coded (rate-1/4 LDPC) frame-decode rate via `receive_multi_with_sync` (real
Schmidl-Cox sync + CFO + per-bin n0 estimate + channel-aware LLR), Eb/N0 in dB:

```
            18dB  22dB  26dB  30dB
Good        2/8   7/8   8/8   8/8
Moderate    3/8   7/8   8/8   8/8
Poor        1/8   3/8   8/8   8/8
```

All three conditions reach 8/8 by ~26 dB. Poor's cliff is ~2-4 dB worse than
Good/Moderate. Watterson params: Good Δτ=0.5ms/2σ=0.1Hz, Moderate 1.0ms/0.5Hz,
Poor 2.0ms/1.0Hz. Delay spread ≤96 samples ≪ 512-sample CP (no ISI); Doppler
≤1Hz, coherence ≥1s ≫ symbol (53ms), so the per-symbol pilot equalizer tracks it
feedforward. **My conclusion: no explicit Doppler/time tracker is needed for
Good/Moderate/Poor** — confirm or refute (Flutter @10Hz is out of acceptance
scope).

## Proposed Step-3 gates

### Gate A: success-rate table (frame-error-rate)
Assert, through the production sync path at a STATED Eb/N0 where all three are
robust, a decode-rate floor per condition. From the data, Eb/N0 = 28 dB (between
the 26 dB all-8/8 point and 30 dB) — or state 30 dB for margin. Seed sweep
(16 seeds), assert ≥ N/16 per condition. Report the full FER-vs-Eb/N0 curve.
**Q1:** what STATED Eb/N0 + per-condition pass threshold is credible and
non-flaky given the measured cliffs? Is a single stated Eb/N0 enough, or should
the gate assert the Poor cliff is within ~X dB of Good?

### Gate B: coded-vs-uncoded LDPC coding-gain curve
The hard part is methodology. Coded decode is frame-CRC all-or-nothing
(FloorRate14 returns Err on CRC fail → no output bits), so a post-decode "BER"
on failed frames isn't available without a decode-anyway path. Uncoded
(IdentityFec passthrough, hard-decision LLR sign) gives a clean raw BER.

Options I'm weighing:
- (a) **AWGN-only waterfall** (no fade): uncoded BPSK BER vs Eb/N0 (matches
  theory Q(√(2Eb/N0))) AND coded FER (or post-decode BER) vs Eb/N0; coding gain =
  Eb/N0 shift at a reference. Cleanest dB number, but "over AWGN" not "over
  fading" — does that satisfy "expected LDPC coding gain"?
- (b) **Over the fade**: uncoded BER has an irreducible floor at nulls (a deep
  null erases bits no matter the SNR); coded FER drops to 0. The "gain" is then
  ~infinite at low BER — honest but not a clean dB figure.
- (c) Report BOTH: AWGN waterfall for the dB coding-gain number, plus the
  fade FER table (Gate A) for the realistic-channel story.

**Q2:** which is the honest, standard way to show "LDPC coding gain in dB"? I
lean (c). For the AWGN coding-gain number, what reference metric/BER and what
gain magnitude should I expect for a rate-1/4 LDPC at ~648/1296/2048 block over
BPSK (ballpark dB, so I can sanity-check the measurement)?
**Q3:** For the coded arm's "BER", is FER the honest metric (frame
all-or-nothing), or should I add a decode-returns-bits-on-CRC-fail path to count
true post-decode BER? I'd rather use FER for coded + BER for uncoded and label
them as such, plotting on a shared Eb/N0 axis — acceptable, or misleading?
**Q4:** Uncoded BER needs many bits for low-BER points; the payload is 200 info
bits/frame. Accumulate over many seeds/frames per Eb/N0 point (how many bits for
a credible 1e-3 / 1e-4 floor)?
**Q5:** Anything physically wrong in the plan, or a cleaner way to satisfy the
acceptance criteria? Keep it honest (gate on physics): no inflated gain claims.

Be terse; confirm/refute the no-Doppler-tracker conclusion, pick the coding-gain
methodology, and give the stated Eb/N0 + thresholds + expected gain ballpark.
