//! The modulation/demodulation seam.
//!
//! [`Waveform`] is the single trait a new RF format implements. The HF
//! [`FloorWaveform`] wraps `sonde-phy`'s wide-band low-density floor. A
//! VHF/UHF FM variant ("Sonde FM") is a new `impl Waveform` — see the crate
//! README's "FM extension point". Nothing in this trait assumes SSB, a 2.4 kHz
//! passband, or HF fading; an FM waveform overrides [`Waveform::encode`] /
//! [`Waveform::decode_scan`] with FM-appropriate pre-emphasis handling,
//! deviation/PAPR control, and channel model.

use sonde_fec::codec::{FloorRate14Codec, OfdmAdaptiveCodec};
use sonde_fec::codes::{BlockN, WifiLdpcRate};
use sonde_phy::error::PhyError;
use sonde_phy::modes::ModeFamily;
use sonde_phy::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use sonde_phy::robustness_floor::narrow_fsk::{NarrowFskFloor, NfskDecode};
use sonde_phy::robustness_floor::wideband_lowdensity::{
    SyncDecodeOutcome, WidebandLowDensityFloor,
};

/// The standard Sonde waveform registry — the full adaptation ladder, fastest
/// rung first (RX try-order): `ofdm-wide` → `ofdm-mid` → `ofdm-narrow` →
/// `floor-wblo` → `floor-nfsk`. Hand this to [`crate::SondePhy::with_waveforms`]
/// for a runtime that auto-detects + per-mode-routes the whole ladder. All five
/// are hardware-free (the radio is the only hardware seam).
pub fn standard_waveforms() -> Vec<Box<dyn Waveform>> {
    vec![
        Box::new(OfdmMainWaveform::wide()),
        Box::new(OfdmMainWaveform::mid()),
        Box::new(OfdmMainWaveform::narrow()),
        Box::new(FloorWaveform::new()),
        Box::new(NfskWaveform::new()),
    ]
}

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

    /// The stable mode identity this waveform implements (e.g. `"ofdm-wide"`,
    /// `"floor-wblo"`), matching [`sonde_phy::modes::ModeDescriptor::short_name`].
    /// `SondePhy` uses it to route a resolved mode to the *specific* waveform —
    /// not just the family — so a multi-rung ladder within one family
    /// (ofdm-narrow/mid/wide) selects the right rung on TX and labels the right
    /// mode on RX. `None` means "family-routed only" (a waveform that is the sole
    /// member of its family doesn't need a name).
    fn mode_name(&self) -> Option<&'static str> {
        None
    }

    /// Net info bitrate (information bits per second) this mode carries, used by
    /// the runtime to derive the over's Eb/N0 from the reported `SNR_2500`
    /// (`Eb/N0 = SNR_2500 − 10log10(R_info / 2500)`). `None` when unknown.
    fn info_bitrate_bps(&self) -> Option<f32> {
        None
    }

    /// Stable id of the SNR estimator this waveform uses (e.g. `"ofdm-pilot"`,
    /// `"nfsk-tone"`) — per-family estimator bias is real, so the link knows which
    /// estimator domain a reported SNR came from (Codex review C5).
    fn estimator_id(&self) -> &'static str {
        "none"
    }
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

    fn mode_name(&self) -> Option<&'static str> {
        Some("floor-wblo")
    }

    fn info_bitrate_bps(&self) -> Option<f32> {
        Some(self.inner.info_bitrate_bps())
    }

    fn estimator_id(&self) -> &'static str {
        "ofdm-pilot"
    }
}

/// OFDM main-family waveform — a faster, higher-SNR family above the
/// [`FloorWaveform`] on the adaptation ladder (sonde-c7i / sonde-99l.6). Each
/// instance is ONE rung: `ofdm-wide` / `ofdm-mid` / `ofdm-narrow`, all QPSK
/// (2 bits/sub-carrier) + WiFi LDPC N1296 R1/2, differing only in OFDM bandwidth
/// (Wide 2300 Hz → Mid 1000 Hz → Narrow 500 Hz). Reuses the floor engine's
/// mode-agnostic preamble + Schmidl-Cox sync + pilot-equalized demod
/// ([`WidebandLowDensityFloor::with_params_constellation_fec`]). Physics-gated
/// over AWGN per mode in `sonde-phy/tests/ofdm_main_gate.rs`.
pub struct OfdmMainWaveform {
    inner: WidebandLowDensityFloor,
    mode_name: &'static str,
}

impl OfdmMainWaveform {
    /// Construct the `ofdm-wide` rung (Wide / QPSK / N1296 R1/2).
    pub fn new() -> Self {
        Self::wide()
    }

    /// `ofdm-wide` — Wide params (2300 Hz), the fastest OFDM rung.
    pub fn wide() -> Self {
        Self::rung(OfdmModeName::Wide, "ofdm-wide")
    }

    /// `ofdm-mid` — Mid params (1000 Hz).
    pub fn mid() -> Self {
        Self::rung(OfdmModeName::Mid, "ofdm-mid")
    }

    /// `ofdm-narrow` — Narrow params (500 Hz), the most robust OFDM rung.
    pub fn narrow() -> Self {
        Self::rung(OfdmModeName::Narrow, "ofdm-narrow")
    }

    fn rung(mode: OfdmModeName, mode_name: &'static str) -> Self {
        Self {
            inner: WidebandLowDensityFloor::with_params_constellation_fec(
                OfdmParams::for_mode(mode),
                2, // QPSK
                Box::new(OfdmAdaptiveCodec::new(BlockN::N1296, WifiLdpcRate::R1_2)),
            ),
            mode_name,
        }
    }
}

impl Default for OfdmMainWaveform {
    fn default() -> Self {
        Self::new()
    }
}

impl Waveform for OfdmMainWaveform {
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
                family: ModeFamily::OfdmMain,
                snr_2500_db,
            }),
            SyncDecodeOutcome::DetectedDecodeFailed { snr_2500_db, .. } => {
                DecodeScan::Detected { snr_2500_db }
            }
            SyncDecodeOutcome::NoSignal => DecodeScan::NoSignal,
        }
    }

    fn family(&self) -> ModeFamily {
        ModeFamily::OfdmMain
    }

    fn mode_name(&self) -> Option<&'static str> {
        Some(self.mode_name)
    }

    fn info_bitrate_bps(&self) -> Option<f32> {
        Some(self.inner.info_bitrate_bps())
    }

    fn estimator_id(&self) -> &'static str {
        "ofdm-pilot"
    }
}

/// Narrow-FSK deep-floor waveform `floor-nfsk` (RobustnessFloor family): a
/// noncoherent 8-FSK mode for crowded/narrow band slots — the most robust rung,
/// below the wide-band floor (sonde-99l.4). Self-synchronises on the shared
/// Schmidl-Cox preamble and recovers a length-delimited, CRC-verified frame.
/// Physics-gated over AWGN in `sonde-phy/tests/nfsk_floor_gate.rs`. Reports a
/// narrowband `SNR_2500` (best-tone vs off-tone power, referenced to 2500 Hz) —
/// every mode now reports honest channel quality, not just the OFDM family.
pub struct NfskWaveform {
    inner: NarrowFskFloor,
}

impl NfskWaveform {
    /// Construct the narrow-FSK deep-floor waveform.
    pub fn new() -> Self {
        Self {
            inner: NarrowFskFloor::new(),
        }
    }
}

impl Default for NfskWaveform {
    fn default() -> Self {
        Self::new()
    }
}

impl Waveform for NfskWaveform {
    fn encode(&self, payload: &[u8]) -> Result<Vec<f32>, PhyError> {
        self.inner.transmit_with_preamble(payload)
    }

    fn decode_scan(&self, samples: &[f32]) -> DecodeScan {
        match self.inner.receive_scan(samples) {
            NfskDecode::Frame {
                payload,
                snr_2500_db,
            } => DecodeScan::Frame(DecodedFrame {
                payload,
                family: ModeFamily::RobustnessFloor,
                snr_2500_db,
            }),
            NfskDecode::Detected { snr_2500_db } => DecodeScan::Detected { snr_2500_db },
            NfskDecode::NoSignal => DecodeScan::NoSignal,
        }
    }

    fn family(&self) -> ModeFamily {
        ModeFamily::RobustnessFloor
    }

    fn mode_name(&self) -> Option<&'static str> {
        Some("floor-nfsk")
    }

    fn info_bitrate_bps(&self) -> Option<f32> {
        Some(self.inner.info_bitrate_bps())
    }

    fn estimator_id(&self) -> &'static str {
        "nfsk-tone"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfsk_waveform_round_trips_a_payload() {
        let wf = NfskWaveform::new();
        let payload = b"crowded band";
        let samples = wf.encode(payload).expect("encode");
        let mut captured = vec![0.0f32; 600];
        captured.extend_from_slice(&samples);
        match wf.decode_scan(&captured) {
            DecodeScan::Frame(frame) => {
                assert_eq!(frame.payload, payload);
                assert_eq!(frame.family, ModeFamily::RobustnessFloor);
            }
            other => panic!("expected an nFSK frame, got {other:?}"),
        }
        assert_eq!(wf.mode_name(), Some("floor-nfsk"));
    }

    #[test]
    fn ofdm_main_waveform_round_trips_a_payload() {
        let wf = OfdmMainWaveform::new();
        let payload = b"ofdm-main end to end";
        let samples = wf.encode(payload).expect("encode");
        let mut captured = vec![0.0f32; 500];
        captured.extend_from_slice(&samples);
        match wf.decode_scan(&captured) {
            DecodeScan::Frame(frame) => {
                assert_eq!(frame.payload, payload);
                assert_eq!(frame.family, ModeFamily::OfdmMain);
            }
            other => panic!("expected an OFDM-main frame, got {other:?}"),
        }
    }

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
