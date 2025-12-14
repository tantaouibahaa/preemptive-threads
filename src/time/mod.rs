//! Time management and timer interrupt handling.
//!
//! This module provides timer interrupt handling, time slice accounting,
//! and preemption support for the threading system.

pub mod tick;
pub mod timer;

pub use tick::{TickCounter, TimeSlice};
pub use timer::{Timer, TimerConfig, TimerError, PreemptGuard, IrqGuard};

/// Get monotonic time - alias for Instant::now() for compatibility
pub fn get_monotonic_time() -> Instant {
    Instant::now()
}

/// Nanoseconds since some arbitrary epoch.
///
/// This is used for high-resolution timing and scheduling decisions.
/// The actual epoch is implementation-defined and may vary between architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u64);

impl Instant {
    /// Create a new instant from nanoseconds since epoch.
    pub fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }
    
    /// Get nanoseconds since epoch.
    pub fn as_nanos(self) -> u64 {
        self.0
    }
    
    /// Get nanoseconds since epoch as u128 for calculations.
    pub fn as_nanos_u128(self) -> u128 {
        self.0 as u128
    }
    
    /// Get the current instant.
    ///
    /// This reads the current time from the ARM Generic Timer.
    pub fn now() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // Read ARM Generic Timer counter
            let cnt: u64;
            unsafe {
                core::arch::asm!(
                    "mrs {}, cntpct_el0",
                    out(reg) cnt,
                    options(nostack, nomem, preserves_flags)
                );
            }
            Self(cnt)
        }

        #[cfg(not(target_arch = "aarch64"))]
        {
            // Fallback for testing on non-ARM hosts
            Self(0)
        }
    }
    
    /// Calculate duration since another instant.
    ///
    /// # Panics
    ///
    /// Panics if `earlier` is after `self`.
    pub fn duration_since(self, earlier: Instant) -> Duration {
        Duration::from_nanos(self.0 - earlier.0)
    }
    
}

impl core::ops::Add<Duration> for Instant {
    type Output = Self;

    fn add(self, duration: Duration) -> Self {
        Self(self.0 + duration.as_nanos())
    }
}

/// A duration of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration(u64);

impl Duration {
    /// Create a duration from nanoseconds.
    pub fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }
    
    /// Create a duration from microseconds.
    pub fn from_micros(micros: u64) -> Self {
        Self(micros * 1_000)
    }
    
    /// Create a duration from milliseconds.
    pub fn from_millis(millis: u64) -> Self {
        Self(millis * 1_000_000)
    }
    
    /// Get nanoseconds in this duration.
    pub fn as_nanos(self) -> u64 {
        self.0
    }
    
    /// Get nanoseconds as u128 for calculations.
    pub fn as_nanos_u128(self) -> u128 {
        self.0 as u128
    }
    
    /// Get microseconds in this duration.
    pub fn as_micros(self) -> u64 {
        self.0 / 1_000
    }
    
    /// Get milliseconds in this duration.
    pub fn as_millis(self) -> u64 {
        self.0 / 1_000_000
    }
}

/// Frequency in Hz for timer interrupts.
pub const TIMER_FREQUENCY_HZ: u32 = 1000; // 1 kHz = 1ms time slices

/// Default quantum duration in nanoseconds (1ms).
pub const DEFAULT_QUANTUM_NS: u64 = 1_000_000;