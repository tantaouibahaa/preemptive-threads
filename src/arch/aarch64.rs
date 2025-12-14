//! AArch64 (ARM64) architecture implementation.
//!
//! This module provides ARM64-specific context switching, interrupt handling,
//! FPU/NEON management, and SVE support for high-performance computing.

use super::Arch;
use core::arch::asm;
use portable_atomic::{AtomicU64, Ordering};

/// AArch64 architecture implementation.
pub struct Aarch64Arch;

/// AArch64 saved context structure.
///
/// Contains all general-purpose registers, stack pointer, and NEON/FPU state
/// needed to save and restore thread execution state.
#[repr(C)]
#[derive(Debug)]
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
    pub neon_state: [u128; 32], // v0-v31 NEON registers
    #[cfg(feature = "full-fpu")]
    pub fpcr: u32, // Floating-point control register
    #[cfg(feature = "full-fpu")]
    pub fpsr: u32, // Floating-point status register
}

impl Default for Aarch64Context {
    fn default() -> Self {
        Self {
            x: [0; 31],
            sp: 0,
            pc: 0,
            pstate: 0x3c5, // Default PSTATE (EL0, interrupts enabled)
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

/// Type alias for compatibility with other modules.
pub type SavedContext = Aarch64Context;

impl Arch for Aarch64Arch {
    type SavedContext = Aarch64Context;

    unsafe fn context_switch(prev: *mut Self::SavedContext, next: *const Self::SavedContext) {
        unsafe {
            asm!(
                // Save current context
                "stp x0, x1, [{prev}, #0]",
                "stp x2, x3, [{prev}, #16]", 
                "stp x4, x5, [{prev}, #32]",
                "stp x6, x7, [{prev}, #48]",
                "stp x8, x9, [{prev}, #64]",
                "stp x10, x11, [{prev}, #80]",
                "stp x12, x13, [{prev}, #96]",
                "stp x14, x15, [{prev}, #112]",
                "stp x16, x17, [{prev}, #128]",
                "stp x18, x19, [{prev}, #144]",
                "stp x20, x21, [{prev}, #160]",
                "stp x22, x23, [{prev}, #176]",
                "stp x24, x25, [{prev}, #192]",
                "stp x26, x27, [{prev}, #208]",
                "stp x28, x29, [{prev}, #224]",
                "str x30, [{prev}, #240]",
                
                // Save stack pointer and link register (return address)
                "mov x0, sp",
                "str x0, [{prev}, #248]",    // sp offset
                "adr x0, 1f",                // get return address
                "str x0, [{prev}, #256]",    // pc offset
                
                // Save processor state
                "mrs x0, nzcv",
                "str x0, [{prev}, #264]",    // pstate offset
                
                // Load new context
                "ldp x0, x1, [{next}, #0]",
                "ldp x2, x3, [{next}, #16]",
                "ldp x4, x5, [{next}, #32]",
                "ldp x6, x7, [{next}, #48]",
                "ldp x8, x9, [{next}, #64]",
                "ldp x10, x11, [{next}, #80]",
                "ldp x12, x13, [{next}, #96]",
                "ldp x14, x15, [{next}, #112]",
                "ldp x16, x17, [{next}, #128]",
                "ldp x18, x19, [{next}, #144]",
                "ldp x20, x21, [{next}, #160]",
                "ldp x22, x23, [{next}, #176]",
                "ldp x24, x25, [{next}, #192]",
                "ldp x26, x27, [{next}, #208]",
                "ldp x28, x29, [{next}, #224]",
                "ldr x30, [{next}, #240]",
                
                // Restore stack pointer  
                "ldr x0, [{next}, #248]",
                "mov sp, x0",
                
                // Restore processor state
                "ldr x0, [{next}, #264]",
                "msr nzcv, x0",
                
                // Jump to new context
                "ldr x0, [{next}, #256]",    // load pc
                "br x0",                     // branch to new context
                
                "1:",                        // return label for save
                prev = in(reg) prev,
                next = in(reg) next,
                clobber_abi("C")
            );
        }
    }

    #[cfg(feature = "full-fpu")]
    unsafe fn save_fpu(ctx: &mut Self::SavedContext) {
        unsafe {
            asm!(
                // Save NEON/FPU registers v0-v31
                "stp q0, q1, [{ctx}, #272]",      // neon_state offset
                "stp q2, q3, [{ctx}, #304]",
                "stp q4, q5, [{ctx}, #336]",
                "stp q6, q7, [{ctx}, #368]",
                "stp q8, q9, [{ctx}, #400]",
                "stp q10, q11, [{ctx}, #432]",
                "stp q12, q13, [{ctx}, #464]",
                "stp q14, q15, [{ctx}, #496]",
                "stp q16, q17, [{ctx}, #528]",
                "stp q18, q19, [{ctx}, #560]",
                "stp q20, q21, [{ctx}, #592]",
                "stp q22, q23, [{ctx}, #624]",
                "stp q24, q25, [{ctx}, #656]",
                "stp q26, q27, [{ctx}, #688]",
                "stp q28, q29, [{ctx}, #720]",
                "stp q30, q31, [{ctx}, #752]",
                
                // Save FPCR and FPSR
                "mrs x0, fpcr",
                "str w0, [{ctx}, #784]",          // fpcr offset  
                "mrs x0, fpsr", 
                "str w0, [{ctx}, #788]",          // fpsr offset
                ctx = in(reg) ctx,
                lateout("x0") _,
                options(nostack)
            );
        }
    }

    #[cfg(feature = "full-fpu")]
    unsafe fn restore_fpu(ctx: &Self::SavedContext) {
        unsafe {
            asm!(
                // Restore FPCR and FPSR first
                "ldr w0, [{ctx}, #784]",          // fpcr offset
                "msr fpcr, x0",
                "ldr w0, [{ctx}, #788]",          // fpsr offset  
                "msr fpsr, x0",
                
                // Restore NEON/FPU registers v0-v31
                "ldp q0, q1, [{ctx}, #272]",      // neon_state offset
                "ldp q2, q3, [{ctx}, #304]",
                "ldp q4, q5, [{ctx}, #336]",
                "ldp q6, q7, [{ctx}, #368]",
                "ldp q8, q9, [{ctx}, #400]",
                "ldp q10, q11, [{ctx}, #432]",
                "ldp q12, q13, [{ctx}, #464]",
                "ldp q14, q15, [{ctx}, #496]",
                "ldp q16, q17, [{ctx}, #528]",
                "ldp q18, q19, [{ctx}, #560]",
                "ldp q20, q21, [{ctx}, #592]",
                "ldp q22, q23, [{ctx}, #624]",
                "ldp q24, q25, [{ctx}, #656]",
                "ldp q26, q27, [{ctx}, #688]",
                "ldp q28, q29, [{ctx}, #720]",
                "ldp q30, q31, [{ctx}, #752]",
                ctx = in(reg) ctx,
                lateout("x0") _,
                options(nostack)
            );
        }
    }

    fn enable_interrupts() {
        unsafe {
            asm!(
                "msr daifclr, #2",  // Clear IRQ mask (bit 1 of DAIF)
                options(nomem, nostack)
            );
        }
    }

    fn disable_interrupts() {
        unsafe {
            asm!(
                "msr daifset, #2",  // Set IRQ mask (bit 1 of DAIF)
                options(nomem, nostack)
            );
        }
    }

    fn interrupts_enabled() -> bool {
        let daif: u64;
        unsafe {
            asm!(
                "mrs {daif}, daif",
                daif = out(reg) daif,
                options(nostack, readonly)
            );
        }
        (daif & 0x80) == 0  // IRQ bit (bit 7) is clear when interrupts enabled
    }
}

// Timer frequency storage  
static TIMER_FREQ: AtomicU64 = AtomicU64::new(0);

/// Initialize AArch64-specific features.
pub fn init() {
    unsafe {
        // Read the timer frequency from CNTFRQ_EL0
        let freq: u64;
        asm!(
            "mrs {freq}, cntfrq_el0",
            freq = out(reg) freq,
            options(nostack, readonly)
        );
        TIMER_FREQ.store(freq, Ordering::Relaxed);
        
        // Enable the generic timer (CNTP_CTL_EL0)
        asm!(
            "msr cntp_ctl_el0, {val}",
            val = in(reg) 1u64, // Enable timer (bit 0 = 1)
            options(nomem, nostack)
        );
    }
}

/// Set up ARM64 timer for preemption with specified interval in microseconds.
pub unsafe fn setup_preemption_timer(interval_us: u32) -> Result<(), &'static str> {
    let freq = TIMER_FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        return Err("Timer frequency not initialized");
    }
    
    // Calculate ticks for the desired interval
    let ticks = (freq * interval_us as u64) / 1_000_000;
    
    unsafe {
        // Read current timer value
        let current: u64;
        asm!(
            "mrs {current}, cntpct_el0",
            current = out(reg) current,
            options(nostack, readonly)
        );
        
        // Set compare value (current + interval)
        let compare_val = current + ticks;
        asm!(
            "msr cntp_cval_el0, {val}",
            val = in(reg) compare_val,
            options(nomem, nostack)
        );
        
        // Enable timer interrupt
        asm!(
            "msr cntp_ctl_el0, {val}",
            val = in(reg) 1u64, // Enable (bit 0) and unmask (bit 1 = 0)
            options(nomem, nostack)
        );
    }
    
    Ok(())
}

/// Get current ARM64 timestamp counter value.
pub fn get_timestamp() -> u64 {
    let count: u64;
    unsafe {
        asm!(
            "mrs {count}, cntpct_el0",
            count = out(reg) count,
            options(nostack, readonly)
        );
    }
    count
}

/// Convert ARM64 timer ticks to nanoseconds.
pub fn ticks_to_ns(ticks: u64) -> u64 {
    let freq = TIMER_FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        return 0;
    }
    (ticks * 1_000_000_000) / freq
}

/// Convert nanoseconds to ARM64 timer ticks.
pub fn ns_to_ticks(ns: u64) -> u64 {
    let freq = TIMER_FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        return 0;
    }
    (ns * freq) / 1_000_000_000
}

/// AArch64-specific timer interrupt handler.
///
/// This is called from the IRQ vector when the timer fires.
/// It acknowledges the interrupt, triggers a context switch if needed,
/// and re-arms the timer.
pub unsafe fn timer_interrupt_handler() {
    // Clear the timer interrupt by disabling and re-enabling
    unsafe {
        // Disable timer
        asm!(
            "msr cntp_ctl_el0, {val}",
            val = in(reg) 2u64, // Disable (bit 0 = 0) and mask (bit 1 = 1)
            options(nomem, nostack)
        );

        // Get global kernel reference and handle preemption
        // The kernel is registered via Kernel::register_global() at boot
        use crate::arch::DefaultArch;
        use crate::sched::RoundRobinScheduler;
        use crate::kernel::get_global_kernel;

        if let Some(kernel) = get_global_kernel::<DefaultArch, RoundRobinScheduler>() {
            kernel.handle_timer_interrupt();
        }

        // Re-setup timer for next preemption (1ms default)
        if setup_preemption_timer(1000).is_err() {
            // Timer setup failed, disable preemption
            return;
        }
    }
}

/// Memory barrier operations for ARM64.
pub fn memory_barrier_full() {
    unsafe {
        asm!("dsb sy", options(nomem, nostack));
    }
}

pub fn memory_barrier_acquire() {
    unsafe {
        asm!("dsb ld", options(nomem, nostack));
    }
}

pub fn memory_barrier_release() {
    unsafe {
        asm!("dsb st", options(nomem, nostack));
    }
}

/// CPU cache maintenance for ARM64.
pub unsafe fn flush_dcache_range(start: *const u8, len: usize) {
    unsafe {
        let end = start.add(len);
        let mut addr = start as usize & !63; // Align to cache line (64 bytes)
        
        while addr < end as usize {
            asm!(
                "dc civac, {addr}",
                addr = in(reg) addr,
                options(nostack)
            );
            addr += 64; // Next cache line
        }
        
        // Data synchronization barrier
        asm!("dsb sy", options(nostack));
    }
}

/// Invalidate instruction cache for ARM64.
pub unsafe fn flush_icache() {
    unsafe {
        asm!(
            "ic ialluis",  // Invalidate all instruction caches to PoU, Inner Shareable
            "dsb ish",     // Data synchronization barrier
            "isb",         // Instruction synchronization barrier
            options(nomem, nostack)
        );
    }
}