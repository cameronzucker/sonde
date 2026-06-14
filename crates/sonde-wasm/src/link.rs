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
    let diff_bits: u64 = a.iter().zip(b).take(n).map(|(x, y)| (x ^ y).count_ones() as u64).sum();
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
        let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "none", 1).unwrap();
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
        let err = run_link_core(&payload, &off, "ofdm-mid", 80.0, "none", 1).unwrap_err();
        assert!(matches!(err, PhyError::ModeUnavailable(_)));
    }

    #[test]
    fn spectrogram_present_and_band_cropped() {
        let payload: Vec<u8> = (0..50).map(|i| i as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "none", 1).unwrap();
        assert!(r.spectrogram.cols <= 400);
        assert_eq!(r.spectrogram.mag_q.len(), r.spectrogram.rows * r.spectrogram.cols);
        // Lowest freq row >= ~250 Hz band edge.
        assert!(*r.spectrogram.freqs_hz.first().unwrap() >= 200.0);
    }
}
