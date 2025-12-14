# preemptive-threads

Run multiple tasks simultaneously on a Raspberry Pi Zero 2 W — without Linux.

## What Is This?

A tiny kernel that lets you run multiple "threads" on bare metal. The CPU rapidly switches between tasks (1000 times/second), making them appear to run at the same time.

**Perfect for:**
- Learning how operating systems work
- Embedded projects that need multitasking
- Robotics where you need motor control + sensors + communication running together

## Quick Example

```rust
use preemptive_threads::{Kernel, uart_println};

// Create threads that run "simultaneously"
KERNEL.spawn(|| {
    loop {
        uart_println!("Thread 1 says hi!");
        // Do some work...
    }
}, 128);

KERNEL.spawn(|| {
    loop {
        uart_println!("Thread 2 says hi!");
        // Do other work...
    }
}, 128);

// Start running - threads will alternate automatically
KERNEL.start_first_thread();
```

Output:
```
Thread 1 says hi!
Thread 2 says hi!
Thread 1 says hi!
Thread 2 says hi!
...
```

## Getting Started

### 1. Install Tools

```bash
rustup target add aarch64-unknown-none
cargo install cargo-binutils
rustup component add llvm-tools
```

### 2. Build

```bash
cargo build --release --example rpi_kernel --target aarch64-unknown-none
rust-objcopy -O binary target/aarch64-unknown-none/release/examples/rpi_kernel kernel8.img
```

### 3. Test in Emulator (Optional)

```bash
brew install qemu  # or apt install qemu-system-arm on Linux
qemu-system-aarch64 -M raspi3b -kernel kernel8.img -serial stdio -display none
```

### 4. Run on Real Hardware

**You need:**
- Raspberry Pi Zero 2 W
- MicroSD card (any size)
- USB-to-serial adapter (to see output)

**Steps:**
1. Format SD card as FAT32
2. Copy `kernel8.img` to the card
3. Download [Raspberry Pi firmware](https://github.com/raspberrypi/firmware/tree/master/boot) and copy `bootcode.bin`, `start.elf`, `fixup.dat`
4. Create `config.txt`:
   ```
   arm_64bit=1
   kernel=kernel8.img
   ```
5. Wire up serial: GPIO14→RX, GPIO15→TX, GND→GND
6. Open terminal: `screen /dev/tty.usbserial* 115200`
7. Power on!

## Features

| Feature | What It Does |
|---------|--------------|
| Preemptive scheduling | Threads switch automatically (no manual yielding needed) |
| Priority levels | Important threads run first |
| UART output | Print debug messages over serial |
| NEON/FPU support | Floating-point math works in threads |

## Status

**Alpha** - Works, but not battle-tested. Good for learning and experiments.

- ✅ Context switching
- ✅ Timer-based preemption
- ✅ Thread spawning
- ✅ Priority scheduler
- ✅ UART debug output
- ❌ Multi-core (uses 1 of 4 cores)
- ❌ Memory protection
- ❌ Filesystem

## API Reference

```rust
// Create kernel
static KERNEL: Lazy<Kernel<DefaultArch, RoundRobinScheduler>> =
    Lazy::new(|| Kernel::new(RoundRobinScheduler::new(1)));

// Initialize
KERNEL.init().unwrap();

// Spawn thread (closure + priority 0-255)
KERNEL.spawn(|| { /* code */ }, 128).unwrap();

// Start scheduler
KERNEL.start_first_thread();

// Yield current thread voluntarily
preemptive_threads::yield_now();

// Print to serial
uart_println!("Hello {}", name);
```

## Project Structure

```
src/
├── kernel.rs      # Main API (spawn, start)
├── arch/          # Hardware drivers (ARM64, UART, interrupts)
├── sched/         # Scheduler (decides who runs next)
├── thread_new/    # Thread management
├── mem/           # Stack allocation
└── time/          # Time utilities

examples/
└── rpi_kernel.rs  # Example kernel you can modify
```

## Limitations

- **Single core only** - Uses 1 of 4 CPU cores
- **No memory protection** - Threads can crash each other
- **No deallocation** - Uses simple bump allocator
- **Race conditions possible** - You must handle synchronization

## License

MIT OR Apache-2.0

## Contributing

Issues and PRs welcome! This is a learning project.
