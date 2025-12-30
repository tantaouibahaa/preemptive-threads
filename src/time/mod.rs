//! Time management and time slice accounting.
 
use portable_atomic::{AtomicU32, AtomicU64, Ordering};

pub struct TimeSlice {
    vruntime: AtomicU64,
    slice_start: AtomicU64,
    quantum: AtomicU64,
    priority: AtomicU32,
}

impl TimeSlice {
    pub fn new(priority: u8) -> Self {
        let quantum = Self::calculate_quantum(priority);
        Self {
            vruntime: AtomicU64::new(0),
            slice_start: AtomicU64::new(0),
            quantum: AtomicU64::new(quantum),
            priority: AtomicU32::new(priority as u32),
        }
    }

    pub fn start_slice(&self, current_time: Instant) {
        self.slice_start.store(current_time.as_nanos(), Ordering::Release);
    }

    pub fn update_vruntime(&self, current_time: Instant) -> bool {
        let slice_start = self.slice_start.load(Ordering::Acquire);
        let quantum = self.quantum.load(Ordering::Acquire);
        let priority = self.priority.load(Ordering::Acquire);

        if slice_start == 0 {
            return false;
        }

        let elapsed = current_time.as_nanos().saturating_sub(slice_start);
        let priority_factor = Self::calculate_priority_factor(priority as u8);
        let virtual_elapsed = (elapsed * 1000) / priority_factor as u64;

        self.vruntime.fetch_add(virtual_elapsed, Ordering::AcqRel);
        elapsed >= quantum
    }

    pub fn vruntime(&self) -> u64 {
        self.vruntime.load(Ordering::Acquire)
    }

    pub fn set_priority(&self, new_priority: u8) {
        self.priority.store(new_priority as u32, Ordering::Release);
        let new_quantum = Self::calculate_quantum(new_priority);
        self.quantum.store(new_quantum, Ordering::Release);
    }

    pub fn set_custom_duration(&self, duration: Duration) {
        self.quantum.store(duration.as_nanos(), Ordering::Release);
    }

    pub fn priority(&self) -> u8 {
        self.priority.load(Ordering::Acquire) as u8
    }

    fn calculate_quantum(priority: u8) -> u64 {
        let base_quantum = DEFAULT_QUANTUM_NS;
        match priority {
            0..=63 => base_quantum / 2,
            64..=127 => base_quantum,
            128..=191 => base_quantum * 2,
            192..=255 => base_quantum * 4,
        }
    }

    fn calculate_priority_factor(priority: u8) -> u32 {
        match priority {
            0..=63 => 500,
            64..=127 => 1000,
            128..=191 => 1500,
            192..=255 => 2000,
        }
    }
 
    pub fn should_preempt(&self) -> bool {
        let current_time = Instant::now();
        self.update_vruntime(current_time)
    }
}

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
    /// This reads the current time from the ARM Generic Timer and converts
    /// to nanoseconds for consistent time calculations.
    pub fn now() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            // Read ARM Generic Timer counter and frequency
            let cnt: u64;
            let freq: u64;
            unsafe {
                core::arch::asm!(
                    "mrs {}, cntpct_el0",
                    out(reg) cnt,
                    options(nostack, nomem, preserves_flags)
                );
                core::arch::asm!(
                    "mrs {}, cntfrq_el0",
                    out(reg) freq,
                    options(nostack, nomem, preserves_flags)
                );
            }
            // Convert ticks to nanoseconds: ns = ticks * 1_000_000_000 / freq
            // Use u128 to avoid overflow
            let nanos = if freq > 0 {
                ((cnt as u128 * 1_000_000_000) / freq as u128) as u64
            } else {
                0
            };
            Self(nanos)
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