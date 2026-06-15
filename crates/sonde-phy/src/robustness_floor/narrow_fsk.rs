//! Narrow-FSK situational floor mode. Conceptual primitive borrowed
//! from FT8/JS8 weak-signal design (8-FSK; foundation doc §6.1) —
//! primitive only, not specific protocol parameters per
//! `feedback_clean_sheet_concepts_only`. Reserved for crowded-band
//! slots where wide-band isn't available.
//!
//! Noncoherent energy-detector receiver: for each symbol period, FFT
//! the segment (zero-padded to the next power of two) and pick the
//! bin with maximum magnitude across the 8 candidate tone bins.

use crate::audio_io::SAMPLE_RATE_HZ;
use crate::error::PhyError;
use crate::sync::preamble::{PreambleDetector, PreambleGenerator, PREAMBLE_LEN};
use num_complex::Complex;
use rustfft::FftPlanner;

/// Normalised template-MF peak required to declare a preamble present (vs noise).
/// A clean aligned preamble scores ≈1.0; random noise stays well below. 0.5 sits
/// in the gap (validated by the pure-noise gate).
const NFSK_PREAMBLE_THRESHOLD: f32 = 0.5;

const M: usize = 8; // 8-FSK ⇒ 3 bits/symbol
const TONE_SPACING_HZ: f32 = 50.0; // spacing between tones
const SYMBOL_DURATION_SEC: f32 = 0.16; // FT8-class baud as design primitive
const CENTER_FREQ_HZ: f32 = 1500.0; // middle of audio band

/// Outcome of [`NarrowFskFloor::receive_scan`] over one capture window — the
/// three-way split [`crate::SondePhy`]'s RX pump needs for honest FER accounting,
/// mirroring the floor's `SyncDecodeOutcome`.
#[derive(Debug, Clone, PartialEq)]
pub enum NfskDecode {
    /// No preamble above the detector threshold — window held only noise.
    NoSignal,
    /// Preamble acquired but the framed payload failed its CRC (or was truncated)
    /// — a genuine frame error.
    Detected,
    /// Clean decode: preamble acquired, length-delimited payload recovered + CRC OK.
    Frame(Vec<u8>),
}

/// CRC-32 (IEEE 802.3, reflected) over `bytes`. Inlined because `sonde-phy` cannot
/// depend on `sonde-fec` (that edge would cycle: `sonde-fec` depends on
/// `sonde-phy`'s `FecCodec` trait). Used to validate the self-delimited nFSK frame.
fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// 8-FSK noncoherent floor mode for crowded-band slots. Three bits
/// per symbol, 50 Hz tone spacing, 0.16 s symbol duration, centered
/// at 1500 Hz.
pub struct NarrowFskFloor;

impl NarrowFskFloor {
    /// Construct the floor. The mode has no tunable state today —
    /// all parameters are pinned per the foundation-doc §6.1 design
    /// primitives.
    pub fn new() -> Self {
        Self
    }

    /// Occupied bandwidth in Hz, including a one-tone-spacing guard
    /// band on each side of the active tone cluster.
    pub fn occupied_bandwidth_hz(&self) -> f32 {
        (M as f32 - 1.0) * TONE_SPACING_HZ + 2.0 * TONE_SPACING_HZ
    }

    fn samples_per_symbol(&self) -> usize {
        (SAMPLE_RATE_HZ as f32 * SYMBOL_DURATION_SEC) as usize
    }

    fn tone_freq_hz(&self, idx: usize) -> f32 {
        let low = CENTER_FREQ_HZ - (M as f32 / 2.0 - 0.5) * TONE_SPACING_HZ;
        low + idx as f32 * TONE_SPACING_HZ
    }

    /// Modulate a byte payload to a stream of FSK tones at 48 kHz f32
    /// audio sample rate.
    pub fn transmit(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        let mut bits: Vec<u8> = Vec::with_capacity(payload.len() * 8);
        for byte in payload {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1);
            }
        }
        while bits.len() % 3 != 0 {
            bits.push(0);
        }
        let n_symbols = bits.len() / 3;
        let sps = self.samples_per_symbol();
        let mut samples = Vec::with_capacity(n_symbols * sps);
        for sym_idx in 0..n_symbols {
            let tone_idx = ((bits[sym_idx * 3] as usize) << 2)
                | ((bits[sym_idx * 3 + 1] as usize) << 1)
                | (bits[sym_idx * 3 + 2] as usize);
            let f = self.tone_freq_hz(tone_idx);
            for n in 0..sps {
                let t = n as f32 / SAMPLE_RATE_HZ as f32;
                samples.push((2.0 * std::f32::consts::PI * f * t).sin());
            }
        }
        Ok(samples)
    }

    /// Demodulate the audio stream back to a byte payload. Trailing
    /// zero bytes from the bit-padding are trimmed; multi-symbol
    /// framing arrives in Phase 10.
    pub fn receive(&self, samples: &[f32]) -> Result<Vec<u8>, PhyError> {
        let sps = self.samples_per_symbol();
        let n_symbols = samples.len() / sps;
        let mut planner = FftPlanner::<f32>::new();
        let fft_size = sps.next_power_of_two();
        let fft = planner.plan_fft_forward(fft_size);

        let mut bits = Vec::with_capacity(n_symbols * 3);
        for sym_idx in 0..n_symbols {
            let mut buf: Vec<Complex<f32>> = samples[sym_idx * sps..sym_idx * sps + sps]
                .iter()
                .map(|s| Complex::new(*s, 0.0))
                .collect();
            buf.resize(fft_size, Complex::new(0.0, 0.0));
            fft.process(&mut buf);

            let mut best_tone = 0usize;
            let mut best_mag = 0.0_f32;
            for tone_idx in 0..M {
                let f = self.tone_freq_hz(tone_idx);
                let bin = (f * fft_size as f32 / SAMPLE_RATE_HZ as f32).round() as usize;
                let m = buf[bin].norm();
                if m > best_mag {
                    best_mag = m;
                    best_tone = tone_idx;
                }
            }
            bits.push(((best_tone >> 2) & 1) as u8);
            bits.push(((best_tone >> 1) & 1) as u8);
            bits.push((best_tone & 1) as u8);
        }
        let trim = bits.len() - (bits.len() % 8);
        let bits = &bits[..trim];
        let mut bytes = Vec::with_capacity(bits.len() / 8);
        for chunk in bits.chunks(8) {
            let mut b = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                b |= bit << (7 - i);
            }
            bytes.push(b);
        }
        let last_nonzero = bytes
            .iter()
            .rposition(|&b| b != 0)
            .map(|i| i + 1)
            .unwrap_or(0);
        bytes.truncate(last_nonzero);
        Ok(bytes)
    }

    /// Modulate `bytes` (MSB-first) to 8-FSK tones, zero-padding the final symbol
    /// to a whole 3-bit group.
    fn modulate_bytes(&self, bytes: &[u8]) -> Vec<f32> {
        let mut bits: Vec<u8> = Vec::with_capacity(bytes.len() * 8);
        for byte in bytes {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1);
            }
        }
        while bits.len() % 3 != 0 {
            bits.push(0);
        }
        let sps = self.samples_per_symbol();
        let mut samples = Vec::with_capacity(bits.len() / 3 * sps);
        for grp in bits.chunks(3) {
            let tone_idx = ((grp[0] as usize) << 2) | ((grp[1] as usize) << 1) | (grp[2] as usize);
            let f = self.tone_freq_hz(tone_idx);
            for n in 0..sps {
                let t = n as f32 / SAMPLE_RATE_HZ as f32;
                samples.push((2.0 * std::f32::consts::PI * f * t).sin());
            }
        }
        samples
    }

    /// The self-delimited nFSK frame as bytes: `[len:2 BE][payload][crc32:4 BE]`,
    /// CRC over `[len][payload]`. Length-prefix + CRC replace the old lossy
    /// trailing-zero trim, giving a real NoSignal/Detected/Frame split.
    fn frame_bytes(payload: &[u8]) -> Result<Vec<u8>, PhyError> {
        if payload.len() > u16::MAX as usize {
            return Err(PhyError::PayloadTooLarge {
                actual: payload.len(),
                capacity: u16::MAX as usize,
            });
        }
        let mut frame = Vec::with_capacity(2 + payload.len() + 4);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        frame.extend_from_slice(payload);
        let crc = crc32_ieee(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        Ok(frame)
    }

    /// Modulate `payload` into a self-synchronising buffer: a mode-agnostic
    /// Schmidl-Cox preamble (shared with the OFDM floor) followed by the framed
    /// 8-FSK body. This is what lets the nFSK mode be a registry [`crate::SondePhy`]
    /// waveform — the receiver finds the preamble in an arbitrary capture window,
    /// then demodulates the length-delimited frame.
    pub fn transmit_with_preamble(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        let frame = Self::frame_bytes(payload)?;
        let mut out = PreambleGenerator::new().generate();
        out.extend(self.modulate_bytes(&frame));
        Ok(out)
    }

    /// Demodulate the `n_symbols` 8-FSK symbols starting at `base` to a byte stream
    /// (MSB-first, whole bytes only). Shared by [`Self::receive`] and
    /// [`Self::receive_scan`].
    fn demod_bytes(&self, samples: &[f32], base: usize, n_symbols: usize) -> Vec<u8> {
        let sps = self.samples_per_symbol();
        let mut planner = FftPlanner::<f32>::new();
        let fft_size = sps.next_power_of_two();
        let fft = planner.plan_fft_forward(fft_size);
        let mut bits = Vec::with_capacity(n_symbols * 3);
        for s in 0..n_symbols {
            let start = base + s * sps;
            if start + sps > samples.len() {
                break;
            }
            let mut buf: Vec<Complex<f32>> = samples[start..start + sps]
                .iter()
                .map(|s| Complex::new(*s, 0.0))
                .collect();
            buf.resize(fft_size, Complex::new(0.0, 0.0));
            fft.process(&mut buf);
            let mut best_tone = 0usize;
            let mut best_mag = 0.0_f32;
            for tone_idx in 0..M {
                let f = self.tone_freq_hz(tone_idx);
                let bin = (f * fft_size as f32 / SAMPLE_RATE_HZ as f32).round() as usize;
                let m = buf[bin].norm();
                if m > best_mag {
                    best_mag = m;
                    best_tone = tone_idx;
                }
            }
            bits.push(((best_tone >> 2) & 1) as u8);
            bits.push(((best_tone >> 1) & 1) as u8);
            bits.push((best_tone & 1) as u8);
        }
        bits.chunks(8)
            .filter(|c| c.len() == 8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |b, (i, &bit)| b | (bit << (7 - i)))
            })
            .collect()
    }

    /// Locate the preamble in `samples` and demodulate the framed payload —
    /// the self-syncing receive path for registry use. Returns
    /// [`NfskDecode::NoSignal`] when no preamble is found, [`NfskDecode::Detected`]
    /// when the preamble was acquired but the frame failed (truncated / CRC), and
    /// [`NfskDecode::Frame`] on a clean, CRC-verified decode.
    pub fn receive_scan(&self, samples: &[f32]) -> NfskDecode {
        // Use the template matched-filter peak (`peak_normalized`) for timing: its
        // argmax IS the true preamble start (non-coherent two-half combine, ≈1.0
        // when aligned), unlike the Schmidl-Cox `scan` plateau which the OFDM floor
        // compensates for in its own private path. A normalized peak below
        // threshold is noise ⇒ NoSignal.
        let (start, corr) = match PreambleDetector::new().peak_normalized(samples) {
            Some((s, c)) if c >= NFSK_PREAMBLE_THRESHOLD => (s, c),
            _ => return NfskDecode::NoSignal,
        };
        let _ = corr;
        let body_start = start + PREAMBLE_LEN;
        if body_start >= samples.len() {
            return NfskDecode::Detected;
        }
        let sps = self.samples_per_symbol();
        let avail_symbols = (samples.len() - body_start) / sps;
        let bytes = self.demod_bytes(samples, body_start, avail_symbols);
        // Need at least the 2-byte length header.
        if bytes.len() < 2 {
            return NfskDecode::Detected;
        }
        let len = u16::from_be_bytes([bytes[0], bytes[1]]) as usize;
        let frame_len = 2 + len + 4;
        if bytes.len() < frame_len {
            return NfskDecode::Detected;
        }
        let crc_got = u32::from_be_bytes([
            bytes[2 + len],
            bytes[2 + len + 1],
            bytes[2 + len + 2],
            bytes[2 + len + 3],
        ]);
        if crc32_ieee(&bytes[..2 + len]) != crc_got {
            return NfskDecode::Detected;
        }
        NfskDecode::Frame(bytes[2..2 + len].to_vec())
    }
}

impl Default for NarrowFskFloor {
    fn default() -> Self {
        Self::new()
    }
}
