# Converge: real synchronization in the loop — Schmidl-Cox CFO + timing + clock (sonde-xhw.3)

Context: clean-sheet HF OFDM data modem, Rust. AGPLv3. The "robustness floor"
mode is wide-band BPSK-per-subcarrier OFDM + rate-1/4 LDPC. Governing discipline:
GATE ON PHYSICS — a capability is not done until a measured end-to-end decode
through a realistic impaired channel proves it. Loopback/green-CI do not count.

## Fixed parameters (do not propose changing the waveform)
- SR = 48000 Hz. Wide mode: FFT N=2048, CP=512 (25%), so symbol stride = 2560.
- Occupied band: 99 contiguous subcarriers centered at 1500 Hz, bandwidth ~2300
  Hz. Subcarrier spacing Δf = 48000/2048 = **23.4 Hz**. Pilots every 4th occupied
  bin (~25 pilots). Data ~74 bins/symbol. BPSK per subcarrier.
- TX frame layout today: `[preamble (192-sample Re{ZC}, root 25)][coded OFDM symbols]`
  with raised-cosine inter-symbol windowing inside the CP (FFT body untouched,
  decode bit-identical on clean path).
- RX demod (per symbol): drop CP → FFT(2048) → pilot-aided channel estimate
  (NOT zero-forced; channel-aware soft LLR) → LDPC. The per-symbol pilot
  equalizer absorbs a CONSTANT phase and a per-subcarrier linear phase RAMP
  (= timing offset within the CP). It does NOT absorb a frequency shift.

## Measured baseline (committed)
Current production sync = a single REAL Zadoff-Chu correlator, no CFO/timing
recovery. Decodes at CFO 0/1/5 Hz; COLLAPSES at ≥20 Hz (AWGN 25 dB). Cause: a
100 Hz CFO ≈ 4.3 subcarrier spacings — it slides the spectrum off the pilot bins
and injects inter-carrier interference; the per-symbol equalizer cannot recover
a multi-bin shift. So CFO must be corrected in the time domain BEFORE the FFT.

## Real-HF impairments the modem must correct (the gate)
Simultaneously: carrier offset ±~100 Hz; sample-clock error ±some ppm;
fractional-sample frame timing; over a Watterson (Good/Moderate) fade at a
stated Eb/N0. THROUGH the production receive path (`receive_multi_with_sync`).

## Channelization convention (fixed across harnesses)
real audio → analytic (Hilbert via FFT: zero negative freqs, double positives)
→ complex impairment (CFO/Watterson) → AWGN (Gaussian) → Re{·}. At RX, to
derotate by a frequency offset on a REAL passband signal we form the analytic
signal of the captured audio, multiply by e^(−j2π·f·n/SR), take Re{·}.

## PROPOSED DESIGN (challenge every step)

### 1. Schmidl-Cox repeated-pair preamble (REPLACES the single ZC for sync)
TX a preamble of two identical halves: `[h | h]`, h = a length-H CAZAC (ZC)
segment, emitted as Re{·} of the complex sequence. Because Re{c[n+H]}=Re{c[n]}
when c repeats, the real passband preamble also has two identical halves.
- Keep the existing 192-sample single-ZC ALSO, or replace it? Proposal: the new
  sync preamble = repeated pair; total length 2H. I/Q magnitude matched filter
  (below) still does coarse frame acquisition on it (the pair is also a valid
  correlation template). Pick H for CFO unambiguous range ±SR/(2H).
- For ±100 Hz unambiguous with margin: H=160 → ±150 Hz (total 320 samples), or
  H=192 → ±125 Hz (total 384). **Q1: which H** — trade unambiguous range vs
  estimator variance vs airtime? Is ±125 (H=192) enough headroom over ±100, or
  go H=160 for ±150?

### 2. Coarse frame acquisition: complex (I/Q) magnitude matched filter
REUSE the parked sonde-64w.3 detector: correlate real RX against Re{ZC} AND
Im{ZC}, peak on √(c_re²+c_im²) — invariant to the channel's complex phase
rotation (a real-only correlator collapses at ≈90° rotation). Threshold 0.40
(measured noise floor ~0.375, faded-preamble peak ~0.44). That branch also
found SYNC_WINDOW_GUARD_SAMPLES should be ~32 (not 128 — 128 over-steepens the
phase ramp on accurately-detected frames). **Q2:** with a REPEATED-PAIR preamble
the magnitude MF template is now the pair; does the autocorrelation side-lobe of
[h|h] (a peak also at lag ±H) risk a half-preamble-off lock? Mitigation: use the
S&C plateau metric M(d)=|P(d)|²/R(d)² for coarse timing instead of / in addition
to the MF? Or MF on a SINGLE h then S&C-correlate the two halves for CFO only?

### 3. Coarse CFO estimate + derotation
At the detected preamble start, form analytic RX; compute
P = Σ_{i=0}^{H-1} conj(r[i])·r[i+H]; CFO = arg(P)·SR/(2π·H). Derotate the entire
analytic body by e^(−j2π·CFO·n/SR), take Re{·}, hand the derotated REAL samples
to the existing `receive_multi`. **Q3:** is taking Re{·} after derotation and
re-running the existing real→complex→FFT demod correct, or should I keep the
signal complex end-to-end through demod (the demod currently does
`Complex::new(s,0)` then full FFT, which re-creates the negative-frequency image
— harmless for a real signal but is it lossy after derotation)? I want minimal
change to the bit-identical clean path.

### 4. Fine CFO + timing + clock tracking via PILOTS (feedforward)
For OFDM I propose pilot-aided feedforward over time-domain Gardner:
- Per symbol s, after FFT, fit the pilot phases vs subcarrier index k to a line:
  slope → residual timing offset τ_s (samples); intercept → common phase φ_s.
- Track τ_s vs s: a linear trend in τ_s = sample-CLOCK error (ppm); the slope
  gives ε. Re-position each symbol's FFT window by the accumulated drift so it
  stays centered in the CP (feedforward timing + clock tracking, no resampler).
- Track φ_s vs s: residual common phase rotation per symbol = residual CFO after
  coarse correction; a linear φ_s trend → fine-CFO; correct as a per-symbol
  derotation (or fold into the equalizer, which already absorbs constant phase).
**Q4:** Is pilot-phase-slope feedforward timing the right OFDM tool, making the
existing time-domain Gardner (symbol_timing.rs) the WRONG instrument for the
OFDM floor (it's an early-late single-carrier detector; the pilots already give
me timing for free)? The task brief says "wire Gardner" but I'd rather wire the
physically-correct mechanism and keep Gardner for the FSK family. Confirm or
correct. **Q5:** For ±100 ppm over a ~10-symbol (~25600-sample, ~0.5 s) frame,
total drift ≈ 2.6 samples — well inside the 512 CP. So is explicit resampling
even needed, or does keeping the window in the CP (via Q4 tracking) + the
per-symbol equalizer's phase-ramp absorption suffice for the gate? I want to
avoid a fractional resampler if pilot-tracked windowing is sufficient at the
ppm/frame-length of the gate. State the regime where resampling becomes
mandatory.

### 5. The gate (replaces the scratch zz_ baseline test)
Extend the impairment harness: analytic lift → apply CFO (±100) → Watterson
(Good/Moderate) → resample by ±ppm (fractional, to inject clock error) → AWGN at
stated Eb/N0 → Re{·} → `receive_multi_with_sync`. Assert decode of the real
rate-1/4 LDPC payload. Report a decode-rate vs CFO curve over a seed sweep. Also
remove the hand-aligned `equalizer_seed_robust_good_sync_bypassed` bypass so the
fading tests decode THROUGH production sync. **Q6:** what Eb/N0 / SNR-in-2500-Hz
should the combined gate state to be a credible HF operating point (the fading
tests use 30 dB to isolate the equalizer; should the combined-impairment gate
hold at 30 dB, or is a lower, more honest HF SNR the right bar)? **Q7:** how to
inject a clean fractional ppm resample in the harness (band-limited sinc /
linear / FFT-domain phase ramp)? Which is faithful enough without contaminating
the measurement?

## Specific questions, terse answers please
Q1 H/preamble length. Q2 repeated-pair MF side-lobe / S&C plateau. Q3 derotate-
then-Re vs complex-through-demod. Q4 pilot-slope timing vs Gardner. Q5 is a
resampler needed at gate ppm/frame-length. Q6 gate Eb/N0. Q7 ppm-resample method
in harness. Plus: any step that is physically wrong, or any failure mode I've
missed (e.g. CFO–timing coupling, ICI residual after coarse CFO, pilot-phase
unwrapping across the band, half-preamble false lock). Be terse; confirm or
correct, and give the numbers (H, threshold, Eb/N0, ppm regime).

---

## CONVERGED (Codex review by agent towhee-slate-opossum, 2026-06-14)

- **Q1**: H=160, total preamble 320 samples; CFO range ±150 Hz. ZC root must be
  coprime with 160 → **root 29** (25 has gcd 5 with 160).
- **Q2**: full `[h|h]` I/Q-magnitude MF for acquisition (single-half → twin
  peaks → half-preamble false lock). Recalibrate the 0.40 threshold for the new
  template. S&C M(d) as optional refine/veto.
- **Q3**: derotate analytic → `Re{}` → existing real demod. Pad the Hilbert/FFT
  so edge transients don't reach the preamble.
- **Q4**: pilot-phase-**slope** feedforward timing (use *differential* pilot
  phase so channel group delay isn't read as timing), NOT Gardner. Gardner stays
  the FSK-family substrate; it is the wrong instrument for the OFDM floor.
- **Q5**: NO resampler for the gate (100 ppm × 25.6k samples = 2.6 samples drift
  ≪ 512 CP). Window-tracking + the per-symbol phase-ramp equalizer suffice.
  Resampler mandatory only for >~27 s frames or larger SRO → deferred (file bd).
- **Q6**: gate Eb/N0 — Good 14 dB, Moderate 18 dB (require Moderate). 30 dB is
  diagnostic only. SNR-in-2500 ≈ Eb/N0 − 8.6 dB.
- **Q7**: inject clock error via a deterministic band-limited windowed-sinc
  resampler on the complex analytic signal; AWGN AFTER resampling. (FFT phase
  ramp = fractional delay only, NOT clock error.)
- **Eb/N0 knob** (discrete-time AWGN convention, matches Step 0): the
  `add_noise(snr_db)` argument for a target Eb/N0 over a buffer of length L
  carrying N_info net info bits is `snr_db = ebn0_db + 10·log10(N_info / L)`.
- **Watch**: residual CFO after coarse correction must stay < 1–2 Hz (CPE fixes
  phase, not ICI); pilot-phase unwrapping across the sparse grid; Hilbert edges.
