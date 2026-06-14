//! Step 0 physics gate (sonde-xhw.1): uncoded BPSK/QPSK BER vs Eb/N0 must
//! match the theoretical erfc/Q-function within ~1 dB. This is the methodology
//! reference: until it passes, every SNR/BER/throughput number in the project
//! is void (loopback/green-CI/sim sweeps are not evidence).
//!
//! ## Eb/N0 mapping (the standard reference, replacing ad-hoc "SNR")
//!
//! `AwgnGenerator::add_noise(sig, snr_db)` adds complex AWGN of total variance
//! `N0 = Es / 10^(snr_db/10)`, split N0/2 per quadrature (the generator draws
//! unit-variance complex Gaussian). With the project's unit-energy symbols
//! (`Es = 1`):
//!   - BPSK: 1 bit/symbol ⇒ Eb = Es. So `snr_db = Eb/N0 (dB)` directly, and the
//!     real-axis decision sees noise variance N0/2 ⇒ BER = Q(√(2·Eb/N0)).
//!   - QPSK (Gray): 2 bits/symbol ⇒ Eb = Es/2. So `snr_db = Eb/N0 (dB) + 3.01`,
//!     and per-bit BER = Q(√(2·Eb/N0)) — identical to BPSK, the Gray-QPSK result.
//!
//! Theory: BER = 0.5·erfc(√(Eb/N0_linear)) = Q(√(2·Eb/N0_linear)).

use hf_channel_sim::AwgnGenerator;
use sonde_phy::constellations::{Constellation, Mapper};

/// erfc via Abramowitz & Stegun 7.1.26 (max abs error ~1.5e-7) — adequate for a
/// ~1 dB tolerance gate without pulling a libm dependency into tests.
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * z);
    let poly = t
        * (0.254829592
            + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    let approx = poly * (-z * z).exp();
    if x >= 0.0 {
        approx
    } else {
        2.0 - approx
    }
}

/// Theoretical uncoded BER for BPSK/Gray-QPSK at a given Eb/N0 (dB).
fn theory_ber(ebn0_db: f64) -> f64 {
    let ebn0 = 10f64.powf(ebn0_db / 10.0);
    0.5 * erfc(ebn0.sqrt())
}

/// Deterministic LCG bit source (test-local; reproducible).
fn bits(n: usize, mut state: u64) -> Vec<u8> {
    (0..n)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 63) & 1) as u8
        })
        .collect()
}

/// Measured BER for `constellation` at the target Eb/N0 (dB). `snr_offset_db` is
/// the Es/N0 − Eb/N0 gap (0 for BPSK, +3.01 for QPSK).
fn measured_ber(
    constellation: Constellation,
    ebn0_db: f64,
    snr_offset_db: f64,
    n_bits: usize,
    seed: u64,
) -> f64 {
    let mapper = Mapper::new(constellation);
    let tx_bits = bits(n_bits, seed);
    let mut syms = mapper.map(&tx_bits);
    AwgnGenerator::new(seed ^ 0xBE11).add_noise(&mut syms, ebn0_db + snr_offset_db);
    let rx_bits = mapper.hard_demap(&syms);
    let errors = tx_bits
        .iter()
        .zip(rx_bits.iter())
        .filter(|(a, b)| a != b)
        .count();
    errors as f64 / n_bits as f64
}

/// Assert the measured BER corresponds to within ±1 dB of the requested Eb/N0,
/// by checking it lies between theory(Eb/N0 + 1 dB) and theory(Eb/N0 − 1 dB).
fn assert_within_1db(name: &str, ebn0_db: f64, measured: f64) {
    let lo = theory_ber(ebn0_db + 1.0); // better channel ⇒ lower BER bound
    let hi = theory_ber(ebn0_db - 1.0); // worse channel ⇒ higher BER bound
    println!(
        "{name} Eb/N0={ebn0_db:>4.1} dB  measured BER={measured:.3e}  theory={:.3e}  [±1dB: {lo:.3e}..{hi:.3e}]",
        theory_ber(ebn0_db)
    );
    assert!(
        measured >= lo * 0.5 && measured <= hi * 2.0,
        "{name} BER {measured:.3e} at {ebn0_db} dB is outside the ±1 dB band [{lo:.3e}, {hi:.3e}] \
         (with statistical slack) — modulation does not match theory"
    );
}

#[test]
fn bpsk_ber_matches_theory_within_1db() {
    // ~1e6 bits keeps the highest-Eb/N0 point statistically meaningful.
    let n = 1_000_000;
    for &ebn0 in &[0.0, 2.0, 4.0, 6.0] {
        let m = measured_ber(Constellation::Bpsk, ebn0, 0.0, n, 0xB95C ^ (ebn0 as u64));
        assert_within_1db("BPSK", ebn0, m);
    }
}

#[test]
fn qpsk_ber_matches_theory_within_1db() {
    let n = 1_000_000;
    for &ebn0 in &[0.0, 2.0, 4.0, 6.0] {
        // QPSK: Es/N0 = Eb/N0 + 3.01 dB (2 bits/symbol).
        let m = measured_ber(Constellation::Qpsk, ebn0, 3.0103, n, 0x9F5C ^ (ebn0 as u64));
        assert_within_1db("QPSK", ebn0, m);
    }
}
