//! Platform-specific timer implementations for preemptive scheduling

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static PREEMPTION_PENDING: AtomicBool = AtomicBool::new(false);
static PREEMPTION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Signal handler that just sets a flag - actual scheduling happens outside signal context
/// 
/// # Safety
/// This function is called from signal context and only uses async-signal-safe operations.
/// It only modifies atomic variables and performs no memory allocation or complex operations.
#[cfg(target_os = "linux")]
pub unsafe extern "C" fn signal_safe_handler(_sig: i32) {
    // Only use async-signal-safe operations here
    PREEMPTION_PENDING.store(true, Ordering::Release);
    PREEMPTION_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Check if preemption is pending (called from normal context)
pub fn is_preemption_pending() -> bool {
    PREEMPTION_PENDING.load(Ordering::Acquire)
}

/// Clear preemption pending flag
pub fn clear_preemption_pending() {
    PREEMPTION_PENDING.store(false, Ordering::Release);
}

/// Get total preemption count for statistics
pub fn get_preemption_count() -> u64 {
    PREEMPTION_COUNT.load(Ordering::Relaxed)
}

/// Platform-specific timer implementation for Linux using timerfd
#[cfg(target_os = "linux")]
pub mod linux_timer {
    
    pub fn init_preemption_timer(_interval_ms: u64) -> Result<(), &'static str> {
        // For a complete implementation, you would:
        // 1. Create a timerfd using timerfd_create()
        // 2. Set it up with timerfd_settime()
        // 3. Use signalfd() or signal handlers
        // 4. Or use a separate thread with epoll/poll
        
        // For now, return an error suggesting cooperative scheduling
        Err("Hardware timer preemption not implemented - use cooperative yield points")
    }
    
    pub fn stop_preemption_timer() {
        // Would close the timerfd and clean up
    }
}

/// Platform-specific timer implementation for macOS/BSD
#[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
pub mod bsd_timer {
    
    pub fn init_preemption_timer(_interval_ms: u64) -> Result<(), &'static str> {
        // For BSD systems, you would use kqueue with EVFILT_TIMER
        // or setitimer() if available
        Err("Hardware timer preemption not implemented for BSD - use cooperative yield points")
    }
    
    pub fn stop_preemption_timer() {
        // Would clean up kqueue timer
    }
}

/// Fallback implementation for other platforms
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd")))]
pub mod generic_timer {
    
    pub fn init_preemption_timer(_interval_ms: u64) -> Result<(), &'static str> {
        Err("Hardware timer preemption not supported on this platform")
    }
    
    pub fn stop_preemption_timer() {
        // No-op
    }
}

/// Initialize platform-appropriate preemption timer
pub fn init_preemption_timer(interval_ms: u64) -> Result<(), &'static str> {
    #[cfg(target_os = "linux")]
    return linux_timer::init_preemption_timer(interval_ms);
    
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
    return bsd_timer::init_preemption_timer(interval_ms);
    
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd")))]
    return generic_timer::init_preemption_timer(interval_ms);
}

/// Stop platform-appropriate preemption timer
pub fn stop_preemption_timer() {
    #[cfg(target_os = "linux")]
    linux_timer::stop_preemption_timer();
    
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd"))]
    bsd_timer::stop_preemption_timer();
    
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd", target_os = "netbsd", target_os = "openbsd")))]
    generic_timer::stop_preemption_timer();
}

/// Preemption checkpoint - should be called regularly from normal code
/// This is where actual scheduling decisions are made, outside signal context
pub fn preemption_checkpoint() {
    if is_preemption_pending() {
        clear_preemption_pending();

        // Safe to do complex operations here - we're not in signal context
        // Yield to scheduler
        crate::yield_now();
    }
}

/// Cooperative preemption points - insert these in long-running code
#[macro_export]
macro_rules! preemption_point {
    () => {
        $crate::platform_timer::preemption_checkpoint();
    };
}