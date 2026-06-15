# Handoff — PHY is now multi-mode end-to-end (2 real gated families)

**From:** lupine-kestrel-knoll · **Date:** 2026-06-15
**Durable records:** PRs #49/#50/#52/#55 (all merged), `bd`, the design docs under
`docs/superpowers/specs/2026-06-15-phy-mode-adaptation-quality-design.md`.

## The arc this session
Started on sonde-xhw.5 → operator pivoted to the PHY side of link adaptation
(sonde-99l) → then "build & ship to completion." Codex audit confirmed the PHY was
NOT end-to-end (1 real waveform, OFDM unreachable, tx/rx parallel stack). Closed the
load-bearing gaps:

| PR | What landed |
|---|---|
| #49 | Honest `SNR_2500` reporting + windowed/aged FER + **auto-detect waveform registry** RX pump (sonde-99l.1/.2) |
| #50 | Session handoff + bd export |
| #52 | **TX-fallback fix** — do_tx drops an over with no waveform for its family instead of keying the wrong family (sonde-99l.5) |
| #55 | **First REAL OFDM main mode** — `ofdm-wide` (Wide/QPSK/N1296-R1/2) via a generalized floor engine (`with_params_constellation_fec`), physics-gated over AWGN; **multi-mode auto-detect E2E** with the floor (sonde-c7i) |

## State now: genuinely multi-mode end-to-end
Two real, physics-gated waveform families auto-detect through `PhyTransport`:
- **floor** — Wide BPSK rate-1/4 (decodes ~SNR_2500 ≥ ~0–16 dB)
- **ofdm-wide** — Wide QPSK N1296 R1/2 (FER 0 @ SNR_2500=22 dB; reported 20.8 dB, honest)

`bits_per_sc=1` keeps the gated floor bit-for-bit; the step-3 fading gate stays green.
The link (already on main) consumes the real `SNR_2500` + windowed FER + sample count.

## Remaining roadmap (filed, recipe is established)
- **sonde-99l.6** — OFDM ladder completion: ofdm-mid/ofdm-narrow + higher
  constellations (same `with_params_constellation_fec` recipe), each its own
  FER-vs-SNR_2500 gate. **Blocker:** runtime routes by `ModeFamily` only — a
  multi-rung OFDM ladder needs per-mode identity in do_tx selection + RX labeling.
- **sonde-cyo** — unify sonde-tx/sonde-rx (real-radio CLIs) behind `PhyTransport`,
  or ship a `SondePhy` runtime binary over `SoundcardRadio`. The last big
  end-to-end gap (real-radio path currently bypasses the runtime). RADIO-1 applies.
- **sonde-99l.4** — nFSK deep-floor (needs preamble/self-sync first).
- **sonde-99l.1 cosmetic** — per_subcarrier_snr_db population, ebn0_info_db audit
  field, family/estimator tag (not consumed by the link yet).
- **sonde-lcw.1 (LINK agent owns, in progress)** — ladder-from-registry so OFDM
  rungs are backed by real gated modes; now has 2 real modes to anchor.

## Gates / hygiene
`cargo build/test/clippy(-D warnings)/fmt` green; CI green on every PR.
**RADIO-1: all PHY code; nothing keyed this session.**
Stale local branches to `git branch -d` from a lease-free session: any
`sonde-99l*`, `sonde-c7i*`, `sonde-xhw.4*` merged refs.
