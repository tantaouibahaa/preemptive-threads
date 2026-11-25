//! The single context-switch primitive.
//!
//! All thread switches (yield, block, exit and IRQ preemption) go through
//! [`switch_context`]. It pushes a [`SwitchFrame`] (the AAPCS64 callee-saved
//! registers x19-x30, d8-d15, FPCR/FPSR and DAIF) on the current stack,
//! stores the stack pointer into `*prev_sp`, loads `next_sp` and pops the
//! next thread's frame.
//!
//! Caller-saved state does not need saving here: for a voluntary switch the
//! compiler already treats it as clobbered by the call, and for preemption the
//! IRQ entry path has pushed a full [`super::vectors::TrapFrame`] (all x and
//! v registers, ELR, SPSR, FPCR, FPSR) below the switch frame on the same
//! stack. So a preempted thread's stack looks like
//!
//! ```text
//!   [ thread's own frames ]
//!   [ TrapFrame  (800 B) ]   <- pushed by the IRQ vector
//!   [ IRQ handler frames ]
//!   [ SwitchFrame (176 B) ]  <- saved_sp points here
//! ```
//!
//! and switching back to it simply unwinds both.

use core::arch::global_asm;

/// Callee-saved state pushed by [`switch_context`].
#[repr(C, align(16))]
#[derive(Default)]
pub struct SwitchFrame {
    /// x19..x28.
    pub x19_x28: [u64; 10],
    /// Frame pointer.
    pub x29: u64,
    /// Return address: where the thread resumes.
    pub x30: u64,
    /// d8..d15 (low 64 bits of v8..v15, callee-saved per AAPCS64).
    pub d8_d15: [u64; 8],
    /// DAIF at the time of the switch.
    pub daif: u64,
    /// FPCR (low 32 bits) and FPSR (high 32 bits).
    pub fp_ctl: u64,
}

/// Size of [`SwitchFrame`] in bytes.
pub const SWITCH_FRAME_SIZE: usize = 176;
const _: () = assert!(core::mem::size_of::<SwitchFrame>() == SWITCH_FRAME_SIZE);

global_asm!(
    r#"
.text
// x0 = &mut prev_sp, x1 = next_sp, x2 = value handed to the resumed side.
// Returns x2 (in the context of the thread that is switched in).
.global __pt_switch
__pt_switch:
    sub     sp, sp, #{size}
    stp     x19, x20, [sp, #0]
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     x29, x30, [sp, #80]
    stp     d8, d9, [sp, #96]
    stp     d10, d11, [sp, #112]
    stp     d12, d13, [sp, #128]
    stp     d14, d15, [sp, #144]
    mrs     x9, daif
    mrs     x10, fpcr
    mrs     x11, fpsr
    str     x9, [sp, #160]
    stp     w10, w11, [sp, #168]
    mov     x9, sp
    str     x9, [x0]

    mov     sp, x1
    ldp     x19, x20, [sp, #0]
    ldp     x21, x22, [sp, #16]
    ldp     x23, x24, [sp, #32]
    ldp     x25, x26, [sp, #48]
    ldp     x27, x28, [sp, #64]
    ldp     x29, x30, [sp, #80]
    ldp     d8, d9, [sp, #96]
    ldp     d10, d11, [sp, #112]
    ldp     d12, d13, [sp, #128]
    ldp     d14, d15, [sp, #144]
    ldp     w10, w11, [sp, #168]
    msr     fpcr, x10
    msr     fpsr, x11
    ldr     x9, [sp, #160]
    msr     daif, x9
    add     sp, sp, #{size}
    mov     x0, x2
    ret

// First code run by a new thread. x0 = value from __pt_switch (the previous
// thread), x19 = start argument. Never returns.
.global __pt_thread_entry
__pt_thread_entry:
    mov     x1, x19
    mov     x29, #0
    mov     x30, #0
    bl      {start}
    brk     #0x1
"#,
    size = const SWITCH_FRAME_SIZE,
    start = sym crate::sched::thread_start,
);

extern "C" {
    fn __pt_switch(prev_sp: *mut usize, next_sp: usize, handoff: usize) -> usize;
    fn __pt_thread_entry();
}

/// Switch stacks. Returns `handoff` as passed by whichever thread later
/// switches back to this one.
///
/// # Safety
///
/// IRQs must be masked, `next_sp` must point at a valid [`SwitchFrame`] on a
/// stack that is not in use, and `prev_sp` must stay valid until the
/// current thread is resumed.
#[inline(always)]
pub unsafe fn switch_context(prev_sp: *mut usize, next_sp: usize, handoff: usize) -> usize {
    // SAFETY: forwarded to the caller.
    unsafe { __pt_switch(prev_sp, next_sp, handoff) }
}

/// Build the initial switch frame for a new thread at the top of its stack.
/// Returns the initial saved stack pointer.
///
/// When first switched to, the thread runs `__pt_thread_entry`, which calls
/// `thread_start` with `arg`. IRQs stay masked until
/// `thread_start` has finished the switch bookkeeping, then it unmasks them.
///
/// # Safety
///
/// `stack_top` must be the 16-byte aligned top of a writable stack of at
/// least [`SWITCH_FRAME_SIZE`] bytes.
pub unsafe fn init_stack(stack_top: usize, arg: usize) -> usize {
    let sp = stack_top - SWITCH_FRAME_SIZE;
    let frame = SwitchFrame {
        x19_x28: [arg as u64, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        x29: 0,
        x30: __pt_thread_entry as *const () as usize as u64,
        d8_d15: [0; 8],
        daif: 0x3c0,
        fp_ctl: 0,
    };
    // SAFETY: the caller guarantees the memory is a writable, aligned stack.
    unsafe { core::ptr::write(sp as *mut SwitchFrame, frame) };
    sp
}
