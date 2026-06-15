//! Diagnostic (sonde-vb9): PHY-level flat-AWGN coding-gain sweep.
//!
//! Drives the floor's coded path (`WidebandLowDensityFloor::with_fec`
//! FloorRate14) and uncoded path (default IdentityFec) over a flat AWGN channel
//! at the SAMPLE level, with rate-aware Eb/N0 so coded and uncoded are compared
//! at matched info-bit energy. `receive_multi` (no sync) isolates the OFDM demod
//! LLR path from preamble detection / CFO.
//!
//! The codec-level sweep (sonde-fec/tests/awgn_coding_gain.rs) already proved
//! the LDPC + SPA give ~7 dB gain with textbook LLRs. This measures how much of
//! that gain survives the PHY demod. The coded-vs-uncoded dB gap is
//! scale-invariant, so it is immune to the absolute-Eb/N0 labeling issue
//! (sonde-xhw.5).
//!
//! `#[ignore]` (long-running). Run:
//!   cargo test -p sonde-phy --test floor_awgn_coding_gain -- --ignored --nocapture

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;

/// Regression guard for the sonde-vb9 fix (NOT ignored — runs in CI). Through
/// the production `receive_multi` path over flat AWGN, the rate-1/4 coded floor
/// must decode at an operating Eb/N0 where the per-symbol-pilot demod could not
/// (it needed ~12 dB; the time-smoothed estimate decodes by ~6 dB). Asserting at
/// 8 dB with a comfortable margin locks the ~4–6 dB recovery without flaking.
#[test]
fn coded_decodes_over_awgn_at_operating_point() {
    let payload_bytes = 58usize; // exactly one FloorRate14 block (480 info bits)
    let info_bits = payload_bytes * 8;
    let frames = 12usize;
    let ebn0_db = 8.0_f32;
    let mut rng = ChaCha8Rng::seed_from_u64(0x00B9_0000_0008_u64);
    let coded = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let ok = (0..frames)
        .filter(|_| {
            let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.gen()).collect();
            let tx = coded.transmit_multi(&payload).unwrap();
            let rx = add_awgn(&tx, info_bits, ebn0_db, &mut rng);
            matches!(coded.receive_multi(&rx), Ok(p) if p == payload)
        })
        .count();
    assert!(
        ok >= 10,
        "rate-1/4 coded floor decoded only {ok}/{frames} at Eb/N0={ebn0_db} dB over flat AWGN \
         — the sonde-vb9 pilot-smoothing recovery regressed (pre-fix this point was ~0)"
    );
}

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn mean_power(s: &[f32]) -> f32 {
    s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32
}

/// Add real AWGN to `samples` for a target Eb/N0 given the info-bit count the
/// frame carries. Eb = P_s·L/K_info; N0 = 2σ²  ⇒  σ² = P_s·L/(K_info·2·Eb/N0).
fn add_awgn(samples: &[f32], info_bits: usize, ebn0_db: f32, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let ebn0_lin = 10f32.powf(ebn0_db / 10.0);
    let ps = mean_power(samples);
    let eb = ps * samples.len() as f32 / info_bits as f32;
    let n0 = eb / ebn0_lin;
    let sigma = (n0 / 2.0).sqrt();
    samples.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}

#[test]
#[ignore = "long-running diagnostic; run with --ignored --nocapture"]
fn floor_awgn_coding_gain_sweep() {
    // 58 B: 58*8 + 16-bit frame length header = 480 = exactly one FloorRate14
    // block (k). 60 B would spill to a SECOND block (496 > 480) and need both
    // CRCs — an unfair effective rate ~0.117, not 1/4 (Codex round-1 catch).
    let payload_bytes = 58usize;
    let info_bits = payload_bytes * 8;
    let frames = 40usize;
    let mut rng = ChaCha8Rng::seed_from_u64(0x05F1_0000_0014_u64);

    let coded = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let uncoded = WidebandLowDensityFloor::new(); // IdentityFec

    println!("\n=== Floor PHY flat-AWGN coding gain (LLR_CLAMP_NUM as compiled) ===");
    println!("payload={payload_bytes}B info_bits={info_bits} frames/pt={frames}");
    println!(
        "{:>7} | {:>10} | {:>11} {:>11}",
        "Eb/N0", "coded FER", "uncoded FER", "uncoded BER"
    );

    for &ebn0_db in &[2.0_f32, 4.0, 6.0, 8.0, 10.0, 12.0] {
        // Coded arm.
        let mut coded_fe = 0usize;
        for _ in 0..frames {
            let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.gen()).collect();
            let tx = coded.transmit_multi(&payload).unwrap();
            let rx = add_awgn(&tx, info_bits, ebn0_db, &mut rng);
            match coded.receive_multi(&rx) {
                Ok(p) if p == payload => {}
                _ => coded_fe += 1,
            }
        }
        let coded_fer = coded_fe as f32 / frames as f32;

        // Uncoded arm (same payload size, IdentityFec).
        let mut unc_fe = 0usize;
        let mut unc_bit_err = 0usize;
        for _ in 0..frames {
            let payload: Vec<u8> = (0..payload_bytes).map(|_| rng.gen()).collect();
            let tx = uncoded.transmit_multi(&payload).unwrap();
            let rx = add_awgn(&tx, info_bits, ebn0_db, &mut rng);
            match uncoded.receive_multi(&rx) {
                Ok(p) => {
                    if p != payload {
                        unc_fe += 1;
                    }
                    for (a, b) in p.iter().zip(payload.iter()) {
                        unc_bit_err += (a ^ b).count_ones() as usize;
                    }
                    if p.len() != payload.len() {
                        unc_bit_err += (payload.len().abs_diff(p.len())) * 8;
                    }
                }
                Err(_) => {
                    unc_fe += 1;
                    unc_bit_err += info_bits; // worst case
                }
            }
        }
        let unc_fer = unc_fe as f32 / frames as f32;
        let unc_ber = unc_bit_err as f32 / (frames * info_bits) as f32;

        println!("{ebn0_db:>6.1} | {coded_fer:>10.4} | {unc_fer:>11.4} {unc_ber:>11.2e}");
    }
    println!("=== end sweep ===\n");
}
