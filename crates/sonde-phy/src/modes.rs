//! Source of truth for what PHY modes exist.
//!
//! Per overview §5.A.1, the PHY is a ladder spanning two
//! architecturally-distinct families. This module enumerates the modes
//! and exposes a `ModeTable` that the rest of the crate reads from.
//!
//! Specific sub-carrier counts, FFT sizes, and symbol rates are pinned
//! later (Phase 6+ for OFDM ladder, Phase 8 for floor); this skeleton
//! locks in the family + naming structure first.

/// The two architecturally-distinct PHY mode families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeFamily {
    /// Bit-adaptive OFDM main throughput family (overview §5.A.1).
    OfdmMain,
    /// Robustness floor family (overview §5.A.1). Houses both the
    /// wide-band low-density-constellation OFDM default and the
    /// situational narrow-FSK variant.
    RobustnessFloor,
}

/// Hint from link-adaptation (subsystem #7) or operator selection.
/// PHY MAY override based on channel measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeHint {
    /// "Pick something in the main throughput family; channel measurement
    /// chooses the specific OFDM mode-within-family."
    MainAuto,
    /// "Specific main-family mode pinned." The string is the short_name.
    MainPinned(&'static str),
    /// "Drop to the robustness floor; default wide-band low-density OFDM."
    Floor,
    /// "Drop to the robustness floor; explicitly request the
    /// narrow-FSK variant for a crowded band."
    FloorCrowdedBand,
}

/// An immutable mode descriptor: the catalog *identity* of a mode.
#[derive(Debug, Clone)]
pub struct ModeDescriptor {
    short_name: &'static str,
    family: ModeFamily,
    /// Canonical cross-build wire id — the value carried in the link's MODE byte
    /// and `rx_rung` feedback. MUST stay in `0..=7` (the `rx_rung` feedback field
    /// is 3 bits) and stable across builds; the link↔PHY registry handshake keys
    /// on it (sonde-ddg / sonde-3tm). Assigned in stable registry order so adding
    /// modes (e.g. QAM) never shifts existing ids.
    wire_id: u8,
}

impl ModeDescriptor {
    /// Stable kebab-case identifier (e.g. `"ofdm-mid"`, `"floor-wblo"`).
    pub fn short_name(&self) -> &'static str {
        self.short_name
    }
    /// Which mode family this descriptor belongs to.
    pub fn family(&self) -> ModeFamily {
        self.family
    }
    /// Canonical cross-build wire id (`0..=7`) — see the field docs.
    pub fn wire_id(&self) -> u8 {
        self.wire_id
    }
}

/// A registered mode's published **capability** — the physics the link's
/// adaptation ladder is built from (sonde-ddg, the PHY half of the link↔PHY
/// registry handshake sonde-3tm). Distinct from [`ModeDescriptor`] (catalog
/// identity): a capability is a mode that the runtime actually *registered* and
/// whose knee/airtime/capacity are *measured*. The link sorts its ladder by
/// `knee_snr_db`, sizes overs from `per_frame_airtime`/`per_frame_payload_bytes`,
/// and addresses the mode by `mode_name` (`ModeHint::MainPinned`) / `wire_id`
/// (MODE byte). Same-build-only: no cross-build capability negotiation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModeCapability {
    /// Canonical wire id (`0..=7`), from [`ModeDescriptor::wire_id`].
    pub wire_id: u8,
    /// Stable mode identity, for `ModeHint::MainPinned`.
    pub mode_name: &'static str,
    /// Waveform family (the link derives its 3 adaptation tiers from this).
    pub family: ModeFamily,
    /// Estimator-domain `SNR_2500` FER knee (dB) — measured physics, the value
    /// the link's reported SNR is compared against (with the +3 dB upshift margin).
    pub knee_snr_db: f32,
    /// Wall-clock airtime of ONE PHY `send_frame` carrying `per_frame_payload_bytes`.
    pub per_frame_airtime: core::time::Duration,
    /// Payload bytes one PHY frame carries (the link's per-frame fragment size).
    pub per_frame_payload_bytes: u16,
}

/// Resolved mode after applying `ModeHint` + channel measurement.
pub type ResolvedMode = ModeDescriptor;

/// Read-only mode catalogue.
pub struct ModeTable {
    modes: Vec<ModeDescriptor>,
}

impl Default for ModeTable {
    fn default() -> Self {
        Self {
            modes: vec![
                // OFDM main family — placeholders; bandwidth-per-mode
                // pins in Phase 7. Three modes is a starting point per
                // PHY spec §3.Q1 ("ARDOP uses 4; sonde may use fewer
                // or more"); empirical channel-sim sweep settles count.
                // Canonical wire_ids are STABLE (never reordered): 0..=2 OFDM
                // QPSK rungs, 3..=4 floor family, 5..=7 reserved for future QAM
                // rungs — so adding modes never shifts an existing id.
                ModeDescriptor {
                    short_name: "ofdm-narrow",
                    family: ModeFamily::OfdmMain,
                    wire_id: 2,
                },
                ModeDescriptor {
                    short_name: "ofdm-mid",
                    family: ModeFamily::OfdmMain,
                    wire_id: 1,
                },
                ModeDescriptor {
                    short_name: "ofdm-wide",
                    family: ModeFamily::OfdmMain,
                    wire_id: 0,
                },
                // Floor family — default + situational
                ModeDescriptor {
                    short_name: "floor-wblo",
                    family: ModeFamily::RobustnessFloor,
                    wire_id: 3,
                },
                ModeDescriptor {
                    short_name: "floor-nfsk",
                    family: ModeFamily::RobustnessFloor,
                    wire_id: 4,
                },
            ],
        }
    }
}

impl ModeTable {
    /// Enumerate the distinct mode families represented in this table.
    pub fn distinct_families(&self) -> Vec<ModeFamily> {
        let mut out = Vec::new();
        for m in &self.modes {
            if !out.contains(&m.family) {
                out.push(m.family);
            }
        }
        out
    }

    /// Resolve a `ModeHint` to a concrete `ResolvedMode`. For
    /// `ModeHint::MainAuto` the channel SNR (in dB) selects across the
    /// OFDM ladder; missing measurement falls back to the Mid mode.
    /// Phase 11 re-pegs the thresholds against channel-sim sweeps.
    pub fn resolve(&self, hint: ModeHint, channel_snr_db: Option<f32>) -> ResolvedMode {
        match hint {
            ModeHint::Floor => self.by_name("floor-wblo"),
            ModeHint::FloorCrowdedBand => self.by_name("floor-nfsk"),
            ModeHint::MainPinned(name) => self.by_name(name),
            ModeHint::MainAuto => {
                let snr = channel_snr_db.unwrap_or(15.0);
                if snr < 0.0 {
                    self.by_name("floor-wblo")
                } else if snr < 10.0 {
                    self.by_name("ofdm-narrow")
                } else if snr < 20.0 {
                    self.by_name("ofdm-mid")
                } else {
                    self.by_name("ofdm-wide")
                }
            }
        }
    }

    fn by_name(&self, name: &str) -> ResolvedMode {
        self.modes
            .iter()
            .find(|m| m.short_name == name)
            .cloned()
            .expect("mode-table short_name must exist; constructor enforces")
    }

    /// Look up a mode descriptor by `short_name`, or `None` if unknown — the
    /// fallible lookup the capability publication uses (vs the infallible
    /// `by_name` on hints the resolver guarantees exist).
    pub fn descriptor(&self, name: &str) -> Option<&ModeDescriptor> {
        self.modes.iter().find(|m| m.short_name == name)
    }

    /// All catalog mode descriptors, in table order.
    pub fn descriptors(&self) -> &[ModeDescriptor] {
        &self.modes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn canonical_wire_ids_are_unique_and_in_range() {
        let table = ModeTable::default();
        let ids: Vec<u8> = table.descriptors().iter().map(|d| d.wire_id()).collect();
        let uniq: BTreeSet<u8> = ids.iter().copied().collect();
        assert_eq!(uniq.len(), ids.len(), "wire_ids must be unique: {ids:?}");
        assert!(
            ids.iter().all(|&id| id <= 7),
            "wire_ids must be 0..=7 (rx_rung is 3 bits): {ids:?}"
        );
    }

    #[test]
    fn wire_ids_are_the_pinned_canonical_assignment() {
        let table = ModeTable::default();
        for (name, want) in [
            ("ofdm-wide", 0u8),
            ("ofdm-mid", 1),
            ("ofdm-narrow", 2),
            ("floor-wblo", 3),
            ("floor-nfsk", 4),
        ] {
            assert_eq!(table.descriptor(name).unwrap().wire_id(), want, "{name}");
        }
    }
}
