//! Mode catalogue for the demo, derived from sonde-phy's ModeTable. Only
//! `floor-wblo` produces real waveforms today; OFDM-Main modes are listed
//! with `implemented: false` until the parallel QAM work lands.

use crate::types::ModeInfo;
use sonde_phy::modes::{ModeHint, ModeTable};
use sonde_phy::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

/// Modes implemented end-to-end today.
pub fn is_implemented(mode_id: &str) -> bool {
    mode_id == "floor-wblo"
}

/// Bandwidth (Hz) for a mode id, from the OFDM grid where applicable.
fn bandwidth_hz(mode_id: &str) -> f32 {
    match mode_id {
        "ofdm-narrow" => 500.0,
        "ofdm-mid" => 1000.0,
        "ofdm-wide" | "floor-wblo" => 2300.0,
        "floor-nfsk" => 500.0,
        _ => 0.0,
    }
}

fn constellation(mode_id: &str) -> &'static str {
    match mode_id {
        "floor-wblo" => "BPSK",
        "floor-nfsk" => "8-FSK",
        // OFDM-Main constellations are pinned by the parallel QAM work.
        _ => "QAM (pending)",
    }
}

fn data_bytes_per_symbol(mode_id: &str) -> usize {
    match mode_id {
        "floor-wblo" => WidebandLowDensityFloor::new().data_bytes_per_symbol(),
        // For unimplemented OFDM modes, report the BPSK-equivalent data-carrier
        // count as a lower bound until QAM loading is known.
        "ofdm-narrow" => OfdmParams::for_mode(OfdmModeName::Narrow).data_indices().len() / 8,
        "ofdm-mid" => OfdmParams::for_mode(OfdmModeName::Mid).data_indices().len() / 8,
        "ofdm-wide" => OfdmParams::for_mode(OfdmModeName::Wide).data_indices().len() / 8,
        _ => 0,
    }
}

/// Full mode catalogue for the UI.
pub fn list_modes() -> Vec<ModeInfo> {
    // ModeTable has no public iterator; enumerate the known ids in ladder order.
    let ids = ["floor-wblo", "ofdm-narrow", "ofdm-mid", "ofdm-wide", "floor-nfsk"];
    ids.iter()
        .map(|&id| {
            let family = if id.starts_with("ofdm") { "OfdmMain" } else { "RobustnessFloor" };
            ModeInfo {
                id: id.to_string(),
                family: family.to_string(),
                constellation: constellation(id).to_string(),
                bandwidth_hz: bandwidth_hz(id),
                data_bytes_per_symbol: data_bytes_per_symbol(id),
                implemented: is_implemented(id),
            }
        })
        .collect()
}

/// Sonde's Auto decision for a measured SNR, clamped to implemented modes.
/// Wraps `ModeTable::resolve(MainAuto, snr)`; if the chosen mode is not yet
/// implemented, fall back to the best implemented mode (`floor-wblo`).
pub fn recommend_mode(snr_db: f32) -> String {
    let table = ModeTable::default();
    let chosen = table.resolve(ModeHint::MainAuto, Some(snr_db));
    let id = chosen.short_name();
    if is_implemented(id) {
        id.to_string()
    } else {
        "floor-wblo".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_mode_is_implemented_and_9_bytes() {
        let modes = list_modes();
        let floor = modes.iter().find(|m| m.id == "floor-wblo").unwrap();
        assert!(floor.implemented);
        assert_eq!(floor.data_bytes_per_symbol, 9);
        assert_eq!(floor.constellation, "BPSK");
    }

    #[test]
    fn ofdm_modes_listed_but_not_implemented() {
        let modes = list_modes();
        let mid = modes.iter().find(|m| m.id == "ofdm-mid").unwrap();
        assert!(!mid.implemented);
    }

    #[test]
    fn recommendation_clamps_to_implemented() {
        // High SNR resolves to ofdm-wide upstream, but it's not implemented,
        // so the demo clamps to floor-wblo.
        assert_eq!(recommend_mode(30.0), "floor-wblo");
        // Negative SNR resolves to floor-wblo directly.
        assert_eq!(recommend_mode(-5.0), "floor-wblo");
    }
}
