# Converge round 2: Step-3 gate (sonde-xhw.4) — POST-vb9 re-measurement

sonde-vb9 is FIXED (adaptive pilot time-smoothing; flat-AWGN coded threshold
~12 dB → ~6 dB). Now building the Step-3 validation gate. Acceptance (operator):
"stated success-rate over Watterson Good/Moderate/Poor at stated Eb/N0, AND a
coded-vs-uncoded BER curve showing LDPC coding gain in dB."

## Fresh measurements (post-vb9)

A) FLAT AWGN, production `receive_multi` (no sync), vb9 honest Eb/N0 (σ² from
   measured TX sample power, rate-aware): rate-1/4 coded FER → 0 by **~6 dB**;
   coded FER 0 @8 dB where uncoded FER is still 1.0.

B) FADING, full production sync path `receive_multi_with_sync`
   (analytic → Watterson → AWGN → real), 8 seeds, one 25-B block. Eb/N0 here is
   the LEGACY harness convention `snr_db = Eb/N0 + 10log10(N_info/buffer_len)`,
   which xhw.5 says OVER-STATES true Eb/N0 by a clean ~6.7 dB:
```
            16dB  20dB
Good        0/8   5/8
Moderate    0/8   5/8
Poor        0/8   1/8
```
   (0/8 at ≤14 harness-dB.) Pre-vb9 this reached 8/8 only at ~26 harness-dB, so
   vb9 bought ~6 dB here too. So fading needs ~20-24 harness-dB (≈13-17 dB true
   after the −6.7 dB calibration) vs flat-AWGN ~6 dB — a ~7 dB fading margin.

## Questions (terse, gate-on-physics)

1. **Is a ~7 dB fading margin over flat AWGN physically expected** for a 2-tap
   Watterson (Good |Y| varies ~16× across band = ~12 dB selective fading; Poor
   worse) bridged by a rate-1/4 LDPC + the channel-aware demod? Or does it smell
   like residual loss (sync acquisition through the fade at low SNR, or the
   adaptive smoother under-smoothing in fading)? Sanity-check the magnitude.

2. **Honest Eb/N0 for the STATED gate.** The legacy harness over-states ~6.7 dB.
   For Gate A I want to state TRUE Eb/N0. Cleanest path: (i) calibrate by adding
   noise from measured signal power like the flat-AWGN harness (σ² = P_recv·L /
   (N_info·2·Eb/N0)) referenced to the unit-power Watterson output, OR (ii) keep
   the legacy harness but subtract a measured constant offset anchored to uncoded
   BPSK theory. Which is the honest, defensible choice, and how do I VALIDATE the
   calibration (uncoded BPSK through the SAME path matching Q(√(2Eb/N0)) within
   ~1 dB on a bare link)?

3. **Gate A shape.** Given Poor cliffs ~?? dB below Good/Moderate, state a single
   Eb/N0 with per-condition thresholds (≥15/16?), or assert "Poor within X dB of
   Good"? What stated Eb/N0 + thresholds are credible and non-flaky? (Gates are
   slow — ~280 s for 168 decodes — so they'll be `#[ignore]`d on-demand with a
   fast CI subset; advise the CI subset.)

4. **Gate B (coding-gain dB).** Confirm: AWGN-only waterfall, coded FER vs
   uncoded BER on a SHARED honest Eb/N0 axis, coding gain = horizontal dB shift
   at a reference. Expected magnitude for rate-1/4 LDPC n=2048 over BPSK? FER for
   coded + BER for uncoded on one axis — acceptable, or add a decode-anyway
   post-FEC BER path?

5. Anything physically wrong, or a cleaner way to satisfy the acceptance.

Read crates/sonde-phy/tests/step3_operating_point.rs (the harness),
crates/sonde-phy/tests/floor_awgn_coding_gain.rs (flat-AWGN, honest Eb/N0),
crates/sonde-phy/tests/noise_estimate_gate.rs (legacy harness Eb/N0). Give the
calibration choice, Gate A stated-Eb/N0 + thresholds, and Gate B method.
