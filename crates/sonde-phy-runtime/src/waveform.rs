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
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

/// One successfully demodulated frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFrame {
    /// FEC-corrected payload bytes (rate-1/4 LDPC via [`FloorWaveform`]'s
    /// injected `FloorRate14Codec`).
    pub payload: Vec<u8>,
    /// Mode family the frame was demodulated under.
    pub family: ModeFamily,
    /// Aggregate frame SNR in dB, when the waveform measures one. `None` until
    /// the floor exposes its sub-carrier SNR estimator (tracked by the same
    /// follow-up that lands `doppler_spread_hz`, per `phy_api`).
    pub frame_snr_db: Option<f32>,
}

/// A modulation/demodulation format. Implementors are `Send` so the runtime
/// worker thread can own one.
pub trait Waveform: Send {
    /// Modulate `payload` into a self-synchronising sample buffer (preamble +
    /// body) ready to hand to a [`crate::Radio`].
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError>;

    /// Scan `samples` for a frame and demodulate it. Returns `None` when no
    /// frame is present (the common case for an RX window that caught only
    /// noise). Returns `Some` on a clean decode.
    fn decode_scan(&self, samples: &[f32]) -> Option<DecodedFrame>;

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

    fn decode_scan(&self, samples: &[f32]) -> Option<DecodedFrame> {
        match self.inner.receive_multi_with_sync(samples) {
            Ok((_offset, payload)) => Some(DecodedFrame {
                payload,
                family: ModeFamily::RobustnessFloor,
                frame_snr_db: None,
            }),
            Err(_) => None,
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

        let frame = wf.decode_scan(&captured).expect("a frame is decoded");
        assert_eq!(frame.payload, payload);
        assert_eq!(frame.family, ModeFamily::RobustnessFloor);
    }

    #[test]
    fn floor_waveform_returns_none_on_pure_noise() {
        let wf = FloorWaveform::new();
        let silence = vec![0.0f32; 4096];
        assert!(wf.decode_scan(&silence).is_none());
    }
}
