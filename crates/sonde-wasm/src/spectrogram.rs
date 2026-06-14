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
        for col in &columns {
            mag_db.push(col[r]);
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

    SpectrogramGrid {
        rows,
        cols,
        freqs_hz,
        times_s,
        mag_q,
    }
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
