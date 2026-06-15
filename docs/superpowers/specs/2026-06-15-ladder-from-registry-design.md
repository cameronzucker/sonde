# Design — link ladder from the registered-mode registry (C7 realization)

**bd:** `sonde-lcw.1` (P2) · **Status:** draft for Codex review · **Agent:** lupine-yew-harrier
**Realizes:** C7 + §10ter.5 of `2026-06-15-phy-mode-adaptation-quality-design.md` (sonde-99l).
**Honesty:** link-correct over the channel model; RADIO-1 — nothing keyed.

## 1. The reality this must encode (audited on `main`, post-#49)

- The PHY registry holds **one** real `Waveform`: `FloorWaveform` (family `RobustnessFloor`,
  wideband low-density floor = link **rung 3**, `ModeHint::Floor`). `SondePhy::with_waveforms`
  exists but is constructed with `[FloorWaveform]`.
- **No OFDM `Waveform`** (rungs 0–2, `ModeHint::MainAuto`) — building blocks only (sonde-c7i).
- **nFSK deep-floor not wrapped** as a `Waveform` (rung 4, `ModeHint::FloorCrowdedBand`).
- So the only **registered, physics-gated, selectable** mode today is **rung 3**. The current ladder
  default-selects **rung 1 (OFDM)** with a **fabricated** `snr_floor_db` (18/8/0) and **no waveform** —
  the link is configured to use a mode the PHY cannot produce. C7 forbids exactly this.
- Real floor knee (estimator-domain, measured by the runtime estimator): `floor_threshold_sweep.rs`
  reports **SNR_2500 ≈ 16 dB** at the decode knee. `channel_quality().snr_2500_db()` is the link's
  primary input.

## 2. Decisions

### 2.1 Availability is a per-rung property, mirroring the PHY registry (static, documented)

Each ladder `Rung` gains `available: bool` + an **estimator-domain** `snr_floor_db`. A rung is
`available` IFF a real, registered, physics-gated `Waveform` + measured knee exists for it. Today:

| rung | mode | family | available | snr_floor_db (SNR_2500) |
|---|---|---|---|---|
| 0–2 | OFDM | OFDM | **false** | placeholder (pending sonde-c7i) |
| 3 | wideband floor | floor | **true** | **16.0** (floor_threshold_sweep.rs) |
| 4 | deep-floor nFSK | deep-floor | **false** | placeholder (pending nFSK `Waveform` wrap) |

**Why static, not a runtime query (flag for operator):** `PhyTransport` exposes no registry
enumeration and the sans-IO `Connection` cannot read runtime state; the link mirrors the PHY's
registered+gated set statically. The single source of truth would be a PHY-exposed registry the link
queries, but that is a cross-crate API the PHY task owns — out of this link-scoped change. The static
table is the honest, correct realization now; updating it when modes land is a one-line change.
**Rung ids stay stable (wire `MODE` byte contract)** — availability gates *selection*, not the id space.

### 2.2 Selection only ever returns an available rung

- `default_rung()` / `base_rung()` replace the `DEFAULT_RUNG`/`BASE_RUNG` consts → the **most-robust
  available** rung (today 3). Guarantees the handshake + the P1 fallback target are real modes.
- `adapt_rung` / `recommended_rung` / `route` skip unavailable rungs in every search (downshift
  candidate, upshift candidate, family-step cap).
- `apply_rung`/`follow_mode` **clamp** any target to the nearest **available** rung (toward more
  robust) — defends against a peer advertising a rung this build doesn't have (version skew).

### 2.3 Preserve the multi-rung ALGORITHM coverage with a synthetic ladder

The adaptation algorithm (raw-SNR downshift, smoothed-SNR+margin upshift, FER gate, family cap) is
rich but, with one available rung today, untestable over the real ladder. So the algorithm is
parameterized over a ladder slice (`adapt_rung_with(rungs, …)`); the public `adapt_rung` uses the
real (floor-only) ladder, and unit tests exercise the algorithm with a **synthetic all-available**
ladder. Production behavior (hold the one available rung) is tested separately. Coverage survives for
when OFDM/nFSK land.

### 2.4 Consume `snr_2500_db`

`Driver`/`Link` feed `observe_quality` from `q.snr_2500_db()` (the named primary), not the
`aggregate_snr_db()` alias.

## 3. Honest consequence (state plainly)

With one registered mode, the adaptation loop is **inert** (nothing to adapt *to*) — but now for the
right reason: the link holds the single real mode instead of default-selecting a fabricated one. The
machinery is correct and exercised by the synthetic-ladder algorithm tests; it gains range the moment
nFSK (deep-floor) and the OFDM family register real, gated waveforms. This is the gate-on-physics
discipline: no fabricated mode is ever selectable.

## 4. Gates (channel model; RADIO-1 — nothing keyed)

- mac unit tests: the real ladder reports only rung 3 available; `default_rung()==base_rung()==3`;
  OFDM/deep-floor rungs are never returned by `adapt_rung`/`recommended_rung`/`route`;
  `adapt_rung_with(synthetic_all_available)` still proves the down/up/margin/FER/family algorithm.
- conn/driver: the link holds the single available rung under any feedback (cannot select a
  fabricated mode), and delivery is byte-exact; `observe_quality` consumes `snr_2500_db`.
- No regression: existing link gates G1–G9 + the workspace green.
