# Architecture

This document describes how preemptive-threads gets from reset to four cores
switching threads: the boot path, the memory map, the context layout, the
interrupt flow and the SMP scheduler.

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
 │   FPCR, FPSR, q0-q31         │
 ├──────────────────────────────┤
 │ irq_exit / schedule frames   │
 ├──────────────────────────────┤
 │ SwitchFrame (176 B)          │  pushed by __pt_switch
 │   x19-x28, x29, x30,         │
 │   d8-d15, DAIF, FPCR|FPSR    │
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
      virt: loop { IAR ─► handle ─► EOIR }   (GICv2; INTID 30 = timer, SGI 1 = resched)
      Pi:   read COREn_IRQ_SOURCE            (bit 1 = CNTPNSIRQ, bit 4 = mailbox 0)
      timer:  re-arm CNTP_CVAL (next tick), wake expired sleepers,
              charge the time slice, maybe set need_resched
      resched IPI: set need_resched
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
