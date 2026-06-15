//! Sonde PHY through the REAL hf-channel-sim Watterson channel (sonde-imh).
//!
//! This is the "one-line swap" the `ber_vs_snr_sweep` example anticipated: the AWGN
//! placeholder becomes the real ITU-R F.520 Watterson channel — the SAME channel the
//! ARDOP demo runs through. It prints Sonde's actual OFDM BER vs SNR for Ideal (AWGN
//! only) and Poor (2 ms / 1 Hz multipath fading), so Sonde's current PHY can be
//! compared head-to-head with the ARDOP baseline on an identical channel.
//!
//! DSP only: this modulates/demodulates audio-band SAMPLES in memory. It does NOT
//! touch sonde-tx or any PTT/rig crate, so nothing can key a radio (RADIO-1 safe).
//!
//! Run: `cargo run --release --example sonde_through_channel -p sonde-phy`
//!
//! NOTE: Sonde's PHY is UNFINISHED — there is a known open LDPC coding-gain / Eb-N0
//! calibration bug (sonde-vb9), so these numbers are real-but-not-yet-validated. The
//! point is to wire Sonde to the channel and see where it stands today, honestly.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use sonde_phy::audio_io::SAMPLE_RATE_HZ;
use sonde_phy::ofdm_main::ofdm_params::{OfdmModeName, OfdmParams};
use sonde_phy::ofdm_main::receiver::OfdmReceiver;
use sonde_phy::ofdm_main::transmitter::OfdmTransmitter;

const TRIALS: usize = 60;

/// Apply the optional Watterson multipath (stateful across symbols), then AWGN at a
/// per-symbol target SNR (the controlled SNR a BER sweep needs).
fn channel(
    samples: &[f32],
    chan: &mut Option<WattersonChannel>,
    awgn: &mut AwgnGenerator,
    snr_db: f64,
) -> Vec<f32> {
    let mut iq: Vec<Complex<f32>> = samples.iter().map(|&s| Complex::new(s, 0.0)).collect();
    if let Some(ch) = chan.as_mut() {
        iq = ch.process_block(&iq);
    }
    awgn.add_noise(&mut iq, snr_db);
    iq.iter().map(|c| c.re).collect()
}

fn ber_ofdm(mode: OfdmModeName, cond: Option<ChannelCondition>, snr_db: f64) -> f32 {
    let params = OfdmParams::for_mode(mode);
    let bits_per_sc = vec![2u8; params.subcarrier_indices().len()];
    let tx = OfdmTransmitter::new(&params);
    let rx = OfdmReceiver::new(&params);
    // One channel instance per sweep point → the fading evolves continuously across
    // the trial symbols (a realistic fading average rather than a frozen tap set).
    let mut chan = cond.map(|c| WattersonChannel::from_condition(1, c, f64::from(SAMPLE_RATE_HZ)));
    let mut awgn = AwgnGenerator::new(0x00C0_FFEE);

    let n_data_bits: usize = bits_per_sc
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            !params
                .pilot_indices()
                .contains(&params.subcarrier_indices()[*i])
        })
        .map(|(_, b)| *b as usize)
        .sum();

    let (mut errors, mut total) = (0usize, 0usize);
    for trial in 0..TRIALS {
        let payload_bits: Vec<u8> = (0..n_data_bits).map(|i| ((i + trial) % 2) as u8).collect();
        let samples = tx.modulate_one_symbol(&payload_bits, &bits_per_sc);
        let impaired = channel(&samples, &mut chan, &mut awgn, snr_db);
        let llrs = rx.demodulate_one_symbol(&impaired, &bits_per_sc);
        let recovered: Vec<u8> = llrs.iter().map(|l| if *l >= 0.0 { 0 } else { 1 }).collect();
        for (a, b) in recovered.iter().zip(payload_bits.iter()) {
            if a != b {
                errors += 1;
            }
            total += 1;
        }
    }
    errors as f32 / total.max(1) as f32
}

fn main() {
    println!("# sonde-phy through hf-channel-sim (ITU-R F.520 Watterson)");
    println!("# sample_rate_hz = {SAMPLE_RATE_HZ}");
    println!("mode,condition,snr_db,ber");
    for mode in [OfdmModeName::Narrow, OfdmModeName::Mid, OfdmModeName::Wide] {
        for (cname, cond) in [("ideal", None), ("poor", Some(ChannelCondition::Poor))] {
            for snr_db in (0..=25).step_by(5) {
                let ber = ber_ofdm(mode, cond, f64::from(snr_db));
                println!("ofdm-{mode:?},{cname},{snr_db},{ber:.4}");
            }
        }
    }
}
