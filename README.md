# preemptive-threads

Bare metal preemptive multithreading for Raspberry Pi Zero 2 W. No OS, just Rust.

## What it does

Runs multiple threads on bare metal ARM64. Timer interrupts switch between them every 1ms.

```
[Thread 1] Started!
[Thread 2] Started!
[Thread 3] Started!
[Thread 1] counter = 5000000
[Thread 2] counter = 5000000
[Thread 3] counter = 5000000
```

## Quick start

```bash
# Install tools
rustup toolchain install nightly
rustup target add aarch64-unknown-none
brew install qemu  # or apt install qemu-system-arm

# Test in QEMU
make test-virt
```

## Run on real hardware

```bash
# Build
cargo +nightly build --release --example rpi_kernel --target aarch64-unknown-none
rust-objcopy -O binary target/aarch64-unknown-none/release/examples/rpi_kernel kernel8.img

# Copy to FAT32 SD card:
# - kernel8.img
# - bootcode.bin, start.elf, fixup.dat (from RPi firmware repo)
# - config.txt with: arm_64bit=1 and kernel=kernel8.img

# Wire serial: GPIO14->RX, GPIO15->TX, GND->GND
# Connect: screen /dev/tty.usbserial* 115200
```

## How it works

The tricky part: context switching from an IRQ handler. ARM64's `eret` restores registers from the exception frame, undoing any switch you did.

Solution: save/restore directly to thread context structs instead of the stack.

```
IRQ fires
  -> save regs to current thread's context
  -> scheduler picks next thread
  -> load regs from next thread's context
  -> eret returns to new thread
```

## Status

Working:
- Preemptive context switching
- Timer-based preemption (1ms)
- Priority scheduler
- Round-robin for equal priorities
- UART output

Not done:
- Multi-core (uses 1 of 4 cores)
- Memory protection
- Thread join/exit

## Files

```
src/
  kernel.rs          - spawn, start, preemption
  arch/
    aarch64.rs       - context switching
    aarch64_vectors.rs - IRQ handlers
  sched/rr.rs        - round-robin scheduler

examples/
  rpi_kernel.rs      - example kernel
```

## License

MIT OR Apache-2.0
