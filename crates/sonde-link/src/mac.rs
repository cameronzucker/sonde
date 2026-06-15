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
    /// Waveform-family id (0 = OFDM, 1 = floor, 2 = deep-floor). A measured SNR is
    /// only predictive *within* a family — a clean floor decode does not prove a
    /// wide-OFDM decode — so mid-session *upshift* may not skip a family
    /// ([`adapt_rung`]); downshift (safety) may cross families freely. See the
    /// symmetric-SNR adaptation design F1.
    family: u8,
}

/// dB of effective-SNR penalty at FER = 1.0 (a fully-failing channel reads as
/// ~40 dB worse than its raw SNR, forcing a descent down the ladder). Used by the
/// **connect-time** [`route`]/[`recommended_rung`] selection only — the
/// mid-session [`adapt_rung`] uses FER as a confidence gate instead (design F4).
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
            family: 0, // OFDM
        },
        Rung {
            mode: ModeHint::MainAuto,
            strategy: ArqStrategy::SelectiveRepeat,
            window: 6,
            snr_floor_db: 8.0,
            over_airtime_ms: 500,
            per_over_mtu: 512,
            family: 0, // OFDM
        },
        Rung {
            mode: ModeHint::MainAuto,
            strategy: ArqStrategy::SelectiveRepeat,
            window: 4,
            snr_floor_db: 0.0,
            over_airtime_ms: 800,
            per_over_mtu: 256,
            family: 0, // OFDM
        },
        Rung {
            mode: ModeHint::Floor,
            strategy: ArqStrategy::WholeMessage,
            window: 1,
            snr_floor_db: -12.0,
            over_airtime_ms: 3_000,
            per_over_mtu: 64,
            family: 1, // floor
        },
        Rung {
            mode: ModeHint::FloorCrowdedBand,
            strategy: ArqStrategy::WholeMessage,
            window: 1,
            snr_floor_db: f32::NEG_INFINITY, // the bottom rung always qualifies
            over_airtime_ms: 30_000,
            per_over_mtu: 16,
            family: 2, // deep-floor
        },
    ]
}

/// Number of rungs on the adaptation ladder (id 0 = fastest, NUM_RUNGS-1 = base).
pub const NUM_RUNGS: u8 = 5;
/// The most-robust bottom rung (deep-floor nFSK) — the universal failure-
/// convergence target for the link's BASE-fallback (design P1).
pub const BASE_RUNG: u8 = NUM_RUNGS - 1;
/// Index of the ladder rung used when no channel measurement exists yet
/// (missing SNR ⇒ the mid OFDM rung, per design §6 / the PHY's `MainAuto`).
pub const DEFAULT_RUNG: u8 = 1;

/// SNR margin (dB) required *above* a faster rung's floor before upshifting onto it
/// — the dead-band that stops flapping at a rung boundary (adaptation design F2).
pub const ADAPT_UPSHIFT_MARGIN_DB: f32 = 3.0;
/// Minimum FER sample count before FER is credible enough to gate a shift (F4).
const FER_MIN_SAMPLES: u32 = 4;
/// Upshift only when the (credible) FER is at or below this — actual decode success
/// confirms the climb, regardless of how good the SNR looks (F4).
const FER_UPSHIFT_MAX: f32 = 0.05;
/// A credible FER at or above this forces a downshift on its own (F4).
const FER_DOWNSHIFT_MIN: f32 = 0.20;

/// Build a [`Route`] from a ladder rung, optionally capping the window to the
/// frames a payload actually needs.
fn build_route(r: &Rung, window_cap: Option<u32>) -> Route {
    let window = match window_cap {
        Some(cap) => r.window.min(cap),
        None => r.window,
    };
    Route {
        mode: r.mode,
        strategy: r.strategy,
        window: WindowParams { window },
        profile: ModeProfile::new(
            std::time::Duration::from_millis(r.over_airtime_ms),
            r.per_over_mtu,
        ),
    }
}

/// The full [`Route`] for an explicit ladder rung id (clamped to the ladder).
/// Used by mid-session mode adaptation to address a specific mode.
pub fn rung(id: u8) -> Route {
    let rungs = ladder();
    let idx = (id as usize).min(rungs.len() - 1);
    build_route(&rungs[idx], None)
}

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
    let idx = recommended_rung(quality) as usize;
    let r = &rungs[idx];
    let frames_needed = payload_len.div_ceil(r.per_over_mtu.max(1)).max(1) as u32;
    build_route(r, Some(frames_needed))
}

/// The ladder rung id recommended for a channel of the given quality (no payload
/// cap). Higher FER / lower SNR ⇒ a higher (more robust) rung id.
pub fn recommended_rung(quality: &ChannelQualityReport) -> u8 {
    let rungs = ladder();
    let idx = match effective_snr_db(quality) {
        None => DEFAULT_RUNG as usize,
        Some(eff) => rungs
            .iter()
            .position(|r| eff >= r.snr_floor_db)
            .unwrap_or(rungs.len() - 1),
    };
    idx as u8
}

/// The waveform family of a ladder rung (0 = OFDM, 1 = floor, 2 = deep-floor).
/// The link resets its SNR estimate when this changes (SNR is mode-conditioned).
pub fn family_of(id: u8) -> u8 {
    let rungs = ladder();
    rungs[(id as usize).min(rungs.len() - 1)].family
}

/// The most-robust (highest-id) rung in the given waveform `family`.
fn most_robust_in_family(rungs: &[Rung], family: u8) -> u8 {
    rungs
        .iter()
        .rposition(|r| r.family == family)
        .unwrap_or(rungs.len() - 1) as u8
}

/// Mid-session rung adaptation from a channel **measurement** (symmetric-SNR
/// adaptation design). The receiver calls this to choose the rung it recommends the
/// peer use; the sender obeys it (worse-direction-wins). This is distinct from the
/// connect-time [`route`]/[`recommended_rung`] (which fold FER into an SNR penalty);
/// here FER is a **confidence gate**, not a penalty (design F4).
///
/// - **Downshift** on `snr_raw` (no smoothing lag): the most-robust rung the raw
///   SNR actually supports — authoritative, may jump several rungs and cross
///   families (more-robust is always safe). A credibly-high FER also forces a step.
/// - **Upshift** only when `snr_smoothed` clears a faster rung's floor **+
///   [`ADAPT_UPSHIFT_MARGIN_DB`]** *and* FER is credibly low, capped to **one
///   waveform-family step** per call (a clean decode on the current mode does not
///   prove a different waveform decodes).
/// - **Else** hold (inside the dead-band, or no credible measurement).
pub fn adapt_rung(current: u8, snr_raw: f32, snr_smoothed: f32, fer: f32, fer_samples: u32) -> u8 {
    let rungs = ladder();
    let cur = (current as usize).min(rungs.len() - 1) as u8;

    // Downshift: the fastest rung the RAW SNR supports; if that is more robust than
    // where we are, drop straight to it (multi-step, cross-family allowed).
    if snr_raw.is_finite() {
        let supported = rungs
            .iter()
            .position(|r| snr_raw >= r.snr_floor_db)
            .unwrap_or(rungs.len() - 1) as u8;
        if supported > cur {
            return supported;
        }
    }
    // FER-driven downshift: credibly failing ⇒ one rung more robust.
    if fer_samples >= FER_MIN_SAMPLES && fer >= FER_DOWNSHIFT_MIN && cur < BASE_RUNG {
        return cur + 1;
    }
    // Upshift: smoothed SNR clears (faster floor + margin) AND FER is credibly low.
    if snr_smoothed.is_finite() && fer_samples >= FER_MIN_SAMPLES && fer <= FER_UPSHIFT_MAX {
        if let Some(up) = rungs
            .iter()
            .position(|r| snr_smoothed >= r.snr_floor_db + ADAPT_UPSHIFT_MARGIN_DB)
        {
            let up = up as u8;
            if up < cur {
                let cur_fam = rungs[cur as usize].family;
                let up_fam = rungs[up as usize].family;
                if up_fam == cur_fam {
                    return up; // free within a family
                }
                // Crossing up a family: land at the most-robust rung of the family
                // one step faster — never skip past it in a single decision.
                return most_robust_in_family(&rungs, cur_fam - 1).max(up);
            }
        }
    }
    cur
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

    #[test]
    fn rung_addresses_specific_ladder_modes() {
        let top = rung(0);
        assert_eq!(top.mode, ModeHint::MainAuto);
        assert_eq!(top.strategy, ArqStrategy::SelectiveRepeat);
        assert!(top.window.window > 1, "fast rung is not payload-capped");

        let base = rung(BASE_RUNG);
        assert_eq!(base.mode, ModeHint::FloorCrowdedBand);
        assert_eq!(base.strategy, ArqStrategy::WholeMessage);
        assert_eq!(base.window.window, 1, "deep-floor base is whole-message");
        // Out-of-range clamps to the base rung.
        assert_eq!(rung(99), rung(BASE_RUNG));
    }

    #[test]
    fn recommended_rung_increases_as_the_channel_worsens() {
        assert!(
            recommended_rung(&quality(25.0, 100, 0)) < recommended_rung(&quality(-25.0, 100, 0))
        );
        assert_eq!(recommended_rung(&quality(-25.0, 100, 0)), BASE_RUNG);
    }

    // ---- adapt_rung: measurement-based, symmetric, FER-gated (sonde-qnq) --------

    #[test]
    fn adapt_downshifts_to_the_rung_raw_snr_supports_multistep_and_crossfamily() {
        // From rung 1, a collapse to -15 dB supports only BASE → jump straight there.
        assert_eq!(adapt_rung(1, -15.0, -15.0, 0.0, 10), BASE_RUNG);
        // A sag to 5 dB supports rung 2 (floor 0 dB) → 1 → 2.
        assert_eq!(adapt_rung(1, 5.0, 5.0, 0.0, 10), 2);
    }

    #[test]
    fn adapt_fer_forces_a_downshift_even_at_good_snr_but_only_when_credible() {
        // Raw SNR fine for rung 1, but credibly-high FER → step down one.
        assert_eq!(adapt_rung(1, 20.0, 20.0, 0.5, 8), 2);
        // Same FER with too few samples ⇒ not credible ⇒ no FER-driven shift.
        assert_eq!(adapt_rung(1, 20.0, 20.0, 0.5, 1), 1);
    }

    #[test]
    fn adapt_upshift_needs_the_margin_above_the_floor() {
        // 20 dB clears rung 1 (8+3) but NOT rung 0 (18+3=21) → hold at rung 1.
        assert_eq!(adapt_rung(1, 20.0, 20.0, 0.0, 8), 1);
        // 25 dB clears rung 0's floor+margin → climb to 0 (free within OFDM family).
        assert_eq!(adapt_rung(1, 25.0, 25.0, 0.0, 8), 0);
    }

    #[test]
    fn adapt_upshift_is_gated_by_credible_low_fer() {
        // Great SNR but credibly-high FER → FER-downshift wins, never an upshift.
        assert_eq!(adapt_rung(2, 25.0, 25.0, 0.30, 8), 3);
        // Great SNR, low FER, but too few samples → hold (not yet credible to climb).
        assert_eq!(adapt_rung(2, 25.0, 25.0, 0.0, 1), 2);
    }

    #[test]
    fn adapt_upshift_across_a_family_is_capped_to_one_step() {
        // At rung 3 (floor family) a great channel only steps into the OFDM family
        // at its most-robust rung (2) — it does not leap to rung 0.
        assert_eq!(adapt_rung(3, 25.0, 25.0, 0.0, 8), 2);
        // The next decision then climbs freely within OFDM.
        assert_eq!(adapt_rung(2, 25.0, 25.0, 0.0, 8), 0);
    }

    #[test]
    fn adapt_holds_in_the_dead_band_and_without_a_measurement() {
        // 12 dB at rung 1: supports rung 1, and 12 < rung-1-floor+margin for going
        // faster ⇒ neither up nor down ⇒ hold (the dead-band).
        assert_eq!(adapt_rung(1, 12.0, 12.0, 0.0, 8), 1);
        // No measurement (NaN SNR, no samples) ⇒ hold the current rung.
        assert_eq!(adapt_rung(1, f32::NAN, f32::NAN, 0.0, 0), 1);
    }
}
