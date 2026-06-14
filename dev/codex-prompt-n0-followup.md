# Re-converge: n0 estimate broke high-SNR fading — add channel-estimate-error term (sonde-gtg)

Following your earlier convergence (empty-bin median/ln2 per-symbol n0), I built
it and MEASURED a problem you flagged in Q5 (channel-estimate error). Need to fix
the design.

## What I built
`n0_thermal = median(|freq[k]|²)/ln2` over empty bins (guard 8), per symbol, fed
to the channel-aware LLR `metric(c) = −|y − h·c|²/n0`, with the soft-LLR clamp
now scaled as `2.0/n0` (was fixed ±20 at the old n0=0.1, to preserve the
nulled-vs-strong reliability range).

## Measured failure
The pre-existing Watterson Good/Moderate fading e2e gate (rate-1/4 LDPC, decode
through the production path) at `add_noise = 30 dB` (very high SNR) now FAILS with
the estimate, but PASSES with a fixed `n0 = 0.1` control. Measured `n0_thermal`
on the first body symbol, Good fade:
- add_noise 30 dB → n0_thermal ≈ 5e-5   (fixed legacy = 0.1, i.e. 2000× larger)
- add_noise 20 dB → n0_thermal ≈ 3.4e-4
- add_noise 10 dB → n0_thermal ≈ 2.9e-3
The differential gate uses Eb/N0 (snr_db = Eb/N0 + 10log10(Ninfo/Lbuf), offset
≈ −25.7 dB), so Eb/N0 20 dB ≈ add_noise −5.7 dB → n0_thermal ≈ 0.1.

## My diagnosis (confirm/correct)
The effective noise on the LLR residual `y − h_est·c` is `n0_thermal + var(e)`,
e = channel-estimate error. With pilots every 4 bins + linear interpolation, near
a deep Watterson null (|H| varies 16–27× across the band) the interpolation has a
large DETERMINISTIC curvature bias that does NOT vanish at high SNR. The legacy
fixed 0.1 was effectively that channel-estimate-error floor. My thermal-only n0
is correct at LOW Eb/N0 (thermal dominates, exceeds 0.1 → that's the intended
win — fixed 0.1 under-estimates there and fails) but UNDER-estimates at high SNR
(ignores var(e)) → over-confident LLRs at nulls → decode breaks.

## Proposed fix
`n0_eff = n0_thermal + var(e)`, with `var(e)` estimated from a LEAVE-ONE-OUT
pilot interpolation residual: for each interior pilot p, predict H[p] by linear
interp from its two neighbour pilots, and measure `|H_obs[p] − H_pred[p]|²`;
`var(e) ≈ mean (or median) of that over pilots`, scaled for the fact that data
bins sit at ≤ half the pilot spacing (so their interp error is a fraction of the
pilot-to-pilot residual). This is self-calibrating: small in flat regions, large
near nulls — exactly where the near-erasure must kick in.

## Questions (terse)
Q1: Is `n0_eff = n0_thermal + var(e)` right, or should it be per-BIN
(`n0_eff[k] = n0_thermal + var(e)[k]` with var(e) interpolated between pilots),
so a bin near a null gets a larger effective noise than a bin in a flat region?
Per-bin is more correct but more code — worth it, or is a single per-symbol
scalar var(e) enough to recover high-SNR fading + keep the low-SNR win?
Q2: The leave-one-out pilot residual measures error at the PILOT spacing (4
bins); data bins are 1–3 bins from a pilot. What scale factor maps pilot-spacing
interp-residual → data-bin var(e)? For linear interp the error grows ~quadratically
with distance from the nearest sample; give a concrete factor (or say "use the
residual as-is, it's the right order").
Q3: mean vs median for var(e) over pilots (nulls are sparse outliers — median
under-weights them, but they're exactly where it matters)?
Q4: Does scaling the LLR clamp as 2.0/n0_eff still make sense, or does the var(e)
term change the clamp logic?
Q5: Anything simpler that's still physically honest — e.g. a fixed var(e) floor
(the measured ~0.1) added to thermal? I'd rather estimate it, but if the
leave-one-out residual is fragile at the gate SNRs, a measured constant floor
might be the defensible MVP. Your call on estimate-vs-floor.
Q6: Gate unchanged? (estimated n0_eff decodes ≥6/8 at a low Eb/N0 where fixed-0.1
gets ≤ est−3, AND high-SNR fading e2e still passes.) Any Eb/N0 you'd target to
show the clearest cliff-shift without flakiness?

Be terse; confirm/correct and give numbers (per-bin vs scalar, scale factor,
mean vs median, clamp).

---

## CONVERGED (Codex round 2, agent gorge-isthmus-fern, 2026-06-14)

Per-bin effective noise (RX already loops per subcarrier, so per-bin is worth it):
```
r_i      = Hobs[p_i] - 0.5*(Hobs[p_{i-1}] + Hobs[p_{i+1}])   # LOO pilot residual (complex)
q_i      = max(|r_i|^2 - 1.5*n0_thermal, 0)                  # de-noised curvature power
u        = (k - p_left)/(p_right - p_left)                   # data-bin position (0.25/0.5/0.75 @ D=4)
q_local  = max(q_left, q_right)                              # adjacent-max
var_e[k] = n0_thermal*((1-u)^2 + u^2) + (u*(1-u))^2 * q_local
n0_eff[k]= n0_thermal + var_e[k]
```
- n0_thermal: empty-bin MEDIAN/ln2 (unchanged). var(e): pilot-noise propagation
  `((1-u)^2+u^2)*n0_thermal` PLUS curvature `(u(1-u))^2 * q_local`.
- LLR clamp = `2.0 / n0_eff[k]` (per-bin; for the fixed-0.1 control arm this is
  2.0/0.1 = 20, reproducing the legacy ±20 exactly).
- Curvature scale factors (LOO@spacing-8 → data-bin): u=0.25/0.75 → 9/256≈0.035;
  u=0.5 → 1/16=0.0625; the (u(1-u))^2 kernel encodes this.
- Gate: keep high-SNR Watterson Good/Moderate e2e @ add_noise=30 dB; differential
  low-SNR gate target Eb/N0=20 dB first (22 if too conservative); don't require
  18 dB until the ignored sweep proves it stable.
