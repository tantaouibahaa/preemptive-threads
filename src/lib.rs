#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

//! Preemptive, SMP threading for bare-metal AArch64.
//!
//! Runs on the Raspberry Pi Zero 2 W / Pi 3 (BCM2837, 4x Cortex-A53) and
//! on QEMU (`raspi3b` and `virt` with the `qemu-virt` feature) with no
//! operating system underneath.
//!
//! - Timer-driven preemption on all four cores, per-core run queues with
//!   four priority bands, work stealing and reschedule IPIs.
//! - Round-robin or FIFO policy within a band.
//! - Blocking primitives: [`sync::Mutex`] (priority inheritance),
//!   [`sync::Semaphore`], [`sync::Condvar`], bounded [`sync::channel()`],
//!   [`thread::sleep`] and [`thread::JoinHandle::join`] returning the
//!   thread's value.
//! - A real heap (`alloc` works), stack pooling and reclamation of
//!   finished threads.
//!
//! # Using it
//!
//! The crate owns the boot code, exception vectors, heap and panic handler.
//! An application provides `kernel_main` and hands control to [`start`]:
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use preemptive_threads::{self as pt, println, thread, Config};
//!
//! #[no_mangle]
//! fn kernel_main() -> ! {
//!     pt::start(Config::default(), app)
//! }
//!
//! fn app() {
//!     let h = thread::spawn(|| 6 * 7);
//!     println!("answer = {}", h.join());
//! }
//! ```
//!
//! Link with `rpi0w2.ld` (default) or `qemu_virt.ld` (`qemu-virt`
//! feature); this crate's build script does that for its own examples.

extern crate alloc;

#[cfg(not(target_arch = "aarch64"))]
compile_error!("preemptive-threads only supports bare-metal aarch64 (--target aarch64-unknown-none)");

pub mod arch;
#[macro_use]
pub mod console;
pub mod errors;
mod kernel;
pub mod mem;
pub mod platform;
pub mod sched;
pub mod sync;
pub mod thread;
pub mod time;

pub use errors::SpawnError;
pub use kernel::{boot_el, start, stats, Config, CpuStats, KernelStats};
pub use sched::{priority, SchedPolicy, ThreadId, ThreadState};
pub use thread::{current_cpu, current_thread_id, sleep, spawn, yield_now, Builder, JoinHandle, Thread};
pub use time::{Duration, Instant};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    arch::irq_disable();
    kernel::stop_other_cpus();
    use core::fmt::Write;
    let _ = writeln!(console::PanicWriter, "\n*** PANIC on cpu {}: {}", arch::cpu_id(), info);
    arch::semihosting::exit(101)
}
