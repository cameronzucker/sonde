//! Apply the HF channel sim to real audio: lift f32 -> Complex<f32>, run the
//! Watterson multipath channel, add AWGN in place, project back to real.

use hf_channel_sim::{AwgnGenerator, ChannelCondition, WattersonChannel};
use num_complex::Complex;
use sonde_phy::audio_io::SAMPLE_RATE_HZ;

fn parse_condition(name: &str) -> ChannelCondition {
    match name {
        "good" | "clean" => ChannelCondition::Good,
        "moderate" => ChannelCondition::Moderate,
        "poor" => ChannelCondition::Poor,
        "flutter" => ChannelCondition::Flutter,
        _ => ChannelCondition::Good,
    }
}

/// Returns `(observed_real, clean_complex, observed_complex)`.
/// `clean_complex` is the channel-free (preamble+symbols) signal lifted to
/// complex; `observed_complex` is after multipath+AWGN. Both are returned so
/// the caller can estimate SNR. `observed_real` is what the spectrogram and
/// decoder consume.
pub fn apply_channel(
    samples: &[f32],
    snr_db: f64,
    condition: &str,
    seed: u64,
) -> (Vec<f32>, Vec<Complex<f32>>, Vec<Complex<f32>>) {
    let clean: Vec<Complex<f32>> = samples.iter().map(|&s| Complex::new(s, 0.0)).collect();

    // "none" skips the Watterson multipath entirely (AWGN-only channel).
    let mut observed = if condition == "none" {
        clean.clone()
    } else {
        let mut chan = WattersonChannel::from_condition(seed, parse_condition(condition), SAMPLE_RATE_HZ as f64);
        chan.process_block(&clean)
    };

    let mut awgn = AwgnGenerator::new(seed ^ 0xA5A5_A5A5_A5A5_A5A5);
    awgn.add_noise(&mut observed, snr_db);

    let observed_real: Vec<f32> = observed.iter().map(|c| c.re).collect();
    (observed_real, clean, observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_snr_clean_channel_barely_changes_signal() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let (observed, _clean, _obs_c) = apply_channel(&samples, 60.0, "good", 42);
        assert_eq!(observed.len(), samples.len());
        // At 60 dB SNR with the Good channel, energy is preserved within a
        // loose tolerance (multipath rotates phase but conserves power).
        let e_in: f32 = samples.iter().map(|s| s * s).sum();
        let e_out: f32 = observed.iter().map(|s| s * s).sum();
        assert!((e_out / e_in - 1.0).abs() < 0.5, "energy ratio {}", e_out / e_in);
    }

    #[test]
    fn low_snr_adds_substantial_noise_energy() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        let (_o_hi, _, _) = apply_channel(&samples, 40.0, "good", 7);
        let (o_lo, _, _) = apply_channel(&samples, -5.0, "good", 7);
        let e_lo: f32 = o_lo.iter().map(|s| s * s).sum();
        let e_sig: f32 = samples.iter().map(|s| s * s).sum();
        // At -5 dB, noise power should exceed signal power.
        assert!(e_lo > e_sig, "low-snr energy {} should exceed signal {}", e_lo, e_sig);
    }

    #[test]
    fn deterministic_for_same_seed() {
        let samples: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.03).cos() * 0.2).collect();
        let (a, _, _) = apply_channel(&samples, 5.0, "moderate", 99);
        let (b, _, _) = apply_channel(&samples, 5.0, "moderate", 99);
        assert_eq!(a, b);
    }
}
