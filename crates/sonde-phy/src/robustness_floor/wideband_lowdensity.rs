//! Default robustness floor mode: wide-band low-density-constellation
//! OFDM. BPSK per sub-carrier across ~2.3 kHz with FEC composition.
//!
//! The floor runs ONE coded path: payload → codeword-spanning framing
//! (length header in the first FEC block) → per-block `FecCodec::encode`
//! → BPSK OFDM symbols (one block's coded bits packed across N symbols,
//! the last symbol zero-padded) → optional preamble. RX inverts it:
//! per block, demodulate its symbols to soft LLRs, trim the trailing
//! OFDM-pad LLRs back to `block_coded_bits()`, then `decode_soft`.
//! Block 0 is decoded first to read the length/block-count header.
//!
//! The codec is a `Box<dyn FecCodec>` from subsystem #4. [`Self::new`]
//! defaults to [`IdentityFec`] (pass-through, for sim-isolation BER
//! characterization); [`Self::with_fec`] injects a concrete codec.

use crate::coded_modulation::{FecCodec, IdentityFec};
use crate::error::PhyError;
use crate::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use crate::ofdm_main::receiver::OfdmReceiver;
use crate::ofdm_main::transmitter::OfdmTransmitter;
use crate::robustness_floor::coded_framing::{
    blocks_from_first_block, deframe_info_bits, frame_info_bits,
};
use crate::sync::preamble::{PreambleDetector, PreambleGenerator};

/// Sample count of the Zadoff-Chu preamble emitted by
/// [`WidebandLowDensityFloor::transmit_with_preamble`]. Matches the
/// pin in [`crate::sync::preamble`].
pub const PREAMBLE_LEN_SAMPLES: usize = 192;

/// Default robustness floor: wide-band OFDM, BPSK on every occupied
/// sub-carrier, with a composed FEC codec. Strategic posture is "go
/// wider, not denser" — see overview §5.A.1.
pub struct WidebandLowDensityFloor {
    params: OfdmParams,
    fec: Box<dyn FecCodec>,
}

impl WidebandLowDensityFloor {
    /// Floor with the default sim-isolation codec ([`IdentityFec`]),
    /// block sized to one OFDM symbol's data-bit capacity.
    pub fn new() -> Self {
        let params = OfdmParams::for_mode(OfdmModeName::Wide);
        let block = params.data_indices().len();
        Self {
            params,
            fec: Box::new(IdentityFec::new(block)),
        }
    }

    /// Floor with an injected concrete codec (e.g. `FloorRate14Codec`).
    pub fn with_fec(fec: Box<dyn FecCodec>) -> Self {
        Self {
            params: OfdmParams::for_mode(OfdmModeName::Wide),
            fec,
        }
    }

    /// Borrowed access to the underlying OFDM parameter set.
    pub fn params(&self) -> &OfdmParams {
        &self.params
    }

    /// BPSK on every occupied sub-carrier — entries at pilot positions
    /// are ignored by the transmitter / receiver but follow the same
    /// index convention as [`OfdmParams::subcarrier_indices`].
    pub fn bits_per_subcarrier(&self) -> Vec<u8> {
        vec![1; self.params.subcarrier_indices().len()]
    }

    /// Sample count of one OFDM symbol (FFT body + cyclic prefix).
    pub fn symbol_size_samples(&self) -> usize {
        self.params.fft_size() + self.params.cp_len()
    }

    /// Data bits carried by one BPSK OFDM symbol (one per data
    /// sub-carrier).
    fn data_bits_per_symbol(&self) -> usize {
        self.params.data_indices().len()
    }

    /// OFDM symbols needed to carry one FEC block's coded bits, with
    /// the last symbol zero-padded to full data capacity.
    fn symbols_per_block(&self) -> usize {
        self.fec
            .block_coded_bits()
            .div_ceil(self.data_bits_per_symbol())
    }

    /// Modulate one OFDM symbol from `coded_bits` (≤ one symbol's data
    /// capacity), zero-padding the remaining data sub-carriers.
    fn modulate_coded_symbol(&self, coded_bits: &[u8]) -> Vec<f32> {
        let bits_per_sc = self.bits_per_subcarrier();
        let mut sym_bits = coded_bits.to_vec();
        sym_bits.resize(self.data_bits_per_symbol(), 0);
        let tx = OfdmTransmitter::new(&self.params);
        tx.modulate_one_symbol(&sym_bits, &bits_per_sc)
    }

    /// Demodulate one OFDM symbol to soft LLRs (one per data
    /// sub-carrier; positive ⇒ bit 0). LLRs are NOT hard-decided here —
    /// the soft values flow straight to `FecCodec::decode_soft`.
    fn demodulate_coded_symbol(&self, samples: &[f32]) -> Vec<f32> {
        let bits_per_sc = self.bits_per_subcarrier();
        let rx = OfdmReceiver::new(&self.params);
        rx.demodulate_one_symbol(samples, &bits_per_sc)
    }

    /// Modulate one OFDM symbol carrying `payload` through the coded
    /// path. Errors with [`PhyError::PayloadTooLarge`] when the payload
    /// exceeds u16::MAX bytes.
    pub fn transmit(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        self.transmit_multi(payload)
    }

    /// Demodulate a coded frame back to its byte payload.
    pub fn receive(&self, samples: &[f32]) -> Result<Vec<u8>, PhyError> {
        self.receive_multi(samples)
    }

    /// Modulate `payload` (≤ u16::MAX bytes) through the coded floor
    /// path: codeword-spanning framing → per-block FEC encode → BPSK
    /// OFDM symbols.
    ///
    /// Errors with [`PhyError::PayloadTooLarge`] when the payload
    /// exceeds u16::MAX bytes.
    pub fn transmit_multi(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        if payload.len() > u16::MAX as usize {
            return Err(PhyError::PayloadTooLarge {
                actual: payload.len(),
                capacity: u16::MAX as usize,
            });
        }
        let block_info = self.fec.block_info_bits();
        let info = frame_info_bits(payload, block_info);
        let dps = self.data_bits_per_symbol();
        let mut out = Vec::new();
        for block in info.chunks(block_info) {
            let coded = self.fec.encode(block);
            for chunk in coded.chunks(dps) {
                out.extend_from_slice(&self.modulate_coded_symbol(chunk));
            }
        }
        Ok(out)
    }

    /// Demodulate a coded frame produced by [`Self::transmit_multi`].
    /// Block 0 is decoded first to read the length/block-count header,
    /// then the remaining blocks; the concatenated info-bit stream is
    /// deframed back to the payload.
    ///
    /// Returns [`PhyError::FrameDetect`] when the input is shorter than
    /// one coded block or a block is truncated, and
    /// [`PhyError::FecDecode`] when the codec rejects a block.
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
            llrs.truncate(block_coded);
            self.fec
                .decode_soft(&llrs)
                .map_err(|e| PhyError::FecDecode(e.to_string()))
        };
        let block_info = self.fec.block_info_bits();
        let first = decode_block(0)?;
        let n_blocks = blocks_from_first_block(&first, block_info);
        let mut info = first;
        for blk in 1..n_blocks {
            info.extend_from_slice(&decode_block(blk)?);
        }
        deframe_info_bits(&info)
    }

    /// Modulate one coded frame carrying `payload`, prefixed with the
    /// Zadoff-Chu preamble defined in [`crate::sync::preamble`]. Output
    /// layout:
    ///
    /// ```text
    /// [preamble (192 samples)][coded OFDM symbols]
    /// ```
    ///
    /// This is the over-the-air frame format. Bare [`Self::transmit`]
    /// emits only the symbols — suitable for back-to-back loopback
    /// where alignment is implicit. Pair this with
    /// [`Self::receive_with_sync`] on receive.
    pub fn transmit_with_preamble(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        let preamble = PreambleGenerator::new().generate();
        debug_assert_eq!(
            preamble.len(),
            PREAMBLE_LEN_SAMPLES,
            "preamble length pin diverged from PREAMBLE_LEN_SAMPLES",
        );
        let body = self.transmit(payload)?;
        let mut out = Vec::with_capacity(preamble.len() + body.len());
        out.extend_from_slice(&preamble);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Scan `samples` for the preamble, then decode the coded frame that
    /// follows. Returns `(preamble_start_sample, payload)`.
    ///
    /// Returns [`PhyError::FrameDetect`] when:
    /// - no preamble is found above the detector's correlation
    ///   threshold (per [`PreambleDetector::scan`]'s docs);
    /// - the body after the preamble is truncated.
    pub fn receive_with_sync(&self, samples: &[f32]) -> Result<(usize, Vec<u8>), PhyError> {
        self.receive_multi_with_sync(samples)
    }

    /// Modulate a coded frame prefixed with the Zadoff-Chu preamble.
    /// Output layout:
    ///
    /// ```text
    /// [ preamble (192 samples) ][ coded OFDM symbols ]
    /// ```
    ///
    /// Composition of [`Self::transmit_multi`] + the preamble. This is
    /// the **over-the-air frame format** — pairs with
    /// [`Self::receive_multi_with_sync`] on the decode side.
    pub fn transmit_multi_with_preamble(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        let preamble = PreambleGenerator::new().generate();
        debug_assert_eq!(preamble.len(), PREAMBLE_LEN_SAMPLES);
        let body = self.transmit_multi(payload)?;
        let mut out = Vec::with_capacity(preamble.len() + body.len());
        out.extend_from_slice(&preamble);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Scan `samples` for the preamble, then decode the coded frame that
    /// follows. Returns `(preamble_start_sample, payload)`.
    ///
    /// Returns [`PhyError::FrameDetect`] when:
    /// - no preamble is found above the detector's correlation
    ///   threshold;
    /// - no body samples follow the preamble, or the body is truncated.
    pub fn receive_multi_with_sync(&self, samples: &[f32]) -> Result<(usize, Vec<u8>), PhyError> {
        let detector = PreambleDetector::new();
        let detection = detector.scan(samples).ok_or_else(|| {
            PhyError::FrameDetect(
                "preamble not detected in input (signal too weak or no preamble \
                 present); pass a longer/cleaner capture"
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
        let payload = self.receive_multi(body)?;
        Ok((detection.start_sample, payload))
    }
}

impl Default for WidebandLowDensityFloor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coded_round_trip_identity_fec_various_lengths() {
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
            assert_eq!(
                decoded,
                payload,
                "coded round-trip for {} bytes",
                payload.len()
            );
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

    #[test]
    fn transmit_with_preamble_starts_with_preamble_samples() {
        // First PREAMBLE_LEN_SAMPLES of the output must EQUAL the
        // PreambleGenerator's output bit-for-bit. Confirms the order
        // of the layout is [preamble][symbol], not the reverse.
        let floor = WidebandLowDensityFloor::new();
        let preamble_expected = PreambleGenerator::new().generate();
        let samples = floor.transmit_with_preamble(b"X").unwrap();
        for (i, (&got, &want)) in samples
            .iter()
            .take(PREAMBLE_LEN_SAMPLES)
            .zip(preamble_expected.iter())
            .enumerate()
        {
            assert!(
                (got - want).abs() < 1e-6,
                "preamble sample {i} differs: got {got}, want {want}",
            );
        }
    }

    #[test]
    fn preamble_roundtrip_aligned_recovers_payload() {
        // Clean back-to-back: encode → preamble + symbols → decode.
        let floor = WidebandLowDensityFloor::new();
        let payload = b"SYNC!";
        let samples = floor.transmit_with_preamble(payload).unwrap();
        let (start, decoded) = floor.receive_with_sync(&samples).unwrap();
        assert_eq!(start, 0, "preamble should start at sample 0");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn preamble_roundtrip_with_leading_silence_recovers_payload() {
        // Operator captured a WAV that includes some leading silence
        // before the preamble. The detector should find the preamble
        // at the correct offset and slice the body correctly.
        let floor = WidebandLowDensityFloor::new();
        let payload = b"OFFSET";
        let core = floor.transmit_with_preamble(payload).unwrap();
        let leading_silence = vec![0.0_f32; 1_000];
        let mut samples = leading_silence.clone();
        samples.extend_from_slice(&core);
        let (start, decoded) = floor.receive_with_sync(&samples).unwrap();
        let offset_err = (start as i64 - leading_silence.len() as i64).unsigned_abs() as usize;
        assert!(
            offset_err <= 2,
            "detected start {} should be within ±2 of leading silence {} samples",
            start,
            leading_silence.len()
        );
        assert_eq!(decoded, payload);
    }

    #[test]
    fn preamble_roundtrip_with_trailing_noise_recovers_payload() {
        // Capture continues past the body — e.g. trailing radio noise,
        // key-up tail. The decoder should ignore everything after the
        // declared frame.
        let floor = WidebandLowDensityFloor::new();
        let payload = b"TAIL";
        let core = floor.transmit_with_preamble(payload).unwrap();
        let mut samples = core.clone();
        let mut state: u32 = 0xDEAD_BEEF;
        for _ in 0..5_000 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let v = ((state >> 16) as i16 as f32) / 32_768.0 * 0.05;
            samples.push(v);
        }
        let (start, decoded) = floor.receive_with_sync(&samples).unwrap();
        assert_eq!(start, 0, "preamble should still align at sample 0");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn receive_with_sync_returns_frame_detect_on_pure_silence() {
        let floor = WidebandLowDensityFloor::new();
        let silence = vec![0.0_f32; 10_000];
        let err = floor.receive_with_sync(&silence).unwrap_err();
        assert!(matches!(err, PhyError::FrameDetect(_)));
    }

    #[test]
    fn receive_with_sync_returns_frame_detect_on_random_noise() {
        // High-amplitude random noise should NOT have correlation above
        // the detector threshold; if it does find a spurious peak we
        // just assert no panic.
        let floor = WidebandLowDensityFloor::new();
        let mut samples = Vec::with_capacity(10_000);
        let mut state: u32 = 0x1234_5678;
        for _ in 0..10_000 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let v = ((state >> 16) as i16 as f32) / 32_768.0;
            samples.push(v);
        }
        let result = floor.receive_with_sync(&samples);
        let _ = result;
    }

    #[test]
    fn transmit_with_preamble_length_is_preamble_plus_body() {
        // Output is preamble (192) + the coded body. Cross-check that
        // transmit_with_preamble is exactly the bare body plus the
        // preamble length.
        let floor = WidebandLowDensityFloor::new();
        let body = floor.transmit(b"hi").unwrap();
        let samples = floor.transmit_with_preamble(b"hi").unwrap();
        assert_eq!(samples.len(), PREAMBLE_LEN_SAMPLES + body.len());
    }

    #[test]
    fn bare_transmit_is_deterministic() {
        // transmit's output is deterministic for a given payload.
        let floor = WidebandLowDensityFloor::new();
        let a = floor.transmit(b"OLD").unwrap();
        let b = floor.transmit(b"OLD").unwrap();
        assert_eq!(a, b, "transmit() must be deterministic");
        let with_preamble = floor.transmit_with_preamble(b"OLD").unwrap();
        assert_eq!(with_preamble.len(), a.len() + PREAMBLE_LEN_SAMPLES);
    }

    // ─── Multi-symbol framing roundtrips ────────────────────────────

    fn assert_multi_roundtrip(payload: &[u8]) {
        let floor = WidebandLowDensityFloor::new();
        let samples = floor.transmit_multi(payload).unwrap();
        let decoded = floor.receive_multi(&samples).unwrap();
        assert_eq!(
            decoded,
            payload,
            "roundtrip failed for {}-byte payload",
            payload.len()
        );
    }

    #[test]
    fn multi_roundtrip_empty_payload() {
        assert_multi_roundtrip(b"");
    }

    #[test]
    fn multi_roundtrip_1_byte_payload() {
        assert_multi_roundtrip(b"X");
    }

    #[test]
    fn multi_roundtrip_5_byte_payload() {
        assert_multi_roundtrip(b"HELLO");
    }

    #[test]
    fn multi_roundtrip_7_byte_payload() {
        assert_multi_roundtrip(b"BORDER!");
    }

    #[test]
    fn multi_roundtrip_8_byte_payload() {
        assert_multi_roundtrip(b"OVERFLOW");
    }

    #[test]
    fn multi_roundtrip_10_byte_payload() {
        assert_multi_roundtrip(b"TenBytePay");
    }

    #[test]
    fn multi_roundtrip_100_byte_payload() {
        let payload: Vec<u8> = (0..100).map(|i| (i % 251) as u8).collect();
        assert_multi_roundtrip(&payload);
    }

    #[test]
    fn multi_roundtrip_1000_byte_payload() {
        // Stress: tests that no off-by-one in the block count + length
        // header arithmetic shifts the alignment.
        let payload: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        assert_multi_roundtrip(&payload);
    }

    #[test]
    fn multi_roundtrip_preserves_trailing_zero_bytes() {
        // The length header keeps trailing 0x00 recoverable.
        let payload = b"AB\x00\x00\x00";
        assert_multi_roundtrip(payload);
    }

    #[test]
    fn multi_roundtrip_preserves_leading_zero_bytes() {
        let payload = b"\x00\x00DATA";
        assert_multi_roundtrip(payload);
    }

    #[test]
    fn multi_roundtrip_all_zeros_payload() {
        assert_multi_roundtrip(&[0u8; 30]);
    }

    #[test]
    fn transmit_multi_payload_too_large_rejects() {
        let floor = WidebandLowDensityFloor::new();
        let oversized = vec![0u8; u16::MAX as usize + 1];
        let err = floor.transmit_multi(&oversized).unwrap_err();
        assert!(matches!(err, PhyError::PayloadTooLarge { .. }));
    }

    #[test]
    fn receive_multi_rejects_input_shorter_than_one_block() {
        let floor = WidebandLowDensityFloor::new();
        let too_short = vec![0.0_f32; 10];
        let err = floor.receive_multi(&too_short).unwrap_err();
        assert!(matches!(err, PhyError::FrameDetect(_)));
    }

    #[test]
    fn transmit_multi_does_not_use_preamble() {
        // First samples of transmit_multi should NOT match the
        // Zadoff-Chu preamble. Preamble integration is a separate path.
        let floor = WidebandLowDensityFloor::new();
        let preamble = PreambleGenerator::new().generate();
        let multi = floor.transmit_multi(b"AB").unwrap();
        let mut matches = 0;
        for (a, b) in multi.iter().zip(preamble.iter()).take(50) {
            if (a - b).abs() < 1e-6 {
                matches += 1;
            }
        }
        assert!(
            matches < 30,
            "multi output {matches}/50 samples match preamble — looks preamble-prefixed"
        );
    }

    // ─── Multi-symbol + preamble composition ────────────────────────

    fn assert_multi_sync_roundtrip(payload: &[u8]) {
        let floor = WidebandLowDensityFloor::new();
        let samples = floor.transmit_multi_with_preamble(payload).unwrap();
        let (start, decoded) = floor.receive_multi_with_sync(&samples).unwrap();
        assert_eq!(start, 0, "preamble should start at sample 0");
        assert_eq!(
            decoded,
            payload,
            "multi+preamble roundtrip failed for {}-byte payload",
            payload.len()
        );
    }

    #[test]
    fn multi_with_preamble_length_equals_preamble_plus_multi() {
        let floor = WidebandLowDensityFloor::new();
        let multi = floor.transmit_multi(b"hi").unwrap();
        let combined = floor.transmit_multi_with_preamble(b"hi").unwrap();
        assert_eq!(combined.len(), PREAMBLE_LEN_SAMPLES + multi.len());
    }

    #[test]
    fn multi_with_preamble_roundtrip_1_byte_payload() {
        assert_multi_sync_roundtrip(b"X");
    }

    #[test]
    fn multi_with_preamble_roundtrip_9_byte_payload() {
        assert_multi_sync_roundtrip(b"NINEBYTES");
    }

    #[test]
    fn multi_with_preamble_roundtrip_100_byte_payload() {
        let payload: Vec<u8> = (0..100).map(|i| (i % 251) as u8).collect();
        assert_multi_sync_roundtrip(&payload);
    }

    #[test]
    fn multi_with_preamble_roundtrip_1000_byte_payload() {
        let payload: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        assert_multi_sync_roundtrip(&payload);
    }

    #[test]
    fn multi_with_preamble_roundtrip_empty_payload() {
        assert_multi_sync_roundtrip(b"");
    }

    #[test]
    fn multi_with_preamble_roundtrip_preserves_trailing_zeros() {
        assert_multi_sync_roundtrip(b"AB\x00\x00\x00");
    }

    #[test]
    fn multi_with_preamble_handles_leading_silence() {
        let floor = WidebandLowDensityFloor::new();
        let payload: Vec<u8> = (0..50).map(|i| (i * 7 % 251) as u8).collect();
        let core = floor.transmit_multi_with_preamble(&payload).unwrap();
        let leading_silence = vec![0.0_f32; 2_000];
        let mut samples = leading_silence.clone();
        samples.extend_from_slice(&core);
        let (start, decoded) = floor.receive_multi_with_sync(&samples).unwrap();
        let offset_err = (start as i64 - leading_silence.len() as i64).unsigned_abs() as usize;
        assert!(
            offset_err <= 2,
            "detected start {start} should be within ±2 of leading silence {} samples",
            leading_silence.len()
        );
        assert_eq!(decoded, payload);
    }

    #[test]
    fn multi_with_preamble_returns_frame_detect_on_silence() {
        let floor = WidebandLowDensityFloor::new();
        let silence = vec![0.0_f32; 20_000];
        let err = floor.receive_multi_with_sync(&silence).unwrap_err();
        assert!(matches!(err, PhyError::FrameDetect(_)));
    }

    #[test]
    fn multi_with_preamble_starts_with_preamble_samples() {
        let floor = WidebandLowDensityFloor::new();
        let preamble = PreambleGenerator::new().generate();
        let combined = floor.transmit_multi_with_preamble(b"hi").unwrap();
        for (i, (&got, &want)) in combined
            .iter()
            .take(PREAMBLE_LEN_SAMPLES)
            .zip(preamble.iter())
            .enumerate()
        {
            assert!(
                (got - want).abs() < 1e-6,
                "preamble sample {i} differs: got {got}, want {want}"
            );
        }
    }

    // ─── coded-path error rejection (Task B3) ──────────────────────────

    #[test]
    fn receive_multi_rejects_truncated_multiblock() {
        // Block 0's header declares many blocks; supplying only block 0's
        // samples must surface FrameDetect (the later blocks are truncated).
        let floor = WidebandLowDensityFloor::new();
        let payload: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
        let full = floor.transmit_multi(&payload).unwrap();
        let one_block = floor.symbol_size_samples() * floor.symbols_per_block();
        assert!(full.len() > one_block, "test needs a multi-block payload");
        let truncated = &full[..one_block];
        assert!(matches!(
            floor.receive_multi(truncated),
            Err(PhyError::FrameDetect(_))
        ));
    }

    #[test]
    fn receive_multi_with_sync_rejects_truncated_after_preamble() {
        let floor = WidebandLowDensityFloor::new();
        let payload: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
        let full = floor.transmit_multi_with_preamble(&payload).unwrap();
        let trunc_len =
            PREAMBLE_LEN_SAMPLES + floor.symbol_size_samples() * floor.symbols_per_block();
        let truncated = &full[..trunc_len];
        assert!(matches!(
            floor.receive_multi_with_sync(truncated),
            Err(PhyError::FrameDetect(_))
        ));
    }
}
