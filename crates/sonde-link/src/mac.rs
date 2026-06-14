//! Link adaptation + MAC routing (#7 plumbing, design §6).
//!
//! [`route`] is a pure decision function: it maps the observed
//! [`ChannelQualityReport`] (FER + aggregate SNR) and the pending payload size
//! to a [`Route`] — which mode to hint, which ARQ strategy to run, how wide a
//! window, and the airtime-aware [`ModeProfile`] that derives all link timers.
//!
//! The adaptation **ladder** descends from the fast bit-adaptive OFDM family,
//! through the wide-band low-density floor, to the FT8-class deep-floor nFSK
//! bottom rung. High FER does not drop the link — it *degrades down the ladder*
//! (shrinking the window and lengthening the airtime-derived timers). The floor
//! rungs run the degenerate `WholeMessage` strategy (W=1, no SACK — the
//! canonical floor "no NACK" model).
//!
//! Numeric profile parameters here are **illustrative link-side defaults** until
//! the PHY exposes real per-mode `ModeProfile`s; the ladder *structure* and the
//! routing policy are what this module pins. Single mode is available today, so
//! the mode-STEP is stubbed-but-not-faked: the policy is live and unit-tested
//! over profiles, and steps the moment >1 real profile exists.

use sonde_phy::modes::ModeHint;
use sonde_phy::phy_api::ChannelQualityReport;

use crate::arq::DEFAULT_WINDOW;
use crate::profile::ModeProfile;

/// The ARQ strategy a route selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArqStrategy {
    /// Windowed selective repeat (cumulative + SACK). OFDM family.
    SelectiveRepeat,
    /// Degenerate floor strategy: window 1, no SACK, resend-until-acked.
    WholeMessage,
}

/// Window sizing for the selected route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowParams {
    /// Frames in flight per over.
    pub window: u32,
}

/// A complete routing decision for the next over.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// Mode hint to transmit under.
    pub mode: ModeHint,
    /// ARQ strategy to run.
    pub strategy: ArqStrategy,
    /// Window sizing.
    pub window: WindowParams,
    /// Airtime-aware profile that derives all link timers for this rung.
    pub profile: ModeProfile,
}

/// One rung of the adaptation ladder.
struct Rung {
    mode: ModeHint,
    strategy: ArqStrategy,
    window: u32,
    /// Minimum effective SNR (dB) to sit on this rung.
    snr_floor_db: f32,
    over_airtime_ms: u64,
    per_over_mtu: usize,
}

/// dB of effective-SNR penalty at FER = 1.0 (a fully-failing channel reads as
/// ~40 dB worse than its raw SNR, forcing a descent down the ladder).
const FER_PENALTY_DB: f32 = 40.0;

/// The ladder, fastest/most-fragile first → slowest/most-robust last.
fn ladder() -> [Rung; 5] {
    [
        Rung {
            mode: ModeHint::MainAuto,
            strategy: ArqStrategy::SelectiveRepeat,
            window: DEFAULT_WINDOW,
            snr_floor_db: 18.0,
            over_airtime_ms: 300,
            per_over_mtu: 1024,
        },
        Rung {
            mode: ModeHint::MainAuto,
            strategy: ArqStrategy::SelectiveRepeat,
            window: 6,
            snr_floor_db: 8.0,
            over_airtime_ms: 500,
            per_over_mtu: 512,
        },
        Rung {
            mode: ModeHint::MainAuto,
            strategy: ArqStrategy::SelectiveRepeat,
            window: 4,
            snr_floor_db: 0.0,
            over_airtime_ms: 800,
            per_over_mtu: 256,
        },
        Rung {
            mode: ModeHint::Floor,
            strategy: ArqStrategy::WholeMessage,
            window: 1,
            snr_floor_db: -12.0,
            over_airtime_ms: 3_000,
            per_over_mtu: 64,
        },
        Rung {
            mode: ModeHint::FloorCrowdedBand,
            strategy: ArqStrategy::WholeMessage,
            window: 1,
            snr_floor_db: f32::NEG_INFINITY, // the bottom rung always qualifies
            over_airtime_ms: 30_000,
            per_over_mtu: 16,
        },
    ]
}

/// Index of the ladder rung used when no channel measurement exists yet
/// (missing SNR ⇒ the mid OFDM rung, per design §6 / the PHY's `MainAuto`).
const DEFAULT_RUNG: usize = 1;

/// Effective SNR = raw aggregate SNR penalized by frame-error rate. `None` when
/// no frames have been observed yet (SNR is NaN).
fn effective_snr_db(q: &ChannelQualityReport) -> Option<f32> {
    let snr = q.aggregate_snr_db();
    if snr.is_nan() {
        return None;
    }
    Some(snr - q.frame_error_rate() * FER_PENALTY_DB)
}

/// Choose a [`Route`] for a payload of `payload_len` bytes over a channel of the
/// given quality. The window never exceeds the frames the payload actually
/// needs (no point opening eight slots for a one-frame message).
pub fn route(payload_len: usize, quality: &ChannelQualityReport) -> Route {
    let rungs = ladder();
    let idx = match effective_snr_db(quality) {
        None => DEFAULT_RUNG,
        Some(eff) => rungs
            .iter()
            .position(|r| eff >= r.snr_floor_db)
            .unwrap_or(rungs.len() - 1),
    };
    let r = &rungs[idx];
    let profile = ModeProfile::new(
        std::time::Duration::from_millis(r.over_airtime_ms),
        r.per_over_mtu,
    );
    let frames_needed = payload_len.div_ceil(r.per_over_mtu.max(1)).max(1) as u32;
    Route {
        mode: r.mode,
        strategy: r.strategy,
        window: WindowParams {
            window: r.window.min(frames_needed),
        },
        profile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quality(snr_db: f32, total: u32, failed: u32) -> ChannelQualityReport {
        ChannelQualityReport::from_parts(Vec::new(), snr_db, total, failed, None)
    }

    #[test]
    fn clean_high_snr_channel_picks_fast_ofdm_selective_repeat() {
        let r = route(8192, &quality(25.0, 100, 0));
        assert_eq!(r.mode, ModeHint::MainAuto);
        assert_eq!(r.strategy, ArqStrategy::SelectiveRepeat);
        assert!(r.window.window > 1);
    }

    #[test]
    fn high_fer_degrades_the_rung_even_at_decent_snr() {
        // 20 dB raw, but half the frames are failing ⇒ effective ~0 dB ⇒ a
        // narrower, slower rung than SNR alone would pick.
        let clean = route(8192, &quality(20.0, 100, 0));
        let degraded = route(8192, &quality(20.0, 100, 50));
        assert!(
            degraded.window.window < clean.window.window,
            "high FER must shrink the window"
        );
        assert!(
            degraded.profile.over_airtime() >= clean.profile.over_airtime(),
            "high FER must not speed the timers up"
        );
    }

    #[test]
    fn low_snr_drops_to_the_floor_whole_message() {
        let r = route(8192, &quality(-6.0, 100, 0));
        assert_eq!(r.mode, ModeHint::Floor);
        assert_eq!(r.strategy, ArqStrategy::WholeMessage);
        assert_eq!(r.window.window, 1);
    }

    #[test]
    fn very_low_snr_uses_the_deep_floor_nfsk_bottom_rung() {
        let r = route(8192, &quality(-25.0, 100, 0));
        assert_eq!(r.mode, ModeHint::FloorCrowdedBand);
        assert_eq!(r.strategy, ArqStrategy::WholeMessage);
        // The deep-floor over is tens of seconds (airtime-aware timers).
        assert!(r.profile.over_airtime() >= std::time::Duration::from_secs(10));
    }

    #[test]
    fn no_measurement_falls_back_to_mid_ofdm() {
        let r = route(8192, &ChannelQualityReport::empty());
        assert_eq!(r.mode, ModeHint::MainAuto);
        assert_eq!(r.strategy, ArqStrategy::SelectiveRepeat);
    }

    #[test]
    fn ladder_over_airtime_increases_monotonically_down_the_rungs() {
        // Descending SNR ⇒ non-decreasing over-airtime (slower, more robust).
        let snrs = [25.0f32, 12.0, 3.0, -6.0, -25.0];
        let airtimes: Vec<_> = snrs
            .iter()
            .map(|&s| route(8192, &quality(s, 100, 0)).profile.over_airtime())
            .collect();
        for w in airtimes.windows(2) {
            assert!(
                w[1] >= w[0],
                "airtime must not decrease as the channel worsens"
            );
        }
    }

    #[test]
    fn window_never_exceeds_frames_the_payload_needs() {
        // A tiny payload on a clean channel: window capped to a single frame.
        let r = route(10, &quality(25.0, 100, 0));
        assert_eq!(r.window.window, 1);
    }
}
