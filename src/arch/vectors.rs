//! Exception vector table and trap frames.
//!
//! Threads run at EL1h, so every exception taken from a thread lands in the
//! "current EL with SPx" group and the trap frame is pushed on the
//! interrupted thread's own stack. That is what makes preemption simple: the
//! IRQ handler may call into the scheduler and switch to another thread; the
//! interrupted thread's frame stays on its stack and is popped (followed by
//! `eret`) whenever the thread is switched back in.
//!
//! IRQ handling runs on a per-core IRQ stack (see `irq_stack_top` in
//! [`crate::sched::percpu`]); the scheduler is entered only after switching
//! back to the thread stack.
//!
//! Synchronous exceptions, FIQs, SErrors and anything from a lower EL are
//! fatal: the handler dumps ESR/ELR/FAR/SPSR and the registers, then halts.

use core::arch::global_asm;

/// Registers saved on every exception entry.
///
/// Layout (bytes): x0-x30 at 0..248, ELR at 248, SPSR at 256, FPCR at 264,
/// FPSR at 272, q0-q31 at 288..800.
#[repr(C, align(16))]
pub struct TrapFrame {
    /// General purpose registers x0-x30.
    pub x: [u64; 31],
    /// Exception link register (return address).
    pub elr: u64,
    /// Saved program status register.
    pub spsr: u64,
    /// Floating-point control register.
    pub fpcr: u64,
    /// Floating-point status register.
    pub fpsr: u64,
    _pad: u64,
    /// SIMD/FP registers v0-v31.
    pub q: [u128; 32],
}

/// Size of [`TrapFrame`] in bytes.
pub const TRAP_FRAME_SIZE: usize = 800;
const _: () = assert!(core::mem::size_of::<TrapFrame>() == TRAP_FRAME_SIZE);

global_asm!(
    r#"
.macro PT_SAVE_REST
    stp     x2, x3, [sp, #16]
    stp     x4, x5, [sp, #32]
    stp     x6, x7, [sp, #48]
    stp     x8, x9, [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    mrs     x9, elr_el1
    stp     x30, x9, [sp, #240]
    mrs     x9, spsr_el1
    mrs     x10, fpcr
    stp     x9, x10, [sp, #256]
    mrs     x9, fpsr
    str     x9, [sp, #272]
    stp     q0, q1, [sp, #288]
    stp     q2, q3, [sp, #320]
    stp     q4, q5, [sp, #352]
    stp     q6, q7, [sp, #384]
    stp     q8, q9, [sp, #416]
    stp     q10, q11, [sp, #448]
    stp     q12, q13, [sp, #480]
    stp     q14, q15, [sp, #512]
    stp     q16, q17, [sp, #544]
    stp     q18, q19, [sp, #576]
    stp     q20, q21, [sp, #608]
    stp     q22, q23, [sp, #640]
    stp     q24, q25, [sp, #672]
    stp     q26, q27, [sp, #704]
    stp     q28, q29, [sp, #736]
    stp     q30, q31, [sp, #768]
.endm

.macro PT_ENTRY handler
    .balign 128
    sub     sp, sp, #{frame}
    stp     x0, x1, [sp, #0]
    b       \handler
.endm

.macro PT_FATAL kind
    PT_SAVE_REST
    mov     x0, sp
    mov     x1, #\kind
    bl      {fatal}
    b       .
.endm

.section .vectors, "ax"
.balign 2048
.global __pt_vectors
__pt_vectors:
    // Current EL with SP0
    PT_ENTRY __pt_fatal_0
    PT_ENTRY __pt_fatal_1
    PT_ENTRY __pt_fatal_2
    PT_ENTRY __pt_fatal_3
    // Current EL with SPx (threads and kernel run here)
    PT_ENTRY __pt_sync_el1h
    PT_ENTRY __pt_irq_el1h
    PT_ENTRY __pt_fatal_6
    PT_ENTRY __pt_fatal_7
    // Lower EL, AArch64
    PT_ENTRY __pt_fatal_8
    PT_ENTRY __pt_fatal_9
    PT_ENTRY __pt_fatal_10
    PT_ENTRY __pt_fatal_11
    // Lower EL, AArch32
    PT_ENTRY __pt_fatal_12
    PT_ENTRY __pt_fatal_13
    PT_ENTRY __pt_fatal_14
    PT_ENTRY __pt_fatal_15

.text
__pt_fatal_0:  PT_FATAL 0
__pt_fatal_1:  PT_FATAL 1
__pt_fatal_2:  PT_FATAL 2
__pt_fatal_3:  PT_FATAL 3
__pt_sync_el1h: PT_FATAL 4
__pt_fatal_6:  PT_FATAL 6
__pt_fatal_7:  PT_FATAL 7
__pt_fatal_8:  PT_FATAL 8
__pt_fatal_9:  PT_FATAL 9
__pt_fatal_10: PT_FATAL 10
__pt_fatal_11: PT_FATAL 11
__pt_fatal_12: PT_FATAL 12
__pt_fatal_13: PT_FATAL 13
__pt_fatal_14: PT_FATAL 14
__pt_fatal_15: PT_FATAL 15

__pt_irq_el1h:
    PT_SAVE_REST
    mov     x19, sp                 // x19 = trap frame on the thread stack
    mrs     x0, tpidr_el1
    bl      {irq_stack_top}         // per-core IRQ stack
    mov     sp, x0
    bl      {dispatch}              // acknowledge + handle + EOI
    mov     sp, x19
    bl      {irq_exit}              // may switch threads; returns when we run again
    b       __pt_trap_return

.global __pt_trap_return
__pt_trap_return:
    ldp     q0, q1, [sp, #288]
    ldp     q2, q3, [sp, #320]
    ldp     q4, q5, [sp, #352]
    ldp     q6, q7, [sp, #384]
    ldp     q8, q9, [sp, #416]
    ldp     q10, q11, [sp, #448]
    ldp     q12, q13, [sp, #480]
    ldp     q14, q15, [sp, #512]
    ldp     q16, q17, [sp, #544]
    ldp     q18, q19, [sp, #576]
    ldp     q20, q21, [sp, #608]
    ldp     q22, q23, [sp, #640]
    ldp     q24, q25, [sp, #672]
    ldp     q26, q27, [sp, #704]
    ldp     q28, q29, [sp, #736]
    ldp     q30, q31, [sp, #768]
    ldp     x9, x10, [sp, #256]
    msr     spsr_el1, x9
    msr     fpcr, x10
    ldr     x9, [sp, #272]
    msr     fpsr, x9
    ldp     x30, x9, [sp, #240]
    msr     elr_el1, x9
    ldp     x0, x1, [sp, #0]
    ldp     x2, x3, [sp, #16]
    ldp     x4, x5, [sp, #32]
    ldp     x6, x7, [sp, #48]
    ldp     x8, x9, [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    add     sp, sp, #{frame}
    eret
"#,
    frame = const TRAP_FRAME_SIZE,
    fatal = sym fatal_exception,
    irq_stack_top = sym crate::sched::percpu::irq_stack_top,
    dispatch = sym crate::kernel::irq_dispatch,
    irq_exit = sym crate::kernel::irq_exit,
);

extern "C" {
    /// The vector table (2 KiB aligned, installed in VBAR_EL1 at boot).
    pub static __pt_vectors: u8;
}

const KINDS: [&str; 16] = [
    "Synchronous (EL1t)",
    "IRQ (EL1t)",
    "FIQ (EL1t)",
    "SError (EL1t)",
    "Synchronous (EL1h)",
    "IRQ (EL1h)",
    "FIQ (EL1h)",
    "SError (EL1h)",
    "Synchronous (lower EL, AArch64)",
    "IRQ (lower EL, AArch64)",
    "FIQ (lower EL, AArch64)",
    "SError (lower EL, AArch64)",
    "Synchronous (lower EL, AArch32)",
    "IRQ (lower EL, AArch32)",
    "FIQ (lower EL, AArch32)",
    "SError (lower EL, AArch32)",
];

fn exception_class(esr: u64) -> &'static str {
    match (esr >> 26) & 0x3f {
        0x00 => "unknown reason",
        0x01 => "WFI/WFE trap",
        0x07 => "FP/SIMD access trap",
        0x0e => "illegal execution state",
        0x15 => "SVC (AArch64)",
        0x16 => "HVC (AArch64)",
        0x17 => "SMC (AArch64)",
        0x18 => "MSR/MRS trap",
        0x20 => "instruction abort (lower EL)",
        0x21 => "instruction abort",
        0x22 => "PC alignment fault",
        0x24 => "data abort (lower EL)",
        0x25 => "data abort",
        0x26 => "SP alignment fault",
        0x2c => "FP exception",
        0x2f => "SError",
        0x30 | 0x31 => "breakpoint",
        0x32 | 0x33 => "software step",
        0x34 | 0x35 => "watchpoint",
        0x3c => "BRK instruction",
        _ => "other",
    }
}

/// Dump an unexpected exception and stop the system.
extern "C" fn fatal_exception(frame: &TrapFrame, kind: u64) -> ! {
    let (esr, far): (u64, u64);
    // SAFETY: reading syndrome registers has no side effects.
    unsafe {
        core::arch::asm!("mrs {}, esr_el1", "mrs {}, far_el1", out(reg) esr, out(reg) far);
    }
    let mut w = crate::console::PanicWriter;
    use core::fmt::Write;
    let _ = writeln!(
        w,
        "\n*** FATAL EXCEPTION on cpu {}: {} ***",
        super::cpu_id(),
        KINDS[(kind & 15) as usize]
    );
    let _ = writeln!(
        w,
        "ESR_EL1={:#018x} (EC={:#04x}: {})\nELR_EL1={:#018x}\nFAR_EL1={:#018x}\nSPSR_EL1={:#018x}",
        esr,
        (esr >> 26) & 0x3f,
        exception_class(esr),
        frame.elr,
        far,
        frame.spsr
    );
    for i in (0..31).step_by(2) {
        if i + 1 < 31 {
            let _ = writeln!(w, "x{:<2}={:#018x}  x{:<2}={:#018x}", i, frame.x[i], i + 1, frame.x[i + 1]);
        } else {
            let _ = writeln!(w, "x{:<2}={:#018x}", i, frame.x[i]);
        }
    }
    let _ = writeln!(w, "sp ={:#018x}", frame as *const _ as usize + TRAP_FRAME_SIZE);
    super::semihosting::exit(3);
}
