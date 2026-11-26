//! User mode (EL0).
//!
//! The kernel runs user code on an ordinary kernel thread: the thread calls
//! [`run_user`] with a [`TrapFrame`] holding the user registers, the CPU
//! drops to EL0, and `run_user` returns when the user code takes a
//! synchronous exception (system call, fault) or was asked to stop
//! ([`crate::thread::Thread::interrupt_user`]). The OS layer handles the
//! exception in normal kernel code, with IRQs enabled and free to block,
//! updates the frame (e.g. the return value in x0) and calls `run_user`
//! again. A process "exits" simply by not calling it again.
//!
//! Interrupts taken at EL0 do not return from `run_user`: they go through
//! the regular IRQ path, including preemption, and resume user code
//! directly (see [`crate::arch::vectors`]).
//!
//! System call ABI (the OS layer interprets it): `svc #0`, number in x8,
//! arguments in x0-x5, result in x0 (negative errno on failure).

use super::vectors::{TrapFrame, __pt_run_user, LEAVE_SYNC};
use crate::sched;
use core::arch::asm;
use core::fmt;

/// Why [`run_user`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserExit {
    /// The user code took a synchronous exception. `esr` and `far` are
    /// ESR_EL1 and FAR_EL1; decode them with [`classify`].
    Exception {
        /// Exception syndrome.
        esr: u64,
        /// Fault address (valid for aborts and alignment faults).
        far: u64,
    },
    /// [`crate::thread::Thread::interrupt_user`] was called for this thread.
    Interrupted,
}

/// Run user code with the register state in `frame` until it traps.
///
/// The frame's SPSR is sanitised to EL0t with all interrupts unmasked
/// (only the NZCV flags are kept), so user code always runs preemptibly.
/// When this returns, `frame` holds the complete user state at the time of
/// the exception, with ELR pointing after the `svc` for a system call.
/// The caller's IRQ mask state is restored before returning.
pub fn run_user(frame: &mut TrapFrame) -> UserExit {
    let irq = super::irq_save();
    if sched::take_user_interrupt() {
        super::irq_restore(irq);
        return UserExit::Interrupted;
    }
    frame.spsr &= 0xf000_0000;
    // SAFETY: IRQs are masked; `frame` is a valid, exclusively borrowed
    // trap frame on this thread's stack (it lives in our caller's frame,
    // above the context block __pt_run_user pushes); the user state it
    // holds is confined to EL0 by the sanitised SPSR.
    let why = unsafe { __pt_run_user(frame) };
    let exit = if why == LEAVE_SYNC {
        let (esr, far): (u64, u64);
        // SAFETY: reading syndrome registers has no side effects. IRQs are
        // still masked, so nothing has overwritten them since the trap.
        unsafe { asm!("mrs {}, esr_el1", "mrs {}, far_el1", out(reg) esr, out(reg) far, options(nomem, nostack)) };
        UserExit::Exception { esr, far }
    } else {
        UserExit::Interrupted
    };
    super::irq_restore(irq);
    exit
}

/// Called by the EL0 IRQ path after `irq_exit`: whether to return from
/// `run_user` instead of resuming user code.
pub(crate) extern "C" fn el0_irq_leave() -> bool {
    sched::take_user_interrupt()
}

/// A fault caused by user code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// Load from an address that is unmapped or not readable at EL0.
    Read,
    /// Store to an address that is unmapped or not writable at EL0.
    Write,
    /// Instruction fetch from an address that is unmapped or not
    /// executable at EL0.
    Exec,
    /// Misaligned PC, SP or data access.
    Alignment,
    /// Undefined or privileged instruction, or illegal execution state.
    IllegalInstruction,
    /// `brk` instruction.
    Breakpoint,
    /// Trapped floating-point exception.
    FloatingPoint,
    /// Any other exception class.
    Other(u8),
}

impl FaultKind {
    /// Conventional signal number for the fault (SIGSEGV, SIGBUS, ...).
    pub fn signal(self) -> i32 {
        match self {
            FaultKind::Read | FaultKind::Write | FaultKind::Exec => 11,
            FaultKind::Alignment => 7,
            FaultKind::IllegalInstruction | FaultKind::Other(_) => 4,
            FaultKind::Breakpoint => 5,
            FaultKind::FloatingPoint => 8,
        }
    }
}

impl fmt::Display for FaultKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FaultKind::Read => write!(f, "segfault (read)"),
            FaultKind::Write => write!(f, "segfault (write)"),
            FaultKind::Exec => write!(f, "segfault (exec)"),
            FaultKind::Alignment => write!(f, "alignment fault"),
            FaultKind::IllegalInstruction => write!(f, "illegal instruction"),
            FaultKind::Breakpoint => write!(f, "breakpoint"),
            FaultKind::FloatingPoint => write!(f, "floating-point exception"),
            FaultKind::Other(ec) => write!(f, "unhandled exception (EC {:#04x})", ec),
        }
    }
}

/// A decoded synchronous exception from EL0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserTrap {
    /// `svc #imm`.
    Syscall(u16),
    /// `wfi` or `wfe` (trapped so user code cannot idle a core).
    WaitForInterrupt,
    /// A fault: the process should be killed.
    Fault {
        /// What went wrong.
        kind: FaultKind,
        /// The faulting data or instruction address (the PC for
        /// instruction faults that have no separate address).
        addr: u64,
    },
}

/// Decode the syndrome of an exception from EL0. `pc` is the user PC
/// (the trap frame's ELR).
pub fn classify(esr: u64, far: u64, pc: u64) -> UserTrap {
    let ec = ((esr >> 26) & 0x3f) as u8;
    let iss = esr & 0x01ff_ffff;
    let fault = |kind| UserTrap::Fault { kind, addr: far };
    let at_pc = |kind| UserTrap::Fault { kind, addr: pc };
    match ec {
        0x15 => UserTrap::Syscall((iss & 0xffff) as u16),
        0x01 => UserTrap::WaitForInterrupt,
        0x24 => {
            // Data abort from a lower EL: DFSC 0b100001 is an alignment
            // fault, WnR (bit 6) tells loads from stores.
            if iss & 0x3f == 0x21 {
                fault(FaultKind::Alignment)
            } else if iss & (1 << 6) != 0 {
                fault(FaultKind::Write)
            } else {
                fault(FaultKind::Read)
            }
        }
        0x20 => fault(FaultKind::Exec),
        0x22 => fault(FaultKind::Alignment),
        0x26 => at_pc(FaultKind::Alignment),
        0x00 | 0x07 | 0x0e | 0x18 => at_pc(FaultKind::IllegalInstruction),
        0x3c => at_pc(FaultKind::Breakpoint),
        0x2c => at_pc(FaultKind::FloatingPoint),
        other => at_pc(FaultKind::Other(other)),
    }
}
