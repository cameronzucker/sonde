//! Link adaptation + MAC routing (#7 plumbing, design §6).
//!
//! The adaptation **ladder** is a [`Ladder`] value: an ordered set of rungs,
//! fastest/most-fragile first → slowest/most-robust last. Each rung maps an
//! observed [`ChannelQualityReport`] (FER + aggregate SNR) and the pending
//! payload size to a [`Route`] — which mode to hint, which ARQ strategy to run,
//! how wide a window, and the airtime-aware [`ModeProfile`] that derives all link
//! timers.
//!
//! The ladder descends from the fast bit-adaptive OFDM family, through the
//! wide-band low-density floor, to the FT8-class deep-floor nFSK bottom rung.
//! High FER does not drop the link — it *degrades down the ladder* (shrinking the
//! window and lengthening the airtime-derived timers). The floor rungs run the
//! degenerate `WholeMessage` strategy (W=1, no SACK — the canonical floor "no
//! NACK" model).
//!
//! **The ladder is a value, not a global.** [`Ladder::standard`] is today's
//! built-in mirror of the PHY's registered waveforms (the wideband floor is the
//! one real rung). The dynamic link↔PHY registry handshake (sonde-3tm) builds a
//! `Ladder` from the modes the PHY *publishes* and injects it into the
//! [`crate::conn::Connection`]; the routing/adaptation policy here is what each
//! rung pins. Single mode is available today, so the mode-STEP is
//! stubbed-but-not-faked: the policy is live and unit-tested over a synthetic
//! all-available ladder, and steps the moment >1 real rung exists.

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
#[derive(Debug, Clone)]
struct Rung {
    mode: ModeHint,
    strategy: ArqStrategy,
    window: u32,
    /// Minimum effective SNR (dB) to sit on this rung.
    snr_floor_db: f32,
    /// Airtime-aware profile from which all link timers for this rung derive.
    profile: ModeProfile,
    /// Waveform-family id (0 = OFDM, 1 = floor, 2 = deep-floor). A measured SNR is
    /// only predictive *within* a family — a clean floor decode does not prove a
    /// wide-OFDM decode — so mid-session *upshift* may not skip a family
    /// ([`Ladder::adapt_rung`]); downshift (safety) may cross families freely. See
    /// the symmetric-SNR adaptation design F1.
    family: u8,
    /// Whether this rung is backed by a **real, registered, physics-gated PHY
    /// waveform + an estimator-domain `snr_floor_db` knee**. A rung that is *not*
    /// available is **never selectable** — `adapt_rung`/`recommended_rung`/`route`
    /// skip it and [`Ladder::clamp_available`] rounds any request to a real rung
    /// (C7). Rung ids stay stable (the wire `MODE` byte is a protocol contract);
    /// availability only gates selection.
    available: bool,
}

/// dB of effective-SNR penalty at FER = 1.0 (a fully-failing channel reads as
/// ~40 dB worse than its raw SNR, forcing a descent down the ladder). Used by the
/// **connect-time** [`Ladder::route`]/[`Ladder::recommended_rung`] selection only
/// — the mid-session [`Ladder::adapt_rung`] uses FER as a confidence gate instead
/// (design F4).
const FER_PENALTY_DB: f32 = 40.0;

/// Number of rungs in the standard ladder's id space (id 0 = fastest,
/// NUM_RUNGS-1 = deepest). This is the stable wire `MODE`-byte id space for the
/// built-in ladder; not every id is *selectable* (see availability).
pub const NUM_RUNGS: u8 = 5;

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

/// Convenience: build the standard ladder's profile from raw airtime/MTU.
fn profile_ms(over_airtime_ms: u64, per_over_mtu: usize) -> ModeProfile {
    ModeProfile::new(
        std::time::Duration::from_millis(over_airtime_ms),
        per_over_mtu,
    )
}

/// The adaptation ladder — an ordered set of rungs, fastest first.
///
/// Construct it from the PHY's published modes (sonde-3tm registry handshake) or
/// use [`Ladder::standard`] for the built-in mirror of the registered waveforms.
#[derive(Debug, Clone)]
pub struct Ladder {
    rungs: Vec<Rung>,
}

impl Ladder {
    /// The built-in ladder: a mirror of the PHY's registered, physics-gated
    /// waveforms. Today only the wideband floor (`FloorWaveform`, rung 3) is a
    /// registered `Waveform` with a measured estimator-domain knee, so it is the
    /// only `available` rung. The OFDM rungs (0–2) have no `Waveform` (sonde-c7i)
    /// and the deep-floor nFSK rung (4) is coded but not yet wrapped as a
    /// `Waveform` — both carry **placeholder** `snr_floor_db` and are **not
    /// selectable**. (The dynamic registry handshake replaces this static mirror
    /// with a `Ladder` built from what the PHY actually publishes.)
    pub fn standard() -> Self {
        Self {
            rungs: vec![
                Rung {
                    mode: ModeHint::MainAuto,
                    strategy: ArqStrategy::SelectiveRepeat,
                    window: DEFAULT_WINDOW,
                    snr_floor_db: 18.0, // placeholder — no OFDM waveform (sonde-c7i)
                    profile: profile_ms(300, 1024),
                    family: 0, // OFDM
                    available: false,
                },
                Rung {
                    mode: ModeHint::MainAuto,
                    strategy: ArqStrategy::SelectiveRepeat,
                    window: 6,
                    snr_floor_db: 8.0, // placeholder — no OFDM waveform (sonde-c7i)
                    profile: profile_ms(500, 512),
                    family: 0, // OFDM
                    available: false,
                },
                Rung {
                    mode: ModeHint::MainAuto,
                    strategy: ArqStrategy::SelectiveRepeat,
                    window: 4,
                    snr_floor_db: 0.0, // placeholder — no OFDM waveform (sonde-c7i)
                    profile: profile_ms(800, 256),
                    family: 0, // OFDM
                    available: false,
                },
                Rung {
                    mode: ModeHint::Floor,
                    strategy: ArqStrategy::WholeMessage,
                    window: 1,
                    // Estimator-domain FER-knee of the wideband floor, measured by
                    // the runtime estimator in the SNR_2500 reference
                    // (crates/sonde-phy/tests/floor_threshold_sweep.rs): the floor
                    // decodes at ~SNR_2500 16 dB.
                    snr_floor_db: 16.0,
                    profile: profile_ms(3_000, 64),
                    family: 1,       // floor
                    available: true, // FloorWaveform — the one registered, gated waveform
                },
                Rung {
                    mode: ModeHint::FloorCrowdedBand,
                    strategy: ArqStrategy::WholeMessage,
                    window: 1,
                    snr_floor_db: f32::NEG_INFINITY, // placeholder — nFSK not wrapped as a Waveform
                    profile: profile_ms(30_000, 16),
                    family: 2, // deep-floor
                    available: false,
                },
            ],
        }
    }

    /// The standard ladder's *structure* (modes/strategies/windows/knees/families/
    /// availability) but with every rung carrying the same supplied `profile`. The
    /// back-compat path for [`crate::conn::Connection`] constructors that inject a
    /// single profile: a [`Ladder::rung`] swap on `apply_rung` is then a no-op for
    /// the profile, preserving the injected timers/MTU while the registry-built
    /// ladder (per-rung profiles) is the real fix.
    pub fn standard_with_uniform_profile(profile: ModeProfile) -> Self {
        let mut l = Self::standard();
        for r in &mut l.rungs {
            r.profile = profile.clone();
        }
        l
    }

    /// Number of rungs in this ladder's id space.
    pub fn num_rungs(&self) -> u8 {
        self.rungs.len() as u8
    }

    /// The most-robust **available** rung id — the universal failure-convergence
    /// target for the link's BASE-fallback (design P1) and the [`Ladder::clamp_available`]
    /// fallback. With the registry mirror this is the deepest *available* rung, not
    /// necessarily the deepest id (C7): today the wideband floor (rung 3), since the
    /// deep-floor nFSK rung is not yet a registered waveform.
    pub fn base_rung(&self) -> u8 {
        most_robust_available(&self.rungs)
    }

    /// The rung used when no channel measurement exists yet (handshake / bootstrap).
    /// Must be a real, registered mode (C7) and identical on both ends of a
    /// same-build link. The safe choice with no measurement is the most-robust
    /// **available** rung (guaranteed to connect); the measurement loop then climbs.
    pub fn default_rung(&self) -> u8 {
        self.base_rung()
    }

    /// Round a requested rung id to a **selectable (available)** one: the nearest
    /// available rung that is *at least as robust* (id ≥ requested) — a conservative
    /// ceiling toward robustness (Codex C7/clamp guidance); if none is that robust,
    /// the most-robust available. Guarantees nothing fabricated is ever transmitted,
    /// and absorbs an *inbound* id this build does not have.
    pub fn clamp_available(&self, id: u8) -> u8 {
        let start = (id as usize).min(self.rungs.len());
        self.rungs[start..]
            .iter()
            .position(|r| r.available)
            .map(|off| (start + off) as u8)
            .unwrap_or_else(|| most_robust_available(&self.rungs))
    }

    /// The full [`Route`] for an explicit ladder rung id (clamped to the ladder).
    /// Used by mid-session mode adaptation to address a specific mode.
    pub fn rung(&self, id: u8) -> Route {
        let idx = (id as usize).min(self.rungs.len() - 1);
        build_route(&self.rungs[idx], None)
    }

    /// Choose a [`Route`] for a payload of `payload_len` bytes over a channel of the
    /// given quality. The window never exceeds the frames the payload actually
    /// needs (no point opening eight slots for a one-frame message).
    pub fn route(&self, payload_len: usize, quality: &ChannelQualityReport) -> Route {
        let idx = self.recommended_rung(quality) as usize;
        let r = &self.rungs[idx];
        let frames_needed = payload_len.div_ceil(r.profile.per_over_mtu().max(1)).max(1) as u32;
        build_route(r, Some(frames_needed))
    }

    /// The ladder rung id recommended for a channel of the given quality (no payload
    /// cap), restricted to **available** rungs (C7). No measurement ⇒ [`Ladder::default_rung`];
    /// otherwise the fastest available rung whose estimator-domain floor the effective
    /// SNR clears, else the most-robust available rung ([`Ladder::base_rung`]).
    pub fn recommended_rung(&self, quality: &ChannelQualityReport) -> u8 {
        match effective_snr_db(quality) {
            None => self.default_rung(),
            Some(eff) => (0..self.rungs.len())
                .find(|&i| self.rungs[i].available && eff >= self.rungs[i].snr_floor_db)
                .map(|i| i as u8)
                .unwrap_or_else(|| self.base_rung()),
        }
    }

    /// The waveform family of a ladder rung (0 = OFDM, 1 = floor, 2 = deep-floor).
    /// The link resets its SNR estimate when this changes (SNR is mode-conditioned).
    pub fn family_of(&self, id: u8) -> u8 {
        self.rungs[(id as usize).min(self.rungs.len() - 1)].family
    }

    /// The smallest per-frame payload capacity across all **available** rungs — the
    /// fragment size the link enqueues at (gap inventory B4). Sizing every fragment
    /// to the deepest *available* rung guarantees a committed DATA seq stays
    /// transmittable on any mode it may be retransmitted under after a downshift:
    /// the ARQ keys retransmits on a **stable** per-fragment seq, so an oversized
    /// committed fragment could never be re-sent on a slower mode (re-fragmenting
    /// would renumber the stream and break the peer's reassembly). Available-only,
    /// per the same-build / availability-gated selection scope. At least 1.
    pub fn min_available_fragment_bytes(&self) -> usize {
        self.rungs
            .iter()
            .filter(|r| r.available)
            .map(|r| r.profile.per_over_mtu())
            .min()
            .unwrap_or(1)
            .max(1)
    }

    /// Mid-session rung adaptation from a channel **measurement** (symmetric-SNR
    /// adaptation design). The receiver calls this to choose the rung it recommends
    /// the peer use; the sender obeys it (worse-direction-wins). FER is a
    /// **confidence gate**, not a penalty (design F4). Only **available** rungs are
    /// ever returned (C7).
    ///
    /// - **Downshift** on `snr_raw` (no smoothing lag): the most-robust rung the raw
    ///   SNR actually supports — authoritative, may jump several rungs and cross
    ///   families (more-robust is always safe). A credibly-high FER also forces a step.
    /// - **Upshift** only when `snr_smoothed` clears a faster rung's floor **+
    ///   [`ADAPT_UPSHIFT_MARGIN_DB`]** *and* FER is credibly low, capped to **one
    ///   waveform-family step** per call.
    /// - **Else** hold (the dead-band, or no credible measurement).
    pub fn adapt_rung(
        &self,
        current: u8,
        snr_raw: f32,
        snr_smoothed: f32,
        fer: f32,
        fer_samples: u32,
    ) -> u8 {
        let rungs = &self.rungs;
        let n = rungs.len();
        let cur = (current as usize).min(n - 1) as u8;
        let avail = |i: usize| rungs[i].available;

        // Downshift: the fastest AVAILABLE rung the RAW SNR supports; if more robust
        // than where we are, drop straight to it (multi-step, cross-family allowed).
        if snr_raw.is_finite() {
            match (0..n).find(|&i| avail(i) && snr_raw >= rungs[i].snr_floor_db) {
                Some(supported) if supported as u8 > cur => return supported as u8,
                None => {
                    // No available rung supports this SNR ⇒ the most-robust available.
                    let mr = most_robust_available(rungs);
                    if mr > cur {
                        return mr;
                    }
                }
                _ => {}
            }
        }
        // FER-driven downshift: credibly failing ⇒ the next more-robust AVAILABLE rung.
        if fer_samples >= FER_MIN_SAMPLES && fer >= FER_DOWNSHIFT_MIN {
            if let Some(next) = ((cur as usize + 1)..n).find(|&i| avail(i)) {
                return next as u8;
            }
        }
        // Upshift: smoothed SNR clears (faster floor + margin) AND FER is credibly low.
        if snr_smoothed.is_finite() && fer_samples >= FER_MIN_SAMPLES && fer <= FER_UPSHIFT_MAX {
            if let Some(up) = (0..n).find(|&i| {
                avail(i) && snr_smoothed >= rungs[i].snr_floor_db + ADAPT_UPSHIFT_MARGIN_DB
            }) {
                let up = up as u8;
                if up < cur {
                    let cur_fam = rungs[cur as usize].family;
                    if rungs[up as usize].family == cur_fam {
                        return up; // free within a family
                    }
                    // Crossing up: land at the most-robust available rung of the family
                    // one step faster — never skip past it; if none available, hold.
                    return match most_robust_available_in_family(rungs, cur_fam - 1) {
                        Some(boundary) => boundary.max(up),
                        None => cur,
                    };
                }
            }
        }
        cur
    }
}

/// The most-robust (highest-id) **available** rung in `rungs`, or the deepest id if
/// none is available (degenerate; should not happen — the floor is always real).
fn most_robust_available(rungs: &[Rung]) -> u8 {
    (0..rungs.len())
        .rev()
        .find(|&i| rungs[i].available)
        .unwrap_or(rungs.len() - 1) as u8
}

/// The most-robust (highest-id) **available** rung in the given waveform `family`,
/// if any.
fn most_robust_available_in_family(rungs: &[Rung], family: u8) -> Option<u8> {
    (0..rungs.len())
        .rev()
        .find(|&i| rungs[i].available && rungs[i].family == family)
        .map(|i| i as u8)
}

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
        profile: r.profile.clone(),
    }
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

/// The most-robust **available** rung id of the standard built-in ladder — the
/// canonical failure-convergence target. Free-function convenience for callers
/// (and gates) that reference the built-in ladder rather than an injected one.
pub fn base_rung() -> u8 {
    Ladder::standard().base_rung()
}

/// The default rung of the standard built-in ladder (identical to [`base_rung`]
/// today). Free-function convenience for the canonical built-in ladder.
pub fn default_rung() -> u8 {
    Ladder::standard().default_rung()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A synthetic ladder with ALL rungs available, using the classic illustrative
    /// floors (18/8/0/−12/−∞) and the standard ladder's distinct per-rung profiles
    /// — for exercising the multi-rung adaptation ALGORITHM and the per-rung
    /// `ModeProfile` swap even though the real ladder has a single available rung
    /// today (C7). Visible to sibling-module tests within the crate.
    pub(crate) fn all_available_ladder() -> Ladder {
        let mut l = Ladder::standard();
        let floors = [18.0f32, 8.0, 0.0, -12.0, f32::NEG_INFINITY];
        for (i, r) in l.rungs.iter_mut().enumerate() {
            r.available = true;
            r.snr_floor_db = floors[i];
        }
        l
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::all_available_ladder;
    use super::*;

    fn quality(snr_db: f32, total: u32, failed: u32) -> ChannelQualityReport {
        ChannelQualityReport::from_parts(Vec::new(), snr_db, total, failed, None)
    }

    // ---- the real ladder is registry-honest: only the floor is selectable (C7) --

    #[test]
    fn real_ladder_has_only_the_wideband_floor_available() {
        let l = Ladder::standard();
        let avail: Vec<usize> = (0..l.rungs.len())
            .filter(|&i| l.rungs[i].available)
            .collect();
        assert_eq!(
            avail,
            vec![3],
            "today only rung 3 (wideband floor) is registered"
        );
        assert_eq!(l.rungs[3].mode, ModeHint::Floor);
        assert_eq!(
            l.rungs[3].snr_floor_db, 16.0,
            "estimator-domain floor knee (SNR_2500)"
        );
    }

    #[test]
    fn default_and_base_are_the_only_real_mode_not_a_fabricated_ofdm() {
        // The bug C7 fixes: DEFAULT must not be a fabricated OFDM rung.
        let l = Ladder::standard();
        assert_eq!(l.default_rung(), 3);
        assert_eq!(l.base_rung(), 3);
        assert_ne!(l.rung(l.default_rung()).mode, ModeHint::MainAuto);
        assert!(l.rungs[l.default_rung() as usize].available);
        // Free-function convenience mirrors the standard ladder.
        assert_eq!(base_rung(), 3);
        assert_eq!(default_rung(), 3);
    }

    #[test]
    fn clamp_available_rounds_any_request_to_a_real_rung() {
        let l = Ladder::standard();
        // Fabricated/unavailable ids round to the real floor (ceiling toward robust).
        for id in 0..=NUM_RUNGS + 3 {
            assert_eq!(
                l.clamp_available(id),
                3,
                "id {id} clamps to the one real rung"
            );
        }
    }

    #[test]
    fn selection_never_returns_an_unavailable_rung_on_the_real_ladder() {
        let l = Ladder::standard();
        // No matter the channel, today every selector returns the one real rung.
        for snr in [-30.0f32, -6.0, 0.0, 16.0, 25.0, 40.0] {
            assert_eq!(l.recommended_rung(&quality(snr, 100, 0)), 3);
            assert_eq!(l.route(8192, &quality(snr, 100, 0)).mode, ModeHint::Floor);
            assert_eq!(
                l.adapt_rung(3, snr, snr, 0.0, 10),
                3,
                "no other mode to move to"
            );
        }
        assert_eq!(l.recommended_rung(&ChannelQualityReport::empty()), 3);
    }

    #[test]
    fn ladder_over_airtime_increases_monotonically_down_the_id_space() {
        // The descriptor ordering invariant holds across the full id space.
        let l = Ladder::standard();
        for w in l.rungs.windows(2) {
            assert!(w[1].profile.over_airtime() >= w[0].profile.over_airtime());
        }
    }

    // ---- adaptation ALGORITHM, over a synthetic all-available ladder ------------

    #[test]
    fn adapt_downshifts_to_the_rung_raw_snr_supports_multistep_and_crossfamily() {
        let l = all_available_ladder();
        // From rung 1, a collapse to -15 dB supports only the deepest → jump there.
        assert_eq!(l.adapt_rung(1, -15.0, -15.0, 0.0, 10), NUM_RUNGS - 1);
        // A sag to 5 dB supports rung 2 (floor 0 dB) → 1 → 2.
        assert_eq!(l.adapt_rung(1, 5.0, 5.0, 0.0, 10), 2);
    }

    #[test]
    fn adapt_fer_forces_a_downshift_even_at_good_snr_but_only_when_credible() {
        let l = all_available_ladder();
        assert_eq!(l.adapt_rung(1, 20.0, 20.0, 0.5, 8), 2);
        assert_eq!(l.adapt_rung(1, 20.0, 20.0, 0.5, 1), 1);
    }

    #[test]
    fn adapt_upshift_needs_the_margin_above_the_floor() {
        let l = all_available_ladder();
        // 20 dB clears rung 1 (8+3) but NOT rung 0 (18+3=21) → hold at rung 1.
        assert_eq!(l.adapt_rung(1, 20.0, 20.0, 0.0, 8), 1);
        // 25 dB clears rung 0's floor+margin → climb to 0.
        assert_eq!(l.adapt_rung(1, 25.0, 25.0, 0.0, 8), 0);
    }

    #[test]
    fn adapt_upshift_is_gated_by_credible_low_fer() {
        let l = all_available_ladder();
        assert_eq!(l.adapt_rung(2, 25.0, 25.0, 0.30, 8), 3);
        assert_eq!(l.adapt_rung(2, 25.0, 25.0, 0.0, 1), 2);
    }

    #[test]
    fn adapt_upshift_across_a_family_is_capped_to_one_step() {
        let l = all_available_ladder();
        // At rung 3 (floor) a great channel only steps into OFDM at its most-robust
        // rung (2), not a leap to rung 0; the next decision then climbs within OFDM.
        assert_eq!(l.adapt_rung(3, 25.0, 25.0, 0.0, 8), 2);
        assert_eq!(l.adapt_rung(2, 25.0, 25.0, 0.0, 8), 0);
    }

    #[test]
    fn adapt_holds_in_the_dead_band_and_without_a_measurement() {
        let l = all_available_ladder();
        assert_eq!(l.adapt_rung(1, 12.0, 12.0, 0.0, 8), 1);
        assert_eq!(l.adapt_rung(1, f32::NAN, f32::NAN, 0.0, 0), 1);
    }

    #[test]
    fn adapt_skips_an_unavailable_rung_within_the_ladder() {
        // Mark the fastest rung unavailable: a great channel must stop at the
        // fastest *available* rung (1), never selecting the unavailable rung 0.
        let mut l = all_available_ladder();
        l.rungs[0].available = false;
        assert_eq!(l.adapt_rung(2, 40.0, 40.0, 0.0, 8), 1);
        // And a collapse skips an unavailable deepest rung, landing on the deepest
        // available one.
        let mut l2 = all_available_ladder();
        l2.rungs[NUM_RUNGS as usize - 1].available = false;
        assert_eq!(l2.adapt_rung(1, -30.0, -30.0, 0.0, 8), NUM_RUNGS - 2);
    }

    #[test]
    fn min_available_fragment_bytes_is_the_deepest_available_rungs_capacity() {
        // B4: fragments must fit the SMALLEST reachable mode so a downshift (or a
        // retransmit after one) never orphans an oversized fragment. Today only the
        // wideband floor (64 B) is available, so that is the global minimum.
        assert_eq!(Ladder::standard().min_available_fragment_bytes(), 64);
        // With the whole ladder available, the deepest rung (deep-floor, 16 B) binds.
        assert_eq!(all_available_ladder().min_available_fragment_bytes(), 16);
    }

    #[test]
    fn per_rung_profiles_differ_so_apply_rung_can_swap_timers() {
        // The all-available ladder carries the standard distinct per-rung profiles,
        // so a rung swap changes the airtime-derived timers (the B3 enabler).
        let l = all_available_ladder();
        assert_ne!(
            l.rung(0).profile.over_airtime(),
            l.rung(3).profile.over_airtime(),
            "fast OFDM rung and the floor must have distinct over-airtimes"
        );
    }
}
