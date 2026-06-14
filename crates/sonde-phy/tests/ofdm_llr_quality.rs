//! Diagnostic (sonde-vb9): isolate WHERE the coded path loses its gain.
//!
//! Decodes a real FloorRate14 codeword through the BARE OFDM symbol path
//! (`OfdmTransmitter::modulate_one_symbol` → AWGN → `OfdmReceiver::
//! demodulate_one_symbol` → `codec.decode_soft`) — NO PAPR soft-clip, NO
//! inter-symbol windowing, NO framing. Compare against:
//! (A) the proven codec-level textbook-LLR result (~7 dB gain), and
//! (B) the full `transmit_multi` path (which shows ~0 gain).
//! If THIS bare path shows gain, the culprit is clip/windowing in
//! `transmit_multi`. If it does NOT, the culprit is the OFDM modulate/demodulate
//! + channel-aware-LLR path itself.
//!
//! Also reports the demod LLR reliability ranking: sign-error-rate among the
//! low-|LLR| third vs the high-|LLR| third of data bits. If |LLR| does not rank
//! correctness, the soft information is broken (SPA degenerates to hard).
//!
//! Run: cargo test -p sonde-phy --test ofdm_llr_quality -- --ignored --nocapture

use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
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

/// Modulate `coded_bits` (length n) across bare OFDM symbols (BPSK, no clip, no
/// windowing). Returns (samples, symbol_len).
fn modulate_bare(coded: &[u8], params: &OfdmParams) -> (Vec<f32>, usize) {
    let bits_per_sc = vec![1u8; params.subcarrier_indices().len()];
    let dps = params.data_indices().len();
    let tx = OfdmTransmitter::new(params);
    let mut out = Vec::new();
    let mut sym_len = 0;
    for chunk in coded.chunks(dps) {
        let mut bits = chunk.to_vec();
        bits.resize(dps, 0);
        let s = tx.modulate_one_symbol(&bits, &bits_per_sc);
        sym_len = s.len();
        out.extend_from_slice(&s);
    }
    (out, sym_len)
}

/// Demodulate bare OFDM symbols back to LLRs (length n after truncation).
fn demodulate_bare(samples: &[f32], params: &OfdmParams, n: usize) -> Vec<f32> {
    let bits_per_sc = vec![1u8; params.subcarrier_indices().len()];
    let rx = OfdmReceiver::new(params);
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

#[test]
#[ignore = "long-running diagnostic; run with --ignored --nocapture"]
fn bare_ofdm_coded_decode_and_llr_ranking() {
    let params = OfdmParams::for_mode(OfdmModeName::Wide);
    let codec = FloorRate14Codec::new();
    let k = codec.block_info_bits();
    let n = codec.block_coded_bits();
    let frames = 40usize;
    let mut rng = ChaCha8Rng::seed_from_u64(0xB47E_0FD3_u64);

    println!("\n=== BARE OFDM coded decode (no clip/window/framing) ===");
    println!("k={k} n={n} frames/pt={frames}");
    println!(
        "{:>7} | {:>10} | {:>11} {:>11} {:>11}",
        "Eb/N0", "coded FER", "raw BER", "lo|LLR| BER", "hi|LLR| BER"
    );

    for &ebn0_db in &[2.0_f32, 3.0, 4.0, 5.0, 6.0] {
        let ebn0_lin = 10f32.powf(ebn0_db / 10.0);
        let mut fe = 0usize;
        let mut raw_err = 0usize;
        let mut raw_tot = 0usize;
        let mut lo_err = 0usize;
        let mut lo_tot = 0usize;
        let mut hi_err = 0usize;
        let mut hi_tot = 0usize;

        for _ in 0..frames {
            let info: Vec<u8> = (0..k).map(|_| rng.gen_range(0u8..2)).collect();
            let coded = codec.encode(&info);
            let (tx, _sym_len) = modulate_bare(&coded, &params);

            // Rate-aware Eb/N0: Eb = P_s·L/k_info; N0 = 2σ².
            let ps = tx.iter().map(|x| x * x).sum::<f32>() / tx.len() as f32;
            let eb = ps * tx.len() as f32 / k as f32;
            let n0 = eb / ebn0_lin;
            let sigma = (n0 / 2.0).sqrt();
            let rx: Vec<f32> = tx.iter().map(|&x| x + sigma * gaussian(&mut rng)).collect();

            let llrs = demodulate_bare(&rx, &params, n);

            // Reliability ranking + raw BER vs the known coded bits.
            let mut mags: Vec<(f32, bool)> = llrs
                .iter()
                .zip(coded.iter())
                .map(|(&l, &b)| {
                    let decided = if l < 0.0 { 1u8 } else { 0u8 };
                    let wrong = decided != b;
                    if wrong {
                        raw_err += 1;
                    }
                    raw_tot += 1;
                    (l.abs(), wrong)
                })
                .collect();
            mags.sort_by(|a, b| a.0.total_cmp(&b.0));
            let third = mags.len() / 3;
            for (i, (_m, wrong)) in mags.iter().enumerate() {
                if i < third {
                    lo_tot += 1;
                    if *wrong {
                        lo_err += 1;
                    }
                } else if i >= mags.len() - third {
                    hi_tot += 1;
                    if *wrong {
                        hi_err += 1;
                    }
                }
            }

            match codec.decode_soft(&llrs) {
                Ok(d) if d == info => {}
                _ => fe += 1,
            }
        }
        let fer = fe as f32 / frames as f32;
        let raw_ber = raw_err as f32 / raw_tot as f32;
        let lo_ber = lo_err as f32 / lo_tot.max(1) as f32;
        let hi_ber = hi_err as f32 / hi_tot.max(1) as f32;
        println!("{ebn0_db:>6.1} | {fer:>10.4} | {raw_ber:>11.2e} {lo_ber:>11.2e} {hi_ber:>11.2e}");
    }
    println!("(lo|LLR| BER >> hi|LLR| BER ⇒ magnitudes rank reliability; ≈ ⇒ soft info broken)");
    println!("=== end ===\n");
}
