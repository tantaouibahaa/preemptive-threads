# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
uses [Semantic Versioning](https://semver.org/).

## [2.0.0] - unreleased

The repository grows from a threading library into **pt-os**, a tiny
multicore OS: isolated user programs at EL0, system calls, a RAM file
system, pipes, an interactive console and a shell, on all three QEMU
machines in CI. The core crate gains the mechanisms pt-os is built on. It
is now a Cargo workspace: the core crate stays at the root, `os/` is the
kernel and `user/` holds the programs.

### Added
- Core crate: EL0 support. `arch::user::run_user` enters user code with
  `eret` and comes back on any exception or a kill. There are lower-EL
  sync and IRQ vectors, fault decoding (`classify`, `FaultKind`), and
  SP_EL0, TPIDR_EL0 and TTBR0 per thread.
- Core crate: user address spaces. 4 KiB page tables in a 1 GiB user
  window, AP/nG/UXN/PXN bits, an 8-bit ASID allocator, broadcast TLB
  invalidation, and a physical frame allocator over the RAM the kernel
  does not use.
- Core crate: interruptible waits (`Thread::interrupt`,
  `wait_until_interruptible`, `sleep_interruptible`) and per-thread CPU
  time.
- Core crate: the PL011 receive interrupt on QEMU `virt` (GIC SPI 33) and
  the Pi (BCM2835 IRQ 57), with a console receive hook.
- pt-os (`os/`): a process table, `spawn`/`wait`/`kill`, process groups,
  a static ELF64 loader with W^X, VMAs, `sbrk`, and checked
  `copy_from_user`/`copy_to_user`. A user fault kills only the process,
  with one line giving the fault, address and pc. Every frame and the ASID
  are returned on exit.
- pt-os: 26 system calls (`svc #0`, number in x8): processes, files,
  directories, pipes, `dup2`, `procinfo`/`sysinfo`, the foreground group
  and console `ioctl`s.
- pt-os: a writable ramfs unpacked from a cpio initrd generated at build
  time, per-process fd tables, pipes with EOF and `EPIPE`, `/dev/console`
  and `/dev/null`.
- pt-os: a console line discipline (echo, backspace, Ctrl-U, Ctrl-D, raw
  mode, Ctrl-C kills the foreground group), and `/bin/sh` started at
  boot and restarted if it exits.
- `user/ulib`: `_start` with argv, system call wrappers,
  `println!`/`eprintln!`, a global allocator over `sbrk`, `File`,
  `LineReader`, `SpawnAttr`.
- Userland:
  - `sh`: pipelines, `<` `>` `>>`, `&` jobs, `;` and `&&` lists, `jobs`,
    `wait`, `kill`, `cd`, `pwd`, `exit`, `help`. Ctrl-C interrupts the
    foreground job, or the job a `wait` is waiting for.
  - `ls`, `cat`, `echo`, `wc`, `grep`, `ps` and `top` (per-core load).
  - `primes`, `mandel` (4 worker processes over pipes), `stress`,
    `crash`, `spin`, `sleep`, `rm`, `mkdir` and `hello`.
- Tests:
  - `os/examples/os_test.rs`, 21 OS tests: faults, isolation, ASIDs,
    32 processes over 4 cores, VFS, pipes, `EINTR`, the line discipline,
    Ctrl-C, a shell session, and resources back at their baseline.
  - `scripts/shell-session.py`, 17 checks typed into the real serial
    port.
  - `make test` runs the thread tests, the OS tests and the shell session
    on `virt`, `virt-el2` and `raspi3b`, locally and in CI.
- `make run-os-virt` / `make run-os-raspi3b` (interactive shell), `make
  build/pt-os-kernel8.img` (Pi image).
- `scripts/shell-session.py --record` and `scripts/cast2gif.py`, which
  produced `docs/demo.gif`.
- `docs/ARCHITECTURE.md` section 6: address spaces, EL0/EL1
  transitions, the path of a system call, the file system, pipes, the
  console, jobs and Ctrl-C, and the process lifecycle.

### Changed
- The README leads with pt-os. The library documentation is shorter,
  and the real-Pi guide includes `kernel_address=0x80000`.
- The workspace crates are versioned 2.0.0.

### Fixed
- The thread test kernel's watchdog is disarmed before the verdict, so
  it cannot fire after a non-semihosting exit.

### Not yet verified
- Real Raspberry Pi Zero 2 W hardware. Everything is verified on QEMU
  `raspi3b`, which boots the image the way the firmware does.

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
