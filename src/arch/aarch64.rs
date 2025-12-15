//! AArch64 (ARM64) architecture implementation.
//!
//! This module provides ARM64-specific context switching, interrupt handling,
//! FPU/NEON management, and SVE support for high-performance computing.

use super::Arch;
use core::arch::asm;
use portable_atomic::{AtomicU64, AtomicPtr, Ordering};
use core::ptr::null_mut;

/// Global context pointer for IRQ save (where to save interrupted thread's context).
/// This is set by the kernel before enabling interrupts.
pub static IRQ_SAVE_CTX: AtomicPtr<Aarch64Context> = AtomicPtr::new(null_mut());

/// Global context pointer for IRQ load (where to load next thread's context from).
/// This is updated by the scheduler when a context switch is needed.
pub static IRQ_LOAD_CTX: AtomicPtr<Aarch64Context> = AtomicPtr::new(null_mut());

/// Dedicated IRQ stack (4KB should be plenty for IRQ handling)
/// This prevents us from using/corrupting the interrupted thread's stack.
#[repr(C, align(16))]
pub struct IrqStack {
    data: [u8; 4096],
}

/// The IRQ stack instance
#[no_mangle]
pub static mut IRQ_STACK: IrqStack = IrqStack { data: [0; 4096] };

/// Get the top of the IRQ stack (stack grows down)
#[inline]
pub fn irq_stack_top() -> *mut u8 {
    // Use raw pointer to avoid mutable reference to static
    unsafe {
        let ptr = core::ptr::addr_of_mut!(IRQ_STACK);
        (*ptr).data.as_mut_ptr().add(4096)
    }
}

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
        // Use x16 (IP0) and x17 (IP1) as our base pointers.
        // These are the intra-procedure-call scratch registers, perfect for trampolines.
        unsafe {
            asm!(
                // Move input pointers to IP0/IP1
                "mov x16, {prev}",            // x16 = prev context pointer
                "mov x17, {next}",            // x17 = next context pointer

                // === SAVE CURRENT CONTEXT to x16 ===
                // Save SP and return address first
                "mov x15, sp",                // use x15 as temp
                "str x15, [x16, #248]",       // save sp
                "adr x15, 1f",                // return address
                "str x15, [x16, #256]",       // save pc
                "mrs x15, nzcv",
                "str x15, [x16, #264]",       // save pstate

                // Save all general-purpose registers x0-x30
                "stp x0, x1, [x16, #0]",
                "stp x2, x3, [x16, #16]",
                "stp x4, x5, [x16, #32]",
                "stp x6, x7, [x16, #48]",
                "stp x8, x9, [x16, #64]",
                "stp x10, x11, [x16, #80]",
                "stp x12, x13, [x16, #96]",
                "stp x14, x15, [x16, #112]",
                "stp x16, x17, [x16, #128]",  // save original x16, x17 (clobbered values OK)
                "stp x18, x19, [x16, #144]",
                "stp x20, x21, [x16, #160]",
                "stp x22, x23, [x16, #176]",
                "stp x24, x25, [x16, #192]",
                "stp x26, x27, [x16, #208]",
                "stp x28, x29, [x16, #224]",
                "str x30, [x16, #240]",

                // === LOAD NEW CONTEXT from x17 ===
                // First restore SP and PSTATE
                "ldr x15, [x17, #248]",       // load new sp
                "mov sp, x15",
                "ldr x15, [x17, #264]",       // load new pstate
                "msr nzcv, x15",
                "ldr x30, [x17, #256]",       // load new pc into link register

                // Load all GP registers x0-x15, x18-x29
                // Skip x16, x17 - we need x17 as base
                "ldp x0, x1, [x17, #0]",
                "ldp x2, x3, [x17, #16]",
                "ldp x4, x5, [x17, #32]",
                "ldp x6, x7, [x17, #48]",
                "ldp x8, x9, [x17, #64]",
                "ldp x10, x11, [x17, #80]",
                "ldp x12, x13, [x17, #96]",
                "ldp x14, x15, [x17, #112]",
                // Skip x16, x17 for now
                "ldp x18, x19, [x17, #144]",
                "ldp x20, x21, [x17, #160]",
                "ldp x22, x23, [x17, #176]",
                "ldp x24, x25, [x17, #192]",
                "ldp x26, x27, [x17, #208]",
                "ldp x28, x29, [x17, #224]",

                // Load x16, then x17 last (x17 is our base pointer)
                "ldr x16, [x17, #128]",       // load x16
                "ldr x17, [x17, #136]",       // load x17 (finally done with base)

                // Jump to new context via link register
                "ret",

                "1:",                         // return point when switched back
                prev = in(reg) prev,
                next = in(reg) next,
                // x15-x17 are clobbered
                out("x15") _,
                out("x16") _,
                out("x17") _,
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
///
/// # Safety
///
/// Must be called from privileged mode (EL1). Modifies system timer registers.
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
/// It performs the scheduler decision and updates IRQ_LOAD_CTX if a context
/// switch is needed. The actual context switch happens when the IRQ handler
/// loads from IRQ_LOAD_CTX before returning.
///
/// # Safety
///
/// Must only be called from the IRQ exception handler in privileged mode.
/// IRQ_SAVE_CTX must have been set to the current thread's context.
pub unsafe fn timer_interrupt_handler() {
    unsafe {
        // Disable timer temporarily
        asm!(
            "msr cntp_ctl_el0, {val}",
            val = in(reg) 2u64, // Disable (bit 0 = 0) and mask (bit 1 = 1)
            options(nomem, nostack)
        );

        // Get global kernel reference for scheduler decision
        use crate::arch::DefaultArch;
        use crate::sched::RoundRobinScheduler;
        use crate::kernel::get_global_kernel;

        if let Some(kernel) = get_global_kernel::<DefaultArch, RoundRobinScheduler>() {
            // Handle preemption via IRQ context switching
            kernel.handle_irq_preemption();
        }

        // Re-setup timer for next preemption (1ms default)
        let _ = setup_preemption_timer(1000);
    }
}

/// Set up the IRQ context pointers for a thread that's about to run.
///
/// This must be called before enabling interrupts so that when an IRQ occurs,
/// the handler knows where to save the interrupted thread's context.
///
/// # Safety
///
/// The context pointer must remain valid as long as the thread could be interrupted.
pub unsafe fn set_current_irq_context(ctx: *mut Aarch64Context) {
    IRQ_SAVE_CTX.store(ctx, Ordering::Release);
    IRQ_LOAD_CTX.store(ctx, Ordering::Release);  // Default: return to same thread
}

/// Update the load context pointer for IRQ return.
///
/// Call this from the scheduler when switching to a different thread.
/// The IRQ handler will load from this context when returning.
pub fn set_irq_load_context(ctx: *mut Aarch64Context) {
    IRQ_LOAD_CTX.store(ctx, Ordering::Release);
}

/// Get the current IRQ save context pointer.
pub fn get_irq_save_context() -> *mut Aarch64Context {
    IRQ_SAVE_CTX.load(Ordering::Acquire)
}

/// Get the current IRQ load context pointer.
pub fn get_irq_load_context() -> *mut Aarch64Context {
    IRQ_LOAD_CTX.load(Ordering::Acquire)
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
///
/// # Safety
///
/// The memory range `[start, start + len)` must be valid and properly aligned.
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
///
/// # Safety
///
/// Must be called from privileged mode. Affects all instruction caches.
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