# Handoff — demo sniff-test → modem reality audit → ARDOP pivot

- **Agent:** cardinal-magnolia-fern
- **Date:** 2026-06-14
- **Arc:** started by resuming the Sonde interactive demo (sonde-669); a chain of
  operator sniff-tests exposed that the modem is not physically valid, which
  pivoted the work into an audit, a course-correction direction doc, and a
  re-anchoring of the demo on ARDOP.

## What shipped (all merged to `main`)
1. **PR #4** (early): resolved the demo↔coded-floor conflict, re-pointed the
   `sonde-wasm` engine onto the FEC-coded floor; Pages demo went live.
2. **PR #22**: GitHub Pages deploy workflow (`pages.yml`) + `build-wasm-bundle.sh`;
   Pages enabled; site live at https://cameronzucker.github.io/sonde/.
3. **PR #26 + #27**: `docs/2026-06-14-modem-reality-audit-and-direction.md` — the
   capability audit (real/partial/faked per subsystem) + physics-gated direction
   (P0 methodology → P1 transmittable waveform → P2 sync → P3 coded-mode-over-
   fading → **P3b FT8-class deep-robustness floor** → P4 link layer → P5 on-air).
   **Owner decision recorded: build a REAL interoperable HF modem.** Maps to epics
   `sonde-64w` (PHY/sync/waveform/methodology) and `sonde-lcw` (link layer).
4. **PR #30**: demo frontend fidelity — legible drag-to-inspect waterfall, Web
   Audio playback (`link_audio` wasm export + `audio.js`), recon render-at-rest.
5. **PR #31**: **ARDOP feasibility spike** — `hf-channel-sim/examples/wav_channel.rs`
   (WAV↔channel bridge), `demo/ardop/spike_roundtrip.py`, `demo/ardop/SPIKE.md`.
   Proven: real ardopcf frame round-trips through our sim via WAV; clean
   decode-vs-SNR curve (RS load ramps, cliff −6→−9 dB in the sim's ad-hoc units).

## The pivot (current direction)
The demo's job is now an **internal sniff test**, not a public demo (the operator
can't fact-check DSP, so faithfully-surfaced artifacts are the defense against
plausible-but-wrong AI work). **Remove Sonde from the demo, re-anchor on real
ARDOP run live** (external reference only — never reimplemented, ADR 0014 intact),
**audit the demo against that known-good mode, then add Sonde as a comparison.**
Epic **`sonde-imh`**.

## Key audit findings (for context)
- 🔴 TX waveform non-transmittable (IFFT→CP→`Re{}`, no shaping/filter, Hermitian
  mirror). 🔴 sync mostly dead/test-only. 🔴 SNR/BER/throughput methodology not
  physically meaningful. ⚫ no link layer; never on-air.
- 🟢 REAL building blocks: rate-1/4 LDPC w/ true SPA decoder, pilot equalizer w/
  channel-aware LLRs, standards-pinned Watterson, real PTT. **The team can do real
  DSP** — the gap is integration + physical realism + honest validation.

## State at handoff
- All session branches merged; all my worktrees disposed except the active one.
- **Active worktree:** `worktrees/sonde-imh.1-ardop-live-backend` (this branch) —
  building the ARDOP live demo backend (in progress this session).
- **External, NOT in repo:** `ardopcf` built at `~/Code/ardopcf-spike`
  (github.com/Dinsmoor/ardopcf, reference-only). Needed to run the round-trip.
- Local `main` checkout is behind origin (other live sessions); untouched by me.

## Open work / tracking
- **`sonde-imh.1`** (this worktree): ARDOP live demo backend on the proven
  file-based round-trip + frontend re-anchor; then add Sonde as comparison;
  calibrate the SNR axis vs ARDOP's documented sensitivity (feeds P0).
- **`sonde-669.9`**: demo front-end polish backlog (recognizable recon image,
  waterfall freq/time axis labels, corrupted-regime rendering).
- **`sonde-imh`** (epic), **`sonde-64w`/`sonde-lcw`** (modem direction — owned by
  the parallel modem/link-layer agents; direction doc handed to them).
- **UNOWNED, flag:** P1 transmittable-waveform fix (passband modulation + shaping
  + filtering) — most foundational, no worktree seen for it.

## Pending decisions
- ARDOP live backend language: Python (reuse the proven spike round-trip, fastest)
  vs Rust (matches the workspace). Leaning Python for the demo backend (infra, not
  the modem); revisit if it should be Rust.
- Whether to keep the interim Sonde Pages page live during the ARDOP rebuild
  (operator said "don't care about unpublish").

## Safety
ardopcf is run only against WAV files / virtual audio — never a real sound card or
PTT, no RF. (RADIO-1 governs Sonde's own `sonde-tx`/rig path, not external ARDOP,
but the same no-accidental-TX discipline applies.)
