//! End-to-end: a SITREP-shaped payload + offsets runs through the engine and
//! recovers cleanly at high SNR. Mirrors how the builder output is consumed.

use sonde_wasm::link::run_link_core;
use sonde_wasm::types::{Field, FieldOffsets};

#[test]
fn sitrep_shaped_payload_recovers_at_high_snr() {
    // ~5 KB payload: small text header/body + a pseudo "image" blob.
    let mut payload: Vec<u8> = b"To: EMCOMM-NET\nFrom: KK6XYZ\nSubject: SITREP\nPosition: 34-12.34N\n\nLevee breach.\n--- attachment: recon.jpg ---\n".to_vec();
    let header_end = payload.len();
    let mut state: u32 = 0xC0FF_EE00;
    for _ in 0..4800 {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        payload.push((state >> 16) as u8);
    }
    let off = FieldOffsets {
        total_len: payload.len(),
        fields: vec![
            Field {
                label: "header".into(),
                start: 0,
                end: header_end,
            },
            Field {
                label: "image".into(),
                start: header_end,
                end: payload.len(),
            },
        ],
        image_byte_len: payload.len() - header_end,
    };

    let r = run_link_core(&payload, &off, "floor-wblo", 80.0, "none", 7).unwrap();
    assert!(r.recovered_ok, "5 KB SITREP should recover at 80 dB");
    assert_eq!(r.ber, 0.0);
    // ~5 KB at 9 bytes/symbol ≈ 558 symbols ≈ 30 s of audio.
    assert!(
        r.time_to_deliver_s > 25.0 && r.time_to_deliver_s < 35.0,
        "got {} s",
        r.time_to_deliver_s
    );
    // Every symbol maps to a known field label.
    assert!(r.symbols.iter().all(|s| !s.field.is_empty()));
}
