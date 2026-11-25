# preemptive-threads

Preemptive, SMP threading for bare-metal AArch64, in Rust, with no operating
system underneath. Targets the **Raspberry Pi Zero 2 W** (BCM2837, 4x
Cortex-A53) and runs on QEMU's `raspi3b` and `virt` machines.

The crate owns everything from the first instruction: exception-level drop,
MMU, exception vectors, interrupt controllers, secondary-core bring-up, heap,
scheduler and synchronisation primitives. An application provides one
function and gets threads that are preempted by the timer and spread across
all four cores.

```rust
#![no_std]
#![no_main]
extern crate alloc;
use alloc::vec::Vec;
use preemptive_threads::{self as pt, println, sync::Mutex, thread, Config};

static TOTAL: Mutex<u64> = Mutex::new(0);

#[no_mangle]
fn kernel_main() -> ! {
    pt::start(Config::default(), main)
}

fn main() {
    let workers: Vec<_> = (0..8u64)
        .map(|i| thread::spawn(move || { *TOTAL.lock() += i; i * i }))
        .collect();
    let squares: u64 = workers.into_iter().map(|h| h.join()).sum();
    println!("sum of squares {} total {}", squares, *TOTAL.lock());
}
```

## Status

| What | Status |
|------|--------|
| QEMU `virt`, entered at EL1 (PSCI over HVC) | all 16 tests pass, 4 cores, in CI |
| QEMU `virt,virtualization=on`, entered at EL2 (EL2 to EL1 drop, PSCI over SMC) | all 16 tests pass, 4 cores, in CI |
| QEMU `raspi3b`, flat `kernel8.img` released through the spin-table stub at EL2, like the Pi firmware | all 16 tests pass, 4 cores, in CI |
| Real Raspberry Pi Zero 2 W | **not yet verified on hardware.** The image builds and the boot path mirrors the firmware's (EL2 entry, spin table, ARM-local interrupt controller, PL011 on GPIO14/15). See [Running on a real Pi](#running-on-a-real-pi-zero-2-w) |

## Features

- **Preemption on all four cores.** Each core's generic timer ticks at
  1 kHz (configurable). Round-robin time slices (10 ticks by default) or
  FIFO within a priority band.
- **Scheduler.** Per-core run queues with four priority bands (idle, low,
  normal, high; strict priority between bands), work stealing by idle
  cores, reschedule IPIs (GIC SGIs on `virt`, ARM-local mailboxes on the
  Pi) so a higher-priority thread made ready on another core runs
  immediately, CPU affinity, per-core idle threads.
- **One context-switch path.** Yield, block, exit and IRQ preemption all go
  through the same switch routine. IRQ entry saves x0-x30, ELR, SPSR,
  FPCR/FPSR and all 32 SIMD registers; the switch frame saves the
  callee-saved x19-x30, d8-d15, FPCR/FPSR and DAIF. See
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- **Blocking synchronisation.** `Mutex` with priority inheritance and
  direct hand-off, `Semaphore`, `Condvar`, bounded MPMC `channel`, wait
  queues, `thread::sleep` on a deadline queue, and `JoinHandle::join`
  returning the thread's value.
- **Memory.** A real heap (`linked_list_allocator` over a 64 MiB
  linker-reserved region, IRQ-safe) installed as the global allocator,
  pooled thread stacks with overflow canaries, finished threads reclaimed
  automatically.
- **Hardware bring-up.** EL3/EL2 to EL1 drop (sets HCR_EL2, CNTHCTL_EL2,
  CNTVOFF_EL2, CPTR_EL2, VPIDR/VMPIDR_EL2, SCTLR_EL1). Identity-mapped MMU
  with caches on every core, which is what makes atomics (load/store
  exclusive) work on the BCM2837. GICv2 or BCM2836 ARM-local interrupt
  controller, PSCI `CPU_ON` or spin-table release, PL011 UART at 115200
  baud (IBRD 26 / FBRD 3 at the Pi's 48 MHz UART clock).
- **Crash reports.** Unexpected exceptions print ESR/ELR/FAR/SPSR with a
  decoded exception class and all registers, then halt; panics print the
  message and stop the other cores.

## Quick start

Requirements: Rust 1.88 or newer with the `aarch64-unknown-none` target and
`llvm-tools`, plus QEMU 8 or newer.

```bash
rustup target add aarch64-unknown-none
rustup component add llvm-tools
brew install qemu            # or: sudo apt install qemu-system-arm

make test                    # test kernel on virt, virt-el2 and raspi3b
make test-virt               # just one machine
make run-raspi3b             # demo kernel on QEMU raspi3b (Ctrl-C to quit)
make kernel8.img             # demo image for a real Pi
```

The test kernel (`examples/test_kernel.rs`) exits QEMU through semihosting,
so the exit status is the verdict. Hangs are killed by a watchdog in
`scripts/qemu-run.sh`, and a watchdog thread inside the kernel dumps the
scheduler state if a single test stalls. Output on `raspi3b`:

```text
preemptive-threads test kernel (Raspberry Pi (BCM2837), entered at EL2)
[TEST] smp_boot ... ok (4 cores online; 0 ms)
[TEST] join_returns_value ... ok (16 joins, sum=1240; 1 ms)
[TEST] preemption ... ok (cpu1 preemptions +1; 12 ms)
[TEST] all_threads_complete ... ok (64/64 threads completed; 1 ms)
[TEST] mutex_no_lost_updates ... ok (8 threads x 5000 increments = 40000; 146 ms)
[TEST] channel_ordering ... ok (3000 messages in per-producer order; 7 ms)
[TEST] channel_disconnect ... ok (disconnect reported on both ends; 6 ms)
[TEST] sleep_accuracy ... ok (5ms->5949us 20ms->21176us 50ms->50333us order [10, 20, 30, 40]; 119 ms)
[TEST] all_cores_ran_threads ... ok (cores used mask=0b1111; 65 ms)
[TEST] priority_inheritance ... ok (L boosted 32->224 while H waited; 30 ms)
[TEST] priority_bands ... ok (HIGH preempted NORMAL after 96 us; 5 ms)
[TEST] fifo_policy ... ok (order [0, 1, 10, 11, 20, 21]; 80 ms)
[TEST] semaphore_limits_concurrency ... ok (max concurrent holders 2; 0 ms)
[TEST] condvar_producer_consumer ... ok (2000 items, sum 1999000; 4 ms)
[TEST] fpu_state_preserved ... ok (11013 FP rounds bit-exact across preemptions; 85 ms)
[TEST] threads_reaped ... ok (450 spawn/join cycles, live=1 stacks_in_use=1 heap 2165072B -> 2165072B; 22 ms)
[STATS] cpus=4 switches=53704 preemptions=38 steals=36441 spawned=582 reaped=581 heap_used=2165072B
ALL 16 TESTS PASSED on 4 cores
```

| Test | What it proves |
|------|----------------|
| `smp_boot` | all four cores came online |
| `join_returns_value` | `join()` returns the closure's value (integers and heap types) |
| `preemption` | two non-yielding threads pinned to one core both make progress |
| `all_threads_complete` | 64 threads across two priority bands all finish |
| `mutex_no_lost_updates` | 8 threads x 5000 read-modify-writes under preemption lose nothing |
| `channel_ordering` / `channel_disconnect` | per-producer FIFO order through a bounded channel; disconnect is reported |
| `sleep_accuracy` | sleeps last at least the requested time (and less than +40 ms); sleepers wake in deadline order |
| `all_cores_ran_threads` | unpinned threads run on all four cores |
| `priority_inheritance` | a low-priority mutex owner is boosted past a CPU-hogging medium thread so the high-priority waiter gets the lock |
| `priority_bands` | a high-priority thread preempts a running normal one via IPI without waiting for a time slice |
| `fifo_policy` | under FIFO, equal-priority threads run to completion in arrival order |
| `semaphore_limits_concurrency` | a 2-permit semaphore never admits a third thread |
| `condvar_producer_consumer` | 2000 items pass through a `Mutex` + `Condvar` queue in order |
| `fpu_state_preserved` | floating-point results stay bit-exact across preemptions |
| `threads_reaped` | 450 spawn/join cycles leak no threads, stacks or heap |

## Running on a real Pi Zero 2 W

What you need: a Pi Zero 2 W, a micro-SD card, and (optionally, but needed
to see output) a 3.3 V USB-to-serial adapter.

1. Build the image:

   ```bash
   make kernel8.img
   ```

2. Format the SD card with one FAT32 partition. From the official firmware
   repository (<https://github.com/raspberrypi/firmware>, `boot/` folder),
   copy `bootcode.bin`, `start.elf` and `fixup.dat` to it, plus your
   `kernel8.img`.

3. Create `config.txt` on the card:

   ```ini
   arm_64bit=1
   kernel=kernel8.img
   enable_uart=1
   init_uart_clock=48000000
   dtoverlay=disable-bt
   ```

   The firmware then starts the kernel at EL2 at 0x80000 and parks cores
   1-3 on the spin table, exactly as `make test-raspi3b` emulates.

4. Wire the serial adapter: adapter RX to GPIO14 (pin 8), adapter TX to
   GPIO15 (pin 10), GND to GND (pin 6). Do not connect the adapter's 5 V
   or 3.3 V pin.

5. Open the console at 115200 8N1 (`screen /dev/tty.usbserial-* 115200` on
   macOS, `screen /dev/ttyUSB0 115200` on Linux) and power the Pi.

You should see the banner with `entered at EL2`, `[main] 4 cores online`, then
worker lines every two seconds with `cores used so far 0b....` and a
`[monitor]` line every second whose switch and preemption counters keep
growing. Even without a serial cable, the green ACT LED blinks at 2 Hz once
the scheduler is running.

If nothing happens:

- LED never blinks and nothing on serial: check the card is FAT32, the file
  is called `kernel8.img`, and `arm_64bit=1` is set.
- Garbage on serial: the UART clock is not 48 MHz. Keep
  `init_uart_clock=48000000` in `config.txt`.
- Fewer than 4 cores online: the firmware did not use the spin table.
  Make sure there is no `armstub=` line in `config.txt`.

## Configuration

```rust
pt::start(
    Config { policy: SchedPolicy::RoundRobin, tick_hz: 1000, time_slice_ticks: 10, cpus: 4 },
    main,
);
```

Threads take a name, priority (0-255, higher runs first), stack size and CPU
affinity through `thread::Builder`. Cargo features:

| feature | effect |
|---------|--------|
| (none) | BCM2837 boards: Pi Zero 2 W, Pi 3, QEMU `raspi3b` |
| `qemu-virt` | QEMU `virt` (GICv2, PSCI, UART at 0x0900_0000) |
| `semihosting` | lets `arch::semihosting::exit` stop QEMU; only for emulator builds |

The crate's build script links its examples with `rpi0w2.ld` (or
`qemu_virt.ld`). A kernel in another crate needs `-C link-arg=-T<script>`
with one of those scripts.

## Limitations

- Everything runs at EL1 in one identity-mapped address space. There is no
  user mode or memory protection between threads.
- A panic in any thread stops the whole system (`panic = "abort"`).
- Priority inheritance is one level deep: it is not propagated along chains
  of mutexes. Boosts are exact when nested mutexes unlock in reverse order.
- The timer ticks periodically (1 kHz by default), so sleeps have tick
  granularity. It is not tickless.
- Drivers cover only what the scheduler needs: timer, interrupt controller,
  UART transmit and the ACT LED in the demo. There is no GPU, USB, SD or
  network support.
- At most 4 cores. The heap is a fixed 64 MiB region set in the linker
  scripts.

## Layout

```text
src/
  arch/        boot.rs (EL drop, MMU, SMP entry), vectors.rs (trap frames, crash dumps),
               context.rs (switch), mmu.rs, timer.rs, semihosting.rs
  platform/    rpi.rs + bcm2836.rs (ARM-local IRQs, spin table), virt.rs + gic.rs + psci.rs, pl011.rs
  sched/       mod.rs (switch logic, wake, PI hooks, ticks), percpu.rs, runqueue.rs, sleep.rs
  sync/        spin.rs, waitqueue.rs, mutex.rs, semaphore.rs, condvar.rs, channel.rs
  mem/         heap.rs, stack.rs
  thread.rs    spawn / Builder / JoinHandle / sleep / yield
  kernel.rs    start(), boot entry points, IRQ dispatch, stats
examples/
  test_kernel.rs   self-checking test suite (make test-*)
  rpi_kernel.rs    demo kernel (make kernel8.img)
docs/ARCHITECTURE.md
```

## License

MIT, see [LICENSE-MIT](LICENSE-MIT).
