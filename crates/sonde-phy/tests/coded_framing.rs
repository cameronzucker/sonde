use sonde_phy::robustness_floor::coded_framing::{
    blocks_for_payload, blocks_from_first_block, deframe_info_bits, frame_info_bits, HEADER_BITS,
};

#[test]
fn header_is_16_bits() {
    assert_eq!(HEADER_BITS, 16);
}

#[test]
fn frame_then_deframe_round_trips() {
    let payload = b"hello floor";
    let block_info = 480;
    let framed = frame_info_bits(payload, block_info);
    assert_eq!(framed.len() % block_info, 0);
    let n_blocks = framed.len() / block_info;
    assert_eq!(n_blocks, blocks_for_payload(payload.len(), block_info));
    let out = deframe_info_bits(&framed).expect("deframe");
    assert_eq!(out, payload);
}

#[test]
fn block_count_matches_first_block_header() {
    let payload = vec![0xABu8; 200];
    let block_info = 480;
    let framed = frame_info_bits(&payload, block_info);
    let first_block = &framed[..block_info];
    let declared = blocks_from_first_block(first_block, block_info);
    assert_eq!(declared, framed.len() / block_info);
}

#[test]
fn round_trips_preserve_leading_and_trailing_zero_bytes() {
    let block_info = 480;
    for payload in [
        &b"\x00\x00DATA"[..],
        &b"AB\x00\x00\x00"[..],
        &[0u8; 30][..],
        &b""[..],
    ] {
        let framed = frame_info_bits(payload, block_info);
        assert_eq!(deframe_info_bits(&framed).unwrap(), payload);
    }
}
