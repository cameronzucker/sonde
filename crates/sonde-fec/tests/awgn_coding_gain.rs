//! Diagnostic (sonde-vb9): codec-level AWGN coding-gain sweep.
//!
//! Bypasses the PHY entirely. Feeds the FloorRate14 codec textbook BPSK+AWGN
//! soft information (`LLR = 2y/σ²`) and measures coded frame-error-rate +
//! post-decode info BER against an uncoded-BPSK Monte-Carlo reference at the
//! SAME Eb/N0. This isolates the code+decoder from the PHY's LLR scaling: if
//! coding gain appears here, the bug is in the PHY demod; if it does not, the
//! defect is in the code/decoder.
//!
//! `#[ignore]` (long-running). Run explicitly:
//!   cargo test -p sonde-fec --test awgn_coding_gain -- --ignored --nocapture

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::coded_modulation::FecCodec;

/// One standard normal sample via Box-Muller.
fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

#[test]
#[ignore = "long-running diagnostic; run with --ignored --nocapture"]
fn floor_rate14_awgn_coding_gain_sweep() {
    let codec = FloorRate14Codec::new();
    let k = codec.block_info_bits(); // 480
    let n = codec.block_coded_bits(); // 2048
    let rate = 0.25_f32;

    let frames_per_point = 200usize;
    let mut rng = ChaCha8Rng::seed_from_u64(0xC0D1_06A1_0014_u64);

    println!("\n=== FloorRate14 codec-level AWGN coding gain ===");
    println!("k={k} n={n} rate=1/4 frames/point={frames_per_point} (textbook LLR=2y/sigma^2)");
    println!(
        "{:>7} | {:>9} {:>11} | {:>11}",
        "Eb/N0", "coded FER", "coded BER", "uncoded BER"
    );

    for &ebn0_db in &[0.0_f32, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] {
        let ebn0_lin = 10f32.powf(ebn0_db / 10.0);

        // Coded arm: Es = 1 (x=±1), Es = R·Eb ⇒ σ² = 1/(2·R·Eb/N0).
        let coded_sigma2 = 1.0 / (2.0 * rate * ebn0_lin);
        let coded_sigma = coded_sigma2.sqrt();

        let mut frame_errors = 0usize;
        let mut bit_errors = 0usize;
        for f in 0..frames_per_point {
            let info: Vec<u8> = (0..k).map(|_| rng.gen_range(0u8..2)).collect();
            let coded = codec.encode(&info);
            let llr: Vec<f32> = coded
                .iter()
                .map(|&b| {
                    let x = if b == 0 { 1.0 } else { -1.0 };
                    let y = x + coded_sigma * gaussian(&mut rng);
                    2.0 * y / coded_sigma2
                })
                .collect();
            match codec.decode_soft(&llr) {
                Ok(decoded) => {
                    let bytes_info: Vec<u8> = info.clone();
                    if decoded != bytes_info {
                        frame_errors += 1;
                        bit_errors += decoded
                            .iter()
                            .zip(bytes_info.iter())
                            .filter(|(a, b)| a != b)
                            .count();
                    }
                }
                Err(_) => {
                    frame_errors += 1;
                    // Decode/CRC failed: no reliable bits. Count as worst case
                    // contribution only at the frame level (BER from these is
                    // not observable through the all-or-nothing CRC path).
                }
            }
            let _ = f;
        }
        let coded_fer = frame_errors as f32 / frames_per_point as f32;
        let coded_ber = bit_errors as f32 / (frames_per_point * k) as f32;

        // Uncoded BPSK reference at the same Eb/N0: σ² = 1/(2·Eb/N0).
        let unc_sigma2 = 1.0 / (2.0 * ebn0_lin);
        let unc_sigma = unc_sigma2.sqrt();
        let unc_bits = frames_per_point * k;
        let mut unc_err = 0usize;
        for _ in 0..unc_bits {
            // bit 0 → +1; received < 0 ⇒ decided 1 ⇒ error.
            let y = 1.0 + unc_sigma * gaussian(&mut rng);
            if y < 0.0 {
                unc_err += 1;
            }
        }
        let unc_ber = unc_err as f32 / unc_bits as f32;

        println!("{ebn0_db:>6.1} | {coded_fer:>9.4} {coded_ber:>11.2e} | {unc_ber:>11.2e}");
    }
    println!("=== end sweep ===\n");
}
