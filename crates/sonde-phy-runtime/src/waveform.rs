//! The modulation/demodulation seam.
//!
//! [`Waveform`] is the single trait a new RF format implements. The HF
//! [`FloorWaveform`] wraps `sonde-phy`'s wide-band low-density floor. A
//! VHF/UHF FM variant ("Sonde FM") is a new `impl Waveform` — see the crate
//! README's "FM extension point". Nothing in this trait assumes SSB, a 2.4 kHz
//! passband, or HF fading; an FM waveform overrides [`Waveform::encode`] /
//! [`Waveform::decode_scan`] with FM-appropriate pre-emphasis handling,
//! deviation/PAPR control, and channel model.

use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::error::PhyError;
use sonde_phy::modes::ModeFamily;
use sonde_phy::robustness_floor::wideband_lowdensity::{
    SyncDecodeOutcome, WidebandLowDensityFloor,
};

/// One successfully demodulated frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFrame {
    /// FEC-corrected payload bytes (rate-1/4 LDPC via [`FloorWaveform`]'s
    /// injected `FloorRate14Codec`).
    pub payload: Vec<u8>,
    /// Mode family the frame was demodulated under.
    pub family: ModeFamily,
    /// Channel SNR referenced to a 2500 Hz noise bandwidth, in dB — the
    /// link-facing adaptation reference (see
    /// `docs/superpowers/specs/2026-06-15-phy-mode-adaptation-quality-design.md`).
    /// `None` when the waveform did not measure one.
    pub snr_2500_db: Option<f32>,
}

/// Outcome of [`Waveform::decode_scan`] on one RX window.
///
/// The three variants exist so [`crate::SondePhy`]'s RX pump can keep an honest
/// frame-error rate: a frame that was *detected but failed to decode* is a real
/// error and must count, whereas a window of silence is not. Collapsing both
/// into "no frame" (the old `Option<DecodedFrame>`) made the FER structurally
/// blind to RX failures, which starved subsystem #5's link adaptation.
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeScan {
    /// No frame present — the window caught only noise/silence.
    NoSignal,
    /// A frame was detected (sync/preamble acquired) but it failed to decode
    /// (FEC reject or truncated body). No payload is produced, but the runtime
    /// counts it as a frame error — and still carries the channel SNR measured
    /// from the failed over (`snr_2500_db`, dB, 2500 Hz reference; `None` when no
    /// body followed the preamble). Reporting SNR on failures too avoids the
    /// survivorship bias of measuring only clean decodes (Codex review H1).
    Detected {
        /// Channel SNR (dB, 2500 Hz reference) of the detected-but-failed over.
        snr_2500_db: Option<f32>,
    },
    /// A frame decoded cleanly.
    Frame(DecodedFrame),
}

/// A modulation/demodulation format. Implementors are `Send` so the runtime
/// worker thread can own one.
pub trait Waveform: Send {
    /// Modulate `payload` into a self-synchronising sample buffer (preamble +
    /// body) ready to hand to a [`crate::Radio`].
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError>;

    /// Scan `samples` for a frame and demodulate it. Returns
    /// [`DecodeScan::NoSignal`] when no frame is present (the common case for an
    /// RX window that caught only noise), [`DecodeScan::Detected`] when a frame
    /// was acquired but failed to decode (a real frame error), and
    /// [`DecodeScan::Frame`] on a clean decode.
    ///
    /// `decode_scan` is **mode-agnostic / self-synchronising** — it finds its own
    /// preamble with no external hint. That is what lets [`crate::SondePhy`]'s RX
    /// pump AUTO-DETECT the received mode across a registry of waveforms (run each
    /// candidate's `decode_scan`; whichever syncs wins), so a mid-session mode
    /// switch is never deafening (design 2026-06-15-phy-mode-adaptation-quality §3).
    fn decode_scan(&self, samples: &[f32]) -> DecodeScan;

    /// Cheap, **high-recall** pre-gate for the multi-waveform RX pump: does this
    /// window plausibly contain *this* waveform's signal? The pump skips
    /// [`Self::decode_scan`] (the expensive correlator + FEC) for waveforms whose
    /// `detect` returns `false`. It MUST err toward `true` — a false negative
    /// makes the receiver deaf to a real frame (costly); a false positive only
    /// wastes one decode attempt (cheap) (Codex review C3). The default is the
    /// safe `true` (always attempt the full decode); a waveform overrides it with
    /// a lightweight preamble-energy check once that pays for itself.
    fn detect(&self, _samples: &[f32]) -> bool {
        true
    }

    /// The mode family this waveform serves. Used by [`crate::SondePhy`] to
    /// route a `ModeHint` to the right waveform once more than one is
    /// registered (HF floor + FM).
    fn family(&self) -> ModeFamily;
}

/// HF wide-band low-density floor waveform (`floor-wblo`). Wraps
/// [`WidebandLowDensityFloor`] using its preamble + multi-symbol framing so
/// arbitrary-length payloads self-synchronise.
pub struct FloorWaveform {
    inner: WidebandLowDensityFloor,
}

impl FloorWaveform {
    /// Construct the floor waveform with its pinned Wide-mode params and the
    /// real rate-1/4 LDPC codec ([`FloorRate14Codec`]). The coded path is what
    /// lets the floor decode through Watterson fading (a frequency-selective
    /// null erases uncoded bits irrecoverably); the channel-aware demod turns a
    /// nulled sub-carrier into a low-confidence near-erasure the code bridges.
    pub fn new() -> Self {
        Self {
            inner: WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new())),
        }
    }
}

impl Default for FloorWaveform {
    fn default() -> Self {
        Self::new()
    }
}

impl Waveform for FloorWaveform {
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        self.inner.transmit_multi_with_preamble(payload)
    }

    fn decode_scan(&self, samples: &[f32]) -> DecodeScan {
        match self.inner.receive_multi_with_sync_scan(samples) {
            SyncDecodeOutcome::Frame {
                payload,
                snr_2500_db,
                ..
            } => DecodeScan::Frame(DecodedFrame {
                payload,
                family: ModeFamily::RobustnessFloor,
                snr_2500_db,
            }),
            SyncDecodeOutcome::DetectedDecodeFailed { snr_2500_db, .. } => {
                DecodeScan::Detected { snr_2500_db }
            }
            SyncDecodeOutcome::NoSignal => DecodeScan::NoSignal,
        }
    }

    fn family(&self) -> ModeFamily {
        ModeFamily::RobustnessFloor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_waveform_round_trips_a_payload() {
        let wf = FloorWaveform::new();
        let payload = b"sonde";
        let samples = wf.encode(payload).expect("encode");
        // Prepend leading silence to prove the decoder finds the preamble at a
        // non-zero offset (real captures never start exactly on the symbol
        // boundary).
        let mut captured = vec![0.0f32; 500];
        captured.extend_from_slice(&samples);

        match wf.decode_scan(&captured) {
            DecodeScan::Frame(frame) => {
                assert_eq!(frame.payload, payload);
                assert_eq!(frame.family, ModeFamily::RobustnessFloor);
            }
            other => panic!("expected a decoded frame, got {other:?}"),
        }
    }

    #[test]
    fn floor_waveform_reports_no_signal_on_pure_noise() {
        let wf = FloorWaveform::new();
        let silence = vec![0.0f32; 4096];
        assert_eq!(wf.decode_scan(&silence), DecodeScan::NoSignal);
    }
}
