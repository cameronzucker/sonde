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
use crate::sync::carrier_offset::{analytic_signal, derotate};
use crate::sync::preamble::{PreambleDetector, PreambleGenerator};

/// Sample count of the Schmidl-Cox repeated-pair preamble emitted by
/// [`WidebandLowDensityFloor::transmit_with_preamble`]. Re-exported from
/// [`crate::sync::preamble`] so the two never drift.
pub const PREAMBLE_LEN_SAMPLES: usize = crate::sync::preamble::PREAMBLE_LEN;

/// Early-alignment guard: the symbol FFT window starts this many samples
/// EARLIER than the detected preamble end. The phase-invariant (I/Q) correlator
/// detects the frame accurately, but can still lock the *delayed* path of a
/// multipath channel (Good Δτ=24, Moderate Δτ=48 samples @ 48 kHz), which would
/// bias the window late and spill into the next symbol (ISI). Backing off a
/// small margin biases the window EARLY — within the 512-sample cyclic prefix,
/// where the offset is only a gentle per-sub-carrier phase ramp the channel
/// estimator absorbs (early is safe, late is ISI).
///
/// 32 was chosen empirically (sonde-64w.3): a 40-seed Watterson Good/Moderate
/// sweep decodes every detected frame at any guard in [0, 32], and a small early
/// bias absorbs delayed-path lock. The prior value of 128 over-steepened the
/// phase ramp on *accurately* detected frames and itself cost decodes.
///
/// NOTE: timing is NOT the dominant fading failure mode — frequency-selective
/// nulls are (see [`crate::ofdm_main::receiver`] and the fading gate). This
/// guard is a robustness margin for delayed-path lock, not the fading fix.
const SYNC_WINDOW_GUARD_SAMPLES: usize = 32;

/// Raised-cosine inter-symbol windowing roll-off, in samples. Confined to the
/// 512-sample cyclic-prefix guard so the FFT body stays intact (decode is
/// bit-identical); only the out-of-band spectrum is shaped. 128 ≈ 2.7 ms taper.
const WINDOW_ROLLOFF_SAMPLES: usize = 128;

/// Target peak-to-average power ratio (dB) for the soft-clipped OFDM body.
/// Uncompensated OFDM peaks at ~22 dB (≈10·log10 of the occupied sub-carrier
/// count) — far too high for an SSB PA. A soft clip to 12 dB cuts ~10 dB of PAPR
/// while keeping out-of-band regrowth within the −26 dBc mask (measured in
/// waveform_psd_papr.rs) and leaving decode intact at the high-SNR clean point.
/// Clipping below ~12 dB pushes the spectral regrowth past the mask.
const PAPR_TARGET_DB: f32 = 12.0;

/// Soft-clip `signal` so its peak-to-average power ratio does not exceed
/// `target_papr_db`. The threshold is derived from the signal's own mean power;
/// samples beyond ±A are limited to ±A. Clipping corrupts the FFT body (trading
/// EVM for PAPR) — acceptable for the strong-FEC floor — unlike the lossless
/// inter-symbol windowing.
fn soft_clip_to_papr(signal: &[f32], target_papr_db: f32) -> Vec<f32> {
    if signal.is_empty() {
        return Vec::new();
    }
    let mean_pow = signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32;
    let amp = (mean_pow * 10.0_f32.powf(target_papr_db / 10.0)).sqrt();
    signal.iter().map(|&s| s.clamp(-amp, amp)).collect()
}

/// Three-way outcome of a sync+decode scan, distinguishing "the window held
/// only noise" from "we acquired the preamble but the frame did not decode."
///
/// [`WidebandLowDensityFloor::receive_multi_with_sync`] collapses the latter two
/// into a flat `Err`, which is correct for a caller that just wants the payload.
/// But subsystem #5's link-adaptation needs the distinction: a detected-but-
/// failed frame is a real frame error (it must count toward FER), whereas pure
/// noise is not (counting it would drown the error rate in silence). The floor
/// demod already knows which case it is — it acquires the preamble *before*
/// attempting FEC — so this just surfaces that internal state.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncDecodeOutcome {
    /// No preamble above the detector threshold: the window held only noise.
    NoSignal,
    /// The preamble was acquired (sync locked at `start_sample`) but the coded
    /// frame that followed did not decode — truncated body, or the FEC rejected
    /// a block. A genuine frame error for FER accounting.
    DetectedDecodeFailed {
        /// Sample index where the preamble was detected.
        start_sample: usize,
        /// Channel SNR referenced to 2500 Hz, dB, measured from the (failed)
        /// frame's body symbols. `Some` even though the decode failed — a
        /// detected-but-failed over carries a real channel measurement (Codex
        /// review H1, survivorship bias); `None` only when no body followed the
        /// preamble.
        snr_2500_db: Option<f32>,
    },
    /// Clean decode: preamble acquired at `start_sample`, payload recovered.
    Frame {
        /// Sample index where the preamble was detected.
        start_sample: usize,
        /// FEC-corrected payload bytes.
        payload: Vec<u8>,
        /// Channel SNR referenced to 2500 Hz, dB, measured from the frame body
        /// (estimator-domain; see [`crate::ofdm_main::receiver::OfdmReceiver::estimate_snr_2500_db`]).
        snr_2500_db: Option<f32>,
    },
}

/// Private intermediate: the result of locating + CFO-correcting a frame body,
/// shared by [`WidebandLowDensityFloor::receive_multi_with_sync`] and
/// [`WidebandLowDensityFloor::receive_multi_with_sync_scan`] so the analytic-
/// lift / detect / derotate DSP is written once.
enum SyncLocate {
    /// No preamble above threshold.
    NoPreamble,
    /// Preamble acquired at `start_sample` but no body samples follow it.
    NoBody { start_sample: usize },
    /// Preamble acquired; `corrected[body_start..]` is the derotated body.
    Body {
        start_sample: usize,
        corrected: Vec<f32>,
        body_start: usize,
    },
}

/// Default robustness floor: wide-band OFDM, BPSK on every occupied
/// sub-carrier, with a composed FEC codec. Strategic posture is "go
/// wider, not denser" — see overview §5.A.1.
pub struct WidebandLowDensityFloor {
    params: OfdmParams,
    // `+ Send` so the floor stays `Send` for downstream worker-thread
    // adapters (e.g. sonde-phy-runtime's `FloorWaveform`); a bare
    // `dyn` trait object would strip the auto-trait. All codecs are Send.
    fec: Box<dyn FecCodec + Send>,
    /// When `Some`, the demod uses this FIXED noise variance instead of the
    /// per-symbol empty-bin estimate. Production leaves it `None`; only the
    /// sonde-gtg differential gate's control arm sets it (to `0.1`).
    n0_override: Option<f32>,
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
            n0_override: None,
        }
    }

    /// Floor with an injected concrete codec (e.g. `FloorRate14Codec`).
    pub fn with_fec(fec: Box<dyn FecCodec + Send>) -> Self {
        Self {
            params: OfdmParams::for_mode(OfdmModeName::Wide),
            fec,
            n0_override: None,
        }
    }

    /// Force a FIXED demod noise variance instead of the per-symbol empty-bin
    /// estimate. Diagnostic / differential-gate use only — the production demod
    /// estimates `n0` per symbol (see [`crate::ofdm_main::receiver`]).
    pub fn with_fixed_n0(mut self, n0: f32) -> Self {
        self.n0_override = Some(n0);
        self
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

    /// Demodulate the `spb` OFDM symbols of one coded block (starting at sample
    /// `base`) to per-symbol soft LLRs, TIME-SMOOTHING the pilot channel estimate
    /// across just those symbols ([`OfdmReceiver::demodulate_frame`]). LLRs are
    /// NOT hard-decided here — the soft values flow straight to
    /// `FecCodec::decode_soft`.
    ///
    /// Frame-level (not per-symbol) demod is the fix for sonde-vb9: the floor's
    /// channel is slowly varying, so averaging each pilot across neighbouring
    /// symbols recovers the ~4 dB the independent per-symbol estimate threw away
    /// at the coded path's low per-symbol SNR. Smoothing is scoped to the BLOCK's
    /// own symbols (not the whole capture) so trailing non-signal symbols — the
    /// key-up tail / appended silence past the frame — can never bleed a noise-
    /// only pilot into a real symbol's estimate.
    ///
    /// Returns `FrameDetect` if the block's symbols run past `samples`.
    fn demodulate_block_symbols(
        &self,
        samples: &[f32],
        base: usize,
        spb: usize,
    ) -> Result<Vec<Vec<f32>>, PhyError> {
        let sym = self.symbol_size_samples();
        let mut slices: Vec<&[f32]> = Vec::with_capacity(spb);
        for s in 0..spb {
            let start = base + s * sym;
            if start + sym > samples.len() {
                return Err(PhyError::FrameDetect("coded block truncated".into()));
            }
            slices.push(&samples[start..start + sym]);
        }
        let bits_per_sc = self.bits_per_subcarrier();
        let rx = OfdmReceiver::with_n0_override(&self.params, self.n0_override);
        Ok(rx.demodulate_frame(&slices, &bits_per_sc))
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
        let mut symbols: Vec<Vec<f32>> = Vec::new();
        for block in info.chunks(block_info) {
            let coded = self.fec.encode(block);
            for chunk in coded.chunks(dps) {
                symbols.push(self.modulate_coded_symbol(chunk));
            }
        }
        let windowed = self.window_and_concat(&symbols);
        Ok(soft_clip_to_papr(&windowed, PAPR_TARGET_DB))
    }

    /// Concatenate OFDM symbols with raised-cosine inter-symbol windowing
    /// (overlap-add) to suppress the spectral sidelobes that a hard,
    /// rectangular symbol boundary produces.
    ///
    /// The roll-off is confined to the dropped cyclic prefix: each symbol gets a
    /// cyclic suffix (a copy of the FFT body's head, the natural continuation of
    /// the periodic body), both edges are tapered by a raised-cosine ramp of
    /// [`WINDOW_ROLLOFF_SAMPLES`], and consecutive symbols overlap by that
    /// roll-off. The 2048-sample FFT body is left untouched and the receiver's
    /// `CP + body` stride is unchanged, so demodulation is bit-identical — only
    /// the out-of-band spectrum changes. Output length is
    /// `n_symbols·stride + roll-off`.
    fn window_and_concat(&self, symbols: &[Vec<f32>]) -> Vec<f32> {
        let rho = WINDOW_ROLLOFF_SAMPLES;
        let sym = self.symbol_size_samples();
        let cp = self.params.cp_len();
        if symbols.is_empty() {
            return Vec::new();
        }
        // Rising raised-cosine ramp r[i] over the roll-off; the falling edge is
        // its mirror. (Cross-fade smoothness, not power-complementarity, is what
        // suppresses sidelobes; the region is dropped by the receiver anyway.)
        let ramp: Vec<f32> = (0..rho)
            .map(|i| {
                let x = (i as f32 + 0.5) / rho as f32;
                0.5 - 0.5 * (std::f32::consts::PI * x).cos()
            })
            .collect();
        let total = symbols.len() * sym + rho;
        let mut out = vec![0.0_f32; total];
        for (k, s) in symbols.iter().enumerate() {
            debug_assert_eq!(s.len(), sym);
            // Extended frame: symbol + cyclic suffix (= first `rho` of the FFT
            // body, the periodic continuation past the symbol's end).
            let mut frame = Vec::with_capacity(sym + rho);
            frame.extend_from_slice(s);
            frame.extend_from_slice(&s[cp..cp + rho]);
            for i in 0..rho {
                frame[i] *= ramp[i]; // ramp up (inside the CP guard)
                let tail = sym + rho - 1 - i;
                frame[tail] *= ramp[i]; // ramp down (the suffix)
            }
            let base = k * sym;
            for (i, &v) in frame.iter().enumerate() {
                out[base + i] += v;
            }
        }
        out
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
        // Per block: time-smooth the pilot channel estimate across the block's
        // own symbols (sonde-vb9), then assemble + decode. Scoping the smoothing
        // to the block keeps trailing non-signal symbols out of the average.
        let decode_block = |blk: usize| -> Result<Vec<u8>, PhyError> {
            let base = blk * spb * sym;
            let per_sym = self.demodulate_block_symbols(samples, base, spb)?;
            let mut llrs = Vec::with_capacity(spb * dps);
            for sym_llrs in &per_sym {
                llrs.extend_from_slice(sym_llrs);
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
        match self.locate_synced_body(samples) {
            SyncLocate::NoPreamble => Err(PhyError::FrameDetect(
                "preamble not detected in input (signal too weak or no preamble \
                 present); pass a longer/cleaner capture"
                    .to_string(),
            )),
            SyncLocate::NoBody { start_sample } => Err(PhyError::FrameDetect(format!(
                "preamble detected at sample {start_sample} but no body samples follow"
            ))),
            SyncLocate::Body {
                start_sample,
                corrected,
                body_start,
            } => {
                // Preserve the body decode's error variant (FrameDetect for a
                // truncated block, FecDecode for a codec reject) for callers
                // that distinguish them.
                let payload = self.receive_multi(&corrected[body_start..])?;
                Ok((start_sample, payload))
            }
        }
    }

    /// Like [`Self::receive_multi_with_sync`], but returns a three-way
    /// [`SyncDecodeOutcome`] that separates "no preamble found" ([`NoSignal`])
    /// from "preamble acquired but the frame failed to decode"
    /// ([`DetectedDecodeFailed`]). Subsystem #5 needs that split to count frame
    /// errors honestly (a detected-but-failed frame is a real error; silence is
    /// not). The decode work is identical to `receive_multi_with_sync`; only the
    /// outcome shape differs.
    ///
    /// [`NoSignal`]: SyncDecodeOutcome::NoSignal
    /// [`DetectedDecodeFailed`]: SyncDecodeOutcome::DetectedDecodeFailed
    pub fn receive_multi_with_sync_scan(&self, samples: &[f32]) -> SyncDecodeOutcome {
        match self.locate_synced_body(samples) {
            SyncLocate::NoPreamble => SyncDecodeOutcome::NoSignal,
            SyncLocate::NoBody { start_sample } => SyncDecodeOutcome::DetectedDecodeFailed {
                start_sample,
                snr_2500_db: None,
            },
            SyncLocate::Body {
                start_sample,
                corrected,
                body_start,
            } => {
                let body = &corrected[body_start..];
                // Measure SNR from the body symbols BEFORE attempting the decode,
                // so a detected-but-failed over still carries a channel reading
                // (Codex review H1 — decode-only SNR is survivorship-biased high).
                let snr_2500_db = self.estimate_body_snr(body);
                match self.receive_multi(body) {
                    Ok(payload) => SyncDecodeOutcome::Frame {
                        start_sample,
                        payload,
                        snr_2500_db,
                    },
                    Err(_) => SyncDecodeOutcome::DetectedDecodeFailed {
                        start_sample,
                        snr_2500_db,
                    },
                }
            }
        }
    }

    /// Channel SNR (dB, 2500 Hz reference) of a derotated frame `body`, measured
    /// across its OFDM symbols with the production estimator. `None` when the body
    /// holds less than one full symbol. Uses the same `n0_override` posture as the
    /// demod so a diagnostic fixed-`n0` arm does not silently change the report.
    fn estimate_body_snr(&self, body: &[f32]) -> Option<f32> {
        let sym = self.symbol_size_samples();
        if body.len() < sym {
            return None;
        }
        let n = body.len() / sym;
        let slices: Vec<&[f32]> = (0..n).map(|i| &body[i * sym..(i + 1) * sym]).collect();
        let rx = OfdmReceiver::with_n0_override(&self.params, self.n0_override);
        Some(rx.estimate_snr_2500_db(&slices).snr_2500_db)
    }

    /// Locate the preamble, derotate by the estimated CFO, and slice out the
    /// frame body — the shared front half of both sync-decode entry points.
    fn locate_synced_body(&self, samples: &[f32]) -> SyncLocate {
        // Two-stage synchronization (sonde-xhw.3), all on the analytic signal:
        //   1. Schmidl-Cox `M(d)` for CFO-invariant detection + coarse CFO,
        //   2. derotate + a sharp template MF for exact timing,
        // then derotate the whole capture in the time domain BEFORE the
        // per-symbol FFT. Without CFO correction the floor collapses above ~20 Hz
        // — a ±100 Hz offset is ~4 sub-carrier spacings, which slides the
        // spectrum off the pilot bins (the per-symbol pilot equalizer absorbs a
        // constant phase + a phase ramp, but not a frequency shift). Detection
        // must be CFO-invariant too: a template matched filter's magnitude
        // collapses below the noise floor at ±100 Hz, so we detect on `M(d)`.
        //
        // We pad the analytic lift on both ends so the Hilbert FFT's circular
        // wrap cannot contaminate the frame region (Codex Q3). `Re{analytic(x)}`
        // == `x`, so with a ~0 Hz estimate the projection is the identity and the
        // clean path stays bit-identical.
        const ANALYTIC_PAD: usize = 512;
        let sr = crate::audio_io::SAMPLE_RATE_HZ as f32;
        let mut padded = vec![0.0_f32; samples.len() + 2 * ANALYTIC_PAD];
        padded[ANALYTIC_PAD..ANALYTIC_PAD + samples.len()].copy_from_slice(samples);
        let mut analytic = analytic_signal(&padded);
        let det = match PreambleDetector::new().detect_analytic(&analytic, sr) {
            Some(det) => det,
            None => return SyncLocate::NoPreamble,
        };
        derotate(&mut analytic, det.cfo_hz, sr);
        let corrected: Vec<f32> = analytic[ANALYTIC_PAD..ANALYTIC_PAD + samples.len()]
            .iter()
            .map(|c| c.re)
            .collect();

        // `det.start_sample` is in padded coordinates; convert back.
        let start_sample = det.start_sample.saturating_sub(ANALYTIC_PAD);
        let body_start =
            (start_sample + PREAMBLE_LEN_SAMPLES).saturating_sub(SYNC_WINDOW_GUARD_SAMPLES);
        if body_start >= corrected.len() {
            return SyncLocate::NoBody { start_sample };
        }
        SyncLocate::Body {
            start_sample,
            corrected,
            body_start,
        }
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

    // ─── three-way sync scan (sonde-jt6) ───────────────────────────────

    #[test]
    fn sync_scan_reports_no_signal_on_silence() {
        // Pure noise must read as NoSignal, NOT a frame error — counting
        // silence toward FER would drown the link's adaptation signal.
        let floor = WidebandLowDensityFloor::new();
        let silence = vec![0.0_f32; 20_000];
        assert_eq!(
            floor.receive_multi_with_sync_scan(&silence),
            SyncDecodeOutcome::NoSignal
        );
    }

    #[test]
    fn sync_scan_reports_frame_on_clean_capture() {
        let floor = WidebandLowDensityFloor::new();
        let payload = b"scan ok";
        let samples = floor.transmit_multi_with_preamble(payload).unwrap();
        match floor.receive_multi_with_sync_scan(&samples) {
            SyncDecodeOutcome::Frame {
                start_sample,
                payload: got,
                ..
            } => {
                assert_eq!(start_sample, 0, "clean capture aligns at sample 0");
                assert_eq!(got, payload);
            }
            other => panic!("expected Frame, got {other:?}"),
        }
    }

    #[test]
    fn sync_scan_reports_detected_decode_failed_on_truncated_body() {
        // Same construction as the FrameDetect truncation test above: block 0's
        // header declares a multi-block frame but only one block's samples are
        // present. The preamble IS acquired (so this is not NoSignal); the
        // decode then fails on the missing blocks — a real frame error that must
        // count toward FER. This is the case the flat `Err` could not separate
        // from silence.
        let floor = WidebandLowDensityFloor::new();
        let payload: Vec<u8> = (0..600).map(|i| (i % 251) as u8).collect();
        let full = floor.transmit_multi_with_preamble(&payload).unwrap();
        let trunc_len =
            PREAMBLE_LEN_SAMPLES + floor.symbol_size_samples() * floor.symbols_per_block();
        let truncated = &full[..trunc_len];
        assert!(matches!(
            floor.receive_multi_with_sync_scan(truncated),
            SyncDecodeOutcome::DetectedDecodeFailed { .. }
        ));
    }
}
