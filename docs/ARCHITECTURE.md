# Architecture

This document describes how preemptive-threads gets from reset to four cores
switching threads: the boot path, the memory map, the context layout, the
interrupt flow and the SMP scheduler. Section 6 covers user processes
(`os/`, the `pt-os` kernel): address spaces, EL0 entry and exit, system
calls and the process lifecycle.

## 1. Boot

```text
               reset / firmware
                     │
       ┌─────────────┴──────────────┐
       │ _start (core 0)            │  __pt_secondary_entry (cores 1-3)
       │                            │   ▲ released by PSCI CPU_ON (virt)
       │                            │   │ or by the spin table (Pi)
       ▼                            ▼   │
  __pt_el_to_el1  ◄─────────────────────┘
       │   EL3: SCR_EL3 (NS, RW, HCE), CPTR_EL3 = 0, eret to EL2
       │   EL2: HCR_EL2.RW, CNTHCTL_EL2 |= EL1PCEN|EL1PCTEN, CNTVOFF_EL2 = 0,
       │        CPTR_EL2 (no FP trap), HSTR_EL2 = 0, VPIDR/VMPIDR_EL2,
       │        SCTLR_EL1 = RES1 (MMU off), eret to EL1h with DAIF masked
       ▼
  stack = __boot_stacks_top - cpu * 64 KiB
       │
  core 0 only: BOOT_L1[0] = &BOOT_L2 | table
       ▼
  __pt_cpu_setup: CPACR_EL1.FPEN, MAIR/TCR/TTBR0, TLB + I-cache invalidate,
                  SCTLR_EL1 = M | C | I, VBAR_EL1
       │
  core 0: clear .bss ─► boot_primary(boot_el)      cores 1-3: boot_secondary(cpu)
          TPIDR_EL1 = &PERCPU[0]                    TPIDR_EL1 = &PERCPU[cpu]
          UART, heap, IRQ controller                per-core IRQ routing
          boot context becomes idle thread 0        boot context becomes idle thread
          kernel_main()  (application)              start tick, idle loop
              └─► start(config, main):
                    release cores 1-3, wait for each to come online
                    spawn `main`, start tick, idle loop
```

Entry differs by machine:

| machine | entered at | how secondaries start |
|---------|------------|-----------------------|
| Pi Zero 2 W (firmware `armstub8`) | EL2, cores 1-3 parked | write entry to `0xD8 + 8n`, clean the cache line, `sev` |
| QEMU `raspi3b`, flat image | EL2 (QEMU's spin-table stub) | same as the Pi |
| QEMU `raspi3b`, ELF | EL3, all cores at the entry | `_start` emulates the firmware: secondaries poll the spin table |
| QEMU `virt` | EL1 | PSCI `CPU_ON` over `HVC` |
| QEMU `virt,virtualization=on` | EL2 | PSCI `CPU_ON` over `SMC` |

### Why the MMU is on

With the MMU off, every data access on a Cortex-A53 is Device memory.
Load/store-exclusive (what Rust atomics compile to without LSE) then needs a
global exclusive monitor, which the BCM2837 does not have, so atomics would
spin forever on real hardware. QEMU does not model this. The kernel therefore
enables the MMU with an identity map on every core before any Rust code runs.
The translation tables are `static` data built by `const fn`s; assembly only
patches one descriptor.

* 4 KiB granule, 39-bit VA (T0SZ = 25), walk starts at level 1.
* MAIR: index 0 = Device-nGnRnE, index 1 = Normal WB RW-allocate.
* RAM is Normal, Inner Shareable; MMIO is Device, PXN/UXN.
* Every kernel mapping is EL1-only (AP[2:1] = 00), UXN and global (nG = 0).

| region | Pi (BCM2837) | QEMU virt |
|--------|--------------|-----------|
| 0x0000_0000-0x3EFF_FFFF | RAM (2 MiB blocks) | MMIO (2 MiB blocks) |
| 0x3F00_0000-0x3FFF_FFFF | peripherals (Device) | MMIO |
| 0x4000_0000-0x7FFF_FFFF | ARM-local block (Device) | RAM |
| 0x8000_0000-0xBFFF_FFFF | unmapped | RAM |

Secondary cores read the spin-table slot and their stack with the MMU off.
The boot core cleans the slot's cache line to the point of coherency before
`sev`. Everything a secondary touches after that is accessed with the MMU on,
coherently.

## 2. Context layout

A thread is a TCB plus a stack. The TCB stores one saved stack pointer; all
register state lives on the thread's own stack.

```text
 high addresses
 ┌──────────────────────────────┐ ◄─ stack top
 │ thread's own frames          │
 ├──────────────────────────────┤
 │ TrapFrame (800 B)            │  only if the thread was interrupted:
 │   x0-x30, ELR_EL1, SPSR_EL1, │  pushed by the IRQ vector
 │   FPCR, FPSR, SP_EL0, q0-q31 │  (SP_EL0 only for traps from EL0)
 ├──────────────────────────────┤
 │ irq_exit / schedule frames   │
 ├──────────────────────────────┤
 │ SwitchFrame (192 B)          │  pushed by __pt_switch
 │   x19-x28, x29, x30,         │
 │   d8-d15, DAIF, FPCR|FPSR,   │
 │   SP_EL0, TPIDR_EL0          │
 └──────────────────────────────┘ ◄─ Tcb.saved_sp
 ...
 │ canary (checked on every switch-out)
 └──────────────────────────────┘ ◄─ stack bottom
```

There is exactly one switch routine, `__pt_switch(&prev.saved_sp, next.saved_sp, handoff)`.

* **Yield, block, exit**: the thread calls `schedule()`, which calls
  `__pt_switch`. Only callee-saved state needs saving: the compiler already
  treats everything else as clobbered by the call.
* **Preemption**: the IRQ vector pushes a full TrapFrame, handles the
  interrupt, and if a reschedule is due calls the same `schedule()` from
  `irq_exit` on the thread's stack. When the thread is later switched back
  in, `__pt_switch` returns into `irq_exit`, which returns to the vector,
  which pops the TrapFrame and `eret`s. Preempted and voluntarily switched
  threads are indistinguishable to the scheduler.
* **New threads** get a SwitchFrame whose x30 is `__pt_thread_entry` and
  whose x19 is the TCB. It calls `thread_start`, which finishes the switch
  bookkeeping, unmasks IRQs, runs the closure, publishes the result and
  exits.

`handoff` carries a strong reference to the previous thread across the switch.
The thread that resumes calls `finish_switch(prev)`, which clears
`prev.on_cpu`. If `prev` is dead, it returns the stack to the pool and drops
the TCB.

## 3. Interrupt flow

```text
 timer / IPI asserts IRQ on core n
      │
 vector (EL1h IRQ): push TrapFrame on the current thread's stack
      │
 switch SP to PERCPU[n] IRQ stack (16 KiB)
      │
 irq_dispatch:
      virt: loop { IAR ─► handle ─► EOIR }   (GICv2; INTID 30 = timer, SGI 1 = resched,
                                              SPI 33 = PL011, routed to core 0)
      Pi:   read COREn_IRQ_SOURCE            (bit 1 = CNTPNSIRQ, bit 4 = mailbox 0,
                                              bit 8 = GPU interrupt: BCM2835 IRQ 57 = PL011,
                                              routed to core 0 by GPU_INT_ROUTING)
      timer:  re-arm CNTP_CVAL (next tick), wake expired sleepers,
              charge the time slice, maybe set need_resched
      resched IPI: set need_resched
      UART: drain the RX FIFO into the console's receive handler, clear RXIM/RTIM
      │
 switch SP back to the thread stack
      │
 irq_exit: if need_resched ─► schedule()   (may run other threads for a while)
      │
 pop TrapFrame, eret
```

Interrupts are always acknowledged and completed on the core that took them,
before any switch. IRQs stay masked inside the handler. Kernel spinlocks
that may be taken from IRQ context (`IrqSpinLock`) mask IRQs while held.

## 4. SMP scheduler

```text
      core 0             core 1             core 2             core 3
 ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐
 │ current      │   │ current      │   │ current      │   │ current      │
 │ idle thread  │   │ idle thread  │   │ idle thread  │   │ idle thread  │
 │ run queue    │   │ run queue    │   │ run queue    │   │ run queue    │
 │  high   ───► │   │  high        │   │  high        │   │  high        │
 │  normal ───► │   │  normal      │   │  normal      │   │  normal      │
 │  low    ───► │   │  low         │   │  low         │   │  low         │
 │  idle   ───► │   │  idle        │   │  idle        │   │  idle        │
 │ timer, IRQ   │   │ timer, IRQ   │   │ timer, IRQ   │   │ timer, IRQ   │
 │ stack        │   │ stack        │   │ stack        │   │ stack        │
 └──────┬───────┘   └──────▲───────┘   └──────────────┘   └──────────────┘
        │  enqueue + IPI   │
        └──────────────────┘   idle cores steal from busy queues every tick
```

* **Placement** (`select_cpu`): pinned threads go to their core. Otherwise
  the thread's last core if it is idle, else any idle core, else the core
  with the shortest queue.
* **Preemption between cores**: if the new thread outranks what the target
  core runs (or the target is idle), the target gets `need_resched` and an
  IPI.
* **Pick**: the highest non-empty band of the local queue, round-robin within
  the band, then work stealing (`try_steal_work`), then the idle thread.
* **`on_cpu`**: a thread stays `on_cpu` from the moment it is picked until the
  next thread on that core has finished switching away from it. Pick and
  steal skip such threads, so a thread whose registers are still being saved
  is never resumed elsewhere, and cores never spin waiting for each other.
  When `finish_switch` clears `on_cpu` for a thread queued on an idle core,
  it kicks that core.
* **Policies**: round-robin charges one tick per timer interrupt and
  reschedules when the slice runs out and another thread is waiting. FIFO
  never rotates within a band. A higher band always preempts.

### Blocking

All blocking goes through the same protocol:

1. With IRQs masked, decide to wait while holding the lock that protects the
   condition.
2. Mark the current thread `Blocked` and queue it on a wait queue (or the
   sleep queue) **before** releasing that lock.
3. Release the lock, keeping IRQs masked, and call `schedule()`.

A waker takes the same lock, changes the condition, pops a waiter and calls
`wake()`, which moves it from `Blocked` to `Ready` with a compare-and-swap and
queues it. Because the waiter is published before the lock is released, a
wake-up cannot be lost. Because `schedule()` re-queues the previous thread
only if it is still `Running`, a thread woken between steps 2 and 3 is
never queued twice.

**Interruptible waits.** `WaitQueue::wait_until_interruptible` and
`thread::sleep_interruptible` return `Interrupted` once
`Thread::interrupt()` has been called on the thread, even if it is already
asleep. The waiter records what it is queued on in its TCB (`blocked_on`:
a wait queue's address, or the sleep queue) and checks the sticky
`interrupt_pending` flag under the queue's lock before queueing itself.
`interrupt()` sets the flag, then, holding the TCB's `blocked_on` lock,
takes the thread off that queue and wakes it. Either the waiter sees the
flag or `interrupt()` finds it queued; a waiter is only ever woken by
whoever removed it from the queue, so no stale entries remain. If a
`wake_one` picked the waiter just as it was interrupted, the waiter passes
that wake-up on. pt-os uses this to kill processes blocked in the kernel.

### Mutex priority inheritance

When a thread blocks on a `Mutex` held by a lower-priority owner, the owner's
effective priority is raised to the waiter's. If the owner is queued, it moves
to the higher band. On unlock, the mutex is handed directly to the
highest-priority waiter. That waiter is woken before the old owner drops its
boost, so the old owner cannot be preempted between de-boosting and the
hand-off. The `priority_inheritance` test reproduces the classic
low/medium/high inversion on one core and would hang without this.

## 5. Memory

* **Heap**: `linked_list_allocator` over `__heap_start..__heap_end` (64 MiB,
  reserved in the linker script), behind an `IrqSpinLock`, installed as the
  `#[global_allocator]`.
* **Stacks**: allocated from the heap (default 64 KiB, at least 8 KiB),
  recycled through a pool of up to 32 free stacks. A canary at the bottom is
  checked every time the thread is switched out.
* **Per-core**: 64 KiB boot stack (later the idle thread's stack) and a
  16 KiB IRQ stack.
* **TCBs** are `Arc`s. Owners are the run queue, the current slot of a core,
  a wait queue or the sleep queue, and `Thread` handles. A dead thread is
  reclaimed by the next thread to run on its core.

## 6. User processes (pt-os)

The core crate provides the mechanisms (EL0 entry, lower-EL vectors,
address spaces, ASIDs, frames, interruptible waits, the UART receive
interrupt); the `os/` crate (`pt-os`) adds the policy: the process table,
the ELF loader, user-pointer checks, the system calls, the file system,
pipes and the console. `user/` holds the programs: `ulib` (the user
library), `bin` (the userland in `/bin`) and `tests` (kernel-test programs,
in `/test` of the test kernel only).
A process is **one kernel thread plus an address space**. Everything else
(scheduling, preemption, migration between cores) is the thread machinery
above, unchanged.

### Address spaces

```text
 TTBR0_EL1 = process L1 table | ASID << 48         (the kernel's own: BOOT_L1, ASID 0)

 process L1 (private, 4 KiB)
 ┌───────┬─────────────────────────────────────────────────────────────────┐
 │ [0]   │ ─► BOOT_L2 (shared by pointer)  first GiB in 2 MiB blocks      │  copied from BOOT_L1:
 │ [1]   │ RAM / ARM-local block           (as in BOOT_L1)                │  EL1-only, UXN, global
 │ [2]   │ RAM block (virt)                (as in BOOT_L1)                │
 │ ...   │ invalid                                                         │
 │ [256] │ ─► private L2 ─► L3 tables      user window, 4 KiB pages       │  EL0, nG, PXN
 │ ...   │ invalid                                                         │
 └───────┴─────────────────────────────────────────────────────────────────┘

 user window (VA 0x40_0000_0000 .. 0x41_0000_0000, 1 GiB)
 0x40_0000_0000  ┌──────────────────────────┐
                 │ unmapped (1 MiB)         │  catches small bad pointers
 0x40_0010_0000  ├──────────────────────────┤
                 │ text        R-X          │  PT_LOAD segments, W^X enforced
                 │ rodata      R--          │
                 │ data, bss   RW-          │
 brk_start       ├──────────────────────────┤
                 │ heap        RW-   ▲ sbrk │
                 │            ...           │
 HEAP_LIMIT      ├──────────────────────────┤
                 │ guard page (unmapped)    │
 STACK_BOTTOM    ├──────────────────────────┤
                 │ stack 64 KiB RW-  ▼      │  argv strings, argc/argv/envp/auxv
 0x41_0000_0000  └──────────────────────────┘  (= STACK_TOP = USER_END)
```

* The kernel stays identity-mapped in the low VA range, in every address
  space, through the shared tables. AP bits make it inaccessible from EL0
  and UXN makes it non-executable there (ARMv8 would otherwise allow EL0 to
  execute EL1-only memory).
* User pages are not global, so their TLB entries carry the process's
  8-bit ASID. A switch between processes is one `msr ttbr0_el1` in
  `schedule()` (only when the next thread's value differs from the core's
  current one); no TLB flush.
* Unmapping a page (`sbrk` shrink) does `tlbi vae1is`; dropping an address
  space does `tlbi aside1is` before any frame is freed, and again when the
  ASID is released, so no core keeps a stale translation. Both are
  broadcast to all four cores.
* **ASID exhaustion** is handled the simple way: 255 ASIDs (1-255), one per
  live process. When all are in use `spawn` fails with `EAGAIN`; the
  process table is capped at the same number. No generation rollover.
* **Frames** come from `mem::frame`: all RAM from the end of the kernel
  image (boot stacks and heap included) to `memory_map::RAM_END`
  (virt: 512 MiB, the `-m` the scripts pass; Pi: 448 MiB, below the GPU
  carve-out of a Pi Zero 2 W). A free list threaded through the free frames,
  plus a bump pointer for frames never handed out. Frames are zeroed on
  allocation. The page tables and every user page come from here.
* Code pages are cleaned to the point of unification and the I-caches of
  all cores invalidated (`ic ialluis`) before the program runs.
* **No PAN on the Cortex-A53.** The kernel never dereferences a user
  pointer. `copy_from_user` / `copy_to_user` check the whole range against
  the process's VMAs (and write permission), then translate page by page
  through the process's own tables and copy through the kernel's identity
  map. A bad pointer gives `EFAULT`, never a kernel fault.

### EL0 ↔ EL1

```text
 process thread (EL1, its own kernel stack)
   loop {
     run_user(&mut frame) ──────────────────────────────┐
       mask IRQs, sanitise SPSR (EL0t, DAIF clear)      │
       __pt_run_user: push KCTX {&frame, FPCR,          │
                      x19-x30, d8-d15} ─► SP_EL1        │
       restore user regs from frame, eret ───────────►  EL0: user code runs
                                                        │
       ┌──── sync exception (svc, abort, udf, ...) ◄────┤
       │     vector 0x400: save x0-x30, ELR, SPSR,      │
       │     SP_EL0, FPCR, FPSR, q0-q31 into *frame     │
       │     (address found in KCTX at SP_EL1),         │
       │     pop KCTX, return from __pt_run_user        │
       │                                                │
       │     IRQ ◄──────────────────────────────────────┤
       │     vector 0x480: save user state into *frame  │
       │     IRQ stack: dispatch (timer, IPI)           │
       │     irq_exit ─► schedule()   (preemption,      │
       │                  may resume on another core)   │
       │     asked to leave (kill)? ── no ─► eret to EL0 ┘
       │                         └─ yes ─► return Interrupted
       ▼
     read ESR/FAR, unmask IRQs, return UserExit
     classify: syscall ─► dispatch, x0 = result
               wfi/wfe  ─► yield
               fault    ─► "[pid N] <reason> at <addr>", status -signal
   }
```

* User code always runs with IRQs unmasked, so a loop that never makes a
  system call is still preempted by the timer like any thread. EL0 cannot
  mask interrupts (SCTLR_EL1.UMA = 0), touch the timer (CNTKCTL_EL1 allows
  only counter reads), or use WFI/WFE or cache maintenance.
* SP_EL0 and TPIDR_EL0 are per thread: SP_EL0 is in the trap frame,
  and both are saved and restored in the switch frame, so a process
  switched out in the kernel gets its own values back.
* System calls and faults are handled by ordinary Rust code on the
  process's kernel thread with IRQs enabled, so they can block (sleep,
  wait) and be preempted.
* A fault taken at EL1 is still fatal (register dump and halt); only EL0
  faults kill a process.

### System calls

`svc #0`, number in x8, arguments in x0-x5, result in x0, negative errno on
failure. The table is in `os/src/syscall.rs`:

| nr | call | nr | call | nr | call |
|----|------|----|------|----|------|
| 0 | `exit(code)` | 9 | `getpid()` | 18 | `setfg(pgid)` |
| 1 | `write(fd, buf, len)` | 10 | `sleep(ns)` | 19 | `sysinfo(buf)` |
| 2 | `read(fd, buf, len)` | 11 | `yield()` | 20 | `unlink(path)` |
| 3 | `open(path, flags)` | 12 | `sbrk(incr)` | 21 | `mkdir(path)` |
| 4 | `close(fd)` | 13 | `pipe(fds)` | 22 | `chdir(path)` |
| 5 | `readdir(fd, buf, len)` | 14 | `dup2(old, new)` | 23 | `getcwd(buf, len)` |
| 6 | `spawn(path, argv, attr)` | 15 | `uptime()` | 24 | `stat(path, buf)` |
| 7 | `wait(pid, status, flags)` | 16 | `procinfo(buf, max)` | 25 | `ioctl(fd, req, arg)` |
| 8 | `kill(pid)` | 17 | `cpu()` | | |

* `open` flags: read 1, write 2, create 4, truncate 8, append 16.
* `spawn` takes a path (a bare name is looked up in `/bin`), a
  NULL-terminated argv and an optional 80-byte attribute block: a list of
  (child fd, parent fd) pairs that replaces the inherited fd table, and a
  process group (0 = a new group led by the child). Without it the child
  inherits every fd. The shell always passes the mapping, so pipe ends it
  holds do not leak into children (which would keep readers from ever
  seeing end of file).
* `wait` flag 1 = do not block (returns 0 if no child has exited yet).
* `procinfo` fills 48-byte entries (pid, parent, group, state `R`/`Q`/`S`/`Z`,
  core, mask of cores used, CPU time, mapped pages, open fds, name);
  `sysinfo` gives uptime, free frames and per-core ticks, idle ticks and
  switches. `ps` and `top` are built on them.
* `ioctl`: 1 = console raw mode on/off, 2 = bytes readable without
  blocking, 3 = is this fd the console.

### File system and file descriptors

```text
  process fd table          open files (shared by dup2 / spawn)        objects
  ┌────┐
  │ 0  │──────┐            ┌──────────────────────────┐
  │ 1  │──────┼──────────► │ File { Console }         │ ─────────► line discipline
  │ 2  │──────┘            └──────────────────────────┘
  │ 3  │─────────────────► │ File { Inode, pos, mode }│ ─────────► Inode::File(bytes) in the ramfs
  │ 4  │─────────────────► │ File { PipeRead }        │ ──┐
  └────┘                   │ File { PipeWrite }       │ ──┴──────► Pipe (4 KiB ring, 2 wait queues)
```

* The **ramfs** (`os/src/vfs.rs`) is a tree of `Inode`s: files (a byte
  vector), directories (a sorted map) and devices (`/dev/console`,
  `/dev/null`), each behind a blocking mutex. It is writable: `open` with
  create/truncate/append, `mkdir`, `unlink`, `readdir`, `stat`. Paths are
  absolute or relative to the process's working directory; `.` and `..` are
  resolved lexically.
* The **initrd** is a cpio "newc" archive that `os/build.rs` generates at
  build time from the user programs (`/bin/*`) and `user/rootfs/`
  (`/etc/motd`, `/tmp`), embedded with `include_bytes!` and unpacked into
  the ramfs at boot (`os/src/initrd.rs`). `spawn` loads programs from the
  ramfs, so a program written to `/tmp` can be run too.
* An **fd table** is a vector of `Arc<File>`. A `File` is an open file
  with its offset and access mode; `dup2` and inheritance across `spawn`
  share it (and so the offset), as in POSIX. A process's table is emptied
  when it exits, whatever the reason, which is what closes its pipe ends.
* System calls never hold a user pointer across a blocking operation: data
  moves through a kernel bounce buffer (4 KiB per step) with the checked
  copies above.

### Pipes

A pipe is a 4 KiB ring buffer with reader and writer counts and two wait
queues. The counts follow the `File`s, not the fds: the last `File` for an
end going away (every fd closed, every holder exited) wakes the other side.

* `read` blocks until there is data or no writer is left; then it returns
  what is there, or 0 (end of file).
* `write` blocks while the ring is full and returns once everything is
  written; with no reader left it fails with `EPIPE` (or returns the
  partial count). `ulib`'s print macros exit quietly with status 141 on
  `EPIPE`, like a Unix process killed by SIGPIPE.
* Both waits are interruptible: killing a process blocked on a pipe takes
  effect at once.

### Console and line discipline

```text
 PL011 RX interrupt (core 0) ──► 256-byte ring ──► wake ─┐
 console::inject() (tests) ──────────────────────────────┤
                                                         ▼
 "console" kernel thread (high priority): line discipline
    canonical: echo; backspace/DEL erase; Enter completes the line (CR, LF
               or CR LF); Ctrl-U erases the line; Ctrl-D: end of file on an
               empty line, otherwise hands over the partial line
    raw:       bytes go through unchanged, no echo (top uses it)
    both:      Ctrl-C kills the foreground process group (status -2); with
               no foreground job it abandons the line and hands the reader
               an empty one, so the shell prints a fresh prompt
       │
       ▼ input queue (bytes and end-of-file markers)
 read(0): blocks interruptibly; a canonical read returns at most one line
```

The IRQ handler only queues bytes: echo (which takes the console lock) and
killing processes (kernel locks, freeing memory) happen on the console
thread. The shell gives every job its own process group and makes it the
foreground group with `setfg` while it waits for it; setting the foreground
to 0 also puts the console back into line mode, in case a job left it raw.
Output to fd 1/2 is written to the PL011 under the console lock, one
`write` at a time, so lines from different processes do not interleave.

### Boot flow

```text
 kernel_main ─► pt::start (4 cores) ─► main thread:
   unpack the initrd into the ramfs ─► start the console thread, enable the
   UART interrupt ─► print /etc/motd ─► loop { spawn /bin/sh on the console
   (fds 0-2), wait for it, report its status, start a new one }
```

### Userland

`user/ulib` gives programs `_start` (argc/argv into `main(Args) -> i32`
through `entry!`), the system calls, `print!`/`println!`/`eprintln!`
(formatted on the stack, one `write` per line), a global allocator
(`linked_list_allocator` growing with `sbrk`, so `Vec`/`String`/`format!`
work), `File`, `LineReader`, `read_dir`, `stat`, `SpawnAttr`, `procinfo`
and `sysinfo`. Processes are single threaded, so the allocator needs no lock.

`/bin`: `sh` (quotes, `|` pipelines of any length, `<`, `>`, `>>`, `&` with
`[pid N] started on cpuX`, `jobs`, `wait`, `kill`, `cd`, `pwd`, `exit`,
`help`; reports `[pid N] segfault` / `killed` / `interrupted`), `ls`,
`cat`, `echo`, `wc`, `grep`, `ps`, `top` (per-core load and which process
runs where; any key or `-n N` ends it), `hello`, `primes N` (sieve),
`mandel` (4 worker processes joined through pipes), `stress N` (spawn/exit
loop, checks the free frame count comes back), `crash`, `spin`, `sleep`,
`rm`, `mkdir`.

### Process lifecycle

```text
 spawn(name or ELF, argv)
   ASID + L1/L2 tables ─► load PT_LOAD segments (W^X) ─► map stack, copy argv
   ─► table entry (pid, parent, Running) ─► kernel thread (32 KiB stack)
        │
        ▼
 Running: thread sets TTBR0, loops in run_user / dispatch
        │ exit(code) │ fault: status -signal │ kill: status -9
        ▼
 exit: TTBR0 back to the kernel tables ─► drop the Mm: tlbi aside1is,
       every user frame + page tables back to the frame allocator, ASID freed
       ─► close every fd (pipe peers see EOF / EPIPE)
       ─► children reparented to the kernel (auto-reaped) ─► Zombie(status)
       ─► wake waiters ─► thread exits
        │
        ▼
 wait(pid) by the parent: removes the zombie, returns (pid, status)
```

`kill` sets a flag and calls `Thread::interrupt`. If the process is
running user code on some core, that core gets a reschedule IPI and the
EL0 IRQ path returns from `run_user` instead of resuming it (measured at
under 100 µs in QEMU). If it is blocked in the kernel (console or pipe
I/O, `wait`, `sleep`), its interruptible wait returns `EINTR` at once (see
§4), the system call returns, and the run loop sees the flag before going
back to user mode. Exit status is the `exit` code, or `-signal`: -9
killed, -2 Ctrl-C, -11 segfault, -4 illegal instruction, -7 alignment,
-5 breakpoint, -8 FP.

### Tests

`os/examples/os_test.rs` (`make test-os`) unpacks the initrd plus the test
programs from `user/tests/` (in `/test`) and runs 21 checks: exit codes
through `wait`, writes to kernel memory, executing kernel code and
undefined instructions killing only the process, isolation (another
process's heap address faults; the same VA maps to different frames and
ASIDs), `sbrk` growth and shrink, a syscall-free busy loop being preempted
and killed, TPIDR_EL0 and FP state across preemption, 32 processes over all
4 cores, rejected ELF images, ASID exhaustion; then the file system
(create/write/append/truncate, directories, `readdir`, relative paths,
error cases), fd inheritance with shared offsets and `dup2`, pipe EOF and
`EPIPE` (including a writer blocked on a full pipe), `EINTR`: a kernel
thread's pipe read interrupted, and processes killed while blocked in
console read, pipe read, pipe write and sleep, `procinfo` through `ps`,
the line discipline (bytes injected into the same queue the UART interrupt
fills), Ctrl-C, a whole shell session, and finally the frame and ASID
counts back at their baseline.

`scripts/shell-session.py` (`make test-shell`) boots the real pt-os image
with QEMU's serial port on a pipe and types a session into it a character
at a time, through the PL011 receive interrupt: `echo hi | wc`, `ls /bin`,
`primes 100000 &` and `wait`, `cat /etc/motd > /tmp/x` and `cat /tmp/x`,
`>>`, `<`, a 3-stage pipeline, `crash` (the shell survives), and Ctrl-C on
a running `spin`. It runs on virt, virt-el2 and raspi3b in CI.
