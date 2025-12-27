//! AArch64 (ARM64) architecture implementation.
//!
//! This module provides ARM64-specific context switching, interrupt handling,
//! FPU/NEON management, and SVE support for high-performance computing.

use super::Arch;
use core::arch::asm;
use portable_atomic::{AtomicU64, AtomicPtr, Ordering};
use core::ptr::null_mut;

pub static IRQ_SAVE_CTX: AtomicPtr<Aarch64Context> = AtomicPtr::new(null_mut());


pub static IRQ_LOAD_CTX: AtomicPtr<Aarch64Context> = AtomicPtr::new(null_mut());


#[repr(C, align(16))]
pub struct IrqStack {
    data: [u8; 4096],
}

#[no_mangle]
pub static mut IRQ_STACK: IrqStack = IrqStack { data: [0; 4096] };

#[inline]
pub fn irq_stack_top() -> *mut u8 {
    unsafe {
        let ptr = core::ptr::addr_of_mut!(IRQ_STACK);
        (*ptr).data.as_mut_ptr().add(4096)
    }
}

pub struct Aarch64Arch;

#[repr(C)]
#[derive(Debug)]
pub struct Aarch64Context {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,

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

pub type SavedContext = Aarch64Context;

impl Arch for Aarch64Arch {
    type SavedContext = Aarch64Context;

    unsafe fn context_switch(prev: *mut Self::SavedContext, next: *const Self::SavedContext) {

        unsafe {
            asm!(
                "mov x11, {prev}",
                "mov x9, x11",
                "mov x10, x12",

                "mov x11, sp",
                "str x11, [x9, #248]",
                "adr x11, 1f",
                "str x11, [x9, #256]",
                "mrs x11, nzcv",
                "str x11, [x9, #264]",

                "stp x0, x1, [x9, #0]",
                "stp x2, x3, [x9, #16]",
                "stp x4, x5, [x9, #32]",
                "stp x6, x7, [x9, #48]",
                "stp x8, x9, [x9, #64]",
                "stp x10, x11, [x9, #80]",
                "stp x12, x13, [x9, #96]",
                "stp x14, x15, [x9, #112]",
                "stp x16, x17, [x9, #128]",
                "stp x18, x19, [x9, #144]",
                "stp x20, x21, [x9, #160]",
                "stp x22, x23, [x9, #176]",
                "stp x24, x25, [x9, #192]",
                "stp x26, x27, [x9, #208]",
                "stp x28, x29, [x9, #224]",
                "str x30, [x9, #240]",

                "ldr x11, [x10, #248]",
                "mov sp, x11",
                "ldr x11, [x10, #264]",
                "msr nzcv, x11",


                "ldp x0, x1, [x10, #0]",
                "ldp x2, x3, [x10, #16]",
                "ldp x4, x5, [x10, #32]",
                "ldp x6, x7, [x10, #48]",
                "ldr x8, [x10, #64]",
                "ldp x13, x14, [x10, #104]",
                "ldr x15, [x10, #120]",
                "ldp x16, x17, [x10, #128]",
                "ldp x18, x19, [x10, #144]",
                "ldp x20, x21, [x10, #160]",
                "ldp x22, x23, [x10, #176]",
                "ldp x24, x25, [x10, #192]",
                "ldp x26, x27, [x10, #208]",
                "ldp x28, x29, [x10, #224]",
                "ldr x30, [x10, #240]",

                "ldr x11, [x10, #256]",
                "ldr x9, [x10, #72]",
                "ldr x12, [x10, #96]",
                "ldr x10, [x10, #80]",

                "br x11",

                "1:",
                prev = in(reg) prev,
                out("x9") _,
                out("x10") _,
                out("x11") _,
                out("x12") _,
            );
        }
    }

    #[cfg(feature = "full-fpu")]
    unsafe fn save_fpu(ctx: &mut Self::SavedContext) {
        unsafe {
            asm!(
                "stp q0, q1, [{ctx}, #272]",
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

                "mrs x0, fpcr",
                "str w0, [{ctx}, #784]",
                "mrs x0, fpsr",
                "str w0, [{ctx}, #788]",
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
                "ldr w0, [{ctx}, #784]",
                "msr fpcr, x0",
                "ldr w0, [{ctx}, #788]",
                "msr fpsr, x0",

                "ldp q0, q1, [{ctx}, #272]",
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
                "msr daifclr, #2",
                options(nomem, nostack)
            );
        }
    }

    fn disable_interrupts() {
        unsafe {
            asm!(
                "msr daifset, #2",
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
        (daif & 0x80) == 0
    }
}

static TIMER_FREQ: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    unsafe {
        let freq: u64;
        asm!(
            "mrs {freq}, cntfrq_el0",
            freq = out(reg) freq,
            options(nostack, readonly)
        );
        TIMER_FREQ.store(freq, Ordering::Relaxed);

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

    let ticks = (freq * interval_us as u64) / 1_000_000;

    unsafe {
        let current: u64;
        asm!(
            "mrs {current}, cntpct_el0",
            current = out(reg) current,
            options(nostack, readonly)
        );

        let compare_val = current + ticks;
        asm!(
            "msr cntp_cval_el0, {val}",
            val = in(reg) compare_val,
            options(nomem, nostack)
        );

        asm!(
            "msr cntp_ctl_el0, {val}",
            val = in(reg) 1u64, // Enable (bit 0) and unmask (bit 1 = 0)
            options(nomem, nostack)
        );
    }

    Ok(())
}

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

pub fn ticks_to_ns(ticks: u64) -> u64 {
    let freq = TIMER_FREQ.load(Ordering::Relaxed);
    if freq == 0 {
        return 0;
    }
    (ticks * 1_000_000_000) / freq
}

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
        asm!(
            "msr cntp_ctl_el0, {val}",
            val = in(reg) 2u64,
            options(nomem, nostack)
        );

        use crate::arch::DefaultArch;
        use crate::sched::RoundRobinScheduler;
        use crate::kernel::get_global_kernel;

        if let Some(kernel) = get_global_kernel::<DefaultArch, RoundRobinScheduler>() {
            // Handle preemption via IRQ context switching
            kernel.handle_irq_preemption();
        }

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
    IRQ_LOAD_CTX.store(ctx, Ordering::Release);
}

/// Update the load context pointer for IRQ return.
///
/// Call this from the scheduler when switching to a different thread.
/// The IRQ handler will load from this context when returning.
pub fn set_irq_load_context(ctx: *mut Aarch64Context) {
    IRQ_LOAD_CTX.store(ctx, Ordering::Release);
}

pub fn get_irq_save_context() -> *mut Aarch64Context {
    IRQ_SAVE_CTX.load(Ordering::Acquire)
}

pub fn get_irq_load_context() -> *mut Aarch64Context {
    IRQ_LOAD_CTX.load(Ordering::Acquire)
}

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
        let mut addr = start as usize & !63;

        while addr < end as usize {
            asm!(
                "dc civac, {addr}",
                addr = in(reg) addr,
                options(nostack)
            );
            addr += 64;
        }

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
            "ic ialluis",
            "dsb ish",
            "isb",
            options(nomem, nostack)
        );
    }
}
