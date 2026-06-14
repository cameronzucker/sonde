# Sonde Demo Engine — TX/RX Byte Additions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose the actual decoded bytes from the link so the demo frontend can show a real TX | RX packet console (per-symbol sent-vs-received with flip highlighting) and render genuinely corrupted recovered images.

**Architecture:** Add one additive public method to `sonde-phy`'s `WidebandLowDensityFloor` that syncs on the preamble and returns the recovered payload **plus** each symbol's full decoded bytes. Surface those on `sonde-wasm`'s `LinkResult` (`recovered_bytes`) and `SymbolRec` (`rx_bytes`), populated in `run_link_core`. No DSP changes; existing methods/tests untouched.

**Tech Stack:** Rust 2021, existing `sonde-phy` + `sonde-wasm` crates. Host-tested (`cargo test`).

**Scope:** Engine prerequisite for the frontend (spec `2026-06-14-sonde-demo-frontend-design.md` §3). The frontend itself is a separate plan. Tracked under epic sonde-669.

**Working directory:** the isolated worktree `worktrees/sonde-interactive-demo` (branch `sonde-669/interactive-demo`, which contains the engine from PR #4). Do NOT touch `main`. All paths relative to the worktree root.

**Governance:** conventional commits with BOTH trailers (`Agent: <moniker>`, `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`); no destructive git. Pure DSP-accessor + binding work — does not touch `sonde-tx`/PTT, no radio concern.

**Verification gate (each commit):**
```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

---

## File Structure

**Modified:**
- `crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs` — add `DecodedFrame` struct + `receive_multi_detailed()` method (additive; existing methods unchanged).
- `crates/sonde-wasm/src/types.rs` — add `recovered_bytes` to `LinkResult`, `rx_bytes` to `SymbolRec`.
- `crates/sonde-wasm/src/link.rs` — populate the new fields via `receive_multi_detailed`.

`lib.rs`'s `run_link` wrapper needs NO signature change — the JSON output simply gains the
new fields (it already serializes `LinkResult`).

---

## Task 1: `sonde-phy` — per-symbol detailed decode

**Files:**
- Modify: `crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs`

Adds a public `DecodedFrame` and `receive_multi_detailed()` that mirrors
`receive_multi_with_sync` but also returns each symbol's full decoded bytes (no trailing-zero
trim — uses the existing private `decode_symbol_bytes`). Existing methods are not modified.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `wideband_lowdensity.rs`:
```rust
    #[test]
    fn receive_multi_detailed_clean_returns_payload_and_per_symbol_bytes() {
        let floor = WidebandLowDensityFloor::new();
        let cap = floor.data_bytes_per_symbol();
        let payload: Vec<u8> = (0..20).map(|i| (i % 251) as u8).collect();
        let samples = floor.transmit_multi_with_preamble(&payload).unwrap();
        let frame = floor.receive_multi_detailed(&samples).unwrap();
        assert_eq!(frame.preamble_start, 0);
        assert_eq!(frame.payload, payload);
        // stream = 2-byte len header + 20 payload = 22 bytes -> ceil(22/cap) symbols.
        let expected_symbols = (2 + payload.len()).div_ceil(cap);
        assert_eq!(frame.symbols.len(), expected_symbols);
        // Each per-symbol vec is exactly `cap` bytes (full, untrimmed).
        assert!(frame.symbols.iter().all(|s| s.len() == cap));
        // Concatenated per-symbol bytes, minus the 2-byte header, truncated to
        // declared length, must equal the payload.
        let mut stream: Vec<u8> = frame.symbols.concat();
        let recovered = stream.split_off(2);
        assert_eq!(&recovered[..payload.len()], payload.as_slice());
    }

    #[test]
    fn receive_multi_detailed_rejects_silence() {
        let floor = WidebandLowDensityFloor::new();
        let silence = vec![0.0_f32; 20_000];
        assert!(matches!(
            floor.receive_multi_detailed(&silence),
            Err(PhyError::FrameDetect(_))
        ));
    }

    #[test]
    fn receive_multi_detailed_matches_receive_multi_with_sync_payload() {
        // The detailed path must agree with the existing path on the payload.
        let floor = WidebandLowDensityFloor::new();
        let payload: Vec<u8> = (0..100).map(|i| (i * 3 % 251) as u8).collect();
        let samples = floor.transmit_multi_with_preamble(&payload).unwrap();
        let (_s, p1) = floor.receive_multi_with_sync(&samples).unwrap();
        let frame = floor.receive_multi_detailed(&samples).unwrap();
        assert_eq!(p1, frame.payload);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p sonde-phy receive_multi_detailed`
Expected: FAIL — `DecodedFrame` / `receive_multi_detailed` not found.

- [ ] **Step 3: Implement `DecodedFrame` + `receive_multi_detailed`**

Add the struct just above `impl WidebandLowDensityFloor` (after `PREAMBLE_LEN_SAMPLES`):
```rust
/// Result of [`WidebandLowDensityFloor::receive_multi_detailed`]: the recovered
/// payload plus every symbol's full decoded bytes (untrimmed, `data_bytes_per_symbol`
/// each), for callers (e.g. demo/visualization) that need per-symbol RX data.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Sample index where the preamble was detected.
    pub preamble_start: usize,
    /// The recovered payload (length-prefixed framing stripped + truncated).
    pub payload: Vec<u8>,
    /// Per-symbol decoded bytes in transmit order (each `data_bytes_per_symbol`
    /// long, including the 2-byte length header in symbol 0 and any trailing pad).
    pub symbols: Vec<Vec<u8>>,
}
```
Add the method inside `impl WidebandLowDensityFloor` (e.g. after `receive_multi`):
```rust
    /// Like [`Self::receive_multi_with_sync`], but also returns each symbol's
    /// full decoded bytes. Additive convenience for per-symbol RX inspection;
    /// the payload result is identical to `receive_multi_with_sync`.
    pub fn receive_multi_detailed(&self, samples: &[f32]) -> Result<DecodedFrame, PhyError> {
        let detector = PreambleDetector::new();
        let detection = detector.scan(samples).ok_or_else(|| {
            PhyError::FrameDetect(
                "preamble not detected in input (signal too weak or no preamble present)"
                    .to_string(),
            )
        })?;
        let body_start = detection.start_sample + PREAMBLE_LEN_SAMPLES;
        if body_start >= samples.len() {
            return Err(PhyError::FrameDetect(format!(
                "preamble detected at sample {} but no body samples follow",
                detection.start_sample
            )));
        }
        let body = &samples[body_start..];
        let symbol_size = self.symbol_size_samples();
        let cap = self.data_bytes_per_symbol();
        if body.len() < symbol_size {
            return Err(PhyError::FrameDetect(format!(
                "input shorter than one symbol: have {} samples, need {symbol_size}",
                body.len()
            )));
        }
        let first = self.decode_symbol_bytes(&body[..symbol_size]);
        if first.len() < 2 {
            return Err(PhyError::FrameDetect(
                "first symbol decoded fewer than 2 bytes — cannot read length header".into(),
            ));
        }
        let declared_len = ((first[0] as usize) << 8) | (first[1] as usize);
        let total_len = 2 + declared_len;
        let symbols_needed = total_len.div_ceil(cap);
        if body.len() < symbols_needed * symbol_size {
            return Err(PhyError::FrameDetect(format!(
                "declared length {declared_len} requires {symbols_needed} symbols, \
                 have {} samples",
                body.len()
            )));
        }
        let mut symbols: Vec<Vec<u8>> = Vec::with_capacity(symbols_needed);
        let mut stream: Vec<u8> = Vec::with_capacity(symbols_needed * cap);
        symbols.push(first.clone());
        stream.extend_from_slice(&first);
        for s in 1..symbols_needed {
            let start = s * symbol_size;
            let chunk = self.decode_symbol_bytes(&body[start..start + symbol_size]);
            stream.extend_from_slice(&chunk);
            symbols.push(chunk);
        }
        let payload = stream[2..2 + declared_len].to_vec();
        Ok(DecodedFrame {
            preamble_start: detection.start_sample,
            payload,
            symbols,
        })
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sonde-phy receive_multi_detailed`
Expected: 3 PASS.
Then: `cargo clippy -p sonde-phy --all-targets -- -D warnings` → clean.

- [ ] **Step 5: Commit**
```bash
git add crates/sonde-phy/src/robustness_floor/wideband_lowdensity.rs
git commit  # subject: "feat(sonde-phy): receive_multi_detailed for per-symbol RX bytes"; include both trailers
```

---

## Task 2: `sonde-wasm` — add `recovered_bytes` + `rx_bytes` to types

**Files:**
- Modify: `crates/sonde-wasm/src/types.rs`

- [ ] **Step 1: Add the fields**

In `SymbolRec`, add after `bytes: Vec<u8>` (keep the existing doc comment on `bytes`; update it to say "transmitted/ground-truth"):
```rust
    /// The bytes actually DECODED for this symbol on the RX side (same length
    /// as `bytes` when decode succeeded; empty when the frame failed to sync).
    pub rx_bytes: Vec<u8>,
```
In `LinkResult`, add after `payload_len: usize`:
```rust
    /// The full payload actually recovered on the RX side. Equals the original
    /// payload at high SNR (BER 0); differs (corrupted) at marginal SNR; empty
    /// when the frame failed to sync.
    pub recovered_bytes: Vec<u8>,
```

- [ ] **Step 2: Verify it compiles (will fail to build link.rs until Task 3)**

Run: `cargo build -p sonde-wasm 2>&1 | head -20`
Expected: a compile error in `link.rs` about missing `rx_bytes` / `recovered_bytes` in the
struct literals (that's expected — Task 3 fills them). The `types.rs` change itself is valid.
Do NOT commit yet — Task 3 makes the crate build again (keep the tree green per commit).

(No standalone commit for this task — it is committed together with Task 3 since the crate
does not compile in between.)

---

## Task 3: `sonde-wasm` — populate the new fields in `run_link_core`

**Files:**
- Modify: `crates/sonde-wasm/src/link.rs`

- [ ] **Step 1: Add the failing tests**

Add to the `#[cfg(test)] mod tests` block in `link.rs`:
```rust
    #[test]
    fn clean_link_exposes_recovered_and_rx_bytes() {
        let payload: Vec<u8> = (0..120).map(|i| (i % 251) as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "none", 1).unwrap();
        // Whole recovered payload matches at high SNR.
        assert_eq!(r.recovered_bytes, payload);
        // Every symbol has rx_bytes equal in length to its tx bytes, and on the
        // clean path equal in value too.
        for s in &r.symbols {
            assert_eq!(s.rx_bytes.len(), s.bytes.len());
            assert_eq!(s.rx_bytes, s.bytes, "clean path: rx should equal tx for sym {}", s.idx);
        }
    }

    #[test]
    fn failed_sync_yields_empty_recovered_and_rx() {
        // Poor multipath at low SNR: the bare floor receiver fails to sync.
        let payload: Vec<u8> = (0..120).map(|i| (i % 251) as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", -6.0, "poor", 3).unwrap();
        // (Determinism: if this seed/SNR ever DOES sync, pick another in-test;
        // the invariant is: !recovered_ok => empty recovered_bytes + empty rx.)
        if !r.recovered_ok {
            assert!(r.recovered_bytes.is_empty());
            assert!(r.symbols.iter().all(|s| s.rx_bytes.is_empty()));
        }
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p sonde-wasm clean_link_exposes_recovered`
Expected: FAIL to compile (fields not populated / not present) — consistent with Task 2.

- [ ] **Step 3: Implement — switch to `receive_multi_detailed` and populate fields**

In `link.rs`, update the imports to bring in `DecodedFrame` is NOT needed (we only call the
method). Replace the decode block and the symbol assembly in `run_link_core`.

Current decode block:
```rust
    // Decode.
    let (recovered_ok, ber) = match floor.receive_multi_with_sync(&observed_real) {
        Ok((_start, recovered)) => {
            let ok = recovered == payload;
            (ok, bit_error_rate(&recovered, payload))
        }
        Err(_) => (false, 1.0),
    };
```
Replace with:
```rust
    // Decode (detailed: payload + per-symbol RX bytes).
    let (recovered_ok, ber, recovered_bytes, rx_symbols): (bool, f32, Vec<u8>, Vec<Vec<u8>>) =
        match floor.receive_multi_detailed(&observed_real) {
            Ok(frame) => {
                let ok = frame.payload == payload;
                let ber = bit_error_rate(&frame.payload, payload);
                (ok, ber, frame.payload, frame.symbols)
            }
            Err(_) => (false, 1.0, Vec::new(), Vec::new()),
        };
```
Then, where `symbols` is built, attach the RX bytes. Current line:
```rust
    let symbols = build_symbols(payload, cap, symbol_size, offsets);
```
Replace with:
```rust
    let mut symbols = build_symbols(payload, cap, symbol_size, offsets);
    // Attach per-symbol RX bytes (aligned by index; empty when sync failed).
    for (i, sym) in symbols.iter_mut().enumerate() {
        sym.rx_bytes = rx_symbols.get(i).cloned().unwrap_or_default();
    }
```
In `build_symbols`, the `SymbolRec` literal must now include `rx_bytes`. Add to the literal
(it is overwritten in the loop above, so initialize empty):
```rust
            bytes: chunk.to_vec(),
            rx_bytes: Vec::new(),
            byte_start: payload_byte,
```
Finally add `recovered_bytes` to the `LinkResult { .. }` construction (after `payload_len`):
```rust
        payload_len: payload.len(),
        recovered_bytes,
        preamble_samples: PREAMBLE_LEN_SAMPLES,
```
No `use` change is needed — `receive_multi_with_sync`/`receive_multi_detailed` are methods
called on the `floor` value, not imported names (the `use` only brings in
`WidebandLowDensityFloor` + `PREAMBLE_LEN_SAMPLES`). Just confirm `cargo clippy -D warnings`
is clean (no unused variables introduced).

- [ ] **Step 4: Run tests + full gate**

Run: `cargo test -p sonde-wasm`
Expected: ALL pass (existing + 2 new).
Then: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: green. (`cargo test --workspace` builds `sonde-rx`/`sonde-tx` → needs ALSA; the host has `libasound2-dev`. If it ever fails to link ALSA, scope to `-p sonde-phy -p sonde-wasm` and note it.)

- [ ] **Step 5: Commit (Tasks 2 + 3 together — the crate compiles again here)**
```bash
git add crates/sonde-wasm/src/types.rs crates/sonde-wasm/src/link.rs
git commit  # subject: "feat(sonde-demo): expose recovered_bytes + per-symbol rx_bytes on LinkResult"; include both trailers
```

---

## Task 4: Verify the JSON surface + wasm32 build

**Files:** none (verification only) — optionally extend `crates/sonde-wasm/src/lib.rs` tests.

- [ ] **Step 1: Add a shim test asserting the JSON carries the new fields**

Add to the `#[cfg(test)] mod tests` in `crates/sonde-wasm/src/lib.rs`:
```rust
    #[test]
    fn run_link_json_includes_recovered_and_rx_bytes() {
        let payload: Vec<u8> = (0..60).map(|i| i as u8).collect();
        let offsets = serde_json::json!({
            "total_len": payload.len(),
            "fields": [{"label":"image","start":0,"end":payload.len()}],
            "image_byte_len": payload.len()
        })
        .to_string();
        let json = run_link(&payload, &offsets, "floor-wblo", 80.0, "none", 1);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["recovered_bytes"].is_array());
        assert_eq!(v["recovered_bytes"].as_array().unwrap().len(), payload.len());
        assert!(v["symbols"][0]["rx_bytes"].is_array());
    }
```

- [ ] **Step 2: Run + gate + wasm build**

Run: `cargo test -p sonde-wasm --lib run_link_json_includes`
Expected: PASS.
Run: `cargo build -p sonde-wasm --target wasm32-unknown-unknown`
Expected: compiles (no new deps; should be unaffected).
Run the full gate: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`

- [ ] **Step 3: Commit**
```bash
git add crates/sonde-wasm/src/lib.rs
git commit  # subject: "test(sonde-demo): assert LinkResult JSON carries recovered/rx bytes"; include both trailers
```

---

## Self-Review notes
- `SymbolRec.bytes` (TX ground truth) and `rx_bytes` (decoded) are index-aligned and equal
  length (`cap`), so the frontend can byte-diff them directly for flip highlighting.
- `receive_multi_detailed` is additive; `receive_multi`/`receive_multi_with_sync` and all
  their tests are untouched, minimizing conflict with the parallel framing refactor.
- On sync failure, `recovered_bytes` and every `rx_bytes` are empty — the frontend uses
  that to show the "decode failed" state.
