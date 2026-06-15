# Step 3 (sonde-xhw.4): coded mode validated over realistic fading

**Agent:** bison-slate-gorge · **Date:** 2026-06-15 · **Status:** SHIPPED.

Composes the whole receive chain — real Schmidl-Cox sync (xhw.3) + per-bin n0
estimate (sonde-gtg) + adaptive pilot time-smoothing (sonde-vb9) + rate-1/4 LDPC
(sonde-fec) — and validates it gate-on-physics with an **honest** stated Eb/N0.
Test: `crates/sonde-phy/tests/step3_coded_fading_gate.rs`.

## Honest Eb/N0 (also resolves the sonde-xhw.5 ~6.7 dB over-statement)

Eb/N0 is energy per LDPC info bit, injected from MEASURED signal power:
`σ² = E_signal / (2·K_info·10^(Eb/N0_dB/10))`, `K_info = n_blocks·block_info_bits = 480`.
Codex pinpointed the legacy ~6.7 dB inflation: the old harness used
`K_info = payload_bits (200)` and complex AWGN (missing the real-cast factor of
2) → `10·log10(2·480/200) = 6.8 dB`. The gate's `eb_n0_calibration_matches_bpsk_theory`
test **validates the scale**: bare BPSK over this injection tracks `Q(√(2Eb/N0))`
within a factor of 2 (~1 dB) at 2/4/6/8 dB. (Watterson curves are NOT expected to
match the AWGN Q-law — fading changes the BER law.)

## Gate A — success-rate over Watterson Good/Moderate/Poor

Through the production sync path (`receive_multi_with_sync`), 16 seeds, true Eb/N0:

```
            14dB   16dB   18dB   20dB
Good        12/16  16/16  16/16  16/16
Moderate    13/16  14/16  15/16  16/16
Poor        14/16  15/16  16/16  16/16
```

**Gate asserts at the stated true Eb/N0 = 20 dB: Good/Moderate/Poor all 16/16**
(Poor allowed ≥14/16 as it cliffs last). Pre-vb9 this needed ~26 harness-dB
(≈19 true); the vb9 pilot-smoothing fix bought ~6 dB here too.

## Gate B — LDPC coding gain over AWGN (codec level)

Coded post-FEC BER (decode-anyway via `FloorRate14Codec::decode_soft_payload_unchecked`)
vs uncoded BPSK BER, textbook LLRs, shared honest Eb/N0 axis:

```
 Eb/N0 | coded post-BER | uncoded BER
   1.0 |    8.68e-3     |   5.61e-2
   1.5 |    1.04e-4     |   4.56e-2
   2.0 |    0           |   3.96e-2
   ...
   7.0 |    0           |   9.03e-4
```

**Coding gain @ BER=1e-3 = 5.6 dB** (coded waterfall ~1.2 dB, uncoded ~6.9 dB) —
in the expected 5–7 dB band for a rate-1/4 n=2048 LDPC through the PHY. Gate
asserts ≥ 4 dB. (The "coding gain in dB" is an AWGN-waterfall concept; over a
fade the gain is unbounded at a null — Gate A is the realistic-channel story.)

## Why Gate A's fading margin (~7 dB over flat AWGN) is honest, not a defect

Flat-AWGN coded threshold is ~6 dB (vb9); fading needs ~13 true-dB for the cliff
and ~16–20 for ≥15/16. Codex confirmed a ~7 dB margin is physical for a 2-tap
Watterson with deep frequency-selective nulls bridged by the rate-1/4 code: the
loss is selective-fade erasure + pilot-interpolation uncertainty, not ISI (CP
covers the delay).

## CI vs on-demand

- **CI (fast, not ignored):** `eb_n0_calibration_matches_bpsk_theory`,
  `gate_a_smoke` (3 seeds/condition @20 dB), `gate_b_smoke` (coding gain exists
  @4 dB).
- **On-demand (`#[ignore]`, ~minutes):** `gate_a_fading_fer_sweep` (the full
  16-seed FER table + assertion), `gate_b_coding_gain_sweep` (the dB curve).

## Follow-ups
- `sonde-xhw.5`: the Eb/N0 *measurement* scale is now honest + validated here.
  The remaining xhw.5 scope is relabeling the runtime REPORTING fields
  (`ChannelQualityReport.frame_snr_db` etc.) to the same reference — separate,
  P2.
- Long-frame clock resampler still deferred (sonde-wfa, P3); not needed for
  Good/Moderate/Poor (per-symbol equalizer tracks ≤1 Hz Doppler).
