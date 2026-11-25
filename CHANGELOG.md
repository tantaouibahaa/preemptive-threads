# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
uses [Semantic Versioning](https://semver.org/).

## [1.0.0] - 2026-11-25

A rewrite of the kernel. 1.0 is the first release that is verified end to end:
a self-checking test kernel runs on three QEMU configurations in CI.

### Added
- SMP: all four cores run threads. Secondaries are started with PSCI
  `CPU_ON` on QEMU `virt` and through the firmware spin table on the
  Raspberry Pi. There are per-core run queues, idle threads, IRQ stacks and
  timers, plus work stealing and reschedule IPIs (GIC SGI / ARM-local
  mailbox).
- BCM2836/7 ARM-local interrupt controller driver for the Pi Zero 2 W and
  QEMU `raspi3b`.
- Identity-mapped MMU with caches on every core, required for atomics on
  real Cortex-A53 hardware.
- Complete EL3/EL2 to EL1 drop (CNTHCTL_EL2, CNTVOFF_EL2, CPTR_EL2,
  VPIDR/VMPIDR_EL2, SCTLR_EL1).
- Blocking primitives: wait queues, `Mutex` with priority inheritance and
  hand-off, `Semaphore`, `Condvar`, bounded MPMC `channel`, `sleep` on a
  deadline queue, and `JoinHandle::join` returning the thread's value.
- Kernel heap (`linked_list_allocator`), stack pool with overflow canaries,
  and reaping of finished threads.
- Round-robin (time-sliced) and FIFO policies, four priority bands, CPU
  affinity, runtime statistics (`stats()`), scheduler state dump.
- Fatal exception reports (ESR/ELR/FAR/SPSR, decoded class, registers).
- `examples/test_kernel.rs` with 16 tests and semihosting exit codes;
  `make test-virt`, `make test-virt-el2` and `make test-raspi3b`; GitHub
  Actions runs all three, plus clippy, docs and an MSRV build.
- `docs/ARCHITECTURE.md`, and a real-hardware guide in the README.

### Changed
- Yield, block, exit and IRQ preemption share one context-switch path. IRQs
  save the full register file including all SIMD registers.
- A single concrete global kernel replaces the type-punned
  `get_global_kernel::<A, S>()`. The scheduling policy is a run-time enum.
- PL011 divisor set for the Pi's 48 MHz UART clock (IBRD 26, FBRD 3).
- Builds on stable Rust 1.88 or newer, without `build-std`.
- The license field now matches the shipped license file (MIT).

### Fixed
- The crate did not compile (`kernel.rs`).
- New threads started with IRQs masked and a null return address.
- Cooperative switches left the IRQ save context stale and restored NZCV
  as SPSR.
- The lock-free run queue leaked one allocation per push and could use
  freed nodes.
- `join()` always failed, `current_thread_id()` always returned 1, and
  `ThreadBuilder::spawn` ignored its closure.
- Exception handlers returned into the faulting instruction.

### Removed
- x86-era leftovers: the signal-based timer, the `std-shim` stub
  architecture, unused error types and the stale `fuzz/` targets.
- Debug JSON and `[DEBUG]` output on hot paths.

## [0.6.0] and earlier

Single-core prototypes. Up to 0.1.x they were x86_64 user-space experiments
(signals for preemption). 0.2-0.6 moved to bare-metal AArch64 with
timer-driven switching on one core of the Pi Zero 2 W. These versions are
superseded by 1.0.0.
