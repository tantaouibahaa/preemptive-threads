//! Preemptive Multithreading Kernel for Raspberry Pi Zero 2 W
//!
//! This example demonstrates true preemptive multithreading on bare metal ARM64.
//! Three threads run concurrently, switched automatically by timer interrupts.
//!
//! # Quick Test (QEMU)
//!
//! ```bash
//! make test-virt
//! ```
//!
//! # Building for Real Hardware
//!
//! ```bash
//! cargo +nightly build --release --example rpi_kernel --target aarch64-unknown-none
//! rust-objcopy -O binary target/aarch64-unknown-none/release/examples/rpi_kernel kernel8.img
//! ```
//!
//! # Deploying to Raspberry Pi Zero 2 W
//!
//! 1. Format SD card as FAT32
//! 2. Copy to SD card:
//!    - `kernel8.img` (your kernel)
//!    - `bootcode.bin`, `start.elf`, `fixup.dat` (from RPi firmware)
//! 3. Create `config.txt`:
//!    ```
//!    arm_64bit=1
//!    kernel=kernel8.img
//!    ```
//! 4. Wire serial: GPIO14→RX, GPIO15→TX, GND→GND
//! 5. Connect: `screen /dev/tty.usbserial* 115200`
//! 6. Power on and watch the threads run!

#![no_std]
#![no_main]

extern crate alloc;

use preemptive_threads::{
    arch::DefaultArch,
    sched::FirstComeFirstServeScheduler,
    pl011_println,
    Kernel,
};
use spin::Lazy;

/// Simple bump allocator for the heap.
mod allocator {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;
    use core::ptr::null_mut;
    use core::sync::atomic::{AtomicUsize, Ordering};

    const HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MB

    #[repr(C, align(16))]
    struct Heap {
        data: UnsafeCell<[u8; HEAP_SIZE]>,
        next: AtomicUsize,
    }

    unsafe impl Sync for Heap {}

    static HEAP: Heap = Heap {
        data: UnsafeCell::new([0; HEAP_SIZE]),
        next: AtomicUsize::new(0),
    };

    pub struct BumpAllocator;

    unsafe impl GlobalAlloc for BumpAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let size = layout.size();
            let align = layout.align();

            loop {
                let current = HEAP.next.load(Ordering::Relaxed);
                let aligned = (current + align - 1) & !(align - 1);
                let new_next = aligned + size;

                if new_next > HEAP_SIZE {
                    return null_mut();
                }

                if HEAP
                    .next
                    .compare_exchange(current, new_next, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    let heap_start = HEAP.data.get() as *mut u8;
                    return heap_start.add(aligned);
                }
            }
        }

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
            // Bump allocator doesn't support deallocation
        }
    }

    #[global_allocator]
    static ALLOCATOR: BumpAllocator = BumpAllocator;
}

/// The kernel instance.
static KERNEL: Lazy<Kernel<DefaultArch, FirstComeFirstServeScheduler>> =
    Lazy::new(|| Kernel::new(FirstComeFirstServeScheduler::new(1)));

/// Kernel entry point - called from boot code after hardware init.
#[no_mangle]
pub fn kernel_main() -> ! {
    // Initialize PL011 UART first so we can see output
    // PL011 works on both real hardware and QEMU raspi3b (-serial stdio)
    unsafe {
        preemptive_threads::arch::uart_pl011::init();
    }

    pl011_println!("");
    pl011_println!("========================================");
    pl011_println!("  Preemptive Threads Kernel v0.6.0");
    pl011_println!("  Raspberry Pi Zero 2 W");
    pl011_println!("========================================");
    pl011_println!("");

    // Initialize the kernel
    pl011_println!("[BOOT] Initializing kernel...");
    KERNEL.init().expect("Failed to initialize kernel");
    pl011_println!("[BOOT] Kernel initialized!");

    // Register kernel globally for interrupt handlers
    unsafe {
        KERNEL.register_global();
    }
    pl011_println!("[BOOT] Kernel registered globally");

    // Spawn Thread 1
    pl011_println!("[BOOT] Spawning threads...");
    KERNEL.spawn(
        || {
            pl011_println!("[Thread 1] Started!");
            let mut counter = 0u64;
            loop {
                counter = counter.wrapping_add(1);
                if counter % 5_000_000 == 0 {
                    pl011_println!("[Thread 1] counter = {}", counter);
                    pl011_println!("Yielding to thread 2");
                    KERNEL.yield_now();

                }
            }
        },
        128,
    )
        .expect("Failed to spawn thread 1");

    // Spawn Thread 2
    KERNEL
        .spawn(
            || {
                pl011_println!("[Thread 2] Started!");
                let mut counter = 0u64;
                loop {
                    counter = counter.wrapping_add(1);
                    if counter % 10_000_000 == 0 {
                        pl011_println!("[Thread 2] counter = {}", counter);
                        pl011_println!("Yielding to thread 3");
                        KERNEL.yield_now();
                    }

                }
            },
            128, // Normal priority
        )
        .expect("Failed to spawn thread 2");

    // Spawn Thread 3 (lower priority - will run when higher priority threads idle)
    KERNEL
        .spawn(
            || {
                pl011_println!("[Thread 3] Started!");
                let mut counter = 0u64;
                loop {
                    counter = counter.wrapping_add(1);
                    if counter % 5_000_000 == 0 {
                        pl011_println!("[Thread 3] counter = {}", counter);
                        pl011_println!("Yielding to whom? maybe 1, maybe 2");
                        KERNEL.yield_now();
                    }

                }
            },
            128,
        )
        .expect("Failed to spawn thread 3");
    pl011_println!("[BOOT] 3 threads spawned!");

    // this should be disabled here, when you enable it, the cpu tries to execute some garbage & brreaks the whole program
    pl011_println!("[BOOT] Setting up preemption timer (1ms)...");
    //unsafe {
    //    preemptive_threads::arch::aarch64::setup_preemption_timer(1000)
  //          .expect("Failed to setup timer");
  //  }
  //  pl011_println!("[BOOT] Timer configured!");

    // NOTE: Do NOT enable interrupts here - start_first_thread() handles that
    // after setting up the current thread. This prevents an IRQ from firing
    // before we have a thread context to save to.

    pl011_println!("");
    pl011_println!("[BOOT] Starting scheduler - threads will now run!");
    pl011_println!("========================================");
    pl011_println!("");

    // Start running the first thread - this never returns
    // (also enables interrupts after setting up the thread context)
    KERNEL.start_first_thread();

    // If we somehow get here, halt
    pl011_println!("[ERROR] Scheduler returned unexpectedly!");
    loop {
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}
