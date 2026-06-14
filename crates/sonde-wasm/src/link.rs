//! Run a payload over the simulated link with a given mode and channel, and
//! produce the full `LinkResult` for the frontend.

use crate::channelize::apply_channel;
use crate::modes::is_implemented;
use crate::spectrogram::stft;
use crate::types::{FieldOffsets, LinkResult, SpectrogramGrid, SymbolRec};
use sonde_phy::audio_io::SAMPLE_RATE_HZ;
use sonde_phy::error::PhyError;
use sonde_phy::robustness_floor::coded_framing::{blocks_for_payload, HEADER_BITS};
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

/// Build per-symbol records that mirror the coded floor's real on-air layout.
///
/// The coded floor frames the payload as a global info-bit stream
/// `[16-bit BE len][payload bits][zero pad]` (see [`coded_framing`]), then —
/// with the demo's default rate-1 `IdentityFec` — lays `dps` info bits onto
/// each BPSK OFDM symbol. So symbol `i` carries framed info bits
/// `[i*dps, (i+1)*dps)`. We attribute each PAYLOAD byte to the symbol holding
/// its first (MSB) bit, so the byte ranges the frontend paints stay contiguous
/// and non-overlapping even though `dps` is not a multiple of 8 and individual
/// bytes straddle two symbols.
///
/// NOTE: this 1-symbol-per-block, coded==info mapping is exact only for the
/// rate-1 `IdentityFec` floor the demo uses today. A real codec (coded bits >
/// info bits) would expand each block across more symbols and this mapping
/// would need revisiting.
///
/// [`coded_framing`]: sonde_phy::robustness_floor::coded_framing
fn build_symbols(
    payload: &[u8],
    dps: usize,
    symbol_size: usize,
    offsets: &FieldOffsets,
) -> Vec<SymbolRec> {
    // Smallest payload-byte index whose first (MSB) bit sits at or beyond the
    // framed-stream bit position `bit`. Payload byte k's first bit is at
    // HEADER_BITS + k*8, so this inverts that relation.
    let first_payload_byte = |bit: usize| bit.saturating_sub(HEADER_BITS).div_ceil(8);

    let n_symbols = blocks_for_payload(payload.len(), dps);
    let sr = SAMPLE_RATE_HZ as f32;
    let mut out = Vec::with_capacity(n_symbols);
    for i in 0..n_symbols {
        let byte_start = first_payload_byte(i * dps).min(payload.len());
        let byte_end = first_payload_byte((i + 1) * dps).min(payload.len());
        let sample_start = PREAMBLE_LEN_SAMPLES + i * symbol_size;
        let sample_end = sample_start + symbol_size;
        let field = if byte_end > byte_start {
            field_for_byte(offsets, byte_start)
        } else {
            // Symbol carries only the framing header and/or trailing pad bits.
            "framing/pad".to_string()
        };
        out.push(SymbolRec {
            idx: i,
            sample_start,
            sample_end,
            t_start_s: sample_start as f32 / sr,
            t_end_s: sample_end as f32 / sr,
            bytes: payload[byte_start..byte_end].to_vec(),
            rx_bytes: Vec::new(),
            byte_start,
            byte_end,
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
    let diff_bits: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (x ^ y).count_ones() as u64)
        .sum();
    // Count missing bytes (length mismatch) as fully errored.
    let extra = (a.len().max(b.len()) - n) as u64 * 8;
    (diff_bits + extra) as f32 / ((a.len().max(b.len())) as f32 * 8.0)
}

fn mean_band_snr(
    clean: &[num_complex::Complex<f32>],
    observed: &[num_complex::Complex<f32>],
) -> f32 {
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
///
/// CHANNEL CONDITIONS & RECOVERY: the floor-wblo receiver does not equalize
/// multipath. Only the AWGN-only `"none"` condition recovers the payload
/// cleanly; the Watterson conditions (`good`/`moderate`/`poor`/`flutter`) apply
/// complex multipath fading the bare receiver cannot undo, so they will
/// generally return `recovered_ok: false` (with a well-formed LinkResult and a
/// real spectrogram). That degradation is itself part of the demo — it shows
/// why equalization / mode adaptation matters.
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
    let symbol_size = floor.symbol_size_samples();
    // Info bits carried per BPSK OFDM symbol (one per data sub-carrier). For
    // the rate-1 IdentityFec floor this equals the FEC block size, so one
    // symbol == one block — see [`build_symbols`].
    let dps = floor.params().data_indices().len();

    // Encode (preamble + coded multi-symbol body).
    let tx = floor.transmit_multi_with_preamble(payload)?;
    let total_samples = tx.len();

    // Channel.
    let (observed_real, clean_c, observed_c) = apply_channel(&tx, snr_db, condition, seed);

    // Decode the whole frame. The coded floor decodes per FEC block from soft
    // LLRs — there is no per-symbol byte decode — so per-symbol RX bytes are
    // attributed from the recovered payload over each symbol's byte range.
    //   Ok(p), p == payload → clean recovery (BER 0)
    //   Ok(p), p != payload → synced but corrupted: the length header survived
    //                         but body bits flipped (IdentityFec never rejects)
    //   Err(_)              → preamble miss, or a corrupted header → truncation
    let (recovered_ok, ber, recovered_bytes) = match floor.receive_multi_with_sync(&observed_real) {
        Ok((_start, rx)) => {
            let ok = rx == payload;
            let ber = bit_error_rate(&rx, payload);
            (ok, ber, rx)
        }
        Err(_) => (false, 1.0, Vec::new()),
    };

    let measured_snr_db = mean_band_snr(&clean_c, &observed_c);

    let mut symbols = build_symbols(payload, dps, symbol_size, offsets);
    // Attach per-symbol RX bytes: slice the recovered payload over each
    // symbol's byte range (empty when sync/decode failed, or when the recovered
    // payload is shorter than this symbol's range).
    for sym in symbols.iter_mut() {
        if !recovered_bytes.is_empty() {
            let end = sym.byte_end.min(recovered_bytes.len());
            let start = sym.byte_start.min(end);
            sym.rx_bytes = recovered_bytes[start..end].to_vec();
        }
    }
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
        recovered_bytes,
        preamble_samples: PREAMBLE_LEN_SAMPLES,
        symbol_size_samples: symbol_size,
        total_samples,
        time_to_deliver_s,
        throughput_bps,
        symbols,
        spectrogram,
    })
}

/// Channel-impaired audio samples for the link — the real modulated waveform
/// after the simulated channel (what the waterfall visualizes and the operator
/// can listen to). Encode + channel only; no decode/STFT, so it's cheaper than
/// [`run_link_core`]. Uses the same `WidebandLowDensityFloor::new()` + seed as
/// `run_link_core`, so for identical args the audio matches that run's waveform.
/// Returns `ModeUnavailable` for unimplemented modes.
pub fn link_audio_core(
    payload: &[u8],
    mode_id: &str,
    snr_db: f64,
    condition: &str,
    seed: u64,
) -> Result<Vec<f32>, PhyError> {
    if !is_implemented(mode_id) {
        return Err(PhyError::ModeUnavailable(format!(
            "{mode_id} not implemented yet (pending QAM work)"
        )));
    }
    let floor = WidebandLowDensityFloor::new();
    let tx = floor.transmit_multi_with_preamble(payload)?;
    let (observed_real, _clean, _observed) = apply_channel(&tx, snr_db, condition, seed);
    Ok(observed_real)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Field;

    fn offsets_for(len: usize) -> FieldOffsets {
        FieldOffsets {
            total_len: len,
            fields: vec![
                Field {
                    label: "header".into(),
                    start: 0,
                    end: len / 3,
                },
                Field {
                    label: "body".into(),
                    start: len / 3,
                    end: 2 * len / 3,
                },
                Field {
                    label: "image".into(),
                    start: 2 * len / 3,
                    end: len,
                },
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
        // One symbol per FEC block (rate-1 IdentityFec floor); the symbol count
        // matches the floor's own framing of the 16-bit header + payload bits.
        let dps = WidebandLowDensityFloor::new().params().data_indices().len();
        let expected_symbols = blocks_for_payload(payload.len(), dps);
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
        assert_eq!(
            r.spectrogram.mag_q.len(),
            r.spectrogram.rows * r.spectrogram.cols
        );
        // Lowest freq row >= ~250 Hz band edge.
        assert!(*r.spectrogram.freqs_hz.first().unwrap() >= 200.0);
    }

    #[test]
    fn symbol_payload_ranges_are_contiguous_and_nonoverlapping() {
        // 20-byte payload: framed stream = 16-bit header + 160 payload bits =
        // 176 bits -> ceil(176/74) = 3 symbols.
        let payload: Vec<u8> = (0..20).map(|i| i as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "none", 1).unwrap();
        // Symbol 0 spends its first 16 bits on the length header, leaving 58
        // bits = payload bytes 0..8 (byte 7's first bit, at 72, still lands in
        // symbol 0's [0,74) window even though the byte straddles into symbol 1).
        assert_eq!(r.symbols[0].byte_start, 0);
        assert_eq!(r.symbols[0].byte_end, 8);
        // Contiguous, non-overlapping across all symbols (each byte attributed
        // to the symbol holding its first bit).
        for w in r.symbols.windows(2) {
            assert_eq!(w[0].byte_end, w[1].byte_start, "ranges must be contiguous");
        }
        assert_eq!(
            r.symbols.last().unwrap().byte_end,
            payload.len(),
            "last byte_end clamps to payload len"
        );
    }

    #[test]
    fn clean_link_exposes_recovered_and_rx_bytes() {
        let payload: Vec<u8> = (0..120).map(|i| (i % 251) as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "none", 1).unwrap();
        assert_eq!(r.recovered_bytes, payload);
        for s in &r.symbols {
            assert_eq!(s.rx_bytes.len(), s.bytes.len());
            assert_eq!(
                s.rx_bytes, s.bytes,
                "clean path: rx should equal tx for sym {}",
                s.idx
            );
        }
    }

    #[test]
    fn failed_sync_yields_empty_recovered_and_rx() {
        let payload: Vec<u8> = (0..120).map(|i| (i % 251) as u8).collect();
        let off = offsets_for(payload.len());
        let r = run_link_core(&payload, &off, "floor-wblo", -6.0, "poor", 3).unwrap();
        // Invariant: if the frame didn't recover, recovered_bytes + all rx_bytes are empty.
        if !r.recovered_ok {
            assert!(r.recovered_bytes.is_empty());
            assert!(r.symbols.iter().all(|s| s.rx_bytes.is_empty()));
        }
    }

    #[test]
    fn multipath_condition_is_well_formed_and_does_not_panic() {
        let payload: Vec<u8> = (0..120).map(|i| (i % 251) as u8).collect();
        let off = offsets_for(payload.len());
        // Watterson multipath at 0 dB SNR: the bare floor receiver has no
        // equalizer, so we expect a well-formed result, NOT clean recovery.
        // Observed (poor/0.0/3): recovered_ok=false, ber=1.0.
        let r = run_link_core(&payload, &off, "floor-wblo", 0.0, "poor", 3).unwrap();
        // Well-formed regardless of decode success:
        assert!(r.ber >= 0.0 && r.ber <= 1.0, "ber in [0,1], got {}", r.ber);
        assert!(!r.symbols.is_empty());
        assert_eq!(
            r.spectrogram.mag_q.len(),
            r.spectrogram.rows * r.spectrogram.cols
        );
        // Document the limitation: multipath does not recover with floor-wblo.
        assert!(
            !r.recovered_ok,
            "multipath should not recover with the non-equalizing floor receiver"
        );
    }
}
