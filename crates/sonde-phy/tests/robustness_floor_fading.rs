//! Robustness-floor fading gate (sonde-64w.2). The floor's REAL coded mode
//! (rate-1/4 LDPC) must decode through Watterson Good/Moderate fading, not just
//! AWGN. This is the channel-sim seam that was never wired (sim_adapter.rs was a
//! placeholder), whose clean-only validation hid a frequency-selective-fading
//! demod defect.
//!
//! ## Why this gate uses the real FEC (not IdentityFec)
//!
//! A two-tap Watterson channel `H[k] = (f1 + f2·e^(−jθk))/√2` has deep
//! frequency-selective NULLS when the tap magnitudes are comparable (measured
//! here: |Y| varies 16.5× across the band for Good, 27× for Moderate). At a
//! null the channel erases information — no equalizer can recover an *uncoded*
//! bit there; that is physics, not a software bug. The floor's real operating
//! mode is rate-1/4 LDPC + interleaver precisely to bridge such nulls: the
//! channel-aware demod ([`ofdm_main::receiver`]) turns a nulled sub-carrier into
//! a low-confidence near-erasure, and the code corrects it. So the meaningful
//! fading gate runs the real `FloorRate14Codec`. An IdentityFec control below
//! asserts only that SYNC still works through the fade (not payload recovery —
//! that would be demanding the impossible of an uncoded link).
//!
//! ## Channel model
//!
//! The floor emits REAL passband audio; the Watterson sim is complex-baseband.
//! The faithful application for an OFDM-over-SSB waveform is real audio ->
//! analytic signal (Hilbert) -> complex Watterson -> AWGN -> real projection.
//! High SNR (30 dB) isolates the equalizer/demod from raw BER limits.
//!
//! `sonde-fec` is a **dev-only** dependency of `sonde-phy` here (the gate needs
//! the real codec). The normal dependency edge runs the other way
//! (`sonde-fec -> sonde-phy` for the `FecCodec` trait); Cargo permits the
//! dev-only back-edge and `cargo tree -e normal,dev` resolves it.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::robustness_floor::wideband_lowdensity::WidebandLowDensityFloor;
use sonde_phy::sync::preamble::PreambleDetector;

const SR: f64 = 48_000.0;

/// Analytic signal of a real signal via FFT: zero the negative frequencies,
/// double the positives. `Re{analytic} == original` for a unit channel.
fn analytic(real_sig: &[f32]) -> Vec<Complex<f32>> {
    let n = real_sig.len();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf: Vec<Complex<f32>> = real_sig.iter().map(|&x| Complex::new(x, 0.0)).collect();
    fwd.process(&mut buf);
    let half = n / 2;
    for (k, b) in buf.iter_mut().enumerate() {
        if k == 0 || (n % 2 == 0 && k == half) {
            // DC / Nyquist unchanged
        } else if k < half {
            *b *= 2.0;
        } else {
            *b = Complex::new(0.0, 0.0);
        }
    }
    inv.process(&mut buf);
    let scale = 1.0 / n as f32;
    for b in buf.iter_mut() {
        *b *= scale;
    }
    buf
}

/// real audio -> analytic -> Watterson(condition) -> AWGN(snr) -> real audio.
fn through_channel(clean: &[f32], condition: ChannelCondition, snr_db: f64, seed: u64) -> Vec<f32> {
    let mut ch = WattersonChannel::from_condition(seed, condition, SR);
    let mut a = analytic(clean);
    // Guard tail (>= one CP) so the timing-corrected window has samples to read.
    a.extend(std::iter::repeat(Complex::new(0.0, 0.0)).take(1024));
    let mut faded = ch.process_block(&a);
    AwgnGenerator::new(seed ^ 0xA5A5).add_noise(&mut faded, snr_db);
    faded.iter().map(|c| c.re).collect()
}

/// Deterministic seed for the i-th channel realization (golden-ratio stride).
fn seed_for(i: u64) -> u64 {
    0x9E37_79B9_7F4A_7C15u64.wrapping_mul(i + 1) ^ 0xC0DE_6402
}

/// The regression seed the root-cause analysis converged on.
const CONVERGED_SEED: u64 = 0xC0DE_6402;

// ─── A. Seed-robust coded decode THROUGH the production sync path ────────────
//
// Load-bearing proof that the equalizer fix (channel-aware soft LLR + edge
// extrapolation, sonde-64w.2) AND the rebuilt synchronizer (Schmidl-Cox
// detection + CFO derotation + template-MF timing, sonde-xhw.3) together decode
// seed-robustly through a Watterson fade. This test previously sliced the body
// at the known preamble length to BYPASS a real-ZC correlator that mislocated
// the frame under channel phase rotation; that defect is now fixed, so the
// bypass is removed — decode runs through `receive_multi_with_sync` (real
// acquisition + frequency correction in the loop) or it does not count.

fn assert_coded_decodes_via_sync(condition: ChannelCondition, seed: u64) {
    let payload = b"FLOOR FADING GATE PAYLOAD";
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let clean = floor.transmit_multi_with_preamble(payload).unwrap();
    let rx = through_channel(&clean, condition, 30.0, seed);
    let (_start, decoded) = floor
        .receive_multi_with_sync(&rx)
        .unwrap_or_else(|e| panic!("{condition:?} seed {seed:#x}: coded receive failed: {e:?}"));
    assert_eq!(
        decoded, payload,
        "{condition:?} seed {seed:#x}: coded payload mismatch (via production sync)"
    );
}

#[test]
fn equalizer_seed_robust_good_via_production_sync() {
    for i in 0..8 {
        assert_coded_decodes_via_sync(ChannelCondition::Good, seed_for(i));
    }
}

// ─── B. End-to-end production path on the regression seed ────────────────────
//
// Proves the integrated path (preamble sync → channel-aware demod → rate-1/4
// LDPC) recovers the payload on the converged seed for both conditions through
// `receive_multi_with_sync`. With the sonde-xhw.3 synchronizer the production
// path now tracks perfect-alignment seed-robustness (the earlier sync gap is
// closed); the combined CFO + clock + timing gate lives in
// `tests/sync_impairment_gate.rs`.

fn assert_e2e_decodes(condition: ChannelCondition) {
    let payload = b"FLOOR FADING GATE PAYLOAD";
    let floor = WidebandLowDensityFloor::with_fec(Box::new(FloorRate14Codec::new()));
    let clean = floor.transmit_multi_with_preamble(payload).unwrap();
    let rx = through_channel(&clean, condition, 30.0, CONVERGED_SEED);
    let (_start, decoded) = floor
        .receive_multi_with_sync(&rx)
        .unwrap_or_else(|e| panic!("{condition:?}: E2E coded receive failed: {e:?}"));
    assert_eq!(
        decoded, payload,
        "{condition:?}: E2E coded payload mismatch"
    );
}

#[test]
fn floor_fec_decodes_e2e_watterson_good() {
    assert_e2e_decodes(ChannelCondition::Good);
}

#[test]
fn floor_fec_decodes_e2e_watterson_moderate() {
    assert_e2e_decodes(ChannelCondition::Moderate);
}

/// SYNC control: the preamble must still be DETECTED through the fade. We do
/// NOT assert uncoded payload recovery — a spectral null makes that physically
/// impossible, and asserting failure would be fragile (a future equalizer/seed
/// tweak could let it occasionally recover). This isolates "sync survives the
/// fade" from "the code bridges the null".
fn assert_sync_detects(condition: ChannelCondition) {
    let payload = b"FLOOR FADING GATE PAYLOAD";
    let floor = WidebandLowDensityFloor::new(); // IdentityFec — sync-only control
    let clean = floor.transmit_multi_with_preamble(payload).unwrap();
    let rx = through_channel(&clean, condition, 30.0, 0xC0DE_6402);
    let detection = PreambleDetector::new().scan(&rx);
    assert!(
        detection.is_some(),
        "{condition:?}: preamble must still be detected through the fade"
    );
}

#[test]
fn preamble_sync_survives_watterson_good() {
    assert_sync_detects(ChannelCondition::Good);
}

#[test]
fn preamble_sync_survives_watterson_moderate() {
    assert_sync_detects(ChannelCondition::Moderate);
}
