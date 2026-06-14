//! Codeword-spanning framing for the coded floor path.
//!
//! Layout of the global INFO-bit stream (pre-FEC), one `u8` per bit
//! (LSB), matching the `FecCodec` bit convention:
//!
//! ```text
//! [ len: 16 bits BE ][ payload bits ][ zero pad → multiple of block_info ]
//! ```
//!
//! The length header lives in the FIRST block so the receiver can learn
//! the block count after decoding block 0 alone.

use crate::error::PhyError;

/// Payload-length header width, in bits (u16 big-endian → ≤ 65535 bytes).
pub const HEADER_BITS: usize = 16;

/// Number of FEC blocks a payload occupies given `block_info` info-bits
/// per block.
pub fn blocks_for_payload(payload_len: usize, block_info: usize) -> usize {
    let total = HEADER_BITS + payload_len * 8;
    total.div_ceil(block_info).max(1)
}

/// Expand payload bytes into the padded global info-bit stream.
pub fn frame_info_bits(payload: &[u8], block_info: usize) -> Vec<u8> {
    let n_blocks = blocks_for_payload(payload.len(), block_info);
    let mut bits = Vec::with_capacity(n_blocks * block_info);
    let len = payload.len() as u16;
    for i in (0..HEADER_BITS).rev() {
        bits.push(((len >> i) & 1) as u8);
    }
    for &byte in payload {
        for i in (0..8).rev() {
            bits.push((byte >> i) & 1);
        }
    }
    bits.resize(n_blocks * block_info, 0);
    bits
}

/// Read the block count from the first decoded block's info bits.
pub fn blocks_from_first_block(first_block: &[u8], block_info: usize) -> usize {
    let len = read_header_len(first_block);
    blocks_for_payload(len, block_info)
}

/// Recover payload bytes from the full concatenated info-bit stream.
pub fn deframe_info_bits(info_bits: &[u8]) -> Result<Vec<u8>, PhyError> {
    if info_bits.len() < HEADER_BITS {
        return Err(PhyError::FrameDetect(
            "coded frame shorter than length header".into(),
        ));
    }
    let len = read_header_len(info_bits);
    let need = HEADER_BITS + len * 8;
    if info_bits.len() < need {
        return Err(PhyError::FrameDetect(format!(
            "declared payload {len} bytes needs {need} bits, have {}",
            info_bits.len()
        )));
    }
    let mut out = Vec::with_capacity(len);
    for b in 0..len {
        let mut byte = 0u8;
        for (i, &bit) in info_bits[HEADER_BITS + b * 8..HEADER_BITS + b * 8 + 8]
            .iter()
            .enumerate()
        {
            byte |= bit << (7 - i);
        }
        out.push(byte);
    }
    Ok(out)
}

fn read_header_len(bits: &[u8]) -> usize {
    let mut len = 0usize;
    for &bit in &bits[..HEADER_BITS] {
        len = (len << 1) | (bit as usize & 1);
    }
    len
}
