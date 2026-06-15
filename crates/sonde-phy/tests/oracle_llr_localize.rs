//! Diagnostic (sonde-vb9, Codex-converged): localize the coded-path SNR loss.
//!
//! Three decode arms on the SAME noisy bare-OFDM samples (one FloorRate14 block,
//! no framing / clip / windowing):
//!   (a) production demod — pilot-estimated h, per-bin `n0_eff`;
//!   (b) production demod with FIXED true n0 (`with_n0_override(σ²)`) — pilot h,
//!       honest noise scale (isolates `effective_noise_per_bin`);
//!   (c) ORACLE — known flat channel reference `g[sc]` (noiseless all-bit-0
//!       symbol) and true `σ²`, bypassing pilot estimation entirely.
//!
//! Decision (Codex):
//! - oracle passes, production fails ⇒ pilot-h / n0_eff modeling guilty;
//! - (b) passes but (a) fails ⇒ `effective_noise_per_bin` guilty;
//! - all fail ⇒ waveform/normalization (the real-part mirror discard) guilty.
//!
//! Run: cargo test -p sonde-phy --test oracle_llr_localize -- --ignored --nocapture

use num_complex::Complex;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rustfft::FftPlanner;
use sonde_fec::codec::FloorRate14Codec;
use sonde_phy::coded_modulation::FecCodec;
use sonde_phy::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use sonde_phy::ofdm_main::receiver::OfdmReceiver;
use sonde_phy::ofdm_main::transmitter::OfdmTransmitter;

fn gaussian(rng: &mut ChaCha8Rng) -> f32 {
    let u1: f32 = rng.gen_range(1e-9_f32..1.0);
    let u2: f32 = rng.gen_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
}

fn data_subcarriers(params: &OfdmParams) -> Vec<usize> {
    let pilots: std::collections::HashSet<usize> = params.pilot_indices().iter().copied().collect();
    params
        .subcarrier_indices()
        .iter()
        .copied()
        .filter(|sc| !pilots.contains(sc))
        .collect()
}

/// Strip CP, FFT, unitary scale — same front end as `demodulate_one_symbol`.
fn fft_strip(symbol: &[f32], params: &OfdmParams) -> Vec<Complex<f32>> {
    let mut body: Vec<Complex<f32>> = symbol[params.cp_len()..]
        .iter()
        .map(|&s| Complex::new(s, 0.0))
        .collect();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(params.fft_size());
    fft.process(&mut body);
    let scale = 1.0 / (params.fft_size() as f32).sqrt();
    for c in body.iter_mut() {
        *c *= scale;
    }
    body
}

fn modulate_bare(coded: &[u8], params: &OfdmParams) -> Vec<f32> {
    let bits_per_sc = vec![1u8; params.subcarrier_indices().len()];
    let dps = params.data_indices().len();
    let tx = OfdmTransmitter::new(params);
    let mut out = Vec::new();
    for chunk in coded.chunks(dps) {
        let mut bits = chunk.to_vec();
        bits.resize(dps, 0);
        out.extend_from_slice(&tx.modulate_one_symbol(&bits, &bits_per_sc));
    }
    out
}

/// Noiseless reference `g[sc]` = received value of a +bit-0 BPSK symbol per data
/// subcarrier (folds in channel · constellation amplitude · the real-part TX).
fn measure_g(params: &OfdmParams) -> Vec<Complex<f32>> {
    let dps = params.data_indices().len();
    let bits_per_sc = vec![1u8; params.subcarrier_indices().len()];
    let clean = OfdmTransmitter::new(params).modulate_one_symbol(&vec![0u8; dps], &bits_per_sc);
    let freq = fft_strip(&clean, params);
    data_subcarriers(params)
        .iter()
        .map(|&sc| freq[sc])
        .collect()
}

fn demod_arm(samples: &[f32], params: &OfdmParams, n: usize, n0_override: Option<f32>) -> Vec<f32> {
    let bits_per_sc = vec![1u8; params.subcarrier_indices().len()];
    let rx = OfdmReceiver::with_n0_override(params, n0_override);
    let sym = params.fft_size() + params.cp_len();
    let mut llrs = Vec::new();
    let mut off = 0;
    while off + sym <= samples.len() {
        llrs.extend_from_slice(&rx.demodulate_one_symbol(&samples[off..off + sym], &bits_per_sc));
        off += sym;
    }
    llrs.truncate(n);
    llrs
}

fn oracle_arm(
    samples: &[f32],
    params: &OfdmParams,
    g: &[Complex<f32>],
    sigma2: f32,
    n: usize,
) -> Vec<f32> {
    let dsc = data_subcarriers(params);
    let sym = params.fft_size() + params.cp_len();
    let mut llrs = Vec::new();
    let mut off = 0;
    while off + sym <= samples.len() {
        let freq = fft_strip(&samples[off..off + sym], params);
        for (i, &sc) in dsc.iter().enumerate() {
            // BPSK: bit0→+g, bit1→-g. LLR = (|y+g|²−|y−g|²)/σ² = 4·Re(conj(g)·y)/σ².
            let y = freq[sc];
            let gi = g[i];
            llrs.push(4.0 * (gi.conj() * y).re / sigma2);
        }
        off += sym;
    }
    llrs.truncate(n);
    llrs
}

fn raw_ber(llrs: &[f32], coded: &[u8]) -> f32 {
    let mut e = 0usize;
    for (&l, &b) in llrs.iter().zip(coded.iter()) {
        let d = if l < 0.0 { 1u8 } else { 0u8 };
        if d != b {
            e += 1;
        }
    }
    e as f32 / coded.len() as f32
}

#[test]
#[ignore = "long-running diagnostic; run with --ignored --nocapture"]
fn oracle_vs_production_localize() {
    let params = OfdmParams::for_mode(OfdmModeName::Wide);
    let codec = FloorRate14Codec::new();
    let k = codec.block_info_bits();
    let n = codec.block_coded_bits();
    let frames = 30usize;
    let g = measure_g(&params);
    let mut rng = ChaCha8Rng::seed_from_u64(0x0000_AC1E_u64);

    println!("\n=== Oracle-vs-production localization (one block, bare OFDM) ===");
    println!("|g| mean = {:.4} (flat-channel reference amplitude)", {
        g.iter().map(|c| c.norm()).sum::<f32>() / g.len() as f32
    });
    println!(
        "{:>6} | {:>22} | {:>22} | {:>22}",
        "Eb/N0", "(a) production", "(b) fixed-true-n0", "(c) oracle"
    );
    println!(
        "{:>6} | {:>10} {:>10} | {:>10} {:>10} | {:>10} {:>10}",
        "", "FER", "rawBER", "FER", "rawBER", "FER", "rawBER"
    );

    for &ebn0_db in &[3.0_f32, 5.0, 7.0, 9.0] {
        let ebn0_lin = 10f32.powf(ebn0_db / 10.0);
        let (mut fa, mut fb, mut fc) = (0usize, 0usize, 0usize);
        let (mut ra, mut rb, mut rc) = (0.0f32, 0.0f32, 0.0f32);
        for _ in 0..frames {
            let info: Vec<u8> = (0..k).map(|_| rng.gen_range(0u8..2)).collect();
            let coded = codec.encode(&info);
            let tx = modulate_bare(&coded, &params);
            let ps = tx.iter().map(|x| x * x).sum::<f32>() / tx.len() as f32;
            let eb = ps * tx.len() as f32 / k as f32;
            let n0 = eb / ebn0_lin;
            let sigma = (n0 / 2.0).sqrt();
            let sigma2 = sigma * sigma;
            let rx: Vec<f32> = tx.iter().map(|&x| x + sigma * gaussian(&mut rng)).collect();

            let la = demod_arm(&rx, &params, n, None);
            let lb = demod_arm(&rx, &params, n, Some(sigma2));
            let lc = oracle_arm(&rx, &params, &g, sigma2, n);

            ra += raw_ber(&la, &coded);
            rb += raw_ber(&lb, &coded);
            rc += raw_ber(&lc, &coded);
            if !matches!(codec.decode_soft(&la), Ok(ref d) if *d == info) {
                fa += 1;
            }
            if !matches!(codec.decode_soft(&lb), Ok(ref d) if *d == info) {
                fb += 1;
            }
            if !matches!(codec.decode_soft(&lc), Ok(ref d) if *d == info) {
                fc += 1;
            }
        }
        let fr = frames as f32;
        println!(
            "{ebn0_db:>6.1} | {:>10.3} {:>10.2e} | {:>10.3} {:>10.2e} | {:>10.3} {:>10.2e}",
            fa as f32 / fr,
            ra / fr,
            fb as f32 / fr,
            rb / fr,
            fc as f32 / fr,
            rc / fr
        );
    }
    println!("=== end ===\n");
}
