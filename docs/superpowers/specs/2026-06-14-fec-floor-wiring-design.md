# FEC-first slice — working rate-1/4 floor LDPC, wired end-to-end

> **Status: Approved design (brainstorming output).** bd: `sonde-64w.1`
> (under epic `sonde-64w` — Sonde HF high-speed adaptive stack).
> Subordinate to the canonical tuxlink specs:
> - `clean-sheet-modem-3-phy-waveform.md` (PHY subsystem) — Phase 10 (PHY+FEC integration)
> - `clean-sheet-modem-4-fec.md` (FEC subsystem)
> - `clean-sheet-modem-3-phy-waveform-plan.md` §0 — freezes the soft-LLR bus boundary
>
> Design shape adversarially reviewed by Codex (read-only); converged on the
> boundary architecture and the single-coded-path decision. Codex's five
> refinements are folded into §4–§6 below.

## §1. Goal

The robustness floor mode (`WidebandLowDensityFloor`) transmits and receives
through **real LDPC forward error correction**, reachable from the operational
pipelines (`sonde-tx` / `sonde-rx`), and **proven live by a differential
channel-simulator test**. This is the foundation slice of the HF high-speed
adaptive stack: it makes the soft-LLR FEC bus a wired, exercised reality before
any bit-adaptive / link-adaptation work is built on top of it.

## §2. Why this is one slice, not two

The governing project principle is **vertical integration: no unwired islands.**
The codebase already carries three "built but wired into nothing" artifacts —
`ofdm_main::bit_loader`, `subcarrier_snr`, and the `sonde-fec` crate — each
unit-tested and called by no operational path. This slice must not add a fourth.

A discovery during design makes the coupling unavoidable: **the floor's rate-1/4
LDPC code does not work.** `sonde-fec/src/codes/floor_rate14.rs` documents that
its (3,4)-regular parity-check matrix `H` is rank-deficient, that
`encode::Encoder` **panics** on it, and that the proper construction fix is
deferred (`tuxlink-bbin`). The only `FecCodec` implementor that actually
encodes/decodes today is `OfdmAdaptiveCodec`. There is **no `FloorRate14Codec`
type at all** — only a `CodeFamily::FloorRate14` enum variant and a `build()`
matrix function.

Therefore "wire in the floor codec" has a hard prerequisite: a working floor
codec must first exist. Splitting that into a separate earlier slice would
re-create the exact island pathology this project is trying to kill (a codec
sitting unwired between slices). So both live in one slice, terminating in the
wired pipeline + differential gate.

## §3. Architecture — dependency-inverted soft-LLR bus

```
        owns FecCodec trait + IdentityFec            implements FecCodec
   ┌─────────────────────────────────┐         ┌──────────────────────────┐
   │            sonde-phy            │◄────────│         sonde-fec        │
   │  coded_modulation::FecCodec     │  dep    │  FloorRate14Codec (new)  │
   │  WidebandLowDensityFloor        │         │  OfdmAdaptiveCodec       │
   │    holds Box<dyn FecCodec>      │         │  floor LDPC construction │
   └─────────────────────────────────┘         └──────────────────────────┘
                 ▲                                          ▲
                 │ dep                                      │ dep (NEW)
                 │                                          │
          ┌──────┴──────────────────────────────────────────┴──────┐
          │              sonde-tx  /  sonde-rx                       │
          │   construct FloorRate14Codec, inject via with_fec(...)   │
          └──────────────────────────────────────────────────────────┘
```

`sonde-fec` already depends on `sonde-phy` (it implements the `FecCodec` trait
that lives there). A `sonde-phy → sonde-fec` dependency would therefore be a
**cycle** — which is exactly why the dependency sits commented out today. The
resolution is **dependency inversion**, matching the plan's wording ("PHY
*composes* a `Box<dyn FecCodec>`"):

- `sonde-phy` keeps owning the `FecCodec` trait + `IdentityFec`. **No new
  `sonde-phy → sonde-fec` dependency is added.**
- `WidebandLowDensityFloor` holds a `Box<dyn FecCodec>`; `with_fec(codec)`
  injects a concrete codec.
- **`sonde-tx` and `sonde-rx` gain the `sonde-fec` dependency** and construct +
  inject `FloorRate14Codec`. This injection at the pipeline layer is what makes
  the coded path the one the radio actually uses — the anti-island guard.

## §4. Component A — make the rate-1/4 floor LDPC work (`sonde-fec`)

- Replace the rank-deficient configuration-model `H` with a **full-rank
  construction** (progressive-edge-growth, or a column-pivot that extracts a
  full-rank parity submatrix), so `Encoder::try_new` succeeds and `encode` no
  longer panics. This is the `tuxlink-bbin` fix, pulled into scope.
- Expose a concrete **`FloorRate14Codec`** type implementing `FecCodec`
  (`encode`, `decode_soft`, `rate` = 1/4, `block_info_bits`, `block_coded_bits`),
  reusing the existing CRC + interleaver + sum-product decoder machinery the way
  `OfdmAdaptiveCodec` does.
- **Acceptance for A:** the previously `#[ignore]`d `encoder_handles_floor_rate14`
  test goes green, and a decode-correctness test shows
  `encode → AWGN at a benign SNR → decode_soft` recovers the info bits.

## §5. Component B — compose into the floor over the bus (`sonde-phy` + pipelines)

**Single coded path, codeword-framed** (Codex refinement #1). There is one
transmit/receive path; it always runs a codec. `IdentityFec` (rate 1/1) is the
sim-isolation baseline that preserves today's behavior for the migrated tests;
`FloorRate14Codec` is the operational codec.

**Transmit:**
1. Frame payload bytes → info-bit stream, with a **fixed-width length header as
   the first field of the stream** (refinement #2), so it lands inside the first
   FEC codeword and is itself error-corrected.
2. Segment into `codec.block_info_bits()`-sized blocks (refinement #3 — never
   hardcode 512; the codec's internal CRC may make usable payload capacity
   *less* than the LDPC `k`). Zero-pad the final block.
3. `codec.encode` each block → coded bits.
4. Pack coded bits into the floor's 74-bit BPSK OFDM symbols (zero-pad the final
   symbol), prefixed by the existing Zadoff-Chu preamble.

**Receive:**
1. Preamble-detect (existing sync).
2. For each codeword: demodulate exactly one block's worth of OFDM symbols to
   **soft LLRs**, **trim the OFDM zero-pad LLRs**, and feed the block to
   `codec.decode_soft`. **No hard decision before the decoder** (refinement #5 —
   the floor RX today computes LLRs then immediately hard-decides at
   `wideband_lowdensity.rs:177`, discarding the soft information the LDPC decoder
   needs).
3. Decode the **first** codeword first to read the length header, then decode the
   remaining `ceil`-implied blocks.

**Pipeline wiring:** `sonde-tx` and `sonde-rx` construct `FloorRate14Codec` and
inject it via `with_fec`, replacing the implicit uncoded path.

## §6. Error handling

- A `decode_soft` failure on the **header-bearing first codeword** fails the
  whole frame (remaining length is unknown) — surfaced as an explicit
  `PhyError::FecDecode` / `FrameDetect`, never silent garbage.
- Per-block decode failures propagate as errors; the decoder must not panic on
  uncorrectable input.
- LLR reliability: the OFDM receiver currently uses a hardcoded `n0 = 0.1` for
  LLR magnitude (`ofdm_main/receiver.rs:75`). Sum-product decoding depends on
  reliability calibration, not just LLR sign. **Noise-variance estimation for LLR
  scaling is in scope only if the differential gate (§7) cannot otherwise pass** —
  we let the gate tell us whether calibration is needed rather than speculatively
  building it.

## §7. Testing — the verification spine

1. **Differential channel-sim gate (the un-fakeable wiring proof).** The same
   noisy capture, run through `sonde-tx` encode → `sonde-rx` decode via
   `crates/sonde-phy/tests/sim_adapter.rs` (the designated `hf-channel-sim`
   integration point), **fails to decode with `IdentityFec`** and **succeeds with
   `FloorRate14Codec`**, at defined SNR points with hard pass/fail thresholds
   (no visual-impression checks). This test is impossible to pass unless the real
   codec is genuinely in the operational signal path — so it doubles as the
   anti-island guard for the pipeline injection.
2. **Component-A decode correctness** (§4 acceptance).
3. **Migration of the existing floor tests** (~28 byte-level round-trip tests)
   onto the single coded path with `IdentityFec` as the baseline. Combined with
   the §7.1 gate and the pipeline injection, this ensures the operational path is
   never silently uncoded.

## §8. Explicitly out of scope (deferred — and *not* built unwired)

- ARQ residual-error stats (`sonde-fec::stats`, subsystem #6) — nothing consumes
  them yet; building them now would itself be an island.
- The bit-adaptive OFDM main family + per-sub-carrier bit-loading (PHY phases
  6–7) — later epic slices.
- Closed-loop link adaptation (subsystem #7) — later epic slices.

## §9. References

- Canonical specs: tuxlink `docs/superpowers/specs/clean-sheet-modem-{3-phy-waveform,4-fec}.md`
  and `plans/clean-sheet-modem-3-phy-waveform-plan.md` §0.
- ADR 0014 (clean-sheet, no prior-art examination) — conceptual primitives only.
- Code touchpoints: `sonde-phy/src/coded_modulation.rs`,
  `sonde-phy/src/robustness_floor/wideband_lowdensity.rs`,
  `sonde-fec/src/codec.rs`, `sonde-fec/src/codes/floor_rate14.rs`,
  `sonde-fec/src/encode.rs`.
