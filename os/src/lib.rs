//! pt-os: a tiny multicore OS on top of the `preemptive-threads` kernel.
//!
//! The core crate provides the mechanisms: EL0 entry and exit
//! ([`preemptive_threads::arch::user`]), per-process translation tables with
//! ASIDs ([`preemptive_threads::arch::mmu::AddressSpace`]) and the frame
//! allocator. This crate adds the policy:
//!
//! - [`mm`]: a process's memory map (VMAs, heap, stack) and the checked
//!   `copy_from_user` / `copy_to_user`;
//! - [`elf`]: a static ELF64 loader (PT_LOAD segments, W^X);
//! - [`process`]: the process table, `spawn`, `wait`, `kill` and the
//!   per-process run loop;
//! - [`syscall`]: the system call numbers and dispatcher;
//! - [`vfs`]: the in-memory file system, open files, pipes and fd tables;
//! - [`initrd`]: the embedded cpio archive unpacked into it at boot;
//! - [`console`]: console output and the input line discipline.
//!
//! A process is one kernel thread plus an address space. The thread runs
//! the user code with `run_user` and handles its system calls and faults in
//! ordinary kernel code; see `docs/ARCHITECTURE.md`.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;

pub mod console;
pub mod elf;
pub mod errno;
pub mod initrd;
pub mod mm;
pub mod process;
pub mod syscall;
pub mod vfs;

pub use errno::Errno;
pub use process::{kill, spawn, wait, Image, Output, Pid, Process, SpawnOptions};

/// Resource counters, used to check that exited processes give everything
/// back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OsStats {
    /// Free physical frames.
    pub frames_free: usize,
    /// Frames managed by the frame allocator.
    pub frames_total: usize,
    /// Free ASIDs.
    pub asids_free: usize,
    /// Entries in the process table (running and zombie).
    pub processes: usize,
}

/// Snapshot of the resource counters.
pub fn stats() -> OsStats {
    let f = preemptive_threads::mem::frame_stats();
    OsStats {
        frames_free: f.free,
        frames_total: f.total,
        asids_free: preemptive_threads::arch::mmu::asids_free(),
        processes: process::count(),
    }
}
