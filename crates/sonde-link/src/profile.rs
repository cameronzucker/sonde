//! Mode-derived, airtime-aware link timing.
//!
//! Every link timer (turn-recovery, retransmit/over, keepalive) is a function of
//! the current mode's *over airtime* — the wall-clock duration of one over at that
//! mode. This is mandatory (design §6): a deep-floor (FT8-class) over is tens of
//! seconds, vs sub-second for OFDM, so a flat timer would either thrash the fast
//! modes or declare the slow modes dead before a single over completes. Timers
//! scale linearly with `over_airtime`, so the link is correct across the whole
//! ladder the moment a new mode profile is supplied — it never hardcodes mode
//! specifics.
//!
//! The `ModeProfile` is supplied by the PHY/MAC; until the PHY exposes one, the
//! link uses defaults.

use std::time::Duration;

/// Airtime-aware descriptor of a single PHY mode, from which all link timers are
/// derived. Supplied by the PHY/MAC layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeProfile {
    over_airtime: Duration,
    per_over_mtu: usize,
}

/// Multiplier: while LISTENING, wait this many over-airtimes for the peer's reply
/// over before re-taking the floor (peer turnaround + peer's over + margin).
const TURN_RECOVERY_OVERS: u32 = 2;
/// Multiplier: a sent over must be acknowledged within this many over-airtimes
/// (my over + turnaround + peer's reply over + margin) or the window is retx'd.
const OVER_TIMEOUT_OVERS: u32 = 3;
/// Multiplier: emit a keepalive after this many idle over-airtimes.
const KEEPALIVE_OVERS: u32 = 8;

impl ModeProfile {
    /// Construct a profile from the mode's over airtime and per-over MTU.
    pub fn new(over_airtime: Duration, per_over_mtu: usize) -> Self {
        Self {
            over_airtime,
            per_over_mtu,
        }
    }

    /// The wall-clock duration of one over at this mode.
    pub fn over_airtime(&self) -> Duration {
        self.over_airtime
    }

    /// Payload bytes carriable in one over at this mode.
    pub fn per_over_mtu(&self) -> usize {
        self.per_over_mtu
    }

    /// Time to wait, while LISTENING, for the peer's reply over before re-taking
    /// the floor and retransmitting (the turn-recovery timer, §3.5).
    pub fn turn_recovery_timeout(&self) -> Duration {
        self.over_airtime * TURN_RECOVERY_OVERS
    }

    /// Time to wait for a sent over to be acknowledged before retransmitting the
    /// unacked window in the next over.
    pub fn over_timeout(&self) -> Duration {
        self.over_airtime * OVER_TIMEOUT_OVERS
    }

    /// Idle interval after which a KEEPALIVE is emitted to hold the link up.
    pub fn keepalive_interval(&self) -> Duration {
        self.over_airtime * KEEPALIVE_OVERS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ofdm() -> ModeProfile {
        ModeProfile::new(Duration::from_millis(500), 4096)
    }

    fn deep_floor() -> ModeProfile {
        ModeProfile::new(Duration::from_secs(30), 16)
    }

    #[test]
    fn timers_are_ordered_recovery_lt_over_lt_keepalive() {
        let p = ofdm();
        assert!(p.turn_recovery_timeout() < p.over_timeout());
        assert!(p.over_timeout() < p.keepalive_interval());
    }

    #[test]
    fn timers_scale_linearly_with_over_airtime() {
        let fast = ofdm();
        let slow = deep_floor();
        // 30s / 0.5s = 60x the airtime ⇒ 60x every timer.
        let ratio = slow.over_airtime().as_secs_f64() / fast.over_airtime().as_secs_f64();
        let approx =
            |a: Duration, b: Duration| (a.as_secs_f64() / b.as_secs_f64() - ratio).abs() < 1e-9;
        assert!(approx(
            slow.turn_recovery_timeout(),
            fast.turn_recovery_timeout()
        ));
        assert!(approx(slow.over_timeout(), fast.over_timeout()));
        assert!(approx(slow.keepalive_interval(), fast.keepalive_interval()));
    }

    #[test]
    fn deep_floor_over_timeout_is_tens_of_seconds_not_subsecond() {
        // The whole point of airtime-derived timing: a deep-floor over must not be
        // declared late after a flat sub-second budget.
        assert!(deep_floor().over_timeout() >= Duration::from_secs(30));
    }
}
