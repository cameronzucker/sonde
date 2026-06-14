//! A wall-clock abstraction for the real-time [`Driver`](crate::driver::Driver).
//!
//! The sans-IO [`Connection`](crate::Connection) reads no clock — time is
//! injected. The `Driver` is the one place real time enters; making the clock a
//! trait keeps the driver itself deterministically testable (a manual clock in
//! tests, the system clock in production).

use std::time::Duration;

/// A monotonic clock measured as elapsed time since some fixed origin.
pub trait Clock {
    /// Elapsed time since the clock's origin. Must be monotonic non-decreasing.
    fn now(&self) -> Duration;
}

/// The production clock: elapsed wall-clock time since construction.
pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    /// Start a system clock at the current instant.
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}
