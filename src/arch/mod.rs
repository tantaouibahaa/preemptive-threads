//! AArch64 CPU support: boot, MMU, exception vectors, context switching,
//! user mode (EL0), the generic timer and semihosting.
//!
//! Everything in here is specific to ARMv8-A running at EL1. Board-specific
//! code (interrupt controller, UART, secondary-core release) lives in
//! [`crate::platform`].

pub mod boot;
pub mod context;
pub mod mmu;
pub mod semihosting;
pub mod timer;
pub mod user;
pub mod vectors;

use core::arch::asm;

/// Saved interrupt mask state returned by [`irq_save`].
pub type IrqState = u64;

/// Index of the calling core (MPIDR_EL1.Aff0).
#[inline(always)]
pub fn cpu_id() -> usize {
    let mpidr: u64;
    // SAFETY: reading MPIDR_EL1 has no side effects.
    unsafe { asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags)) };
    (mpidr & 0xff) as usize
}

/// Mask IRQs on this core and return the previous DAIF value.
///
/// Acts as a compiler barrier: memory accesses are not moved across it.
#[inline(always)]
pub fn irq_save() -> IrqState {
    let daif: u64;
    // SAFETY: masking IRQs is always allowed at EL1.
    unsafe { asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif, options(nostack, preserves_flags)) };
    daif
}

/// Restore a DAIF value previously returned by [`irq_save`].
#[inline(always)]
pub fn irq_restore(state: IrqState) {
    // SAFETY: restores a value read from DAIF earlier on this core.
    unsafe { asm!("msr daif, {}", in(reg) state, options(nostack, preserves_flags)) };
}

/// Unmask IRQs on this core.
#[inline(always)]
pub fn irq_enable() {
    // SAFETY: unmasking IRQs is always allowed at EL1.
    unsafe { asm!("msr daifclr, #2", options(nostack, preserves_flags)) };
}

/// Mask IRQs on this core.
#[inline(always)]
pub fn irq_disable() {
    // SAFETY: masking IRQs is always allowed at EL1.
    unsafe { asm!("msr daifset, #2", options(nostack, preserves_flags)) };
}

/// Whether IRQs are currently unmasked on this core.
#[inline(always)]
pub fn irqs_enabled() -> bool {
    let daif: u64;
    // SAFETY: reading DAIF has no side effects.
    unsafe { asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags)) };
    daif & (1 << 7) == 0
}

/// Wait for interrupt.
#[inline(always)]
pub fn wfi() {
    // SAFETY: WFI only suspends the core until an interrupt is pending.
    unsafe { asm!("wfi", options(nomem, nostack, preserves_flags)) };
}

/// Wait for event.
#[inline(always)]
pub fn wfe() {
    // SAFETY: WFE only suspends the core until an event.
    unsafe { asm!("wfe", options(nomem, nostack, preserves_flags)) };
}

/// Send event to all cores.
#[inline(always)]
pub fn sev() {
    // SAFETY: SEV only signals an event.
    unsafe { asm!("dsb sy", "sev", options(nostack, preserves_flags)) };
}

/// Clean and invalidate the data cache line containing `addr` to the point
/// of coherency. Needed when handing data to a core whose MMU is still off.
#[inline(always)]
pub fn dcache_clean_inval(addr: usize) {
    // SAFETY: cache maintenance by VA is harmless on mapped addresses.
    unsafe { asm!("dc civac, {}", "dsb sy", in(reg) addr, options(nostack, preserves_flags)) };
}

/// Current exception level (0-3).
#[inline(always)]
pub fn current_el() -> u8 {
    let el: u64;
    // SAFETY: reading CurrentEL has no side effects.
    unsafe { asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags)) };
    ((el >> 2) & 3) as u8
}

/// Mask all exceptions and stop this core forever.
pub fn halt() -> ! {
    // SAFETY: masking exceptions at EL1 is always allowed.
    unsafe { asm!("msr daifset, #0xf", options(nomem, nostack, preserves_flags)) };
    loop {
        wfe();
    }
}
