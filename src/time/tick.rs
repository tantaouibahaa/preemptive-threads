//! Tick counting and time slice management.

use super::{Duration, Instant, DEFAULT_QUANTUM_NS};
use portable_atomic::{AtomicU64, AtomicU32, Ordering};

/// Global tick counter for system uptime and scheduling.
///
/// This counter is incremented on every timer interrupt and provides
/// a monotonic time source for scheduling decisions.
pub struct TickCounter {
    /// Number of ticks since system start
    ticks: AtomicU64,
    /// Tick frequency in Hz
    frequency: u32,
    /// Nanoseconds per tick
    ns_per_tick: u64,
}

impl TickCounter {
    /// Create a new tick counter with the given frequency.
    ///
    /// # Arguments
    ///
    /// * `frequency` - Timer frequency in Hz
    pub const fn new(frequency: u32) -> Self {
        Self {
            ticks: AtomicU64::new(0),
            frequency,
            ns_per_tick: 1_000_000_000 / frequency as u64,
        }
    }
    
    /// Increment the tick counter (called from timer interrupt).
    ///
    /// This should only be called from the timer interrupt handler.
    pub fn increment(&self) {
        self.ticks.fetch_add(1, Ordering::AcqRel);
    }
    
    /// Get the current tick count.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Acquire)
    }
    
    /// Get the tick frequency in Hz.
    pub fn frequency(&self) -> u32 {
        self.frequency
    }
    
    /// Convert ticks to nanoseconds.
    pub fn ticks_to_nanos(&self, ticks: u64) -> u64 {
        ticks * self.ns_per_tick
    }
    
    /// Convert nanoseconds to ticks.
    pub fn nanos_to_ticks(&self, nanos: u64) -> u64 {
        nanos / self.ns_per_tick
    }
    
    /// Get current time as an instant.
    pub fn now(&self) -> Instant {
        let ticks = self.ticks();
        Instant::from_nanos(self.ticks_to_nanos(ticks))
    }
}

/// Time slice tracking for thread scheduling.
///
/// This tracks how much time a thread has used in its current time slice
/// and determines when preemption should occur.
pub struct TimeSlice {
    /// Virtual runtime for this thread (in nanoseconds)
    vruntime: AtomicU64,
    /// Time when current slice started
    slice_start: AtomicU64,
    /// Duration of current time slice
    quantum: AtomicU64,
    /// Priority level (affects quantum size)
    priority: AtomicU32,
}

impl TimeSlice {
    /// Create a new time slice tracker.
    ///
    /// # Arguments
    ///
    /// * `priority` - Thread priority (0-255, higher = more important)
    pub fn new(priority: u8) -> Self {
        let quantum = Self::calculate_quantum(priority);
        Self {
            vruntime: AtomicU64::new(0),
            slice_start: AtomicU64::new(0),
            quantum: AtomicU64::new(quantum),
            priority: AtomicU32::new(priority as u32),
        }
    }
    
    /// Start a new time slice.
    ///
    /// # Arguments
    ///
    /// * `current_time` - Current system time
    pub fn start_slice(&self, current_time: Instant) {
        self.slice_start.store(current_time.as_nanos(), Ordering::Release);
    }
    
    /// Update virtual runtime based on actual runtime.
    ///
    /// # Arguments
    ///
    /// * `current_time` - Current system time
    ///
    /// # Returns
    ///
    /// `true` if the time slice has expired and preemption should occur.
    pub fn update_vruntime(&self, current_time: Instant) -> bool {
        let slice_start = self.slice_start.load(Ordering::Acquire);
        let quantum = self.quantum.load(Ordering::Acquire);
        let priority = self.priority.load(Ordering::Acquire);
        
        if slice_start == 0 {
            // Slice hasn't started yet
            return false;
        }
        
        let elapsed = current_time.as_nanos() - slice_start;
        
        // Calculate virtual time based on priority
        // Higher priority threads accumulate virtual time slower
        let priority_factor = Self::calculate_priority_factor(priority as u8);
        let virtual_elapsed = (elapsed * 1000) / priority_factor as u64;
        
        // Update virtual runtime
        self.vruntime.fetch_add(virtual_elapsed, Ordering::AcqRel);
        
        // Check if quantum expired
        elapsed >= quantum
    }
    
    /// Get current virtual runtime.
    pub fn vruntime(&self) -> u64 {
        self.vruntime.load(Ordering::Acquire)
    }
    
    /// Set priority and recalculate quantum.
    ///
    /// # Arguments
    ///
    /// * `new_priority` - New priority level (0-255)
    pub fn set_priority(&self, new_priority: u8) {
        self.priority.store(new_priority as u32, Ordering::Release);
        let new_quantum = Self::calculate_quantum(new_priority);
        self.quantum.store(new_quantum, Ordering::Release);
    }
    
    /// Set custom time slice duration.
    ///
    /// # Arguments
    ///
    /// * `duration` - Custom duration for time slices
    pub fn set_custom_duration(&self, duration: Duration) {
        self.quantum.store(duration.as_nanos(), Ordering::Release);
    }
    
    /// Get current priority.
    pub fn priority(&self) -> u8 {
        self.priority.load(Ordering::Acquire) as u8
    }
    
    /// Reset virtual runtime (used for priority inheritance).
    pub fn reset_vruntime(&self, new_vruntime: u64) {
        self.vruntime.store(new_vruntime, Ordering::Release);
    }
    
    /// Check if this time slice should be preempted.
    ///
    /// This is a convenience method that updates virtual runtime
    /// and returns whether preemption should occur.
    pub fn should_preempt(&self) -> bool {
        let current_time = super::Instant::now();
        self.update_vruntime(current_time)
    }
    
    /// Calculate quantum size based on priority.
    ///
    /// Higher priority threads get larger quanta to reduce context switching overhead.
    fn calculate_quantum(priority: u8) -> u64 {
        let base_quantum = DEFAULT_QUANTUM_NS;
        match priority {
            0..=63 => base_quantum / 2,      // Low priority: 0.5ms
            64..=127 => base_quantum,        // Normal priority: 1ms  
            128..=191 => base_quantum * 2,   // High priority: 2ms
            192..=255 => base_quantum * 4,   // Very high priority: 4ms
        }
    }
    
    /// Calculate priority factor for virtual time calculation.
    ///
    /// This determines how fast virtual time accumulates relative to real time.
    fn calculate_priority_factor(priority: u8) -> u32 {
        match priority {
            0..=63 => 500,      // Low priority runs slower in virtual time
            64..=127 => 1000,   // Normal priority: 1:1 virtual to real time
            128..=191 => 1500,  // High priority runs faster in virtual time
            192..=255 => 2000,  // Very high priority runs much faster
        }
    }
}

/// Global tick counter instance.
pub static GLOBAL_TICK_COUNTER: TickCounter = TickCounter::new(super::TIMER_FREQUENCY_HZ);

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_tick_counter() {
        let counter = TickCounter::new(1000); // 1 kHz
        assert_eq!(counter.ticks(), 0);
        assert_eq!(counter.frequency(), 1000);
        
        counter.increment();
        assert_eq!(counter.ticks(), 1);
        
        assert_eq!(counter.ticks_to_nanos(1000), 1_000_000_000); // 1 second
        assert_eq!(counter.nanos_to_ticks(1_000_000_000), 1000);
    }
    
    #[test]
    fn test_time_slice() {
        let slice = TimeSlice::new(100); // Normal priority (64-127 range gets base quantum)
        assert_eq!(slice.priority(), 100);
        assert_eq!(slice.vruntime(), 0);

        let start_time = Instant::from_nanos(1000000);
        slice.start_slice(start_time);

        // Time slice shouldn't expire immediately
        assert!(!slice.update_vruntime(start_time));

        // After quantum duration, it should expire (base quantum for priority 100)
        let end_time = Instant::from_nanos(start_time.as_nanos() + DEFAULT_QUANTUM_NS + 1);
        assert!(slice.update_vruntime(end_time));
    }
    
    #[test]
    fn test_priority_quantum_calculation() {
        let low_prio = TimeSlice::new(32);
        let normal_prio = TimeSlice::new(128);
        let high_prio = TimeSlice::new(200);
        
        // Higher priority should get larger quantum
        assert!(high_prio.quantum.load(Ordering::Acquire) > 
                normal_prio.quantum.load(Ordering::Acquire));
        assert!(normal_prio.quantum.load(Ordering::Acquire) > 
                low_prio.quantum.load(Ordering::Acquire));
    }
}