// SPDX-License-Identifier: AGPL-3.0-only
//! Stream raw S16LE mono PCM through the HF channel sim in real time.
//!
//! Reads 16-bit little-endian mono PCM from stdin, lifts each sample to complex
//! baseband, applies the Watterson multipath channel (stateful across reads, so the
//! fading evolves continuously), adds AWGN at a FIXED noise floor (so silence still
//! carries band noise and a deeply-faded signal is genuinely buried), projects back
//! to the real axis, and writes S16LE to stdout. Optionally tees the impaired output
//! to `--tap`.
//!
//! This is the real-time bridge that splices `hf-channel-sim` into a live modem audio
//! path, e.g.:
//!
//!   arecord ... | hf-channel-pcm --condition poor --snr-db 6 --tap air.raw | aplay ...
//!
//! It replaces the ARDOP demo's AWGN-only Python stand-in with the real ITU-R F.520
//! conditions, and is reusable as a channel fixture for Sonde's own waveform testing.

use std::fs::File;
use std::io::{self, Read, Write};

use clap::Parser;
use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;

#[derive(Parser)]
#[command(about = "Stream S16LE mono PCM through the Watterson HF channel + fixed-floor AWGN")]
struct Args {
    /// none | good | moderate | poor | flutter ("none" = AWGN only, no multipath).
    #[arg(long, default_value = "none")]
    condition: String,
    /// Target SNR in dB vs the fixed reference level (--ref-rms). Negative allowed.
    #[arg(long, default_value_t = 10.0, allow_hyphen_values = true)]
    snr_db: f64,
    /// PCM sample rate (Hz) — sets the Watterson delay-line length + fading rate.
    #[arg(long, default_value_t = 12000.0)]
    sample_rate: f64,
    /// Channel-fading RNG seed (the noise stream uses an independent derived seed).
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Reference TX RMS (int16 units) the SNR is measured against. The constant
    /// noise floor is `ref_rms / 10^(snr_db/20)`. Default ~ardopcf's loopback output.
    #[arg(long, default_value_t = 17900.0)]
    ref_rms: f64,
    /// Read buffer size in samples (latency vs syscall overhead).
    #[arg(long, default_value_t = 256)]
    block: usize,
    /// Also write the impaired on-air PCM to this file (for a live waterfall tap).
    #[arg(long)]
    tap: Option<String>,
}

fn parse_condition(name: &str) -> Option<ChannelCondition> {
    match name {
        "none" => None,
        "good" => Some(ChannelCondition::Good),
        "moderate" => Some(ChannelCondition::Moderate),
        "poor" => Some(ChannelCondition::Poor),
        "flutter" => Some(ChannelCondition::Flutter),
        other => {
            eprintln!("hf-channel-pcm: unknown condition '{other}'");
            std::process::exit(2);
        }
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    let mut channel = parse_condition(&args.condition)
        .map(|c| WattersonChannel::from_condition(args.seed, c, args.sample_rate));
    // Noise stream is independent of the channel-fading stream (different seed), per
    // ITU-R F.1487's decoupling of channel realization from noise realization.
    let mut awgn = AwgnGenerator::new(args.seed ^ 0xA5A5_A5A5_A5A5_A5A5);
    let noise_std = (args.ref_rms / 10f64.powf(args.snr_db / 20.0)) as f32;

    let mut tap = match &args.tap {
        Some(p) => Some(File::create(p)?),
        None => None,
    };

    let mut reader = io::stdin().lock();
    let mut writer = io::stdout().lock();

    let mut raw = vec![0u8; args.block.max(1) * 2];
    let mut carry: Vec<u8> = Vec::new(); // odd trailing byte across reads

    loop {
        let n = match reader.read(&mut raw) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        // Process whatever is available now (low latency); carry an odd trailing byte.
        let mut bytes = carry;
        bytes.extend_from_slice(&raw[..n]);
        let nsamp = bytes.len() / 2;
        carry = bytes[nsamp * 2..].to_vec();
        if nsamp == 0 {
            continue;
        }

        // S16LE -> complex baseband (im = 0), in int16 sample scale.
        let mut iq: Vec<Complex<f32>> = (0..nsamp)
            .map(|i| {
                Complex::new(
                    i16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]) as f32,
                    0.0,
                )
            })
            .collect();

        // Watterson multipath (stateful across reads), then the fixed noise floor.
        if let Some(ch) = channel.as_mut() {
            iq = ch.process_block(&iq);
        }
        awgn.add_noise_fixed(&mut iq, noise_std);

        // Real projection -> S16LE.
        let mut out = Vec::with_capacity(nsamp * 2);
        for c in &iq {
            out.extend_from_slice(&(c.re.round().clamp(-32768.0, 32767.0) as i16).to_le_bytes());
        }
        writer.write_all(&out)?;
        writer.flush()?;
        if let Some(f) = tap.as_mut() {
            f.write_all(&out)?;
            f.flush()?;
        }
    }
    Ok(())
}
