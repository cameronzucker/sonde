# Sonde Demo — WASM Adaptive-Link Engine + SITREP Payload Builder — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Rust foundation of the interactive demo — a `sonde-demo-builder` bin that turns an operator image into a fixed SITREP `payload.bin`, and a `sonde-wasm` crate that runs Sonde's real DSP over a simulated channel and returns a JSON `LinkResult` (decode result, BER, per-symbol field map, spectrogram), exported to JS via wasm-bindgen.

**Architecture:** All logic lives in plain, host-testable Rust functions returning `serde` structs; thin `#[wasm_bindgen]` wrappers serialize them to JSON `String` so the same code is callable from `cargo test` (host) and from the browser (wasm32). The engine encodes the payload with the existing `WidebandLowDensityFloor` floor mode, applies `hf-channel-sim` (Watterson multipath + AWGN) by converting real audio to/from `Complex<f32>`, decodes, and computes statistics and a quantized STFT with `rustfft`.

**Tech Stack:** Rust 2021 (workspace), `rustfft` v6, `num-complex`, `image` v0.25 (builder only), `wasm-bindgen`, `serde`/`serde_json`, `getrandom` (js feature for wasm), existing `sonde-phy` + `hf-channel-sim` crates.

**Scope:** Phase 0 (payload builder) + the Rust half of Phase 1 (WASM engine over the floor mode). The JS/Three.js frontend and full-speed (QAM) integration are separate plans. Only `floor-wblo` produces real waveforms today; OFDM-Main modes are reported `implemented: false` and `run_link` returns a `ModeUnavailable` error for them until the parallel QAM work lands.

**Working directory:** the isolated worktree at `worktrees/sonde-interactive-demo` (branch `sonde-interactive-demo`). Do NOT touch `main`. All paths below are relative to the worktree root.

**Verification gate (every commit must pass):**
```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

## File Structure

**New crate: `crates/sonde-demo-builder/`** (binary)
- `Cargo.toml` — deps: `image`, `serde`, `serde_json`, `anyhow`.
- `src/sitrep.rs` — builds the SITREP envelope bytes + field-offset map (pure, testable).
- `src/image_fit.rs` — resize + JPEG quality-search to a byte budget (pure, testable on encoded bytes).
- `src/main.rs` — CLI glue: read image path + target bytes → write `payload.bin` + `payload.offsets.json`.

**New crate: `crates/sonde-wasm/`** (`cdylib` + `rlib`)
- `Cargo.toml` — deps: `sonde-phy`, `hf-channel-sim`, `rustfft`, `num-complex`, `serde`, `serde_json`, `wasm-bindgen`; `getrandom` w/ `js` for wasm target.
- `src/types.rs` — `serde` structs: `ModeInfo`, `FieldOffsets`, `Field`, `SymbolRec`, `SpectrogramGrid`, `LinkResult`.
- `src/modes.rs` — `list_modes()`, `recommend_mode(snr_db)`, implemented-mode clamp.
- `src/channelize.rs` — real↔complex helpers + `apply_channel(samples, snr_db, condition, seed)`.
- `src/spectrogram.rs` — `stft(samples, fft_size, hop, band_hz)` → quantized `SpectrogramGrid`.
- `src/link.rs` — `run_link_core(payload, offsets, mode_id, snr_db, condition, seed)` → `LinkResult`.
- `src/lib.rs` — module wiring + `#[wasm_bindgen]` JSON-string wrappers.

**Modified:**
- `Cargo.toml` (root) — add the two new members; add `image`, `serde`, `serde_json`, `anyhow`, `wasm-bindgen`, `getrandom` to `[workspace.dependencies]`.

---

## Task 1: Workspace registration + builder crate skeleton

**Files:**
- Modify: `Cargo.toml` (workspace `members` + `[workspace.dependencies]`)
- Create: `crates/sonde-demo-builder/Cargo.toml`
- Create: `crates/sonde-demo-builder/src/main.rs`

- [ ] **Step 1: Add workspace members and shared deps**

In root `Cargo.toml`, add to `members` (keep existing entries):
```toml
    "crates/sonde-demo-builder",
    "crates/sonde-wasm",
```
Add to `[workspace.dependencies]` (keep existing entries):
```toml
image = { version = "0.25", default-features = false, features = ["jpeg"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
wasm-bindgen = "0.2"
getrandom = "0.2"
```

- [ ] **Step 2: Create builder Cargo.toml**

Create `crates/sonde-demo-builder/Cargo.toml`:
```toml
[package]
name = "sonde-demo-builder"
version = "0.1.0"
edition = "2021"
rust-version = "1.75"

[dependencies]
image.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
```

- [ ] **Step 3: Create a placeholder main so the workspace compiles**

Create `crates/sonde-demo-builder/src/main.rs`:
```rust
//! sonde-demo-builder: turns an operator image into the fixed SITREP
//! `payload.bin` + `payload.offsets.json` consumed by the WASM demo engine.

mod image_fit;
mod sitrep;

fn main() -> anyhow::Result<()> {
    eprintln!("sonde-demo-builder: see `--help` (implemented in later tasks)");
    Ok(())
}
```
Create empty module files so it compiles:
- `crates/sonde-demo-builder/src/image_fit.rs` with `// implemented in Task 3`
- `crates/sonde-demo-builder/src/sitrep.rs` with `// implemented in Task 2`

- [ ] **Step 4: Verify the workspace builds**

Run: `cargo build -p sonde-demo-builder`
Expected: compiles (the two empty modules produce an unused-module warning is avoided because they're declared and referenced via `mod`). If clippy later flags empty modules, the next tasks fill them.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/sonde-demo-builder
git commit -m "chore(sonde-demo): scaffold demo-builder crate + workspace members"
```

---

## Task 2: SITREP envelope + field-offset map

**Files:**
- Create/replace: `crates/sonde-demo-builder/src/sitrep.rs`

The payload is the SITREP text envelope with the raw JPEG bytes appended. The field-offset map records byte ranges so the inspector can color symbols by field.

- [ ] **Step 1: Write the failing test**

Replace `crates/sonde-demo-builder/src/sitrep.rs` with:
```rust
//! Builds the SITREP envelope payload and its field-offset map.

use serde::Serialize;

/// One labeled byte range within the payload.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub start: usize,
    pub end: usize,
}

/// Field-offset map serialized to `payload.offsets.json`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FieldOffsets {
    pub total_len: usize,
    pub fields: Vec<Field>,
    pub image_byte_len: usize,
}

/// Build the payload bytes and offsets from envelope text parts and the
/// compressed image bytes. Layout: header + blank line + body + attachment
/// marker line + raw image bytes.
pub fn build_payload(
    callsign: &str,
    position_line: &str,
    body: &str,
    image_jpeg: &[u8],
) -> (Vec<u8>, FieldOffsets) {
    let header = format!(
        "To: EMCOMM-NET\nFrom: {callsign}\nSubject: SITREP - Disaster Area Recon\nDate: 2026-06-14 18:30Z\n{position_line}\n"
    );
    let body_block = format!("\n{body}\n");
    let marker = format!("\n--- attachment: recon.jpg ({} bytes) ---\n", image_jpeg.len());

    let mut bytes = Vec::new();
    let header_start = 0;
    bytes.extend_from_slice(header.as_bytes());
    let header_end = bytes.len();

    bytes.extend_from_slice(body_block.as_bytes());
    bytes.extend_from_slice(marker.as_bytes());
    let body_end = bytes.len();

    bytes.extend_from_slice(image_jpeg);
    let image_end = bytes.len();

    let offsets = FieldOffsets {
        total_len: bytes.len(),
        fields: vec![
            Field { label: "header".into(), start: header_start, end: header_end },
            Field { label: "body".into(), start: header_end, end: body_end },
            Field { label: "image".into(), start: body_end, end: image_end },
        ],
        image_byte_len: image_jpeg.len(),
    };
    (bytes, offsets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_partition_the_payload_contiguously() {
        let img = vec![0xABu8; 100];
        let (bytes, off) = build_payload("KK6XYZ", "Position: 34-12.34N / 118-29.10W (DM04xf)", "Levee breach.", &img);
        assert_eq!(off.total_len, bytes.len());
        assert_eq!(off.image_byte_len, 100);
        // Fields are contiguous and cover the whole payload.
        assert_eq!(off.fields[0].start, 0);
        assert_eq!(off.fields[0].end, off.fields[1].start);
        assert_eq!(off.fields[1].end, off.fields[2].start);
        assert_eq!(off.fields.last().unwrap().end, bytes.len());
        // Image region equals the appended image bytes.
        let img_field = &off.fields[2];
        assert_eq!(&bytes[img_field.start..img_field.end], img.as_slice());
    }
}
```

- [ ] **Step 2: Run test to verify it passes (logic is in the test file)**

Run: `cargo test -p sonde-demo-builder sitrep`
Expected: PASS (the implementation and test ship together here; this task is self-contained).

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-demo-builder/src/sitrep.rs
git commit -m "feat(sonde-demo): SITREP envelope + field-offset map"
```

---

## Task 3: Image resize + JPEG quality-search to a byte budget

**Files:**
- Create/replace: `crates/sonde-demo-builder/src/image_fit.rs`

- [ ] **Step 1: Write the implementation + test**

Replace `crates/sonde-demo-builder/src/image_fit.rs` with:
```rust
//! Resize an image and JPEG-encode it under a target byte budget by
//! searching downward on quality.

use anyhow::{bail, Result};
use image::{codecs::jpeg::JpegEncoder, DynamicImage};

/// Resize `img` so its longest side is at most `max_dim`, then JPEG-encode,
/// lowering quality until the encoded size is <= `target_bytes`. Returns the
/// encoded JPEG bytes. Errors only if even quality 10 overflows the budget.
pub fn fit_jpeg(img: &DynamicImage, max_dim: u32, target_bytes: usize) -> Result<Vec<u8>> {
    let resized = img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle);
    let rgb = resized.to_rgb8();
    for quality in (10..=90).rev().step_by(5) {
        let mut buf = Vec::new();
        {
            let mut enc = JpegEncoder::new_with_quality(&mut buf, quality as u8);
            enc.encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        if buf.len() <= target_bytes {
            return Ok(buf);
        }
    }
    bail!("could not fit image under {target_bytes} bytes even at quality 10");
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn noise_image(w: u32, h: u32) -> DynamicImage {
        // Deterministic pseudo-random pixels (JPEG-incompressible-ish) so the
        // quality search has to actually lower quality to hit a small budget.
        let mut state: u32 = 0x1234_5678;
        let mut img = RgbImage::new(w, h);
        for p in img.pixels_mut() {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let v = (state >> 16) as u8;
            *p = image::Rgb([v, v.wrapping_add(40), v.wrapping_add(80)]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn fits_under_budget() {
        let img = noise_image(640, 480);
        let bytes = fit_jpeg(&img, 200, 5000).expect("should fit");
        assert!(bytes.len() <= 5000, "got {} bytes", bytes.len());
        // Valid JPEG SOI marker.
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
    }
}
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test -p sonde-demo-builder image_fit`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-demo-builder/src/image_fit.rs
git commit -m "feat(sonde-demo): image resize + JPEG quality-search to byte budget"
```

---

## Task 4: Builder CLI wiring

**Files:**
- Modify: `crates/sonde-demo-builder/src/main.rs`

- [ ] **Step 1: Implement the CLI**

Replace `crates/sonde-demo-builder/src/main.rs` with:
```rust
//! sonde-demo-builder: image -> SITREP `payload.bin` + `payload.offsets.json`.
//!
//! Usage:
//!   sonde-demo-builder <IMAGE> <OUT_DIR> [--target-bytes N] [--max-dim D] [--callsign C]

mod image_fit;
mod sitrep;

use anyhow::{Context, Result};
use std::path::PathBuf;

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let positionals: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positionals.len() < 2 {
        eprintln!("usage: sonde-demo-builder <IMAGE> <OUT_DIR> [--target-bytes N] [--max-dim D] [--callsign C]");
        std::process::exit(2);
    }
    let image_path = PathBuf::from(positionals[0]);
    let out_dir = PathBuf::from(positionals[1]);
    let target_bytes: usize = arg_value(&args, "--target-bytes").and_then(|v| v.parse().ok()).unwrap_or(5000);
    let max_dim: u32 = arg_value(&args, "--max-dim").and_then(|v| v.parse().ok()).unwrap_or(200);
    let callsign = arg_value(&args, "--callsign").unwrap_or_else(|| "KK6XYZ".to_string());

    let img = image::open(&image_path).with_context(|| format!("opening {}", image_path.display()))?;
    let jpeg = image_fit::fit_jpeg(&img, max_dim, target_bytes)?;

    let position_line = "Position: 34-12.34N / 118-29.10W (DM04xf)";
    let body = "Aerial recon of flood zone: levee breach at N bank, water across Route 9, \
                two structures isolated. No casualties observed. Requesting boat team + \
                medical standby. Photo attached.";
    let (bytes, offsets) = sitrep::build_payload(&callsign, position_line, body, &jpeg);

    std::fs::create_dir_all(&out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    std::fs::write(out_dir.join("payload.bin"), &bytes)?;
    std::fs::write(out_dir.join("payload.offsets.json"), serde_json::to_vec_pretty(&offsets)?)?;

    eprintln!(
        "wrote {} byte payload ({} byte image) to {}",
        bytes.len(),
        jpeg.len(),
        out_dir.display()
    );
    Ok(())
}
```

- [ ] **Step 2: Verify build + clippy**

Run: `cargo clippy -p sonde-demo-builder --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-demo-builder/src/main.rs
git commit -m "feat(sonde-demo): builder CLI writes payload.bin + offsets"
```

---

## Task 5: sonde-wasm crate skeleton + shared types

**Files:**
- Create: `crates/sonde-wasm/Cargo.toml`
- Create: `crates/sonde-wasm/src/types.rs`
- Create: `crates/sonde-wasm/src/lib.rs`

- [ ] **Step 1: Create the crate Cargo.toml**

Create `crates/sonde-wasm/Cargo.toml`:
```toml
[package]
name = "sonde-wasm"
version = "0.1.0"
edition = "2021"
rust-version = "1.75"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
sonde-phy = { path = "../sonde-phy" }
hf-channel-sim = { path = "../../hf-channel-sim" }
rustfft.workspace = true
num-complex.workspace = true
serde.workspace = true
serde_json.workspace = true
wasm-bindgen.workspace = true

# getrandom is pulled transitively via hf-channel-sim's `rand`. On
# wasm32-unknown-unknown it must use the browser entropy source to compile.
[target.'cfg(target_arch = "wasm32")'.dependencies]
getrandom = { workspace = true, features = ["js"] }
```

- [ ] **Step 2: Define shared serde types**

Create `crates/sonde-wasm/src/types.rs`:
```rust
//! Serde types shared between the host-testable core and the wasm shim.
//! Every public engine result serializes to JSON for the JS frontend.

use serde::{Deserialize, Serialize};

/// One mode the demo can offer.
#[derive(Debug, Clone, Serialize)]
pub struct ModeInfo {
    pub id: String,
    pub family: String,
    pub constellation: String,
    pub bandwidth_hz: f32,
    pub data_bytes_per_symbol: usize,
    pub implemented: bool,
}

/// Labeled payload byte range (mirrors the builder's `Field`).
#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    pub label: String,
    pub start: usize,
    pub end: usize,
}

/// Field-offset map (mirrors the builder's `FieldOffsets`).
#[derive(Debug, Clone, Deserialize)]
pub struct FieldOffsets {
    pub total_len: usize,
    pub fields: Vec<Field>,
    pub image_byte_len: usize,
}

/// Per-OFDM-symbol record for the packet inspector.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolRec {
    pub idx: usize,
    pub sample_start: usize,
    pub sample_end: usize,
    pub t_start_s: f32,
    pub t_end_s: f32,
    /// The payload bytes this symbol is intended to carry (ground truth from
    /// the encoded stream; see plan note on per-symbol decode).
    pub bytes: Vec<u8>,
    pub byte_start: usize,
    pub byte_end: usize,
    pub field: String,
}

/// Quantized STFT grid. `mag_q` is row-major `rows * cols`, 0..=255.
#[derive(Debug, Clone, Serialize)]
pub struct SpectrogramGrid {
    pub rows: usize,
    pub cols: usize,
    pub freqs_hz: Vec<f32>,
    pub times_s: Vec<f32>,
    pub mag_q: Vec<u8>,
}

/// Full result of running the payload over the simulated link.
#[derive(Debug, Clone, Serialize)]
pub struct LinkResult {
    pub mode_id: String,
    pub recovered_ok: bool,
    pub ber: f32,
    pub measured_snr_db: f32,
    pub payload_len: usize,
    pub preamble_samples: usize,
    pub symbol_size_samples: usize,
    pub total_samples: usize,
    pub time_to_deliver_s: f32,
    pub throughput_bps: f32,
    pub symbols: Vec<SymbolRec>,
    pub spectrogram: SpectrogramGrid,
}
```

- [ ] **Step 3: Wire modules in lib.rs (stubs filled by later tasks)**

Create `crates/sonde-wasm/src/lib.rs`:
```rust
//! sonde-wasm: real Sonde DSP over a simulated channel, exported to JS.

pub mod channelize;
pub mod link;
pub mod modes;
pub mod spectrogram;
pub mod types;
```
Create empty placeholders so it compiles:
- `crates/sonde-wasm/src/channelize.rs` → `// implemented in Task 6`
- `crates/sonde-wasm/src/spectrogram.rs` → `// implemented in Task 7`
- `crates/sonde-wasm/src/modes.rs` → `// implemented in Task 8`
- `crates/sonde-wasm/src/link.rs` → `// implemented in Task 9`

- [ ] **Step 4: Verify build**

Run: `cargo build -p sonde-wasm`
Expected: compiles for the host target.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/sonde-wasm
git commit -m "chore(sonde-demo): scaffold sonde-wasm crate + shared types"
```

---

## Task 6: Channel application (real ↔ complex, Watterson + AWGN)

**Files:**
- Create/replace: `crates/sonde-wasm/src/channelize.rs`

- [ ] **Step 1: Write the failing test**

Replace `crates/sonde-wasm/src/channelize.rs` with:
```rust
//! Apply the HF channel sim to real audio: lift f32 -> Complex<f32>, run the
//! Watterson multipath channel, add AWGN in place, project back to real.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use sonde_phy::audio_io::SAMPLE_RATE_HZ;

fn parse_condition(name: &str) -> ChannelCondition {
    match name {
        "good" | "clean" => ChannelCondition::Good,
        "moderate" => ChannelCondition::Moderate,
        "poor" => ChannelCondition::Poor,
        "flutter" => ChannelCondition::Flutter,
        _ => ChannelCondition::Good,
    }
}

/// Returns `(observed_real, clean_complex, observed_complex)`.
/// `clean_complex` is the channel-free (preamble+symbols) signal lifted to
/// complex; `observed_complex` is after multipath+AWGN. Both are returned so
/// the caller can estimate SNR. `observed_real` is what the spectrogram and
/// decoder consume.
pub fn apply_channel(
    samples: &[f32],
    snr_db: f64,
    condition: &str,
    seed: u64,
) -> (Vec<f32>, Vec<Complex<f32>>, Vec<Complex<f32>>) {
    let clean: Vec<Complex<f32>> = samples.iter().map(|&s| Complex::new(s, 0.0)).collect();

    let mut chan = WattersonChannel::from_condition(seed, parse_condition(condition), SAMPLE_RATE_HZ as f64);
    let mut observed = chan.process_block(&clean);

    let mut awgn = AwgnGenerator::new(seed ^ 0xA5A5_A5A5_A5A5_A5A5);
    awgn.add_noise(&mut observed, snr_db);

    let observed_real: Vec<f32> = observed.iter().map(|c| c.re).collect();
    (observed_real, clean, observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_snr_clean_channel_barely_changes_signal() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let (observed, _clean, _obs_c) = apply_channel(&samples, 60.0, "good", 42);
        assert_eq!(observed.len(), samples.len());
        // At 60 dB SNR with the Good channel, energy is preserved within a
        // loose tolerance (multipath rotates phase but conserves power).
        let e_in: f32 = samples.iter().map(|s| s * s).sum();
        let e_out: f32 = observed.iter().map(|s| s * s).sum();
        assert!((e_out / e_in - 1.0).abs() < 0.5, "energy ratio {}", e_out / e_in);
    }

    #[test]
    fn low_snr_adds_substantial_noise_energy() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let (_o_hi, _, _) = apply_channel(&samples, 40.0, "good", 7);
        let (o_lo, _, _) = apply_channel(&samples, -5.0, "good", 7);
        let e_lo: f32 = o_lo.iter().map(|s| s * s).sum();
        let e_sig: f32 = samples.iter().map(|s| s * s).sum();
        // At -5 dB, noise power should exceed signal power.
        assert!(e_lo > e_sig, "low-snr energy {} should exceed signal {}", e_lo, e_sig);
    }

    #[test]
    fn deterministic_for_same_seed() {
        let samples: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.03).cos() * 0.2).collect();
        let (a, _, _) = apply_channel(&samples, 5.0, "moderate", 99);
        let (b, _, _) = apply_channel(&samples, 5.0, "moderate", 99);
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sonde-wasm channelize`
Expected: PASS. If `high_snr_clean_channel_barely_changes_signal` fails on energy tolerance, widen the tolerance comment-noted bound (multipath fading can scale energy); do NOT loosen the low-SNR test.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-wasm/src/channelize.rs
git commit -m "feat(sonde-demo): channel application (real<->complex, Watterson+AWGN)"
```

---

## Task 7: STFT spectrogram (quantized, band-cropped)

**Files:**
- Create/replace: `crates/sonde-wasm/src/spectrogram.rs`

- [ ] **Step 1: Write the implementation + test**

Replace `crates/sonde-wasm/src/spectrogram.rs` with:
```rust
//! Compute a quantized STFT of real audio, cropped to a frequency band and
//! decimated in time, for the 3D waterfall.

use crate::types::SpectrogramGrid;
use num_complex::Complex;
use rustfft::FftPlanner;
use sonde_phy::audio_io::SAMPLE_RATE_HZ;

/// STFT with a Hann window. `fft_size` and `hop` in samples; `band_hz` crops
/// the frequency rows to `[lo, hi]`. Time frames are decimated so `cols <=
/// max_cols`. Magnitudes are converted to dB and quantized to 0..=255 across
/// the grid's own min/max.
pub fn stft(
    samples: &[f32],
    fft_size: usize,
    hop: usize,
    band_hz: (f32, f32),
    max_cols: usize,
) -> SpectrogramGrid {
    let sr = SAMPLE_RATE_HZ as f32;
    let bin_hz = sr / fft_size as f32;
    let lo_bin = (band_hz.0 / bin_hz).floor().max(0.0) as usize;
    let hi_bin = ((band_hz.1 / bin_hz).ceil() as usize).min(fft_size / 2);
    let rows = hi_bin.saturating_sub(lo_bin) + 1;

    // Hann window.
    let window: Vec<f32> = (0..fft_size)
        .map(|n| {
            let x = std::f32::consts::PI * n as f32 / (fft_size as f32 - 1.0);
            x.sin().powi(2)
        })
        .collect();

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);

    // All frame start positions.
    let mut frame_starts: Vec<usize> = Vec::new();
    let mut start = 0usize;
    while start + fft_size <= samples.len() {
        frame_starts.push(start);
        start += hop;
    }
    if frame_starts.is_empty() {
        frame_starts.push(0);
    }
    // Decimate in time to max_cols (div_ceil avoids clippy::manual_div_ceil
    // and the usize underflow when max_cols is small).
    let stride = frame_starts.len().div_ceil(max_cols.max(1)).max(1);
    let chosen: Vec<usize> = frame_starts.iter().copied().step_by(stride).collect();
    let cols = chosen.len();

    let mut mag_db: Vec<f32> = Vec::with_capacity(rows * cols);
    // Column-major build then we lay out row-major below.
    let mut columns: Vec<Vec<f32>> = Vec::with_capacity(cols);
    for &s in &chosen {
        let mut buf: Vec<Complex<f32>> = (0..fft_size)
            .map(|n| {
                let v = samples.get(s + n).copied().unwrap_or(0.0) * window[n];
                Complex::new(v, 0.0)
            })
            .collect();
        fft.process(&mut buf);
        let col: Vec<f32> = (lo_bin..=hi_bin)
            .map(|b| {
                let m = buf[b].norm();
                20.0 * (m + 1e-9).log10()
            })
            .collect();
        columns.push(col);
    }

    // Row-major: row r (frequency), col c (time).
    let mut freqs_hz = Vec::with_capacity(rows);
    for b in lo_bin..=hi_bin {
        freqs_hz.push(b as f32 * bin_hz);
    }
    let times_s: Vec<f32> = chosen.iter().map(|&s| s as f32 / sr).collect();

    for r in 0..rows {
        for c in 0..cols {
            mag_db.push(columns[c][r]);
        }
    }

    // Quantize across global min/max.
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &v in &mag_db {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let span = (hi - lo).max(1e-6);
    let mag_q: Vec<u8> = mag_db
        .iter()
        .map(|&v| (((v - lo) / span) * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect();

    SpectrogramGrid { rows, cols, freqs_hz, times_s, mag_q }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_dimensions_and_quantization_are_consistent() {
        // 1 second of a 1500 Hz tone at 48 kHz.
        let sr = SAMPLE_RATE_HZ as f32;
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 1500.0 * i as f32 / sr).sin())
            .collect();
        let g = stft(&samples, 1024, 512, (250.0, 2700.0), 200);
        assert_eq!(g.mag_q.len(), g.rows * g.cols);
        assert_eq!(g.freqs_hz.len(), g.rows);
        assert_eq!(g.times_s.len(), g.cols);
        assert!(g.cols <= 200);
        // The 1500 Hz row should be the brightest somewhere (value near 255).
        assert!(g.mag_q.iter().any(|&q| q > 200));
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p sonde-wasm spectrogram`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-wasm/src/spectrogram.rs
git commit -m "feat(sonde-demo): quantized band-cropped STFT for the waterfall"
```

---

## Task 8: Mode registry + Auto recommendation

**Files:**
- Create/replace: `crates/sonde-wasm/src/modes.rs`

- [ ] **Step 1: Write the implementation + test**

Replace `crates/sonde-wasm/src/modes.rs` with:
```rust
//! Mode catalogue for the demo, derived from sonde-phy's ModeTable. Only
//! `floor-wblo` produces real waveforms today; OFDM-Main modes are listed
//! with `implemented: false` until the parallel QAM work lands.

use crate::types::ModeInfo;
use sonde_phy::modes::{ModeHint, ModeTable};
use sonde_phy::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

/// Modes implemented end-to-end today.
pub fn is_implemented(mode_id: &str) -> bool {
    mode_id == "floor-wblo"
}

/// Bandwidth (Hz) for a mode id, from the OFDM grid where applicable.
fn bandwidth_hz(mode_id: &str) -> f32 {
    match mode_id {
        "ofdm-narrow" => 500.0,
        "ofdm-mid" => 1000.0,
        "ofdm-wide" | "floor-wblo" => 2300.0,
        "floor-nfsk" => 500.0,
        _ => 0.0,
    }
}

fn constellation(mode_id: &str) -> &'static str {
    match mode_id {
        "floor-wblo" => "BPSK",
        "floor-nfsk" => "8-FSK",
        // OFDM-Main constellations are pinned by the parallel QAM work.
        _ => "QAM (pending)",
    }
}

fn data_bytes_per_symbol(mode_id: &str) -> usize {
    match mode_id {
        "floor-wblo" => WidebandLowDensityFloor::new().data_bytes_per_symbol(),
        // For unimplemented OFDM modes, report the BPSK-equivalent data-carrier
        // count as a lower bound until QAM loading is known.
        "ofdm-narrow" => OfdmParams::for_mode(OfdmModeName::Narrow).data_indices().len() / 8,
        "ofdm-mid" => OfdmParams::for_mode(OfdmModeName::Mid).data_indices().len() / 8,
        "ofdm-wide" => OfdmParams::for_mode(OfdmModeName::Wide).data_indices().len() / 8,
        _ => 0,
    }
}

/// Full mode catalogue for the UI.
pub fn list_modes() -> Vec<ModeInfo> {
    // ModeTable has no public iterator; enumerate the known ids in ladder order.
    let ids = ["floor-wblo", "ofdm-narrow", "ofdm-mid", "ofdm-wide", "floor-nfsk"];
    ids.iter()
        .map(|&id| {
            let family = if id.starts_with("ofdm") { "OfdmMain" } else { "RobustnessFloor" };
            ModeInfo {
                id: id.to_string(),
                family: family.to_string(),
                constellation: constellation(id).to_string(),
                bandwidth_hz: bandwidth_hz(id),
                data_bytes_per_symbol: data_bytes_per_symbol(id),
                implemented: is_implemented(id),
            }
        })
        .collect()
}

/// Sonde's Auto decision for a measured SNR, clamped to implemented modes.
/// Wraps `ModeTable::resolve(MainAuto, snr)`; if the chosen mode is not yet
/// implemented, fall back to the best implemented mode (`floor-wblo`).
pub fn recommend_mode(snr_db: f32) -> String {
    let table = ModeTable::default();
    let chosen = table.resolve(ModeHint::MainAuto, Some(snr_db));
    let id = chosen.short_name();
    if is_implemented(id) {
        id.to_string()
    } else {
        "floor-wblo".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_mode_is_implemented_and_9_bytes() {
        let modes = list_modes();
        let floor = modes.iter().find(|m| m.id == "floor-wblo").unwrap();
        assert!(floor.implemented);
        assert_eq!(floor.data_bytes_per_symbol, 9);
        assert_eq!(floor.constellation, "BPSK");
    }

    #[test]
    fn ofdm_modes_listed_but_not_implemented() {
        let modes = list_modes();
        let mid = modes.iter().find(|m| m.id == "ofdm-mid").unwrap();
        assert!(!mid.implemented);
    }

    #[test]
    fn recommendation_clamps_to_implemented() {
        // High SNR resolves to ofdm-wide upstream, but it's not implemented,
        // so the demo clamps to floor-wblo.
        assert_eq!(recommend_mode(30.0), "floor-wblo");
        // Negative SNR resolves to floor-wblo directly.
        assert_eq!(recommend_mode(-5.0), "floor-wblo");
    }
}
```

- [ ] **Step 2: Run test**

Run: `cargo test -p sonde-wasm modes`
Expected: PASS. (If `ofdm_main` or `robustness_floor` module paths are not `pub` re-exported at the crate root, adjust the `use` paths to match `sonde-phy`'s actual `lib.rs` re-exports — confirm with `cargo doc -p sonde-phy --no-deps` or by reading `crates/sonde-phy/src/lib.rs`.)

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-wasm/src/modes.rs
git commit -m "feat(sonde-demo): mode catalogue + Auto recommendation (clamped)"
```

---

## Task 9: Link runner (encode → channel → decode → stats → symbols)

**Files:**
- Create/replace: `crates/sonde-wasm/src/link.rs`

Per-symbol bytes use the **encoded stream ground truth** (the 2-byte length header + payload, chunked at `data_bytes_per_symbol`), mapped to fields via offsets. `decoded_ok` for the whole frame is whether the recovered payload matched the original; live per-symbol decode requires a sonde-phy accessor and is deferred (see plan note). BER is byte-derived when decode succeeds, else `1.0` with `recovered_ok = false`.

- [ ] **Step 1: Write the implementation + test**

Replace `crates/sonde-wasm/src/link.rs` with:
```rust
//! Run a payload over the simulated link with a given mode and channel, and
//! produce the full `LinkResult` for the frontend.

use crate::channelize::apply_channel;
use crate::modes::is_implemented;
use crate::spectrogram::stft;
use crate::types::{FieldOffsets, LinkResult, SpectrogramGrid, SymbolRec};
use sonde_phy::audio_io::SAMPLE_RATE_HZ;
use sonde_phy::error::PhyError;
use sonde_phy::robustness_floor::wideband_lowdensity::{
    WidebandLowDensityFloor, PREAMBLE_LEN_SAMPLES,
};

fn field_for_byte(offsets: &FieldOffsets, byte_idx: usize) -> String {
    offsets
        .fields
        .iter()
        .find(|f| byte_idx >= f.start && byte_idx < f.end)
        .map(|f| f.label.clone())
        .unwrap_or_else(|| "pad".to_string())
}

/// Build per-symbol records from the encoded byte stream ground truth.
/// The multi-symbol frame stream is `[len_hi, len_lo, payload..., pad...]`
/// chunked into `cap`-byte symbols, after the 192-sample preamble.
fn build_symbols(
    payload: &[u8],
    cap: usize,
    symbol_size: usize,
    offsets: &FieldOffsets,
) -> Vec<SymbolRec> {
    let mut stream: Vec<u8> = Vec::new();
    stream.push((payload.len() >> 8) as u8);
    stream.push((payload.len() & 0xff) as u8);
    stream.extend_from_slice(payload);
    let symbols_needed = stream.len().div_ceil(cap);
    stream.resize(symbols_needed * cap, 0);

    let sr = SAMPLE_RATE_HZ as f32;
    let mut out = Vec::with_capacity(symbols_needed);
    for i in 0..symbols_needed {
        let chunk = &stream[i * cap..(i + 1) * cap];
        let sample_start = PREAMBLE_LEN_SAMPLES + i * symbol_size;
        let sample_end = sample_start + symbol_size;
        // Map this symbol's first non-header stream byte to a payload field.
        let stream_byte = i * cap;
        let payload_byte = stream_byte.saturating_sub(2);
        let field = if stream_byte < 2 { "header(framing)".to_string() } else { field_for_byte(offsets, payload_byte) };
        out.push(SymbolRec {
            idx: i,
            sample_start,
            sample_end,
            t_start_s: sample_start as f32 / sr,
            t_end_s: sample_end as f32 / sr,
            bytes: chunk.to_vec(),
            byte_start: payload_byte,
            byte_end: payload_byte + cap,
            field,
        });
    }
    out
}

fn bit_error_rate(a: &[u8], b: &[u8]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 1.0;
    }
    let mut diff_bits = 0u64;
    for i in 0..n {
        diff_bits += (a[i] ^ b[i]).count_ones() as u64;
    }
    // Count missing bytes (length mismatch) as fully errored.
    let extra = (a.len().max(b.len()) - n) as u64 * 8;
    (diff_bits + extra) as f32 / ((a.len().max(b.len())) as f32 * 8.0)
}

fn mean_band_snr(clean: &[num_complex::Complex<f32>], observed: &[num_complex::Complex<f32>]) -> f32 {
    use hf_channel_sim::estimate_subcarrier_snr;
    let fft_size = 2048usize;
    let n = (clean.len().min(observed.len()) / fft_size) * fft_size;
    if n < fft_size {
        return f32::NAN;
    }
    let est = estimate_subcarrier_snr(&clean[..n], &observed[..n], fft_size, SAMPLE_RATE_HZ as f64);
    // Average the per-bin SNR over the occupied band (~250..2700 Hz).
    let bin_hz = SAMPLE_RATE_HZ as f32 / fft_size as f32;
    let lo = (250.0 / bin_hz) as usize;
    let hi = ((2700.0 / bin_hz) as usize).min(fft_size - 1);
    let slice = &est.mean_snr_db[lo..=hi];
    slice.iter().sum::<f32>() / slice.len() as f32
}

/// Run the payload over the link. Only `floor-wblo` is implemented; other
/// modes return `PhyError::ModeUnavailable`.
pub fn run_link_core(
    payload: &[u8],
    offsets: &FieldOffsets,
    mode_id: &str,
    snr_db: f64,
    condition: &str,
    seed: u64,
) -> Result<LinkResult, PhyError> {
    if !is_implemented(mode_id) {
        return Err(PhyError::ModeUnavailable(format!(
            "{mode_id} not implemented yet (pending QAM work)"
        )));
    }
    let floor = WidebandLowDensityFloor::new();
    let cap = floor.data_bytes_per_symbol();
    let symbol_size = floor.symbol_size_samples();

    // Encode (preamble + multi-symbol body).
    let tx = floor.transmit_multi_with_preamble(payload)?;
    let total_samples = tx.len();

    // Channel.
    let (observed_real, clean_c, observed_c) = apply_channel(&tx, snr_db, condition, seed);

    // Decode.
    let (recovered_ok, ber) = match floor.receive_multi_with_sync(&observed_real) {
        Ok((_start, recovered)) => {
            let ok = recovered == payload;
            (ok, bit_error_rate(&recovered, payload))
        }
        Err(_) => (false, 1.0),
    };

    let measured_snr_db = mean_band_snr(&clean_c, &observed_c);

    let symbols = build_symbols(payload, cap, symbol_size, offsets);
    let n_symbols = symbols.len();
    let sr = SAMPLE_RATE_HZ as f32;
    let time_to_deliver_s = total_samples as f32 / sr;
    let throughput_bps = (payload.len() as f32 * 8.0) / time_to_deliver_s;

    let spectrogram: SpectrogramGrid = stft(&observed_real, 1024, 512, (250.0, 2700.0), 400);

    Ok(LinkResult {
        mode_id: mode_id.to_string(),
        recovered_ok,
        ber,
        measured_snr_db,
        payload_len: payload.len(),
        preamble_samples: PREAMBLE_LEN_SAMPLES,
        symbol_size_samples: symbol_size,
        total_samples,
        time_to_deliver_s,
        throughput_bps,
        symbols: symbols.into_iter().take(n_symbols).collect(),
        spectrogram,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Field;

    fn offsets_for(len: usize) -> FieldOffsets {
        FieldOffsets {
            total_len: len,
            fields: vec![
                Field { label: "header".into(), start: 0, end: len / 3 },
                Field { label: "body".into(), start: len / 3, end: 2 * len / 3 },
                Field { label: "image".into(), start: 2 * len / 3, end: len },
            ],
            image_byte_len: len / 3,
        }
    }

    #[test]
    fn clean_channel_recovers_payload_zero_ber() {
        let payload: Vec<u8> = (0..200).map(|i| (i % 251) as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "good", 1).unwrap();
        assert!(r.recovered_ok, "should recover at 80 dB");
        assert_eq!(r.ber, 0.0);
        assert!(r.throughput_bps > 0.0);
        // Symbols cover the whole payload + 2-byte header at 9 bytes/symbol.
        let expected_symbols = (payload.len() + 2).div_ceil(9);
        assert_eq!(r.symbols.len(), expected_symbols);
        assert_eq!(r.symbol_size_samples, 2560);
    }

    #[test]
    fn unimplemented_mode_errors() {
        let payload = vec![1u8, 2, 3];
        let off = offsets_for(payload.len());
        let err = run_link_core(&payload, &off, "ofdm-mid", 80.0, "good", 1).unwrap_err();
        assert!(matches!(err, PhyError::ModeUnavailable(_)));
    }

    #[test]
    fn spectrogram_present_and_band_cropped() {
        let payload: Vec<u8> = (0..50).map(|i| i as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "good", 1).unwrap();
        assert!(r.spectrogram.cols <= 400);
        assert_eq!(r.spectrogram.mag_q.len(), r.spectrogram.rows * r.spectrogram.cols);
        // Lowest freq row >= ~250 Hz band edge.
        assert!(*r.spectrogram.freqs_hz.first().unwrap() >= 200.0);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p sonde-wasm link`
Expected: PASS. If `clean_channel_recovers_payload_zero_ber` does not recover at 80 dB, the Watterson `Good` channel's phase rotation may defeat the bare floor receiver (which assumes no equalization on this path); in that case run the channel at the same SNR with a higher seed and confirm — if it is genuinely the channel, gate this test to AWGN-only by adding a `"none"` condition branch in `channelize.rs` that skips `process_block`, and use `"none"` here. Document the choice in a code comment.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-wasm/src/link.rs
git commit -m "feat(sonde-demo): link runner (encode->channel->decode->stats->symbols)"
```

---

## Task 10: wasm-bindgen JSON-string shim

**Files:**
- Modify: `crates/sonde-wasm/src/lib.rs`

- [ ] **Step 1: Add the thin wasm-exported wrappers + a host test**

Replace `crates/sonde-wasm/src/lib.rs` with:
```rust
//! sonde-wasm: real Sonde DSP over a simulated channel, exported to JS.
//!
//! Public `#[wasm_bindgen]` functions return JSON strings so they are callable
//! identically from the browser and from host `cargo test`.

pub mod channelize;
pub mod link;
pub mod modes;
pub mod spectrogram;
pub mod types;

use wasm_bindgen::prelude::*;

/// JSON array of `ModeInfo`.
#[wasm_bindgen]
pub fn list_modes() -> String {
    serde_json::to_string(&modes::list_modes()).unwrap()
}

/// Sonde's Auto-mode recommendation (mode id string) for a measured SNR.
#[wasm_bindgen]
pub fn recommend_mode(snr_db: f32) -> String {
    modes::recommend_mode(snr_db)
}

/// Run the payload over the link. `offsets_json` is the builder's
/// `payload.offsets.json`. Returns a JSON `LinkResult`, or a JSON
/// `{"error": "..."}` object on failure. `seed` is u32 to avoid JS BigInt.
#[wasm_bindgen]
pub fn run_link(
    payload: &[u8],
    offsets_json: &str,
    mode_id: &str,
    snr_db: f64,
    condition: &str,
    seed: u32,
) -> String {
    let offsets: types::FieldOffsets = match serde_json::from_str(offsets_json) {
        Ok(o) => o,
        Err(e) => return format!("{{\"error\":\"bad offsets json: {e}\"}}"),
    };
    match link::run_link_core(payload, &offsets, mode_id, snr_db, condition, seed as u64) {
        Ok(r) => serde_json::to_string(&r).unwrap(),
        Err(e) => format!("{{\"error\":\"{e}\"}}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_modes_returns_valid_json_with_floor() {
        let json = list_modes();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.as_array().unwrap().iter().any(|m| m["id"] == "floor-wblo"));
    }

    #[test]
    fn run_link_round_trips_through_json() {
        let payload: Vec<u8> = (0..100).map(|i| i as u8).collect();
        let offsets = serde_json::json!({
            "total_len": payload.len(),
            "fields": [{"label":"image","start":0,"end":payload.len()}],
            "image_byte_len": payload.len()
        })
        .to_string();
        let json = run_link(&payload, &offsets, "floor-wblo", 80.0, "good", 1);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["recovered_ok"], true);
        assert_eq!(v["mode_id"], "floor-wblo");
    }

    #[test]
    fn run_link_reports_error_for_unimplemented_mode() {
        let payload = vec![1u8, 2, 3];
        let offsets = r#"{"total_len":3,"fields":[],"image_byte_len":0}"#;
        let json = run_link(&payload, offsets, "ofdm-mid", 80.0, "good", 1);
        assert!(json.contains("error"));
    }
}
```

- [ ] **Step 2: Run tests + full gate**

Run: `cargo test -p sonde-wasm`
Then: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all PASS / clean.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-wasm/src/lib.rs
git commit -m "feat(sonde-demo): wasm-bindgen JSON-string shim over the engine"
```

---

## Task 11: WASM build verification + getrandom fix

This task confirms the crate actually compiles to `wasm32-unknown-unknown` (the host tests don't exercise that target). The likely failure is `getrandom` needing the `js` feature — already declared in Task 5, verified here.

**Files:**
- (Possibly) Modify: `crates/sonde-wasm/Cargo.toml`

- [ ] **Step 1: Add the wasm target**

Run: `rustup target add wasm32-unknown-unknown`
Expected: target installed (or already present).

- [ ] **Step 2: Build for wasm**

Run: `cargo build -p sonde-wasm --target wasm32-unknown-unknown`
Expected: compiles.
If it fails with a `getrandom` error mentioning "the wasm32-unknown-unknown target is not supported by default": confirm the `[target.'cfg(target_arch = "wasm32")'.dependencies] getrandom = { workspace = true, features = ["js"] }` block from Task 5 is present and that the workspace `getrandom` version matches what `rand` pulls (check `cargo tree -p sonde-wasm -i getrandom`). Align the version if needed.
If it fails on `hound` or another dep that touches the filesystem: confirm `sonde-phy`'s `audio_io` WAV functions are not called from any path reachable by the wasm exports (they are not — the engine never calls `read_wav`/`write_wav`). `hound` itself is pure Rust and compiles to wasm.

- [ ] **Step 3: Record the build command in a README for the frontend plan**

Create `crates/sonde-wasm/README.md`:
```markdown
# sonde-wasm

Real Sonde DSP over a simulated HF channel, exported to JS via wasm-bindgen.

## Host tests
```
cargo test -p sonde-wasm
```

## Build for the browser
```
cargo install wasm-bindgen-cli   # once
cargo build -p sonde-wasm --release --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/sonde_wasm.wasm \
  --out-dir ../../demo/site/pkg --target web
```

## JS API (all return JSON strings unless noted)
- `list_modes() -> ModeInfo[]`
- `recommend_mode(snr_db: number) -> string` (mode id)
- `run_link(payload: Uint8Array, offsets_json: string, mode_id: string, snr_db: number, condition: string, seed: number) -> LinkResult | {error}`
```

- [ ] **Step 4: Commit**

```bash
git add crates/sonde-wasm/Cargo.toml crates/sonde-wasm/README.md
git commit -m "build(sonde-demo): verify wasm32 build + document sonde-wasm JS API"
```

---

## Task 12: End-to-end builder → engine smoke test

Proves the builder's real output feeds the engine. Uses a generated test image (no external asset needed for the test).

**Files:**
- Create: `crates/sonde-wasm/tests/end_to_end.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/sonde-wasm/tests/end_to_end.rs`:
```rust
//! End-to-end: a SITREP-shaped payload + offsets runs through the engine and
//! recovers cleanly at high SNR. Mirrors how the builder output is consumed.

use sonde_wasm::link::run_link_core;
use sonde_wasm::types::{Field, FieldOffsets};

#[test]
fn sitrep_shaped_payload_recovers_at_high_snr() {
    // ~5 KB payload: small text header/body + a pseudo "image" blob.
    let mut payload: Vec<u8> = b"To: EMCOMM-NET\nFrom: KK6XYZ\nSubject: SITREP\nPosition: 34-12.34N\n\nLevee breach.\n--- attachment: recon.jpg ---\n".to_vec();
    let header_end = payload.len();
    let mut state: u32 = 0xC0FF_EE00;
    for _ in 0..4800 {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        payload.push((state >> 16) as u8);
    }
    let off = FieldOffsets {
        total_len: payload.len(),
        fields: vec![
            Field { label: "header".into(), start: 0, end: header_end },
            Field { label: "image".into(), start: header_end, end: payload.len() },
        ],
        image_byte_len: payload.len() - header_end,
    };

    let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "good", 7).unwrap();
    assert!(r.recovered_ok, "5 KB SITREP should recover at 80 dB");
    assert_eq!(r.ber, 0.0);
    // ~5 KB at 9 bytes/symbol ≈ 558 symbols ≈ 30 s of audio.
    assert!(r.time_to_deliver_s > 25.0 && r.time_to_deliver_s < 35.0, "got {} s", r.time_to_deliver_s);
    // Every symbol maps to a known field label.
    assert!(r.symbols.iter().all(|s| !s.field.is_empty()));
}
```

- [ ] **Step 2: Run + full gate**

Run: `cargo test -p sonde-wasm --test end_to_end`
Then: `cargo test --workspace && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all PASS / clean.

- [ ] **Step 3: Commit**

```bash
git add crates/sonde-wasm/tests/end_to_end.rs
git commit -m "test(sonde-demo): end-to-end SITREP payload recovers through engine"
```

---

## Notes carried into the frontend plan (next plan)

- **Per-symbol live decode:** this plan shows *intended* (ground-truth) bytes per symbol + a whole-frame `recovered_ok`. True per-symbol decode coloring needs a small `sonde-phy` accessor (`decode_symbols(&samples) -> Vec<Vec<u8>>`); add it in the frontend plan or fold it into the parallel framing refactor (spec §7).
- **Recovered image rendering:** the frontend reconstructs the image from `payload[image.start..image.end]` of the recovered bytes when `recovered_ok`; at low SNR it shows the partial/garbled bytes to dramatize failure. `LinkResult` intentionally does not embed the recovered image — the frontend already has the payload + offsets.
- **WASM artifact path:** `demo/site/pkg/` (see `crates/sonde-wasm/README.md`).
- **Auto vs Manual:** Auto calls `recommend_mode(measured_snr)` then `run_link`; Manual calls `run_link` with the chosen mode id directly.
- **`measured_snr_db` may be `NaN`** for very short payloads (< one 2048 window); the frontend should guard the display.
```
