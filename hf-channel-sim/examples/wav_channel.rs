//! Apply the HF channel sim to a WAV file (sonde-imh ARDOP spike).
//!
//! Reads a mono 16-bit WAV, lifts each sample to complex baseband, optionally
//! runs the Watterson multipath channel, adds AWGN at a target SNR, and writes a
//! mono 16-bit WAV at the same sample rate. This is the bridge that splices
//! `hf-channel-sim` into an external modem's TX-WAV -> decode-WAV round-trip
//! (e.g. ardopcf `--writetxwav` -> here -> ardopcf `--decodewav`), so a
//! known-good mode's real BER-vs-SNR can be measured *through our channel sim*.
//!
//! Run: `cargo run -p hf-channel-sim --example wav_channel -- \
//!         --input tx.wav --output rx.wav --snr-db 6 --condition none`

use clap::Parser;
use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    input: String,
    #[arg(long)]
    output: String,
    /// AWGN SNR in dB (signal power / noise power over the whole clip).
    /// `allow_hyphen_values` so a negative SNR (e.g. `--snr-db -6`) isn't
    /// mistaken for a flag.
    #[arg(long, default_value_t = 10.0, allow_hyphen_values = true)]
    snr_db: f64,
    /// none | good | moderate | poor | flutter ("none" = AWGN only, no multipath).
    #[arg(long, default_value = "none")]
    condition: String,
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

fn parse_condition(name: &str) -> Option<ChannelCondition> {
    match name {
        "none" => None,
        "good" => Some(ChannelCondition::Good),
        "moderate" => Some(ChannelCondition::Moderate),
        "poor" => Some(ChannelCondition::Poor),
        "flutter" => Some(ChannelCondition::Flutter),
        other => panic!("unknown condition '{other}'"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut reader = hound::WavReader::open(&args.input)?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(format!("expected mono WAV, got {} channels", spec.channels).into());
    }
    let sample_rate = spec.sample_rate as f64;

    // i16 PCM -> normalized complex baseband (im = 0).
    let samples: Vec<i16> = reader.samples::<i16>().collect::<Result<_, _>>()?;
    let mut iq: Vec<Complex<f32>> = samples
        .iter()
        .map(|&s| Complex::new(s as f32 / 32768.0, 0.0))
        .collect();

    // Optional Watterson multipath at the WAV's own sample rate.
    if let Some(cond) = parse_condition(&args.condition) {
        let mut chan = WattersonChannel::from_condition(args.seed, cond, sample_rate);
        iq = chan.process_block(&iq);
    }

    // AWGN at the target SNR.
    AwgnGenerator::new(args.seed ^ 0xA5A5_A5A5_A5A5_A5A5).add_noise(&mut iq, args.snr_db);

    // Real projection -> i16 PCM (clamped).
    let mut writer = hound::WavWriter::create(&args.output, spec)?;
    for c in &iq {
        let v = (c.re * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        writer.write_sample(v)?;
    }
    writer.finalize()?;
    Ok(())
}
