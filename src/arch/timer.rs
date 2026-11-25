//! ARM generic timer (EL1 physical timer, CNTP_*).
//!
//! Each core programs its own comparator; the interrupt is PPI 30 on the GIC
//! and `CNTPNSIRQ` on the BCM2836 local interrupt controller.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};

static FREQ: AtomicU64 = AtomicU64::new(0);

/// Counter frequency in Hz (CNTFRQ_EL0).
#[inline]
pub fn frequency() -> u64 {
    let f = FREQ.load(Ordering::Relaxed);
    if f != 0 {
        return f;
    }
    let f: u64;
    // SAFETY: reading CNTFRQ_EL0 has no side effects.
    unsafe { asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack, preserves_flags)) };
    FREQ.store(f, Ordering::Relaxed);
    f
}

/// Current value of the physical counter.
#[inline]
pub fn counter() -> u64 {
    let c: u64;
    // SAFETY: the ISB keeps the read from being speculated early.
    unsafe { asm!("isb", "mrs {}, cntpct_el0", out(reg) c, options(nomem, nostack, preserves_flags)) };
    c
}

/// Convert nanoseconds to counter ticks (rounding up).
#[inline]
pub fn ns_to_ticks(ns: u64) -> u64 {
    ((ns as u128 * frequency() as u128).div_ceil(1_000_000_000)) as u64
}

/// Convert counter ticks to nanoseconds.
#[inline]
pub fn ticks_to_ns(ticks: u64) -> u64 {
    (ticks as u128 * 1_000_000_000 / frequency() as u128) as u64
}

/// Arm this core's timer to fire at absolute counter value `deadline`.
#[inline]
pub fn set_deadline(deadline: u64) {
    // SAFETY: EL1 owns CNTP_* (CNTHCTL_EL2 is configured at boot).
    unsafe {
        asm!(
            "msr cntp_cval_el0, {}",
            "msr cntp_ctl_el0, {}",
            "isb",
            in(reg) deadline,
            in(reg) 1u64,
            options(nomem, nostack, preserves_flags)
        )
    };
}

/// Disable this core's timer.
pub fn stop() {
    // SAFETY: EL1 owns CNTP_*.
    unsafe { asm!("msr cntp_ctl_el0, {}", in(reg) 0u64, options(nomem, nostack, preserves_flags)) };
}
