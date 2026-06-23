//! One-off knee measurement (sonde-ddg): the estimator-domain SNR_2500 FER-knee
//! per registered mode, to bake as honest published capability constants. The
//! knee is read in REPORTED SNR_2500 (what the link compares against), at the
//! injected level where FER first drops to <= 0.1. Run explicitly:
//!   cargo test -p sonde-phy --test knee_measure -- --ignored --nocapture
//! RADIO-1: nothing keyed.

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use sonde_fec::codec::OfdmAdaptiveCodec;
use sonde_fec::codes::{BlockN, WifiLdpcRate};
use sonde_phy::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use sonde_phy::robustness_floor::narrow_fsk::{NarrowFskFloor, NfskDecode};
use sonde_phy::robustness_floor::wideband_lowdensity::{
    SyncDecodeOutcome, WidebandLowDensityFloor,
};

const SR: f64 = 48_000.0;

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}
fn payload(seed: u64, n: usize) -> Vec<u8> {
    let mut r = ChaCha8Rng::seed_from_u64(seed);
    (0..n).map(|_| r.gen()).collect()
}
fn add_awgn_snr2500(sig: &[f32], snr_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let s: f64 = sig.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / sig.len() as f64;
    let sigma = (s * (SR / 2.0) / (2500.0 * 10f64.powf(snr_db / 10.0))).sqrt() as f32;
    sig.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}
fn add_awgn_wb(sig: &[f32], snr_db: f64, rng: &mut ChaCha8Rng) -> Vec<f32> {
    let s: f64 = sig.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / sig.len() as f64;
    let sigma = (s / 10f64.powf(snr_db / 10.0)).sqrt() as f32;
    sig.iter().map(|&x| x + sigma * gaussian(rng)).collect()
}

fn ofdm(mode: OfdmModeName) -> WidebandLowDensityFloor {
    WidebandLowDensityFloor::with_params_constellation_fec(
        OfdmParams::for_mode(mode),
        2,
        Box::new(OfdmAdaptiveCodec::new(BlockN::N1296, WifiLdpcRate::R1_2)),
    )
}

/// For each injected SNR, return (fer, mean reported SNR_2500).
fn ofdm_point(wf: &WidebandLowDensityFloor, snr: f64, n: u64) -> (f64, f64) {
    let mut rng = ChaCha8Rng::seed_from_u64(0xA0u64 ^ snr.to_bits());
    let (mut fail, mut acc) = (0u64, Vec::new());
    for seed in 0..n {
        let sig = wf.transmit_multi_with_preamble(&payload(seed, 48)).unwrap();
        let mut cap = vec![0.0f32; 400];
        cap.extend_from_slice(&add_awgn_snr2500(&sig, snr, &mut rng));
        match wf.receive_multi_with_sync_scan(&cap) {
            SyncDecodeOutcome::Frame {
                payload: g,
                snr_2500_db,
                ..
            } => {
                if g != payload(seed, 48) {
                    fail += 1;
                }
                if let Some(s) = snr_2500_db {
                    acc.push(s as f64);
                }
            }
            SyncDecodeOutcome::DetectedDecodeFailed { snr_2500_db, .. } => {
                fail += 1;
                if let Some(s) = snr_2500_db {
                    acc.push(s as f64);
                }
            }
            SyncDecodeOutcome::NoSignal => fail += 1,
        }
    }
    let m = if acc.is_empty() {
        f64::NAN
    } else {
        acc.iter().sum::<f64>() / acc.len() as f64
    };
    (fail as f64 / n as f64, m)
}

fn nfsk_point(snr_wb: f64, n: u64) -> (f64, f64) {
    let wf = NarrowFskFloor::new();
    let mut rng = ChaCha8Rng::seed_from_u64(0xF5 ^ snr_wb.to_bits());
    let (mut fail, mut acc) = (0u64, Vec::new());
    for seed in 0..n {
        let pl = payload(seed, 12);
        let sig = wf.transmit_with_preamble(&pl).unwrap();
        let mut cap = vec![0.0f32; 600];
        cap.extend_from_slice(&add_awgn_wb(&sig, snr_wb, &mut rng));
        match wf.receive_scan(&cap) {
            NfskDecode::Frame {
                payload: g,
                snr_2500_db,
            } => {
                if g != pl {
                    fail += 1;
                }
                if let Some(s) = snr_2500_db {
                    acc.push(s as f64);
                }
            }
            NfskDecode::Detected { snr_2500_db } => {
                fail += 1;
                if let Some(s) = snr_2500_db {
                    acc.push(s as f64);
                }
            }
            NfskDecode::NoSignal => fail += 1,
        }
    }
    let m = if acc.is_empty() {
        f64::NAN
    } else {
        acc.iter().sum::<f64>() / acc.len() as f64
    };
    (fail as f64 / n as f64, m)
}

fn floor_wblo() -> WidebandLowDensityFloor {
    WidebandLowDensityFloor::with_fec(Box::new(sonde_fec::codec::FloorRate14Codec::new()))
}

#[test]
#[ignore]
fn measure_floor_knee() {
    let wf = floor_wblo();
    eprintln!("== floor-wblo (inject SNR_2500) ==");
    for step in -6..=10 {
        let snr = step as f64 * 2.0;
        let (fer, rep) = ofdm_point(&wf, snr, 12);
        eprintln!("  inj={snr:+.0}  FER={fer:.2}  reported={rep:.1}");
    }
}

#[test]
#[ignore]
fn measure_knees() {
    for (name, mode) in [
        ("ofdm-wide", OfdmModeName::Wide),
        ("ofdm-mid", OfdmModeName::Mid),
        ("ofdm-narrow", OfdmModeName::Narrow),
    ] {
        let wf = ofdm(mode);
        eprintln!("== {name} (inject SNR_2500) ==");
        for step in -4..=14 {
            let snr = step as f64 * 2.0;
            let (fer, rep) = ofdm_point(&wf, snr, 12);
            eprintln!("  inj={snr:+.0}  FER={fer:.2}  reported={rep:.1}");
        }
    }
    eprintln!("== floor-nfsk (inject whole-band SNR) ==");
    for step in -12..=6 {
        let snr = step as f64 * 2.0;
        let (fer, rep) = nfsk_point(snr, 12);
        eprintln!("  inj_wb={snr:+.0}  FER={fer:.2}  reported_snr2500={rep:.1}");
    }
}
