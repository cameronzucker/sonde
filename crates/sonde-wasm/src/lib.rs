//! sonde-wasm: real Sonde DSP over a simulated channel, exported to JS.
//!
//! Public `#[wasm_bindgen]` functions return JSON strings so they are callable
//! identically from the browser and from host `cargo test`.

pub mod channelize;
pub mod link;
pub mod modes;
pub mod spectrogram;
pub mod types;

use wasm_bindgen::prelude::*;

/// JSON array of `ModeInfo`.
#[wasm_bindgen]
pub fn list_modes() -> String {
    serde_json::to_string(&modes::list_modes()).unwrap()
}

/// Sonde's Auto-mode recommendation for a measured SNR. Returns the mode id as
/// a PLAIN string (not JSON-quoted) — callers use it directly, not via JSON.parse.
#[wasm_bindgen]
pub fn recommend_mode(snr_db: f32) -> String {
    modes::recommend_mode(snr_db)
}

/// Run the payload over the link. `offsets_json` is the builder's
/// `payload.offsets.json`. Returns a JSON `LinkResult`, or a JSON
/// `{"error": "..."}` object on failure. `seed` is u32 to avoid JS BigInt.
#[wasm_bindgen]
pub fn run_link(
    payload: &[u8],
    offsets_json: &str,
    mode_id: &str,
    snr_db: f64,
    condition: &str,
    seed: u32,
) -> String {
    let offsets: types::FieldOffsets = match serde_json::from_str(offsets_json) {
        Ok(o) => o,
        Err(e) => {
            return serde_json::json!({ "error": format!("bad offsets json: {e}") }).to_string()
        }
    };
    match link::run_link_core(payload, &offsets, mode_id, snr_db, condition, seed as u64) {
        Ok(r) => serde_json::to_string(&r).unwrap(),
        Err(e) => serde_json::json!({ "error": e.to_string() }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_modes_returns_valid_json_with_floor() {
        let json = list_modes();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == "floor-wblo"));
    }

    #[test]
    fn run_link_round_trips_through_json() {
        let payload: Vec<u8> = (0..100).map(|i| i as u8).collect();
        let offsets = serde_json::json!({
            "total_len": payload.len(),
            "fields": [{"label":"image","start":0,"end":payload.len()}],
            "image_byte_len": payload.len()
        })
        .to_string();
        let json = run_link(&payload, &offsets, "floor-wblo", 80.0, "none", 1);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["recovered_ok"], true);
        assert_eq!(v["mode_id"], "floor-wblo");
    }

    #[test]
    fn run_link_reports_error_for_unimplemented_mode() {
        let payload = vec![1u8, 2, 3];
        let offsets = r#"{"total_len":3,"fields":[],"image_byte_len":0}"#;
        let json = run_link(&payload, offsets, "ofdm-mid", 80.0, "none", 1);
        assert!(json.contains("error"));
    }

    #[test]
    fn recommend_mode_returns_plain_mode_id() {
        // High SNR clamps to floor-wblo today (OFDM modes unimplemented).
        assert_eq!(recommend_mode(30.0), "floor-wblo");
        assert_eq!(recommend_mode(-5.0), "floor-wblo");
    }
}
