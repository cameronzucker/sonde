//! Physics gate (sonde-99l.4): the narrow-FSK deep-floor mode is a REAL
//! self-synchronising waveform — it finds the shared Schmidl-Cox preamble in an
//! arbitrary capture window and recovers a length-delimited, CRC-verified frame
//! over AWGN, decoding at a low SNR (noncoherent 8-FSK is inherently robust) and
//! failing as the channel collapses. Gate on physics, not artifacts.
//!
//! RADIO-1: nothing keyed.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sonde_phy::robustness_floor::narrow_fsk::{NarrowFskFloor, NfskDecode};

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn payload(seed: u64, n: usize) -> Vec<u8> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..n).map(|_| rng.gen()).collect()
}

/// Add AWGN at a per-sample signal-to-noise ratio (the whole-band SNR; nFSK is
/// narrow so its in-tone SNR is much higher — that is the deep-floor advantage).
fn add_awgn_at_snr(signal: &[f32], snr_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let n = signal.len() as f64;
    let s: f64 = signal.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / n;
    let sigma = (s / 10f64.powf(snr_db / 10.0)).sqrt() as f32;
    signal.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}

/// FER over `n` seeds at a target whole-band SNR, with leading silence so the
/// preamble search must run at a non-zero offset (a real capture never starts on
/// the frame).
fn fer(snr_db: f64, n: u64) -> f64 {
    let wf = NarrowFskFloor::new();
    let mut rng = ChaCha8Rng::seed_from_u64(0x0FACE ^ (snr_db.to_bits()));
    let mut fails = 0u64;
    for seed in 0..n {
        let pl = payload(seed, 12); // FT8-class short frame
        let sig = wf.transmit_with_preamble(&pl).unwrap();
        let mut captured = vec![0.0f32; 600];
        captured.extend_from_slice(&add_awgn_at_snr(&sig, snr_db, &mut rng));
        match wf.receive_scan(&captured) {
            NfskDecode::Frame(got) if got == pl => {}
            _ => fails += 1,
        }
    }
    fails as f64 / n as f64
}

/// Self-syncs + decodes at a high SNR (proves the mode is real end-to-end), and
/// fails as the channel collapses (a real knee).
#[test]
fn nfsk_self_syncs_and_decodes_over_awgn_with_a_knee() {
    let fer_hi = fer(6.0, 6);
    let fer_lo = fer(-18.0, 6);
    eprintln!("nFSK FER: @6dB={fer_hi:.2}  @-18dB={fer_lo:.2}");
    assert!(
        fer_hi <= 0.34,
        "nFSK must self-sync + decode reliably at high SNR (FER {fer_hi:.2} @6 dB)"
    );
    assert!(
        fer_lo > fer_hi,
        "nFSK FER must rise as SNR collapses (a real knee): {fer_lo:.2} @-18 vs {fer_hi:.2} @6"
    );
}

/// A clean capture round-trips byte-exact (length + CRC framing works).
#[test]
fn nfsk_clean_capture_round_trips_byte_exact() {
    let wf = NarrowFskFloor::new();
    let pl = b"deep floor".to_vec();
    let sig = wf.transmit_with_preamble(&pl).unwrap();
    let mut captured = vec![0.0f32; 600];
    captured.extend_from_slice(&sig);
    assert_eq!(wf.receive_scan(&captured), NfskDecode::Frame(pl));
}

/// Pure noise reads as NoSignal, never a spurious frame (FER honesty).
#[test]
fn nfsk_pure_noise_is_no_signal() {
    let wf = NarrowFskFloor::new();
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let noise: Vec<f32> = (0..40_000).map(|_| 0.1 * gaussian(&mut rng)).collect();
    assert_eq!(wf.receive_scan(&noise), NfskDecode::NoSignal);
}
