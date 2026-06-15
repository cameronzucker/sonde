# Handoff — PHY mode-adaptation quality (sonde-99l) shipped; physics-gate epic closed

**From:** lupine-kestrel-knoll · **Date:** 2026-06-15
**State:** PR #49 **MERGED to main**, CI green. Durable records: **PR #49**, `bd`,
the committed design doc
`docs/superpowers/specs/2026-06-15-phy-mode-adaptation-quality-design.md`, and this
doc (committed via a session-closure PR off the same epic).

## What happened this session

Started on sonde-xhw.5 (relabel runtime SNR), but the operator pivoted mid-task to
the **PHY side of link mode-adaptation (sonde-99l / sonde-lcw)** because unexpected
link-layer changes needed coordinated PHY changes. Did that first; it re-shaped
xhw.5.

**Design-first (project discipline):** wrote a PHY-side quality/multi-mode-RX
design, **adversarially Codex-converged** (`codex exec --sandbox read-only`,
verdicts folded as binding §10bis). Then built the buildable-now core (operator
approved "full buildable-now core" + "file the link-side fix, hand me a prompt").

### Landed in PR #49 (merged)
- **sonde-99l.1 (CLOSED)** — honest `SNR_2500` channel-quality reporting + windowed
  FER. `OfdmReceiver::estimate_snr_2500_db` (pre-EQ/pilot-derived, estimator-domain,
  computed on failed decodes too, finite-floor on deep fades). Runtime
  `QualitySnapshot` → bounded recent-over ring (windowed FER + raw per-over SNR +
  staleness aging). `ChannelQualityReport::snr_2500_db()` + `recent_frames_total()`.
  **Gate:** reported SNR tracks injected slope≈1 (AWGN) + rises under Watterson
  (`crates/sonde-phy/tests/phy_quality_reporting_gate.rs`).
- **sonde-99l.2 (CLOSED)** — multi-mode **auto-detect** RX pump: `Worker` holds a
  `Vec<Box<dyn Waveform>>` registry; `SondePhy::with_waveforms`; `Waveform::detect()`
  high-recall pre-gate; TX picks by family. **Gate:**
  `crates/sonde-phy-runtime/tests/registry_autodetect.rs`. *This is the literal
  sonde-99l architectural answer: auto-detect, not pre-tune.*
- **sonde-99l.3(a)** — floor FER-vs-`SNR_2500` threshold sweep (real estimator-domain
  knee: floor decodes @16 dB, fails @0 dB). `crates/sonde-phy/tests/floor_threshold_sweep.rs`.

### Also closed
- **sonde-xhw.5 (CLOSED, superseded)** by 99l.1 — reporting is now `SNR_2500` (not
  Eb/N0). Eb/N0 retained as the GATE reference (Codex C6).
- **sonde-xhw EPIC (CLOSED)** — all 5 children done (original session task item #2).

## Remaining work (filed in bd)

- **sonde-99l.3(b) / sonde-99l.4** — wrap `NarrowFskFloor` as a 2nd real waveform is
  **BLOCKED**: nFSK has no preamble/self-sync, so it can't be a registry `Waveform`
  without adding sync first (a real DSP task + its own physics gate). Registry is
  already proven multi-waveform-ready.
- **sonde-99l.1 follow-ups** (noted in commit): `ebn0_info_db` audit field (C6),
  report family/`estimator_id` tag (C5), `per_subcarrier_snr_db` population (deliv 5).
- **sonde-c7i** — OFDM main-family `Waveform` impl + physics gates (unblocks real
  OFDM ladder rungs 0–2). Large, separate. Registry is ready for it.
- **sonde-lcw.1 (LINK — other agent owns)** — build the ladder from the registry of
  real gated modes so a fabricated OFDM rung is never selectable (C7); consume
  `snr_2500_db`. Paste-ready link-agent prompt is in the operator's session summary.

## Branch / tree / worktree state
- `main`: PR #49 merged (CI green). Main also gained the **sonde-link crate** this
  session (link agent's work landed); the merge converged the
  `recent_frames_total()` accessor cleanly.
- Worktrees disposed (ritual): `sonde-99l-phy-quality-adapt`, `sonde-xhw.5-xhw5-snr-labels`,
  `sonde-xhw.4-coded-fading-validation` (orphan). Merged local branches
  `sonde-99l/phy-quality-adapt` + `sonde-xhw.5/xhw5-snr-labels` deleted; `git worktree
  prune` done.
- Stale local branches still present (cleanup, blocked by lease at close):
  `sonde-xhw.4/coded-fading-validation`, `sonde-xhw.4/step3-gate` — both merged via
  PR #47; safe to `git branch -d` from a lease-holding session.

## Gates
`cargo build/test/clippy(-D warnings)/fmt` green across the workspace pre-merge;
CI green on PR #49. **RADIO-1: PHY code only; nothing was keyed.**
