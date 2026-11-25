//! Monotonic time based on the ARM generic counter.

use crate::arch::timer;
pub use core::time::Duration;

/// A point in time (generic counter value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(u64);

impl Instant {
    /// The current time.
    pub fn now() -> Self {
        Instant(timer::counter())
    }

    /// Raw counter value.
    pub fn ticks(self) -> u64 {
        self.0
    }

    /// Time elapsed since `earlier` (zero if `earlier` is later).
    pub fn duration_since(self, earlier: Instant) -> Duration {
        Duration::from_nanos(timer::ticks_to_ns(self.0.saturating_sub(earlier.0)))
    }

    /// Time elapsed since `self`.
    pub fn elapsed(self) -> Duration {
        Instant::now().duration_since(self)
    }

    /// `self + d`.
    pub fn checked_add(self, d: Duration) -> Option<Instant> {
        let ns = u64::try_from(d.as_nanos()).ok()?;
        self.0.checked_add(timer::ns_to_ticks(ns)).map(Instant)
    }
}

impl core::ops::Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, d: Duration) -> Instant {
        self.checked_add(d).expect("Instant overflow")
    }
}

impl core::ops::Sub for Instant {
    type Output = Duration;
    fn sub(self, other: Instant) -> Duration {
        self.duration_since(other)
    }
}

/// Value of the system counter as a duration (time since the counter started, roughly since power-on).
pub fn uptime() -> Duration {
    Duration::from_nanos(timer::ticks_to_ns(timer::counter()))
}
