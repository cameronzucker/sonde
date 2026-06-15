# Design — PHY-side quality reporting + multi-mode RX for link adaptation

**bd:** `sonde-99l` (under epic `sonde-lcw`) · **Status:** draft for Codex adversarial review
**Agent:** lupine-kestrel-knoll · **Date:** 2026-06-15
**Mates with:** `2026-06-15-symmetric-snr-adaptation-design.md` (link side, sonde-qnq) and
`2026-06-14-link-mode-adaptation-design.md` (P1/§6 background).
**Honesty:** gate on physics (real Eb/N0 + AWGN + BER-vs-theory). The link is already built and
correct over the channel model with *synthetic* SNR; this gives it the *real* signal. **RADIO-1 —
this is PHY code; nothing is keyed; no `sonde-tx`/rig/PTT path is run.**

---

## 0. The contract the link already consumes (do not rebuild)

The sans-IO link (`crates/sonde-link`) is complete. The receiver maps a channel **measurement** to a
target rung in both directions via `mac::adapt_rung(current, snr_raw, snr_smoothed, fer, fer_samples)`
and feeds it via `Connection::observe_quality(...)`. It reads the measurement from
`PhyTransport::channel_quality() -> ChannelQualityReport`. Integrity (CRC32 + ARQ) is independent of
adaptation; adaptation only tunes throughput. **This task supplies the real fields that
`ChannelQualityReport` exposes.** The exact link consumption (mac.rs, qnq branch):

```rust
// effective_snr_db / recommended_rung / adapt_rung all read these accessors:
q.aggregate_snr_db()      // f32 dB; NaN == "no measurement yet"
q.frame_error_rate()      // f32 in [0,1]
// adapt_rung is fed (snr_raw, snr_smoothed, fer, fer_samples); fer_samples comes from the report's
// recent-frame count. Downshift uses snr_raw; upshift uses snr_smoothed + 3 dB and a credible
// fer (>= FER_MIN_SAMPLES = 4, fer <= 0.05).
```

The link ladder (mac.rs `ladder()`), whose `snr_floor_db` values are **the placeholders this design
must replace with real per-mode FER-knees**:

| rung | family | `snr_floor_db` (placeholder) | meaning |
|---|---|---|---|
| 0 | 0 OFDM (wide) | **18.0** | fastest |
| 1 | 0 OFDM (mid)  | **8.0**  | DEFAULT_RUNG |
| 2 | 0 OFDM (narrow) | **0.0** | |
| 3 | 1 floor | **−12.0** | wideband low-density |
| 4 | 2 deep-floor | −∞ | base, always qualifies |

---

## 1. Current PHY reality (audited 2026-06-15 — the honest baseline)

1. **One real `Waveform` impl exists:** `FloorWaveform` (wraps `WidebandLowDensityFloor`),
   `crates/sonde-phy-runtime/src/waveform.rs`. The OFDM family is unwired building-blocks
   (`crates/sonde-phy/src/ofdm_main/*` — no `Waveform` impl). `NarrowFskFloor`
   (`crates/sonde-phy/src/robustness_floor/narrow_fsk.rs`, 8-FSK) is coded but **not** wrapped as a
   `Waveform`.
2. **`decode_scan(&self, samples) -> Option<DecodedFrame>` is mode-agnostic, self-syncing** (no mode
   hint). It already returns the detected `family`. → auto-detect is architecturally cheap.
3. **The runtime holds ONE generic waveform** `Worker<W: Waveform, R: Radio>`; `do_rx` calls
   `self.waveform.decode_scan(&samples)` once per window. No registry.
4. **`frame_snr_db` is hardcoded `None`** in `FloorWaveform::decode_scan` ⇒ `last_frame_snr_db` is
   always `None` ⇒ `aggregate_snr_db()` reports `NaN` ⇒ the link is permanently in its bootstrap
   "no measurement" branch. The adaptation loop has **no teeth today**.
5. **FER is lifetime-cumulative** (`QualitySnapshot{frames_total, frames_failed}` never reset) —
   violates the link's "recent, bounded, fade-reflecting" requirement.
6. **`per_subcarrier_snr_db` is always empty** (runtime passes `Vec::new()`).
7. **Pieces exist but are unwired:** `SubcarrierSnrEstimator` (pilot-aided per-bin Es/N0, on main,
   `crates/sonde-phy/src/subcarrier_snr.rs`); a per-symbol median-empty-bin N0 estimator on branch
   `sonde-gtg/n0-estimate` (`ofdm_main/receiver.rs`).
8. **Physics methodology is settled (Step 0, sonde-xhw.1):** the canonical reference for **gates** is
   **Eb/N0 (energy per LDPC info bit)**; `add_awgn` injects `σ² = E_signal/(2·K_info·10^(Eb/N0/10))`,
   `K_info = 480`, validated to BPSK theory `Q(√(2·Eb/N0))` within ~1 dB
   (`tests/step3_coded_fading_gate.rs`).

---

## 2. THE reference decision (deliverable 1 + 5; supersedes xhw.5's premise)

### 2.1 Two different jobs need two different references

- **Physics gates** answer "is the codec/PHY physically correct?" → **Eb/N0** is right: it normalizes
  out data rate so coded BER can be checked against theory. **Keep Eb/N0 for all gates. Unchanged.**
- **Link adaptation** answers "which mode should we run *now*?" → it needs a number that is
  **monotonic across the ladder at constant TX power**. Eb/N0 is the *wrong* reference here: it
  divides out the bitrate, so a robust low-rate mode and a fast high-rate mode have **nearly the same
  Eb/N0 knee** (that similarity is the *definition* of coding/processing gain). A ladder keyed on
  Eb/N0 floors would be non-monotonic and meaningless.

The quantity that *does* differ across modes at constant TX power, and that the ladder is built to
exploit, is **channel SNR in a fixed reference bandwidth** — the 2500 Hz SSB channel. A slower/narrower
mode spreads the same info over more time/less band ⇒ for the *same* SNR-in-2500-Hz it enjoys a higher
Eb/N0 ⇒ decodes where a fast mode cannot. That is precisely the ladder's purpose.

### 2.2 Decision

> **`ChannelQualityReport::aggregate_snr_db` reports channel SNR referenced to a 2500 Hz noise
> bandwidth (`SNR_2500`), in dB.** It is the average-signal-power-to-noise-power-in-2500-Hz ratio at
> the receiver input, estimated per received over. `per_subcarrier_snr_db` reports per-bin Es/N0 for
> the OFDM family (bonus, for bit-loading/margin).

Relationship to the gate reference (documented once, for cross-checking):
`SNR_2500 (dB) = Eb/N0 (dB) + 10·log10(R_info_bps / 2500)`, where `R_info_bps` is the mode's net info
bitrate. This makes the two references convertible and lets a gate test assert the reported `SNR_2500`
tracks an injected Eb/N0 (see §6).

### 2.3 Consequence for sonde-xhw.5 (the coupling the operator flagged)

xhw.5's task was "relabel runtime SNR reporting to the honest **Eb/N0** reference." Under this design
the runtime-reported field is **`SNR_2500`, not Eb/N0** — because the link needs a mode-comparable
channel number, not a per-bit one. So xhw.5 is **re-scoped, not discarded**: its real content (audit
the reporting fields; make them physically honest; add a test that reported SNR tracks injected noise)
is **absorbed into deliverable 1 here**. The label becomes `SNR_2500`. xhw.5 closes as "superseded by
sonde-99l's reporting work" once this lands. (Flag for the operator; do not unilaterally delete xhw.5
scope.)

### 2.4 Estimation method (floor family, buildable now)

Per received over, after sync + demod:
1. **Noise power N0_bin**: median of `|FFT bin|²` over the unoccupied bins (the sonde-gtg estimator),
   robust to spurs; scale to a per-Hz density.
2. **Signal power S**: sum of `|FFT bin|²` over occupied bins minus their noise contribution
   (`occupied_count · N0_bin`), i.e. the post-equalization signal energy.
3. **`SNR_2500 = 10·log10( S / (N0_per_Hz · 2500) )`**, clamped to a sane range
   (e.g. [−30, +40] dB) and `NaN` only when no over was decoded.

Cross-family comparability (deliverable 5): the *physical* `SNR_2500` is comparable across modes by
construction (fixed reference bandwidth). The *estimator* is mode-conditioned (OFDM uses empty-bin
N0; FSK uses inter-tone N0), so per-family estimator bias differs — which is exactly why the link
**resets its SNR EWMA on a cross-family change** (design F6). We therefore also **tag each report with
the family/mode it pertains to** (§5) so the link knows when the estimator domain changed, rather than
silently trusting a number from a different estimator.

---

## 3. THE architectural answer — multi-mode RX: AUTO-DETECT via a waveform registry (deliverable 4)

**Decision: auto-detect.** The RX pump runs **each registered waveform's `decode_scan` on the
window**; whichever self-syncs and decodes wins; the decoded frame's `MODE` byte is post-decode
*confirmation*, not discovery (matches the link's stated model: "discovery is the PHY's job").

**Why (justification):**
- `decode_scan` is *already* mode-agnostic self-syncing, so auto-detect is a registry iteration, not
  new DSP. Pre-tuning would require *adding* a mode-hint parameter and discarding the self-sync that
  already works — more work for a worse outcome.
- **Adaptation quality:** auto-detect makes a lost downshift announcement **never deafening** — the
  receiver decodes whatever family actually arrives and steps per-rung gracefully. Pre-tuned makes a
  lost announcement = deafness until the link's P1 BASE-fallback recovers it (correct but blunt:
  glitch → floor → climb). The operator's "it's not that hard" intuition is right *because*
  `decode_scan` self-syncs.

**The real cost (the part Codex must weigh):** running every waveform's full `decode_scan` (each with
its own correlator) on every ~0.25 s window is CPU-superlinear in the number of registered families.
Mitigation — a **two-stage pump**:
1. **Cheap per-family preamble pre-gate**: each `Waveform` exposes a lightweight
   `detect(&self, samples) -> Option<DetectHint>` (preamble correlation only, no full FEC decode).
   Run all families' `detect` (cheap); only run the **full** `decode_scan` for families whose preamble
   crossed threshold. With non-overlapping family preambles, ≤1 full decode per window in steady state.
2. Tie-break by strongest preamble correlation if >1 family pre-gates (rare).

This keeps auto-detect's graceful behavior at near pre-tuned cost. **The registry RX pump is built
now even with one registered waveform** (it iterates a 1-element registry) so the seam is ready when
families 2..N land — no runtime refactor later.

**API shape:**
```rust
// SondePhy::new takes a registry instead of a single waveform:
pub fn with_waveforms(waveforms: Vec<Box<dyn Waveform>>, radio: R) -> Self
// (keep new(waveform, radio) as a 1-element convenience constructor.)
// Waveform gains a cheap pre-gate (default impl can fall back to a trial decode_scan):
fn detect(&self, samples: &[f32]) -> Option<DetectHint> { /* default: probe */ }
```

---

## 4. Recent, bounded FER + sample count (deliverable 2)

Replace the lifetime counters with a **bounded ring of recent per-over decode outcomes** in
`QualitySnapshot`:
- Each over yields one of: `Decoded` (clean), `DetectedButFailed` (preamble synced, FEC failed →
  counts as an error), `NoSignal` (no preamble → counts as **nothing**, not an error — you cannot
  measure the SNR of an over you did not receive; the link handles total-loss via turn-recovery).
  This requires `decode_scan` (or the pump) to distinguish detected-but-failed from silence — the
  `DecodeScan{NoSignal, Detected, Frame}` enum from PR #37 is exactly this; wire it through.
- **Window:** last `N = 8` *received* overs (Decoded + DetectedButFailed; NoSignal does not consume a
  slot). 8 reflects "a fade within an over or two" without being a single-frame knife-edge, and gives
  the link ≥ `FER_MIN_SAMPLES = 4` credibility quickly. Documented on the field.
- `frame_error_rate()` = failed / received over the ring; `recent_frames_total()` = received count in
  the ring (the `fer_samples` the link's FER gate needs). **Reset semantics:** the PHY ring is purely
  recent/windowed; the link additionally resets its own FER stats on a rung change (F6) — the PHY does
  not need to know about rungs.

---

## 5. Measurement-domain signalling (deliverable 5)

Add to `ChannelQualityReport`:
- `mode: Option<ResolvedMode>` (or at least `family: Option<u8>`) — the mode/family the report
  pertains to. The link reads this to detect a cross-family estimator-domain change and reset its EWMA
  (F6), instead of inferring it. `aggregate_snr_db` is physically comparable across modes (§2.2); the
  tag guards against per-family *estimator* bias.
- `per_subcarrier_snr_db`: populate for the OFDM family via the existing `SubcarrierSnrEstimator`
  (bonus for bit-loading/margin). Empty for FSK (no subcarriers) — already the documented contract.

---

## 6. Real per-mode decode thresholds (deliverable 3) — honest scope

The link ladder needs each registered mode's **FER-knee expressed in `SNR_2500` dB**. Method: a
**FER-vs-SNR sweep** per mode over calibrated AWGN (extend `examples/ber_vs_snr_sweep.rs` to track
frame success/failure, not just BER), at the `SNR_2500` reference, reading the knee at FER ≈ 0.1.

**What is honestly available now vs deferred:**
- **Floor (wideband low-density), rung 3:** real data exists — coded flat-AWGN threshold ~6 dB Eb/N0
  (post-vb9, PR #42); convert to `SNR_2500` via its info bitrate and sweep to confirm the knee.
- **Deep-floor / nFSK, rung 4:** wrap `NarrowFskFloor` as a `Waveform`, sweep its knee. Buildable.
- **OFDM family, rungs 0–2:** **no honest FER-knee can exist until an OFDM `Waveform` impl exists**
  (it does not — §1.1). Supplying real 18/8/0 dB numbers now would be fabricated. → **rungs 0–2
  remain explicitly placeholder, flagged in the ladder, until the OFDM-waveform epic lands.** I will
  hand the link **real numbers for the floor families now** and a documented "OFDM knees pending
  sonde-<ofdm-waveform>" note. This is the gate-on-physics discipline: no fabricated thresholds.

---

## 7. Register >1 real waveform (deliverable 6) — honest scope

Today: 1 real waveform (`FloorWaveform`). Honestly reachable in this work:
- Wrap `NarrowFskFloor` as a second real `Waveform` (deep-floor family) → **2 real waveforms**,
  exercising the registry RX pump auto-detect path with genuine signals across two families
  (floor ↔ deep-floor).
- The **OFDM family (wide/mid/narrow) waveforms are a separate large epic** (wiring `ofdm_main/*`
  into a `Waveform` + its own physics gates). This design makes the runtime/registry/reporting
  **ready** for them; it does not pretend to deliver them. Flag for the operator.

---

## 8. Build plan (scoped; each item a child of sonde-99l / sonde-lcw)

1. **Reporting refactor (absorbs xhw.5):** `QualitySnapshot` → ring of recent outcomes; compute
   `SNR_2500` in the floor demod; populate `aggregate_snr_db`, windowed `frame_error_rate`,
   `recent_frames_total`; tag report with family; doc the reference once. + gate test (§ below).
2. **Registry RX pump (the sonde-99l answer):** `Waveform::detect` pre-gate; `SondePhy::with_waveforms`;
   two-stage `do_rx`; works with 1 waveform, ready for N.
3. **Second real waveform:** wrap `NarrowFskFloor`; register floor + deep-floor; auto-detect E2E test.
4. **Per-mode threshold sweep:** FER-vs-SNR harness; real floor/deep-floor knees in `SNR_2500`; hand
   to link; OFDM knees flagged pending.
5. **per_subcarrier_snr_db** via `SubcarrierSnrEstimator` for the OFDM family (lands with the OFDM
   waveform epic; estimator wiring stubbed/tested now).

## 9. Gates (physics; channel model; RADIO-1 — nothing keyed)

- **Reporting honesty (the key new gate):** over calibrated AWGN (reuse `add_awgn`), the reported
  `aggregate_snr_db` (`SNR_2500`) tracks the injected level within tolerance across a sweep, and the
  Eb/N0↔SNR_2500 relation (§2.2) holds — i.e. reported SNR is physically honest, not a loopback number.
- **Windowed FER:** inject a mid-stream burst of failures; `frame_error_rate()` rises then recovers
  within the window; `recent_frames_total()` is bounded; NoSignal overs do not inflate FER.
- **Auto-detect:** with floor + deep-floor registered, frames of either family decode without a mode
  hint; a family switch is followed without going deaf.
- **No regressions:** existing PHY physics gates (xhw.1–4, n0, vb9) + link G1–G9 stay green.

## 10bis. Codex-folded fixes (binding — these are the final rules)

Codex adversarial review (2026-06-15, gpt-5.5, xhigh) **confirmed the core direction** — `SNR_2500`
as the reported ladder reference (Q1 agree), auto-detect as the RX architecture (Q3 agree), honest
deferral of OFDM (Q7 agree) — and required these fixes, which are now the spec:

- **C1 (Q1) — pin the reference definition.** Bridge sign confirmed:
  `SNR_2500_dB = Eb/N0_dB + 10·log10(R_info_bps/2500)`, where `R_info_bps` = **net LDPC info bits/sec
  over the same energy/time interval used for `Eb`**. Document explicitly what the energy interval
  includes (preamble, CP, pilots, headers, padding) so the conversion is unambiguous. "Margin to rung
  knee" is *derived* from `SNR_2500`, never a replacement scalar.
- **C2 (Q2) — estimator: pre-equalization / pilot-derived, never post-EQ.** Do **NOT** use
  post-equalized signal power (EQ hides fades / amplifies noise → not receiver-input SNR). Use
  **raw/pre-equalized occupied-bin power**, or the pilot-derived channel-energy `Σ|H[k]|²·Es[k]`.
  Pin the **one-sided vs two-sided / real-audio-mirror 3 dB convention** explicitly or the report is
  off by 3 dB. `S − N` that goes negative at low SNR maps to a **conservative finite floor + a
  validity flag**, never an optimistic clamp. Average `SNR_2500` is honest under Watterson but
  *insufficient* (notches): carry per-subcarrier outage / fade-margin info for the OFDM family.
- **C3 (Q3) — `detect()` is high-recall, not an exclusive classifier.** Preferred shape: a **shared
  preamble acquisition** returns candidate timing/CFO; each plausible family then attempts a full
  decode **at that sync point**. Run decode for *every plausible* candidate, not just the strongest —
  a false negative = deafness (costly), a false positive = wasted CPU (cheap). The pre-gate trims, it
  does not exclude.
- **C4 (Q4) — report RAW per-over SNR + sample count + AGE; window is mode/time-aware.** The PHY
  reports the **raw per-over** SNR plus `recent_frames_total` **and a staleness/age signal**; it does
  **not** pre-smooth (the link owns the EWMA — double-smoothing slows downshift). A `NoSignal` over
  must **age/stale** the report so a clean pre-fade reading does not survive a fade. `N=8` is a
  deep-floor minutes-long window — make the FER window **time-bounded or mode-aware** and document the
  latency. **Partition/reset FER by resolved mode**, not just family (stale failures from the previous
  rung must not poison the next).
- **C5 (Q5) — tag `estimator_id/version`, and calibrate thresholds with the SAME estimator.** Family
  tag is insufficient; carry an `estimator_id`. The per-mode `snr_floor_db` knees MUST be measured
  with the *same* estimator that produces the runtime report ("estimator-domain knees"), so the link
  compares like to like. (Either that, or PHY normalizes the bias — calibration is simpler/honest.)
- **C6 (Q6) — expose BOTH references; `SNR_2500` primary, Eb/N0 retained.** Runtime/API exposes
  `snr_2500_db` (primary, link-consumed) **and** `ebn0_info_db` for the current resolved mode (or
  enough mode metadata to compute it). Eb/N0 stays as the audit/debug/gate reference — the easiest way
  to catch a future 3 dB / rate / bandwidth mistake. **xhw.5 is re-scoped to "report `SNR_2500`
  primary + keep Eb/N0 as audit," not a pure relabel.**
- **C7 (Q7) — placeholders may live in DOCS, never in ACTIVE adaptation code.** A ladder rung with no
  real waveform/profile/measured threshold must be **disabled / marked unavailable / built from the
  runtime registry of actually-available modes** — never selectable. Critical: today `DEFAULT_RUNG = 1`
  is an OFDM rung with a *fabricated* floor; the link must not default-select it. **The ladder should be
  constructed from the registry of registered, physics-gated modes.** (This implies a coordinated
  change on the LINK side — see §10ter; flag for the operator and the link agent.)

**Missed hazards Codex surfaced (now requirements):**

- **H1 — survivorship bias.** SNR computed only from *decoded* frames reads high. Compute SNR for
  `DetectedButFailed` overs too, from **preamble/body energy before FEC** (ties to C2/C4).
- **H2 — constant-TX-power is a hidden assumption.** Per-mode PAPR, soft-clip, occupied bandwidth, and
  radio ALC change actual radiated power → the `SNR_2500` cross-mode comparison is only exact if TX
  power is held. Document the assumption; the per-mode threshold calibration must use each mode's own
  TX chain so the knee absorbs its PAPR/clip penalty.
- **H3 — AWGN knees are insufficient for HF.** Calibrate per-mode thresholds over **Watterson G/M/Poor**
  too (not just AWGN), or add an explicit fade margin. Reuse `hf-channel-sim` `from_condition`.
- **H4 — name the field.** Rename/strongly-doc `aggregate_snr_db` → effectively `snr_2500_db` with
  ENBW + one-sided/two-sided conventions pinned on the accessor.

These fixes update §2, §3, §4, §5, §6, §8 above; the build plan (§8) is amended in §10ter.

## 10ter. Amended build plan + cross-team coordination

1. **Reporting refactor (absorbs + re-scopes xhw.5):** raw per-over `snr_2500_db` (pre-EQ/pilot-derived,
   3 dB convention pinned, finite floor + validity flag, computed for `DetectedButFailed` too) **plus**
   `ebn0_info_db` retained for audit; mode/`estimator_id` tag; mode/time-aware windowed FER with age;
   no PHY-side smoothing. + the reporting-honesty gate (§9) over AWGN **and** Watterson.
2. **Registry RX pump (sonde-99l answer):** shared preamble acquisition → high-recall per-family decode
   at the sync point; `SondePhy::with_waveforms`; 1 waveform works, ready for N.
3. **Second real waveform:** wrap `NarrowFskFloor`; auto-detect E2E across floor ↔ deep-floor.
4. **Estimator-domain per-mode threshold sweep:** FER-vs-`SNR_2500` over AWGN + Watterson, measured
   with the **runtime estimator**, each mode's **own TX chain** (H2); real floor/deep-floor knees;
   OFDM knees flagged pending.
5. **Ladder-from-registry (C7 — LINK-side, coordinate):** the link ladder must be built from
   registered/gated modes so fabricated OFDM rungs are never selectable and `DEFAULT_RUNG` is a real
   mode. This touches `crates/sonde-link` (owned by the link agent). **Coordination item — surface to
   the operator; do not unilaterally edit the link crate.**

## 10. Open questions that drove the Codex review (resolved in §10bis)

- **Q1 — reference choice.** Is `SNR_2500` (channel SNR in 2500 Hz) the right *reported* reference for
  the ladder, vs Es/N0 or a per-mode "implied margin"? Does it correctly make the ladder monotonic?
  Is the Eb/N0↔SNR_2500 conversion (§2.2) the right bridge to the gate methodology?
- **Q2 — estimator validity.** Is median-empty-bin N0 + occupied-bin signal power a sound `SNR_2500`
  estimator for the floor waveform? Bias under fading (Watterson) vs AWGN? Does the curvature term
  from sonde-gtg matter for the *reported* number or only for decode?
- **Q3 — auto-detect cost model.** Is the cheap `detect` pre-gate sound, or do overlapping/weak
  preambles make it unreliable enough that full `decode_scan`-per-family is unavoidable? CPU budget at
  N families on a Pi-class host?
- **Q4 — FER window length.** Is N=8 received-overs right? Interaction with the link's own EWMA and its
  per-rung FER reset — any double-counting or blind spot?
- **Q5 — cross-family comparability.** Is "physically comparable, estimator-biased, tag-the-family"
  the right resolution of design F6, or should the PHY itself normalize estimator bias across families?
- **Q6 — xhw.5 re-scope.** Is folding xhw.5 into deliverable 1 (relabel to `SNR_2500`, not Eb/N0) the
  correct call, or should the runtime expose *both* references?
- **Q7 — honest deferral.** Is shipping real floor/deep-floor thresholds + registry-ready-for-OFDM,
  with OFDM knees explicitly placeholder, the right honesty boundary — or does partial delivery risk
  the link baking in fabricated OFDM numbers?
