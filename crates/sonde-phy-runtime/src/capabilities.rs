//! Per-mode capability publication (sonde-ddg) — the PHY half of the link↔PHY
//! registry handshake (sonde-3tm). The link builds its adaptation ladder from the
//! [`ModeCapability`] slice [`crate::SondePhy::capabilities`] publishes, instead
//! of a hardcoded static mirror, so a registered+gated waveform automatically
//! lights up a real, knee-gated rung.
//!
//! Codex-converged shape: a metadata table keyed by [`Waveform::mode_name`],
//! validated against the registered waveforms; the capability snapshot is taken on
//! `SondePhy` *before* the waveforms move into the worker thread — **no `Waveform`
//! trait change**. Same-build-only (no cross-build capability negotiation).
//!
//! The only value that cannot be derived is the **knee** (measured physics); it
//! lives here as a constant with its provenance. Airtime is *measured* per build
//! by encoding one representative frame (so it can never drift from the waveform);
//! per-frame capacity is the chosen fragment size.

use sonde_phy::audio_io::SAMPLE_RATE_HZ;
use sonde_phy::modes::{ModeCapability, ModeTable};

use crate::waveform::Waveform;

/// Measured estimator-domain `SNR_2500` FER knee (dB) + per-frame payload capacity
/// (bytes), keyed by `mode_name`. KNEES ARE MEASURED PHYSICS — provenance below.
/// Adding a mode (e.g. QAM) adds a row here + a `ModeDescriptor` wire_id.
struct ModeMeta {
    mode_name: &'static str,
    /// Measured estimator-domain SNR_2500 FER(<=0.1) knee, dB.
    knee_snr_db: f32,
    /// Bytes one PHY frame carries (the link's per-frame fragment size). For the
    /// OFDM/floor coded modes this is ~one FEC block's payload; for nFSK an
    /// FT8-class short frame.
    per_frame_payload_bytes: u16,
}

/// Knee provenance (estimator-domain SNR_2500, FER<=0.1): all five measured in one
/// harness, `crates/sonde-phy/tests/knee_measure.rs` (2026-06-16). OFDM knees are
/// also being independently refined by sonde-8xl's per-mode Watterson gates — this
/// table is the single home; 8xl updates these constants when its rigorous gates
/// land. (The floor-wblo knee is 3.4 dB measured here, NOT the 16 dB
/// "reliably-decodes" anchor floor_threshold_sweep prints — that anchor would
/// mis-order the ladder.)
const MODE_META: &[ModeMeta] = &[
    ModeMeta {
        mode_name: "ofdm-wide",
        knee_snr_db: KNEE_OFDM_WIDE,
        per_frame_payload_bytes: 79,
    },
    ModeMeta {
        mode_name: "ofdm-mid",
        knee_snr_db: KNEE_OFDM_MID,
        per_frame_payload_bytes: 79,
    },
    ModeMeta {
        mode_name: "ofdm-narrow",
        knee_snr_db: KNEE_OFDM_NARROW,
        per_frame_payload_bytes: 79,
    },
    ModeMeta {
        mode_name: "floor-wblo",
        knee_snr_db: KNEE_FLOOR_WBLO,
        per_frame_payload_bytes: 58,
    },
    ModeMeta {
        mode_name: "floor-nfsk",
        knee_snr_db: KNEE_NFSK,
        per_frame_payload_bytes: 12,
    },
];

// Measured estimator-domain SNR_2500 FER(<=0.1) knees, from knee_measure.rs
// (2026-06-16; reported-SNR value at the first FER<=0.1 sweep point). These are
// per-mode estimator-domain numbers (Codex C5: each mode's own estimator) — the
// link compares each mode's reported SNR to its own knee and resets across
// families, so cross-mode absolute comparison is approximate by design. NOTE the
// measured ordering: narrowband QPSK out-robusts the wideband BPSK floor (the
// +7 dB narrowband concentration beats the floor's heavier coding) — why we
// publish MEASURED knees, not naming-based assumptions.
const KNEE_OFDM_WIDE: f32 = 6.7;
const KNEE_OFDM_MID: f32 = 3.0;
const KNEE_OFDM_NARROW: f32 = -1.2;
const KNEE_FLOOR_WBLO: f32 = 3.4;
const KNEE_NFSK: f32 = 4.8;

fn meta_for(mode_name: &str) -> Option<&'static ModeMeta> {
    MODE_META.iter().find(|m| m.mode_name == mode_name)
}

/// Build the published [`ModeCapability`] for one registered waveform, or `None`
/// if it is not a catalog mode with measured metadata (e.g. a test double whose
/// `mode_name()` is `None` or unknown — it simply isn't advertised to the link).
/// Airtime is MEASURED by encoding one `per_frame_payload_bytes` frame so it
/// includes preamble + framing and can never drift from the waveform.
pub(crate) fn capability_of(wf: &dyn Waveform, modes: &ModeTable) -> Option<ModeCapability> {
    let name = wf.mode_name()?;
    let meta = meta_for(name)?;
    let desc = modes.descriptor(name)?;
    let frame = wf
        .encode(&vec![0u8; meta.per_frame_payload_bytes as usize])
        .ok()?;
    let per_frame_airtime =
        core::time::Duration::from_secs_f64(frame.len() as f64 / SAMPLE_RATE_HZ as f64);
    Some(ModeCapability {
        wire_id: desc.wire_id(),
        mode_name: name,
        family: desc.family(),
        knee_snr_db: meta.knee_snr_db,
        per_frame_airtime,
        per_frame_payload_bytes: meta.per_frame_payload_bytes,
    })
}

/// Snapshot the capabilities of a waveform registry (publication order = registry
/// order; the link sorts its ladder by knee). Non-catalog waveforms are skipped.
pub(crate) fn snapshot(waveforms: &[Box<dyn Waveform>]) -> Vec<ModeCapability> {
    let modes = ModeTable::default();
    waveforms
        .iter()
        .filter_map(|wf| capability_of(wf.as_ref(), &modes))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard_waveforms;

    #[test]
    fn standard_registry_publishes_complete_capabilities() {
        let caps = snapshot(&standard_waveforms());
        assert_eq!(caps.len(), 5, "all 5 standard modes publish a capability");
        // wire_ids unique + in 0..=7 (the link's 3-bit rx_rung contract).
        let uniq: std::collections::BTreeSet<u8> = caps.iter().map(|c| c.wire_id).collect();
        assert_eq!(uniq.len(), caps.len(), "wire_ids unique");
        assert!(caps.iter().all(|c| c.wire_id <= 7), "wire_ids in 0..=7");
        for c in &caps {
            assert!(c.knee_snr_db.is_finite(), "{} knee finite", c.mode_name);
            assert!(
                c.per_frame_payload_bytes > 0,
                "{} capacity > 0",
                c.mode_name
            );
            assert!(
                c.per_frame_airtime.as_secs_f64() > 0.0,
                "{} airtime > 0 (measured by encoding a frame)",
                c.mode_name
            );
        }
    }

    #[test]
    fn ladder_sorts_by_measured_knee_extremes() {
        let mut caps = snapshot(&standard_waveforms());
        caps.sort_by(|a, b| a.knee_snr_db.partial_cmp(&b.knee_snr_db).unwrap());
        // Pin only the extremes (middle rungs are close + may be refined by 8xl):
        // the measured ladder spans ofdm-narrow (most robust) → ofdm-wide (least).
        assert_eq!(caps.first().unwrap().mode_name, "ofdm-narrow");
        assert_eq!(caps.last().unwrap().mode_name, "ofdm-wide");
    }

    #[test]
    fn empty_registry_publishes_nothing() {
        assert!(snapshot(&[]).is_empty());
    }
}
