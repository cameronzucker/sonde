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
