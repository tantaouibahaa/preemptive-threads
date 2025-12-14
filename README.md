# Preemptive Threads for Raspberry Pi Zero 2 W

A bare-metal preemptive multithreading kernel for the Raspberry Pi Zero 2 W.

## Target Platform

| | |
|---|---|
| **Hardware** | Raspberry Pi Zero 2 W |
| **SoC** | Broadcom BCM2837 |
| **CPU** | ARM Cortex-A53 (quad-core, 64-bit) |
| **Architecture** | AArch64 / ARMv8-A |
| **Environment** | Bare-metal (no operating system) |

## Status

**Alpha** - Core functionality implemented, not yet tested on real hardware.

### Implemented

- Context switching (ARM64 with full register save/restore)
- NEON/FPU state save/restore
- ARM Generic Timer for preemption
- GIC-400 interrupt controller driver
- Thread spawning with closure support
- Round-robin scheduler with priority levels
- Stack pool allocator
- JoinHandle for thread synchronization

### Not Yet Implemented

- Multi-core support (currently single-core only)
- MMU / virtual memory
- UART output for debugging
- Real hardware testing
- Thread-local storage

## Quick Start

### Prerequisites

```bash
# Install Rust (nightly recommended for bare-metal)
rustup install nightly
rustup default nightly

# Add the bare-metal ARM64 target
rustup target add aarch64-unknown-none

# Install rust-src for build-std (optional, for optimized builds)
rustup component add rust-src

# Install objcopy for creating kernel images
cargo install cargo-binutils
rustup component add llvm-tools
```

### Build

```bash
# Build the library
cargo build --release --target aarch64-unknown-none

# Build the example kernel
cargo build --release --example rpi_kernel --target aarch64-unknown-none
```

### Deploy to Raspberry Pi

1. **Convert ELF to raw binary:**
   ```bash
   rust-objcopy -O binary \
       target/aarch64-unknown-none/release/examples/rpi_kernel \
       kernel8.img
   ```

2. **Prepare SD card:**
   - Format a microSD card with FAT32
   - Copy `kernel8.img` to the root
   - Download Raspberry Pi firmware files from [raspberrypi/firmware](https://github.com/raspberrypi/firmware/tree/master/boot):
     - `bootcode.bin`
     - `start.elf`
     - `fixup.dat`
   - Create `config.txt` with:
     ```
     arm_64bit=1
     kernel=kernel8.img
     ```

3. **Boot:**
   - Insert SD card into Pi Zero 2 W
   - Connect power
   - The kernel will start running threads

## Usage

```rust
#![no_std]
#![no_main]

extern crate alloc;

use preemptive_threads::{
    Kernel,
    arch::{Arch, DefaultArch},
    sched::RoundRobinScheduler,
};
use spin::Lazy;

// Use Lazy for runtime initialization
static KERNEL: Lazy<Kernel<DefaultArch, RoundRobinScheduler>> =
    Lazy::new(|| Kernel::new(RoundRobinScheduler::new(1)));

#[no_mangle]
pub fn kernel_main() -> ! {
    KERNEL.init().unwrap();

    unsafe { KERNEL.register_global(); }

    // Spawn threads with closures
    KERNEL.spawn(|| {
        loop {
            // Thread 1 work
            preemptive_threads::yield_now();
        }
    }, 128).unwrap();

    KERNEL.spawn(|| {
        loop {
            // Thread 2 work
            preemptive_threads::yield_now();
        }
    }, 128).unwrap();

    // Enable timer preemption (1ms time slices)
    unsafe {
        preemptive_threads::arch::aarch64::setup_preemption_timer(1000).unwrap();
    }

    // Enable interrupts and start first thread
    DefaultArch::enable_interrupts();
    KERNEL.start_first_thread();

    loop {
        unsafe { core::arch::asm!("wfe"); }
    }
}
```

## Testing

### QEMU Emulation (Development Machine)

You can test on QEMU before deploying to real hardware:

```bash
# Install QEMU
brew install qemu          # macOS
# apt install qemu-system-arm  # Linux

# Build the kernel
cargo build --release --example rpi_kernel --target aarch64-unknown-none

# Convert to binary
rust-objcopy -O binary \
    target/aarch64-unknown-none/release/examples/rpi_kernel \
    kernel8.img

# Run in QEMU (Raspberry Pi 3 machine, closest to Pi Zero 2 W)
qemu-system-aarch64 \
    -M raspi3b \
    -kernel kernel8.img \
    -serial stdio \
    -display none
```

Note: QEMU's Raspberry Pi emulation may differ from real hardware, especially for interrupts and timing.

### Host Unit Tests

For quick iteration without cross-compilation:

```bash
cargo test --features std-shim
```

## Memory Layout

The default linker script (`rpi0w2.ld`) defines:

| Region | Address | Size | Purpose |
|--------|---------|------|---------|
| Kernel code | `0x80000` | Variable | Code and data |
| Vectors | Aligned 2KB | 2 KB | Exception vector table |
| Stack | After BSS | 1 MB | Kernel stack |
| Heap | After stack | 16 MB | Dynamic allocation |

## Project Structure

```
preemptive_threads/
├── src/
│   ├── lib.rs              # Public API exports
│   ├── kernel.rs           # Main kernel API
│   ├── errors.rs           # Error types
│   ├── arch/
│   │   ├── mod.rs          # Arch trait definition
│   │   ├── aarch64.rs      # ARM64 context switch, timer
│   │   ├── aarch64_gic.rs  # GIC-400 interrupt controller
│   │   ├── aarch64_vectors.rs  # Exception handlers
│   │   ├── aarch64_boot.rs # Boot/startup code
│   │   ├── barriers.rs     # Memory barriers
│   │   └── detection.rs    # CPU feature detection
│   ├── sched/
│   │   ├── mod.rs          # Scheduler trait
│   │   └── rr.rs           # Round-robin scheduler
│   ├── mem/
│   │   ├── mod.rs          # Memory exports
│   │   ├── stack_pool.rs   # Stack allocation
│   │   └── arc_lite.rs     # Lightweight Arc
│   ├── thread_new/
│   │   ├── mod.rs          # Thread abstraction
│   │   ├── handle.rs       # JoinHandle
│   │   └── builder.rs      # ThreadBuilder
│   └── time/
│       └── mod.rs          # Time types (Duration, Instant)
├── rpi0w2.ld               # Linker script
├── .cargo/config.toml      # Build configuration
└── examples/
    └── rpi_kernel.rs       # Example kernel
```

## Features

| Feature | Description | Default |
|---------|-------------|---------|
| `full-fpu` | Save/restore NEON/FPU registers on context switch | Yes |
| `std-shim` | Enable std compatibility for host testing | No |

## Hardware Requirements

- Raspberry Pi Zero 2 W
- microSD card (any size, FAT32 formatted)
- 5V power supply (micro USB)
- (Optional) USB-to-serial adapter for UART debugging

## License

MIT OR Apache-2.0

## Contributing

This is an experimental project. Issues and PRs welcome!
