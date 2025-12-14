//! Stub implementation of AArch64 context switching for non-ARM64 targets.
//!
//! This module provides type-compatible stubs for testing on non-ARM64 hosts
//! (e.g., x86_64 macOS/Linux). No actual context switching occurs.

use super::Arch;

/// Saved thread context for AArch64 (stub version).
#[repr(C)]
pub struct Aarch64Context {
    /// General-purpose registers x0-x30
    pub x: [u64; 31],
    /// Stack pointer
    pub sp: u64,
    /// Program counter
    pub pc: u64,
    /// Processor state register
    pub pstate: u64,

    /// NEON/FPU state (when full-fpu feature is enabled)
    #[cfg(feature = "full-fpu")]
    pub neon_state: [u128; 32],
    #[cfg(feature = "full-fpu")]
    pub fpcr: u32,
    #[cfg(feature = "full-fpu")]
    pub fpsr: u32,
}

impl Default for Aarch64Context {
    fn default() -> Self {
        Self {
            x: [0; 31],
            sp: 0,
            pc: 0,
            pstate: 0x3c5,
            #[cfg(feature = "full-fpu")]
            neon_state: [0; 32],
            #[cfg(feature = "full-fpu")]
            fpcr: 0,
            #[cfg(feature = "full-fpu")]
            fpsr: 0,
        }
    }
}

unsafe impl Send for Aarch64Context {}
unsafe impl Sync for Aarch64Context {}

/// Stub alias for SavedContext compatibility.
pub type SavedContext = Aarch64Context;

/// AArch64 architecture implementation (stub for testing).
pub struct Aarch64Arch;

impl Arch for Aarch64Arch {
    type SavedContext = Aarch64Context;

    unsafe fn context_switch(_prev: *mut Self::SavedContext, _next: *const Self::SavedContext) {
        // Stub - no actual context switch on non-ARM64
    }

    #[cfg(feature = "full-fpu")]
    unsafe fn save_fpu(_ctx: &mut Self::SavedContext) {
        // Stub
    }

    #[cfg(feature = "full-fpu")]
    unsafe fn restore_fpu(_ctx: &Self::SavedContext) {
        // Stub
    }

    fn enable_interrupts() {
        // Stub
    }

    fn disable_interrupts() {
        // Stub
    }

    fn interrupts_enabled() -> bool {
        true
    }
}

/// Setup preemption timer (stub).
pub unsafe fn setup_preemption_timer(_interval_us: u64) -> Result<(), &'static str> {
    Ok(())
}

/// Timer interrupt handler (stub).
pub unsafe fn timer_interrupt_handler() {
    // Stub
}
