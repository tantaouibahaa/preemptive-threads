//! AArch64 Exception Vector Table for Raspberry Pi Zero 2 W.
//!
//! ARM64 requires a 2048-byte aligned vector table with 16 entries,
//! each 128 bytes apart. This module defines the vector table and
//! exception handlers.
//!
//! # Exception Classes
//!
//! - Synchronous: Instruction aborts, data aborts, SVCs, etc.
//! - IRQ: Normal interrupts (used for timer preemption)
//! - FIQ: Fast interrupts (not used)
//! - SError: System errors
//!
//! # Exception Levels
//!
//! - Current EL with SP0: Not typically used
//! - Current EL with SPx: Kernel mode exceptions
//! - Lower EL (AArch64): User mode exceptions (not used in bare-metal)
//! - Lower EL (AArch32): 32-bit mode exceptions (not supported)

use core::arch::asm;
#[cfg(target_arch = "aarch64")]
use core::arch::naked_asm;

/// Exception context saved on the stack during exception handling.
#[repr(C)]
pub struct ExceptionContext {
    pub x: [u64; 31],
    pub elr: u64,
    pub spsr: u64,
    pub esr: u64,
    pub far: u64,
}

macro_rules! vector_entry {
    ($handler:ident) => {
        concat!(
            ".align 7\n",
            "b ", stringify!($handler), "\n",
        )
    };
}

/// The exception vector table.
///
/// Must be 2048-byte aligned and placed in the `.vectors` section.
/// Each of the 16 entries is 128 bytes apart.
///
/// # Safety
///
/// This is a naked function that sets up the exception vector table.
/// Must be installed at boot via VBAR_EL1 register.
#[cfg(target_arch = "aarch64")]
#[link_section = ".vectors"]
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn _vectors() {
    naked_asm!(
        ".align 11",  // 2048-byte alignment (2^11)

        // Current EL with SP0 (EL1t)
        vector_entry!(sync_el1t),
        vector_entry!(irq_el1t),
        vector_entry!(fiq_el1t),
        vector_entry!(serror_el1t),

        // Current EL with SPx (EL1h) - This is what we use
        vector_entry!(sync_el1h),
        vector_entry!(irq_el1h),
        vector_entry!(fiq_el1h),
        vector_entry!(serror_el1h),

        // Lower EL using AArch64 (EL0)
        vector_entry!(sync_el0_64),
        vector_entry!(irq_el0_64),
        vector_entry!(fiq_el0_64),
        vector_entry!(serror_el0_64),

        // Lower EL using AArch32 (not supported)
        vector_entry!(sync_el0_32),
        vector_entry!(irq_el0_32),
        vector_entry!(fiq_el0_32),
        vector_entry!(serror_el0_32),
    );
}

// Exception handlers - Current EL with SP0 (shouldn't happen)
#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn sync_el1t() {
    naked_asm!("b ."); // Hang
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn irq_el1t() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn fiq_el1t() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn serror_el1t() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn sync_el1h() {
    naked_asm!(
        "sub sp, sp, #272",
        "stp x0, x1, [sp, #0]",
        "stp x2, x3, [sp, #16]",
        "stp x4, x5, [sp, #32]",
        "stp x6, x7, [sp, #48]",
        "stp x8, x9, [sp, #64]",
        "stp x10, x11, [sp, #80]",
        "stp x12, x13, [sp, #96]",
        "stp x14, x15, [sp, #112]",
        "stp x16, x17, [sp, #128]",
        "stp x18, x19, [sp, #144]",
        "stp x20, x21, [sp, #160]",
        "stp x22, x23, [sp, #176]",
        "stp x24, x25, [sp, #192]",
        "stp x26, x27, [sp, #208]",
        "stp x28, x29, [sp, #224]",
        "str x30, [sp, #240]",

        "mrs x0, elr_el1",
        "mrs x1, spsr_el1",
        "mrs x2, esr_el1",
        "mrs x3, far_el1",
        "stp x0, x1, [sp, #248]",
        "stp x2, x3, [sp, #264]",

        "mov x0, sp",
        "bl sync_exception_handler",

        "ldp x0, x1, [sp, #248]",
        "msr elr_el1, x0",
        "msr spsr_el1, x1",

        "ldp x0, x1, [sp, #0]",
        "ldp x2, x3, [sp, #16]",
        "ldp x4, x5, [sp, #32]",
        "ldp x6, x7, [sp, #48]",
        "ldp x8, x9, [sp, #64]",
        "ldp x10, x11, [sp, #80]",
        "ldp x12, x13, [sp, #96]",
        "ldp x14, x15, [sp, #112]",
        "ldp x16, x17, [sp, #128]",
        "ldp x18, x19, [sp, #144]",
        "ldp x20, x21, [sp, #160]",
        "ldp x22, x23, [sp, #176]",
        "ldp x24, x25, [sp, #192]",
        "ldp x26, x27, [sp, #208]",
        "ldp x28, x29, [sp, #224]",
        "ldr x30, [sp, #240]",
        "add sp, sp, #272",

        "eret",
    );
}

/// IRQ handler - This is the main interrupt entry point for timer preemption.
///
/// This handler saves the interrupted thread's context to IRQ_SAVE_CTX,
/// calls the high-level handler (which may update IRQ_LOAD_CTX),
/// then restores context from IRQ_LOAD_CTX and returns.
///
/// Uses a dedicated IRQ stack to avoid corrupting the interrupted thread's stack.
///
/// Context structure layout (Aarch64Context):
/// - x[0-30]: offsets 0-240 (31 * 8 bytes)
/// - sp: offset 248
/// - pc: offset 256
/// - pstate: offset 264
#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn irq_el1h() {
    naked_asm!(
        // === PHASE 1: Save critical registers to thread stack, then switch to IRQ stack ===
        // Strategy: Use the thread's stack briefly to save x0-x3, x29, x30, ELR, SPSR
        // Then switch to IRQ stack and copy to the save context.

        // Save x0-x3 to thread stack FIRST, before clobbering
        "sub sp, sp, #64",
        "stp x0, x1, [sp, #0]",    // Save original x0, x1
        "stp x2, x3, [sp, #16]",   // Save original x2, x3
        "stp x29, x30, [sp, #32]", // Save x29, x30 too
        "mrs x0, elr_el1",
        "mrs x1, spsr_el1",
        "stp x0, x1, [sp, #48]",   // Save ELR, SPSR

        "add x0, sp, #64",

        "adrp x29, {irq_stack}",
        "add x29, x29, :lo12:{irq_stack}",
        "add x29, x29, #4096",
        "mov x2, sp",
        "mov sp, x29",

        "adrp x29, {irq_save_ctx}",
        "add x29, x29, :lo12:{irq_save_ctx}",
        "ldr x29, [x29]",

        "cbz x29, 2f",

        "ldp x3, x1, [x2, #0]",
        "ldr x3, [x2, #0]",
        "str x3, [x29, #0]",
        "ldr x3, [x2, #8]",
        "str x3, [x29, #8]",
        "ldr x3, [x2, #16]",
        "str x3, [x29, #16]",
        "ldr x3, [x2, #24]",
        "str x3, [x29, #24]",

        "stp x4, x5, [x29, #32]",
        "stp x6, x7, [x29, #48]",
        "stp x8, x9, [x29, #64]",
        "stp x10, x11, [x29, #80]",
        "stp x12, x13, [x29, #96]",
        "stp x14, x15, [x29, #112]",
        "stp x16, x17, [x29, #128]",
        "stp x18, x19, [x29, #144]",
        "stp x20, x21, [x29, #160]",
        "stp x22, x23, [x29, #176]",
        "stp x24, x25, [x29, #192]",
        "stp x26, x27, [x29, #208]",
        "str x28, [x29, #224]",

        "ldp x3, x1, [x2, #32]",   // x3 = original x29, x1 = original x30
        "str x3, [x29, #232]",     // Save x29
        "str x1, [x29, #240]",     // Save x30

        "str x0, [x29, #248]",

        "ldp x3, x1, [x2, #48]",
        "str x3, [x29, #256]",     // PC = ELR
        "str x1, [x29, #264]",     // pstate = SPSR

        "2:",
        "bl irq_handler",

        "adrp x29, {irq_load_ctx}",
        "add x29, x29, :lo12:{irq_load_ctx}",
        "ldr x29, [x29]",

        "cbz x29, 3f",

        "ldr x0, [x29, #264]",
        "msr spsr_el1, x0",
        "ldr x0, [x29, #256]",
        "msr elr_el1, x0",

        "ldr x0, [x29, #248]",
        "mov sp, x0",

        "ldp x0, x1, [x29, #0]",
        "ldp x2, x3, [x29, #16]",
        "ldp x4, x5, [x29, #32]",
        "ldp x6, x7, [x29, #48]",
        "ldp x8, x9, [x29, #64]",
        "ldp x10, x11, [x29, #80]",
        "ldp x12, x13, [x29, #96]",
        "ldp x14, x15, [x29, #112]",
        "ldp x16, x17, [x29, #128]",
        "ldp x18, x19, [x29, #144]",
        "ldp x20, x21, [x29, #160]",
        "ldp x22, x23, [x29, #176]",
        "ldp x24, x25, [x29, #192]",
        "ldp x26, x27, [x29, #208]",
        "ldr x28, [x29, #224]",
        "ldr x30, [x29, #240]",

        "ldr x29, [x29, #232]",

        "eret",

        "3:",
        "b .",

        irq_save_ctx = sym super::aarch64::IRQ_SAVE_CTX,
        irq_load_ctx = sym super::aarch64::IRQ_LOAD_CTX,
        irq_stack = sym super::aarch64::IRQ_STACK,
    );
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn fiq_el1h() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn serror_el1h() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn sync_el0_64() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn irq_el0_64() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn fiq_el0_64() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn serror_el0_64() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn sync_el0_32() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn irq_el0_32() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn fiq_el0_32() {
    naked_asm!("b .");
}

#[cfg(target_arch = "aarch64")]
#[no_mangle]
#[unsafe(naked)]
unsafe extern "C" fn serror_el0_32() {
    naked_asm!("b .");
}

#[no_mangle]
extern "C" fn sync_exception_handler(ctx: *mut ExceptionContext) {
    let ctx = unsafe { &*ctx };

    let esr = ctx.esr;
    let ec = (esr >> 26) & 0x3F;

    match ec {
        0b010101 => {
        }
        0b100000 | 0b100001 => {
            // Instruction abort
            // TODO: Handle or panic
        }
        0b100100 | 0b100101 => {
            // Data abort
            // TODO: Handle or panic
        }
        _ => {
            // Unknown exception - hang
            loop {
                unsafe { asm!("wfe"); }
            }
        }
    }
}

#[no_mangle]
extern "C" fn irq_handler() {
    #[cfg(target_arch = "aarch64")]
    {
        use super::aarch64_gic::{Gic400, TIMER_IRQ, SPURIOUS_IRQ};

        let irq = unsafe { Gic400::acknowledge_interrupt() };

        if irq == SPURIOUS_IRQ {
            return;
        }

        match irq {
            TIMER_IRQ => {
                timer_interrupt_handler();
            }
            _ => {
                // Unknown interrupt - just acknowledge and return
            }
        }

        unsafe { Gic400::end_interrupt(irq); }
    }
}

/// Timer interrupt handler - triggers preemption.
fn timer_interrupt_handler() {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            super::aarch64::timer_interrupt_handler();
        }
    }
}

/// Install the exception vector table.
///
/// # Safety
///
/// Must be called once during system initialization with interrupts disabled.
#[cfg(target_arch = "aarch64")]
pub unsafe fn install_vector_table() {
    unsafe {
        asm!(
            "adr {tmp}, _vectors",
            "msr vbar_el1, {tmp}",
            "isb",
            tmp = out(reg) _,
            options(nomem, nostack)
        );
    }
}

/// Placeholder for non-ARM64 targets (testing).
#[cfg(not(target_arch = "aarch64"))]
pub unsafe fn install_vector_table() {
    // No-op for testing
}

#[cfg(not(target_arch = "aarch64"))]
#[no_mangle]
pub unsafe extern "C" fn _vectors() {
    // No-op for testing
}
