#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unreachable_pub)]

//! Bare-metal preemptive multithreading for Raspberry Pi Zero 2 W.
//!
//! This library provides preemptive multithreading for the Raspberry Pi Zero 2 W
//! (ARM Cortex-A53, AArch64) running in bare-metal mode without an operating system.
//!
//! # Target Platform
//!
//! - **Hardware**: Raspberry Pi Zero 2 W only
//! - **SoC**: Broadcom BCM2837 (ARM Cortex-A53)
//! - **Architecture**: AArch64 (ARM 64-bit)
//! - **Environment**: Bare-metal (no operating system)
//!
//! # Features
//!
//! - `full-fpu`: Enable NEON/FPU save/restore (default)
//! - `std-shim`: Enable compatibility layer for testing on host
//!
//! # Quick Start
//!
//! ```ignore
//! use preemptive_threads::{Kernel, RoundRobinScheduler};
//! use spin::Lazy;
//!
//! static KERNEL: Lazy<Kernel<_, RoundRobinScheduler>> =
//!     Lazy::new(|| Kernel::new(RoundRobinScheduler::new(1)));
//!
//! fn kernel_main() {
//!     KERNEL.init().expect("Failed to initialize kernel");
//!
//!     KERNEL.spawn(|| {
//!         loop { /* thread work */ }
//!     }, 128).expect("Failed to spawn thread");
//!
//!     KERNEL.start_first_thread();
//! }
//! ```
//!
//! # Architecture
//!
//! The library is organized around several key abstractions:
//! - ARM64 context switching with full register save/restore
//! - GIC-400 interrupt controller for timer interrupts
//! - Round-robin scheduler with priority support
//! - Safe memory management for thread stacks

// Core modules
pub mod arch;
pub mod errors;
pub mod kernel;
pub mod mem;
pub mod platform_timer;
pub mod sched;
pub mod thread;
pub mod time;

#[cfg(test)]
extern crate std;

extern crate alloc;

// Panic handler for bare-metal
#[cfg(all(not(test), not(feature = "std-shim")))]
use core::panic::PanicInfo;

#[cfg(all(not(test), not(feature = "std-shim")))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // On panic, disable interrupts and halt
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
    }
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack));
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

// Architecture abstraction
pub use arch::{Arch, DefaultArch};

// Kernel
pub use kernel::Kernel;

// Scheduler
pub use sched::{RoundRobinScheduler, Scheduler};

// Threads
pub use thread::{JoinHandle, Thread, ThreadBuilder, ThreadId, ThreadState};

// Memory management
pub use mem::{Stack, StackPool, StackSizeClass};

// Time
pub use time::{Duration, Instant};

// Errors
pub use errors::{ThreadError, ThreadResult, SpawnError};

// ============================================================================
// Convenience Functions
// ============================================================================

/// Yield the current thread's time slice to the scheduler.
///
/// This is a cooperative yield - the thread voluntarily gives up the CPU
/// to allow other threads to run. The current thread remains runnable
/// and will be scheduled again later.
#[inline]
pub fn yield_now() {
    kernel::yield_current();
}
