# FEC-first floor LDPC wiring — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the robustness-floor mode transmit/receive through real rate-1/4 LDPC error correction, reachable from `sonde-tx`/`sonde-rx`, proven live by a differential channel-sim test.

**Architecture:** Dependency-inverted soft-LLR bus. `sonde-phy` owns the `FecCodec` trait + `IdentityFec`; `sonde-fec` gets a working `FloorRate14Codec`; `sonde-tx`/`sonde-rx` depend on both and inject the concrete codec. The floor runs ONE coded, codeword-per-block, soft-LLR path. No `sonde-phy → sonde-fec` dependency (would be a cycle).

**Tech Stack:** Rust 2021; `bitvec`; existing `sonde-fec` LDPC machinery (`Encoder`, `Decoder`, `append_crc32`, `interleave`); `hf-channel-sim` (`WattersonChannel`, `AwgnGenerator`) for the gate.

**Design spec:** `docs/superpowers/specs/2026-06-14-fec-floor-wiring-design.md`. bd: `sonde-64w.1`.

**Safety:** All work is DSP/codec + channel-sim tests. Nothing here keys a radio or runs TX hardware — agent-safe. Do NOT add or invoke anything under `sonde-tx`'s PTT/audio-device code paths beyond the pure `encode_payload` byte→samples function.

**Commands** (run from the worktree root unless a `-p` crate is named):
```bash
cargo test  -p sonde-fec
cargo test  -p sonde-phy
cargo test  -p sonde-tx
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/sonde-fec/src/codes/floor_rate14.rs` | Floor H construction | Modify — seed-search for a rank-full H |
| `crates/sonde-fec/src/codes/mod.rs` | `CodeFamily::FloorRate14 => build` | Modify — route to full-rank build |
| `crates/sonde-fec/src/codec.rs` | `FloorRate14Codec: FecCodec` | Modify — add the type |
| `crates/sonde-fec/src/encode.rs` | `encoder_handles_floor_rate14` test | Modify — un-ignore |
| `crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs` | Floor PHY: hold `Box<dyn FecCodec>`, single coded path | Modify |
| `crates/sonde-phy/src/robustness_floor/coded_framing.rs` | Bit-stream ↔ codeword-per-block symbol packing + length header | Create |
| `crates/sonde-tx/Cargo.toml`, `src/lib.rs` | Inject `FloorRate14Codec` into `encode_payload` | Modify |
| `crates/sonde-rx/Cargo.toml`, `src/lib.rs` | Inject `FloorRate14Codec` into `decode_one_symbol` | Modify |
| `crates/sonde-tx/tests/fec_differential_gate.rs` | Differential channel-sim gate | Create |

---

## Phase A — Working rate-1/4 floor LDPC (`sonde-fec`)

### Task A1: Seed-search construction for a rank-full floor H

**Files:**
- Modify: `crates/sonde-fec/src/codes/floor_rate14.rs`

The existing `build_with_seed(seed) -> Option<ParityCheckMatrix>` produces a structurally-valid (weights correct) H but, at the fixed `SEED`, its right half is rank-deficient so `Encoder::try_new` fails. `Encoder::try_new` is the authoritative rank oracle. Iterate a deterministic seed sequence until one yields an H that `Encoder::try_new` accepts.

- [ ] **Step 1: Write the failing test** — append to the `tests` module in `floor_rate14.rs`:

```rust
    #[test]
    fn seed_search_finds_rank_full_floor_h() {
        // Riskiest-first spike: a rank-full floor H must exist within the
        // bounded seed budget. If this fails, seed-search is NOT a viable
        // fix and the slice must escalate to a PEG construction (a new task,
        // out of this plan) — STOP and surface that.
        let h = build_rank_full();
        // The encoder is the rank oracle: success ⇒ right half is full-rank.
        assert!(
            crate::encode::Encoder::try_new(&h).is_ok(),
            "build_rank_full produced a rank-deficient H"
        );
        assert_eq!(h.n, N);
        assert_eq!(h.k, K);
    }

    #[test]
    fn rank_full_construction_is_deterministic() {
        let a = build_rank_full();
        let b = build_rank_full();
        assert_eq!(a.rows, b.rows, "construction must be reproducible");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sonde-fec --lib floor_rate14`
Expected: FAIL — `cannot find function build_rank_full`.

- [ ] **Step 3: Implement `build_rank_full`** — add to `floor_rate14.rs` (above the `tests` module). Reuse the existing private `build_with_seed`:

```rust
/// Maximum seeds to try before giving up on seed-search.
const MAX_RANK_SEARCH_SEEDS: u64 = 4096;

/// Construct a floor rate-1/4 H whose right half is full-rank (so the
/// systematic [`crate::encode::Encoder`] accepts it), by iterating a
/// deterministic seed sequence derived from [`SEED`]. Reproducible:
/// always returns the H from the first accepting seed.
///
/// # Panics
/// Panics if no seed in `0..MAX_RANK_SEARCH_SEEDS` yields a rank-full H —
/// that would mean seed-search is not viable and a PEG construction is
/// required (tuxlink-bbin escalation).
pub fn build_rank_full() -> ParityCheckMatrix {
    for i in 0..MAX_RANK_SEARCH_SEEDS {
        let seed = SEED ^ i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        if let Some(h) = build_with_seed(seed) {
            if crate::encode::Encoder::try_new(&h).is_ok() {
                return h;
            }
        }
    }
    panic!(
        "no rank-full floor H within {MAX_RANK_SEARCH_SEEDS} seeds — \
         escalate to PEG construction (tuxlink-bbin)"
    );
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sonde-fec --lib floor_rate14`
Expected: PASS, both new tests. (If `seed_search_finds_rank_full_floor_h` FAILS, STOP — escalate per the test's comment.)

- [ ] **Step 5: Commit**

```bash
git add crates/sonde-fec/src/codes/floor_rate14.rs
git commit -m "feat(sonde-fec): rank-full floor rate-1/4 H via deterministic seed-search

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task A2: Route `CodeFamily::FloorRate14` to the rank-full build + un-ignore encoder test

**Files:**
- Modify: `crates/sonde-fec/src/codes/mod.rs:72` (the `CodeFamily::FloorRate14 => floor_rate14::build()` arm)
- Modify: `crates/sonde-fec/src/encode.rs:210-213` (remove `#[ignore]`)

- [ ] **Step 1: Un-ignore the encoder test** — in `encode.rs`, delete the `#[ignore = "..."]` attribute on `encoder_handles_floor_rate14` (lines ~210-212), leaving the `#[test] fn encoder_handles_floor_rate14()` intact.

- [ ] **Step 2: Run to verify it now fails** (build still uses rank-deficient fixed seed)

Run: `cargo test -p sonde-fec --lib encoder_handles_floor_rate14`
Expected: FAIL — `Encoder::new` panics with `RankDeficient` (the build still routes to the fixed-seed `build()`).

- [ ] **Step 3: Route the family to the rank-full build** — in `codes/mod.rs`, change the match arm:

```rust
        CodeFamily::FloorRate14 => floor_rate14::build_rank_full(),
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sonde-fec --lib encoder_handles_floor_rate14`
Expected: PASS (all-zero info → all-zero codeword, k=512, n=2048).

- [ ] **Step 5: Commit**

```bash
git add crates/sonde-fec/src/codes/mod.rs crates/sonde-fec/src/encode.rs
git commit -m "feat(sonde-fec): FloorRate14 family uses rank-full build; un-ignore encoder test

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task A3: `FloorRate14Codec` implementing `FecCodec`

**Files:**
- Modify: `crates/sonde-fec/src/codec.rs`

Mirror `OfdmAdaptiveCodec` exactly — same CRC + interleave + SPA composition, same bit-layout — but built from `CodeFamily::FloorRate14` with a fixed rate of 1/4. The shared helpers (`bytes_to_bitvec`, `bitvec_to_bytes`, `deinterleave_index_perm`, `INTERLEAVER_ROWS`) already exist in this file; reuse them.

- [ ] **Step 1: Write the failing test** — append to the `tests` module in `codec.rs`:

```rust
    #[test]
    fn floor_rate14_block_sizes_and_rate() {
        let codec = FloorRate14Codec::new();
        // LDPC k=512, minus 32-bit CRC ⇒ 480 payload bits.
        assert_eq!(codec.block_info_bits(), 480);
        assert_eq!(codec.block_coded_bits(), 2048);
        let r = codec.rate();
        assert_eq!((r.num, r.den), (1, 4));
    }

    #[test]
    fn floor_rate14_round_trip_zero_noise() {
        let codec = FloorRate14Codec::new();
        let payload = random_bits(codec.block_info_bits(), 0xF100_0014);
        let encoded = codec.encode(&payload);
        assert_eq!(encoded.len(), codec.block_coded_bits());
        let llrs: Vec<f32> = encoded
            .iter()
            .map(|&b| if b == 0 { 10.0 } else { -10.0 })
            .collect();
        let recovered = codec.decode_soft(&llrs).expect("decode_soft");
        assert_eq!(recovered, payload, "floor codec round-trip lossless");
    }

    #[test]
    fn floor_rate14_corrupted_returns_error() {
        let codec = FloorRate14Codec::new();
        let payload = random_bits(codec.block_info_bits(), 7);
        let encoded = codec.encode(&payload);
        let mut llrs: Vec<f32> = encoded
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect();
        for i in (0..llrs.len()).step_by(2) {
            llrs[i] = -llrs[i]; // half the bits flipped — uncorrectable
        }
        assert!(codec.decode_soft(&llrs).is_err());
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sonde-fec --lib floor_rate14_`
Expected: FAIL — `cannot find type FloorRate14Codec`.

- [ ] **Step 3: Implement `FloorRate14Codec`** — add to `codec.rs` (after the `OfdmAdaptiveCodec` impl block, before the shared free functions):

```rust
/// LDPC codec for the rate-1/4 wide-band low-density robustness floor
/// (`CodeFamily::FloorRate14`). Same CRC + interleave + SPA composition
/// as [`OfdmAdaptiveCodec`]; fixed rate 1/4, n=2048, k=512.
pub struct FloorRate14Codec {
    h: ParityCheckMatrix,
    encoder: LdpcEncoder,
    decoder: LdpcDecoder,
    n: usize,
    ldpc_k: usize,
}

impl FloorRate14Codec {
    /// Build the floor codec. Constructs the rank-full H, encoder, and
    /// SPA decoder up-front; per-encode/decode is then data-plane work.
    pub fn new() -> Self {
        let h = codes::build(CodeFamily::FloorRate14);
        let encoder = LdpcEncoder::new(&h);
        let decoder = LdpcDecoder::new(&h);
        let n = h.n;
        let ldpc_k = h.k;
        Self { h, encoder, decoder, n, ldpc_k }
    }

    /// LDPC payload bits per block (excluding the 32-bit CRC).
    fn payload_bits(&self) -> usize {
        self.ldpc_k - 32
    }
}

impl Default for FloorRate14Codec {
    fn default() -> Self {
        Self::new()
    }
}

impl FecCodec for FloorRate14Codec {
    fn encode(&self, info_bits: &[u8]) -> Vec<u8> {
        assert_eq!(
            info_bits.len(),
            self.payload_bits(),
            "FloorRate14Codec::encode: info_bits {} != payload k {}",
            info_bits.len(),
            self.payload_bits()
        );
        let info = bytes_to_bitvec(info_bits);
        let with_crc = append_crc32(info.as_bitslice());
        debug_assert_eq!(with_crc.len(), self.ldpc_k);
        let codeword = self.encoder.encode(with_crc.as_bitslice());
        debug_assert_eq!(codeword.len(), self.n);
        let interleaved = interleave(codeword.as_bitslice(), INTERLEAVER_ROWS);
        bitvec_to_bytes(interleaved.as_bitslice())
    }

    fn decode_soft(&self, llr: &[f32]) -> Result<Vec<u8>, FecError> {
        if llr.len() != self.n {
            return Err(FecError::DecodeFailure(format!(
                "decode_soft: llr.len() {} != n {}",
                llr.len(),
                self.n
            )));
        }
        let perm = deinterleave_index_perm(self.n, INTERLEAVER_ROWS);
        let deint_llrs: Vec<f32> = (0..self.n).map(|i| llr[perm[i]]).collect();
        let outcome = self.decoder.decode(&deint_llrs, MAX_ITERS_OFDM);
        let info_plus_crc: BitVec<u8> =
            outcome.decoded[..self.ldpc_k].iter().copied().collect();
        verify_crc32(info_plus_crc.as_bitslice()).map_err(|e| {
            FecError::DecodeFailure(format!(
                "CRC mismatch after {} iterations: {e}",
                outcome.iterations_used
            ))
        })?;
        Ok(bitvec_to_bytes(&info_plus_crc[..self.payload_bits()]))
    }

    fn rate(&self) -> CodeRate {
        CodeRate { num: 1, den: 4 }
    }

    fn block_info_bits(&self) -> usize {
        self.payload_bits()
    }

    fn block_coded_bits(&self) -> usize {
        self.n
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sonde-fec --lib floor_rate14_`
Expected: PASS, 3 tests. Then `cargo test -p sonde-fec` — full suite green.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p sonde-fec --all-targets -- -D warnings
git add crates/sonde-fec/src/codec.rs
git commit -m "feat(sonde-fec): FloorRate14Codec — rate-1/4 LDPC over the FecCodec bus

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase B — Compose into the floor over the soft-LLR bus (`sonde-phy`)

> Per ADR 0014, `sonde-phy` does NOT depend on `sonde-fec`. All Phase B tests use
> `IdentityFec` (already in `sonde-phy`) as the codec — the sim-isolation baseline.
> The real `FloorRate14Codec` is injected later from the pipeline crates (Phase C).

### Task B1: `coded_framing` module — length header + codeword-per-block bit packing

**Files:**
- Create: `crates/sonde-phy/src/robustness_floor/coded_framing.rs`
- Modify: `crates/sonde-phy/src/robustness_floor/mod.rs` (add `pub mod coded_framing;`)

Pure bit-plumbing, no DSP, fully unit-testable in isolation. Frame layout: a 16-bit big-endian payload-length header is the first field of the global info-bit stream, followed by payload bits, zero-padded to a whole number of `block_info_bits`-sized blocks.

- [ ] **Step 1: Write the failing test** — `crates/sonde-phy/tests/coded_framing.rs`:

```rust
use sonde_phy::robustness_floor::coded_framing::{
    frame_info_bits, deframe_info_bits, blocks_for_payload, HEADER_BITS,
};

#[test]
fn header_is_16_bits() {
    assert_eq!(HEADER_BITS, 16);
}

#[test]
fn frame_then_deframe_round_trips() {
    let payload = b"hello floor";
    let block_info = 480;
    let framed = frame_info_bits(payload, block_info);
    // length is a whole number of blocks
    assert_eq!(framed.len() % block_info, 0);
    let n_blocks = framed.len() / block_info;
    assert_eq!(n_blocks, blocks_for_payload(payload.len(), block_info));
    let out = deframe_info_bits(&framed).expect("deframe");
    assert_eq!(out, payload);
}

#[test]
fn block_count_matches_first_block_header() {
    // The block count is derivable from the header in the FIRST block alone.
    let payload = vec![0xABu8; 200];
    let block_info = 480;
    let framed = frame_info_bits(&payload, block_info);
    let first_block = &framed[..block_info];
    let declared = blocks_from_first_block(first_block, block_info);
    assert_eq!(declared, framed.len() / block_info);
}

// helper re-exported for the test
use sonde_phy::robustness_floor::coded_framing::blocks_from_first_block;
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sonde-phy --test coded_framing`
Expected: FAIL — unresolved import `coded_framing`.

- [ ] **Step 3: Implement `coded_framing.rs`**:

```rust
//! Codeword-spanning framing for the coded floor path.
//!
//! Layout of the global INFO-bit stream (pre-FEC), one `u8` per bit
//! (LSB), matching the `FecCodec` bit convention:
//!
//! ```text
//! [ len: 16 bits BE ][ payload bits ][ zero pad → multiple of block_info ]
//! ```
//!
//! The length header lives in the FIRST block so the receiver can learn
//! the block count after decoding block 0 alone.

use crate::error::PhyError;

/// Payload-length header width, in bits (u16 big-endian → ≤ 65535 bytes).
pub const HEADER_BITS: usize = 16;

/// Number of FEC blocks a payload occupies given `block_info` info-bits
/// per block.
pub fn blocks_for_payload(payload_len: usize, block_info: usize) -> usize {
    let total = HEADER_BITS + payload_len * 8;
    total.div_ceil(block_info).max(1)
}

/// Expand payload bytes into the padded global info-bit stream.
pub fn frame_info_bits(payload: &[u8], block_info: usize) -> Vec<u8> {
    let n_blocks = blocks_for_payload(payload.len(), block_info);
    let mut bits = Vec::with_capacity(n_blocks * block_info);
    let len = payload.len() as u16;
    for i in (0..HEADER_BITS).rev() {
        bits.push(((len >> i) & 1) as u8);
    }
    for &byte in payload {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1);
        }
    }
    bits.resize(n_blocks * block_info, 0);
    bits
}

/// Read the block count from the first decoded block's info bits.
pub fn blocks_from_first_block(first_block: &[u8], block_info: usize) -> usize {
    let len = read_header_len(first_block);
    blocks_for_payload(len, block_info)
}

/// Recover payload bytes from the full concatenated info-bit stream.
pub fn deframe_info_bits(info_bits: &[u8]) -> Result<Vec<u8>, PhyError> {
    if info_bits.len() < HEADER_BITS {
        return Err(PhyError::FrameDetect(
            "coded frame shorter than length header".into(),
        ));
    }
    let len = read_header_len(info_bits);
    let need = HEADER_BITS + len * 8;
    if info_bits.len() < need {
        return Err(PhyError::FrameDetect(format!(
            "declared payload {len} bytes needs {need} bits, have {}",
            info_bits.len()
        )));
    }
    let mut out = Vec::with_capacity(len);
    for b in 0..len {
        let mut byte = 0u8;
        for i in 0..8 {
            byte |= info_bits[HEADER_BITS + b * 8 + i] << (7 - i);
        }
        out.push(byte);
    }
    Ok(out)
}

fn read_header_len(bits: &[u8]) -> usize {
    let mut len = 0usize;
    for i in 0..HEADER_BITS {
        len = (len << 1) | (bits[i] as usize & 1);
    }
    len
}
```

Add to `robustness_floor/mod.rs`:

```rust
pub mod coded_framing;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sonde-phy --test coded_framing`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/sonde-phy/src/robustness_floor/coded_framing.rs crates/sonde-phy/src/robustness_floor/mod.rs crates/sonde-phy/tests/coded_framing.rs
git commit -m "feat(sonde-phy): coded_framing — length header + codeword-spanning bit packing

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task B2: Floor holds a codec; coded transmit/receive (single path)

**Files:**
- Modify: `crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs`

Give `WidebandLowDensityFloor` a `Box<dyn FecCodec>` field. `new()` defaults to `IdentityFec` sized to one block; add `with_fec(codec)`. Replace the internal byte-level packing of `transmit`/`receive` with the coded, codeword-per-block path. Public method signatures are UNCHANGED (`transmit(&[u8]) -> Vec<f32>`, `receive(&[f32]) -> Vec<u8>`, and the `*_with_preamble` / `*_multi*` variants) so callers don't change.

Per-block symbol packing: each FEC block's `block_coded_bits()` coded bits → `ceil(coded_bits / data_bits_per_symbol)` OFDM symbols (last zero-padded). On RX, demod each block's symbols to soft LLRs, trim the trailing pad LLRs back to `block_coded_bits()`, then `decode_soft`. Decode block 0 first to read the header → block count → decode the rest.

- [ ] **Step 1: Write the failing test** — append to the `tests` module in `wideband_lowdensity.rs`:

```rust
    #[test]
    fn coded_round_trip_identity_fec_various_lengths() {
        // IdentityFec baseline: the coded path must round-trip losslessly
        // on a clean (back-to-back) channel for a range of payload sizes,
        // including trailing/leading zero bytes and multi-block payloads.
        for payload in [
            &b""[..],
            &b"X"[..],
            &b"hello floor mode"[..],
            &[0u8; 30][..],
            &b"AB\x00\x00\x00"[..],
        ] {
            let floor = WidebandLowDensityFloor::new(); // IdentityFec default
            let samples = floor.transmit_multi_with_preamble(payload).unwrap();
            let (start, decoded) = floor.receive_multi_with_sync(&samples).unwrap();
            assert_eq!(start, 0);
            assert_eq!(decoded, payload, "coded round-trip for {} bytes", payload.len());
        }
    }

    #[test]
    fn coded_round_trip_large_multiblock_payload() {
        let floor = WidebandLowDensityFloor::new();
        let payload: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
        let samples = floor.transmit_multi_with_preamble(&payload).unwrap();
        let (_s, decoded) = floor.receive_multi_with_sync(&samples).unwrap();
        assert_eq!(decoded, payload);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sonde-phy --lib coded_round_trip`
Expected: FAIL — old byte-level framing produces different bytes / the methods don't yet route through a codec. (Compile may pass but assertions fail.)

- [ ] **Step 3: Implement the coded path** in `wideband_lowdensity.rs`. Replace the struct + `new()` and rewrite the framing internals:

```rust
use crate::coded_modulation::{FecCodec, IdentityFec};
use crate::robustness_floor::coded_framing::{
    blocks_from_first_block, deframe_info_bits, frame_info_bits,
};

pub struct WidebandLowDensityFloor {
    params: OfdmParams,
    fec: Box<dyn FecCodec>,
}

impl WidebandLowDensityFloor {
    /// Floor with the default sim-isolation codec (`IdentityFec`). The
    /// identity block is one OFDM symbol's data-bit capacity, so the
    /// uncoded baseline frames cleanly.
    pub fn new() -> Self {
        let params = OfdmParams::for_mode(OfdmModeName::Wide);
        let block = params.data_indices().len();
        Self { params, fec: Box::new(IdentityFec::new(block)) }
    }

    /// Floor with an injected concrete codec (e.g. `FloorRate14Codec`
    /// from `sonde-fec`, wired at the pipeline layer).
    pub fn with_fec(fec: Box<dyn FecCodec>) -> Self {
        Self { params: OfdmParams::for_mode(OfdmModeName::Wide), fec }
    }

    fn data_bits_per_symbol(&self) -> usize {
        self.params.data_indices().len()
    }

    fn symbols_per_block(&self) -> usize {
        self.fec.block_coded_bits().div_ceil(self.data_bits_per_symbol())
    }

    /// Render one OFDM symbol from up to `data_bits_per_symbol` coded bits.
    fn modulate_coded_symbol(&self, coded_bits: &[u8]) -> Vec<f32> {
        let bits_per_sc = self.bits_per_subcarrier();
        let mut sym_bits = coded_bits.to_vec();
        sym_bits.resize(self.data_bits_per_symbol(), 0);
        let tx = OfdmTransmitter::new(&self.params);
        tx.modulate_one_symbol(&sym_bits, &bits_per_sc)
    }

    /// Demod one symbol back to soft LLRs across its data sub-carriers.
    fn demodulate_coded_symbol(&self, samples: &[f32]) -> Vec<f32> {
        let bits_per_sc = self.bits_per_subcarrier();
        let rx = OfdmReceiver::new(&self.params);
        rx.demodulate_one_symbol(samples, &bits_per_sc)
    }
}
```

Then rewrite the public framing methods so they all funnel through one coded engine. The bare `transmit`/`receive` encode/decode a single block; `transmit_multi*`/`receive_multi*` handle N blocks. Concretely, replace the bodies of `transmit_multi` and `receive_multi` with:

```rust
    pub fn transmit_multi(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        if payload.len() > u16::MAX as usize {
            return Err(PhyError::PayloadTooLarge {
                actual: payload.len(),
                capacity: u16::MAX as usize,
            });
        }
        let block_info = self.fec.block_info_bits();
        let info = frame_info_bits(payload, block_info); // padded to N blocks
        let dps = self.data_bits_per_symbol();
        let mut out = Vec::new();
        for block in info.chunks(block_info) {
            let coded = self.fec.encode(block); // len == block_coded_bits()
            for chunk in coded.chunks(dps) {
                out.extend_from_slice(&self.modulate_coded_symbol(chunk));
            }
        }
        Ok(out)
    }

    pub fn receive_multi(&self, samples: &[f32]) -> Result<Vec<u8>, PhyError> {
        let dps = self.data_bits_per_symbol();
        let sym = self.symbol_size_samples();
        let spb = self.symbols_per_block();
        let block_coded = self.fec.block_coded_bits();
        if samples.len() < spb * sym {
            return Err(PhyError::FrameDetect(format!(
                "input {} samples < one coded block ({} symbols)",
                samples.len(),
                spb
            )));
        }
        // Decode helper: one block's worth of symbols → info bits.
        let decode_block = |blk: usize| -> Result<Vec<u8>, PhyError> {
            let base = blk * spb * sym;
            let mut llrs = Vec::with_capacity(spb * dps);
            for s in 0..spb {
                let start = base + s * sym;
                if start + sym > samples.len() {
                    return Err(PhyError::FrameDetect("coded block truncated".into()));
                }
                llrs.extend_from_slice(&self.demodulate_coded_symbol(&samples[start..start + sym]));
            }
            llrs.truncate(block_coded); // trim OFDM pad LLRs
            self.fec
                .decode_soft(&llrs)
                .map_err(|e| PhyError::FecDecode(e.to_string()))
        };
        // Block 0 carries the header.
        let block_info = self.fec.block_info_bits();
        let first = decode_block(0)?;
        let n_blocks = blocks_from_first_block(&first, block_info);
        let mut info = first;
        for blk in 1..n_blocks {
            info.extend_from_slice(&decode_block(blk)?);
        }
        deframe_info_bits(&info)
    }
```

Make `transmit`/`receive` (single-block convenience) delegate to the multi path:

```rust
    pub fn transmit(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        self.transmit_multi(payload)
    }
    pub fn receive(&self, samples: &[f32]) -> Result<Vec<u8>, PhyError> {
        self.receive_multi(samples)
    }
```

Keep `transmit_with_preamble`/`receive_with_sync` as the preamble-wrapping composition over `transmit_multi`/`receive_multi` (they already prepend/scan the Zadoff-Chu preamble — repoint them at the multi methods if not already). Keep `symbol_size_samples`, `bits_per_subcarrier`, `params`, `PREAMBLE_LEN_SAMPLES`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sonde-phy --lib coded_round_trip`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs
git commit -m "feat(sonde-phy): floor holds Box<dyn FecCodec>; single coded codeword-per-block path

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task B3: Migrate the existing floor tests onto the coded path

**Files:**
- Modify: `crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs` (the `tests` module)
- Modify: `crates/sonde-phy/tests/floor_wideband.rs` (if present)

The legacy tests assert byte-level sample-count math (`9 bytes/symbol`, `transmit_multi(100 bytes) == 12 symbols`, `data_bytes_per_symbol() == 9`). Those invariants no longer hold under codeword-spanning framing. Keep every ROUND-TRIP test (payload in == payload out); replace exact sample-count assertions with round-trip assertions.

- [ ] **Step 1: Update the legacy tests** — for each test in the `tests` module that asserts a specific sample count or `data_bytes_per_symbol`, either:
  - delete the byte-count-specific assertion and keep the round-trip portion, or
  - if the whole test is byte-count math (`data_bytes_per_symbol_is_positive`, `transmit_multi_length_for_small_payload_is_one_symbol`, `transmit_multi_length_grows_in_symbol_steps`, `transmit_multi_does_not_use_preamble`), replace its body with a coded round-trip on an equivalent payload. Example replacement for `transmit_multi_length_for_small_payload_is_one_symbol`:

```rust
    #[test]
    fn small_payload_coded_round_trip() {
        let floor = WidebandLowDensityFloor::new();
        let samples = floor.transmit_multi(b"HELLO").unwrap();
        let decoded = floor.receive_multi(&samples).unwrap();
        assert_eq!(decoded, b"HELLO");
    }
```

  Keep `preamble_roundtrip_*`, `multi_roundtrip_*`, `multi_with_preamble_roundtrip_*`, and the `FrameDetect`-on-silence/noise tests — they assert round-trip or error behavior that still holds. Remove `bits_per_subcarrier`-internal assumptions only if they reference the removed `data_bytes_per_symbol`.

- [ ] **Step 2: Remove now-dead helpers** — if `data_bytes_per_symbol` / `decode_symbol_bytes` are no longer referenced by any non-test code or test, delete them (YAGNI; they encoded the byte-level path that no longer exists).

- [ ] **Step 3: Run the full floor + phy suite**

Run: `cargo test -p sonde-phy`
Expected: PASS — all floor tests green on the coded path.

- [ ] **Step 4: Lint + commit**

```bash
cargo clippy -p sonde-phy --all-targets -- -D warnings
git add crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs crates/sonde-phy/tests/floor_wideband.rs
git commit -m "test(sonde-phy): migrate floor tests onto the coded path; drop byte-level framing

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Phase C — Wire the pipelines + the differential gate

### Task C1: Inject `FloorRate14Codec` in `sonde-tx`

**Files:**
- Modify: `crates/sonde-tx/Cargo.toml` (add `sonde-fec` dep)
- Modify: `crates/sonde-tx/src/lib.rs:206` (`encode_payload`)

- [ ] **Step 1: Add the dependency** — in `crates/sonde-tx/Cargo.toml` `[dependencies]`:

```toml
sonde-fec.workspace = true
```

- [ ] **Step 2: Write the failing test** — append to the `tests` module in `crates/sonde-tx/src/lib.rs`:

```rust
    #[test]
    fn encode_payload_uses_floor_fec_round_trip() {
        // The operational encode path must produce samples that the
        // matching coded floor decodes back to the payload — only true
        // if FloorRate14Codec is actually injected.
        use sonde_fec::codec::FloorRate14Codec;
        use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;
        let buf = encode_payload(Mode::WideFloor, b"FECWIRED", FrameMode::MultiSync).unwrap();
        let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
        let (_s, decoded) = floor.receive_multi_with_sync(buf.samples()).unwrap();
        assert_eq!(decoded, b"FECWIRED");
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p sonde-tx encode_payload_uses_floor_fec`
Expected: FAIL — `encode_payload` still builds the floor with `::new()` (IdentityFec), so the `FloorRate14Codec` decode mismatches.

- [ ] **Step 4: Inject the codec** — in `encode_payload`, change line 206:

```rust
    let floor = WidebandLowDensityFloor::with_fec(Box::new(sonde_fec::codec::FloorRate14Codec::new()));
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p sonde-tx encode_payload_uses_floor_fec`
Expected: PASS. Then `cargo test -p sonde-tx` — full suite green (other tests already assert round-trip, which still holds).

- [ ] **Step 6: Commit**

```bash
git add crates/sonde-tx/Cargo.toml crates/sonde-tx/src/lib.rs
git commit -m "feat(sonde-tx): inject FloorRate14Codec into encode_payload (real FEC on TX)

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task C2: Inject `FloorRate14Codec` in `sonde-rx`

**Files:**
- Modify: `crates/sonde-rx/Cargo.toml` (add `sonde-fec` dep)
- Modify: `crates/sonde-rx/src/lib.rs:189,194,200` (`decode_one_symbol`)

- [ ] **Step 1: Add the dependency** — in `crates/sonde-rx/Cargo.toml` `[dependencies]`:

```toml
sonde-fec.workspace = true
```

- [ ] **Step 2: Write the failing test** — append to the `tests` module in `crates/sonde-rx/src/lib.rs`:

```rust
    #[test]
    fn decode_one_symbol_uses_floor_fec_round_trip() {
        use sonde_fec::codec::FloorRate14Codec;
        use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;
        let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
        let samples = floor.transmit_multi_with_preamble(b"RXFEC").unwrap();
        let out = decode_one_symbol(Mode::WideFloor, &samples, FrameMode::MultiSync).unwrap();
        assert_eq!(out, b"RXFEC");
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p sonde-rx decode_one_symbol_uses_floor_fec`
Expected: FAIL — `decode_one_symbol` builds the floor with `::new()` (IdentityFec); decoding FloorRate14-encoded samples mismatches.

- [ ] **Step 4: Inject the codec** — replace each `WidebandLowDensityFloor::new()` in `decode_one_symbol` (lines 189, 194, 200) with a local helper. Add at the top of the function:

```rust
    let make_floor = || {
        WidebandLowDensityFloor::with_fec(Box::new(sonde_fec::codec::FloorRate14Codec::new()))
    };
```

and use `make_floor()` in place of each `WidebandLowDensityFloor::new()`.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p sonde-rx decode_one_symbol_uses_floor_fec`
Expected: PASS. Then `cargo test -p sonde-rx` — full suite green.

- [ ] **Step 6: Commit**

```bash
git add crates/sonde-rx/Cargo.toml crates/sonde-rx/src/lib.rs
git commit -m "feat(sonde-rx): inject FloorRate14Codec into decode_one_symbol (real FEC on RX)

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

### Task C3: Differential channel-sim gate (the un-fakeable wiring proof)

**Files:**
- Modify: `crates/sonde-tx/Cargo.toml` (`[dev-dependencies]`: `sonde-rx`, `hf-channel-sim`, `num-complex`)
- Create: `crates/sonde-tx/tests/fec_differential_gate.rs`

The gate: take the SAME noisy capture (floor signal through a Watterson channel + AWGN) and show it **fails** to decode with `IdentityFec` but **succeeds** with `FloorRate14Codec`. The signal is real f32 audio; `hf-channel-sim` operates on `Complex<f32>`, so map real↔complex (imag = 0) around `process_block`, then add AWGN, then take the real part for the receiver.

- [ ] **Step 1: Add dev-dependencies** — in `crates/sonde-tx/Cargo.toml`:

```toml
[dev-dependencies]
sonde-rx.workspace = true
hf-channel-sim.workspace = true
num-complex.workspace = true
```

(If `sonde-rx.workspace`/`hf-channel-sim.workspace` are not yet declared in the root `[workspace.dependencies]`, add `sonde-rx = { path = "crates/sonde-rx" }` and confirm `hf-channel-sim = { path = "hf-channel-sim" }` is present — it already is per the root manifest.)

- [ ] **Step 2: Write the gate test** — `crates/sonde-tx/tests/fec_differential_gate.rs`:

```rust
//! Differential FEC gate: the SAME noisy capture must FAIL to decode with
//! the identity (no-FEC) baseline and SUCCEED with the real rate-1/4 LDPC.
//! This is impossible to pass unless FloorRate14Codec is genuinely in the
//! signal path — it is the anti-island proof for the FEC wiring.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::coded_modulation::IdentityFec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

const SAMPLE_RATE: f64 = 48_000.0;

/// Floor signal → Watterson(Moderate) → AWGN at `snr_db` → real samples.
fn impair(clean: &[f32], snr_db: f64, seed: u64) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, ChannelCondition::Moderate, SAMPLE_RATE);
    let cx: Vec<Complex<f32>> = clean.iter().map(|&s| Complex::new(s, 0.0)).collect();
    let mut faded = ch.process_block(&cx);
    AwgnGenerator::new(seed ^ 0xA5A5).add_noise(&mut faded, snr_db);
    faded.iter().map(|c| c.re).collect()
}

#[test]
fn floor_fec_decodes_where_identity_fails() {
    let payload = b"DIFFERENTIAL GATE PAYLOAD";
    let snr_db = -2.0; // below the uncoded floor's usable point
    let seed = 0xC0DE_F100;

    // Coded TX (the operational codec).
    let tx = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let clean = tx.transmit_multi_with_preamble(payload).unwrap();
    let noisy = impair(&clean, snr_db, seed);

    // Baseline: identity (no FEC) cannot recover the payload from this capture.
    let id_rx = WidebandLowDensityFloor::new(); // IdentityFec
    let id_result = id_rx.receive_multi_with_sync(&noisy);
    let identity_ok = matches!(&id_result, Ok((_, p)) if p == payload);
    assert!(
        !identity_ok,
        "IdentityFec unexpectedly recovered the payload at {snr_db} dB — \
         pick a harder SNR so the differential is meaningful"
    );

    // Real FEC: same capture, recovers the payload.
    let fec_rx = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let (_s, decoded) = fec_rx
        .receive_multi_with_sync(&noisy)
        .expect("FloorRate14Codec must decode the impaired capture");
    assert_eq!(decoded, payload, "rate-1/4 LDPC must recover the payload");
}
```

- [ ] **Step 3: Run the gate**

Run: `cargo test -p sonde-tx --test fec_differential_gate`
Expected: PASS. 

**Tuning note (not a placeholder — an execution instruction):** if `identity_ok` is `true` (identity also decoded) lower `snr_db` toward `-4.0`; if the FEC side fails to decode, raise `snr_db` toward `0.0` or reduce payload size. There exists an SNR band where rate-1/4 LDPC closes and uncoded does not — that band is the gate. If no such band is found across `[-6.0, +2.0]`, STOP: it means the coded path or LLR calibration (`receiver.rs:75` `n0`) needs the noise-variance work flagged in design §6 — open a follow-up task to feed a real `n0` estimate into `demodulate_one_symbol`.

- [ ] **Step 4: Full-workspace gates + commit**

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
git add crates/sonde-tx/Cargo.toml crates/sonde-tx/tests/fec_differential_gate.rs Cargo.toml
git commit -m "test(sonde-tx): differential channel-sim gate — rate-1/4 LDPC decodes where IdentityFec fails

Agent: mesa-falcon-basil
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review

**Spec coverage:**
- Component A (working rate-1/4 LDPC + `FloorRate14Codec`) → Tasks A1–A3. ✓
- Component B (dependency-inverted single coded path, soft-LLR preserved, header-in-first-codeword, `block_info_bits()` not hardcoded) → Tasks B1–B3. ✓
- Pipeline injection (`sonde-tx`/`sonde-rx`) → Tasks C1–C2. ✓
- Differential channel-sim gate → Task C3. ✓
- Codex refinement #1 (per-block codeword framing) → B2 `receive_multi` decodes block-by-block. ✓
- Refinement #2 (header inside first codeword, decoded before trusting) → B1 `frame_info_bits` + B2 `blocks_from_first_block` on block 0. ✓
- Refinement #3 (`block_info_bits()`, CRC reduces capacity) → B1/B2 use `self.fec.block_info_bits()`; A3 sets it to 480 = k−32. ✓
- Refinement #4 (n0 calibration only if the gate needs it) → C3 tuning-note escalation. ✓
- Refinement #5 (IdentityFec default guarded) → C1/C2 inject the real codec; C3 proves the operational path is coded. ✓
- Out-of-scope (ARQ stats, OFDM main family, link adaptation) → not present. ✓

**Type consistency:** `with_fec(Box<dyn FecCodec>)`, `FloorRate14Codec::new()`, `block_info_bits()`/`block_coded_bits()`, `frame_info_bits`/`deframe_info_bits`/`blocks_from_first_block`/`blocks_for_payload`/`HEADER_BITS`, `PhyError::FecDecode`, `process_block`, `add_noise` — all used consistently across tasks and match the verified upstream signatures.

**Risk gates:** A1 Step 1 (seed-search convergence) and C3 Step 3 (SNR band) are the two empirical unknowns; both are riskiest-first and fail loudly with explicit escalation instructions rather than silently.
