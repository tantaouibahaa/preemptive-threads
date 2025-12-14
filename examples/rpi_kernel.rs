//! Minimal bare-metal kernel example for Raspberry Pi Zero 2 W.
//!
//! This example demonstrates basic preemptive multithreading on bare metal.
//!
//! # Building
//!
//! ```bash
//! cargo build --release --example rpi_kernel
//! ```
//!
//! # Deploying
//!
//! 1. Convert ELF to binary:
//!    ```bash
//!    rust-objcopy -O binary target/aarch64-unknown-none/release/examples/rpi_kernel kernel8.img
//!    ```
//!
//! 2. Copy kernel8.img to SD card boot partition
//!
//! 3. Create config.txt on SD card:
//!    ```
//!    arm_64bit=1
//!    kernel=kernel8.img
//!    ```
//!
//! 4. Boot the Raspberry Pi

#![no_std]
#![no_main]

extern crate alloc;

use preemptive_threads::{
    arch::{Arch, DefaultArch},
    sched::RoundRobinScheduler,
    Kernel,
};
use spin::Lazy;

/// Simple bump allocator for the heap.
///
/// In a real kernel, you'd want a more sophisticated allocator.
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
                    return null_mut(); // Out of memory
                }

                if HEAP
                    .next
                    .compare_exchange(current, new_next, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    let heap_start = unsafe { HEAP.data.get() as *mut u8 };
                    return unsafe { heap_start.add(aligned) };
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

/// The kernel instance (static for interrupt handler access).
/// Using single CPU (Pi Zero 2 W has 4 cores but we only use 1 for now).
static KERNEL: Lazy<Kernel<DefaultArch, RoundRobinScheduler>> =
    Lazy::new(|| Kernel::new(RoundRobinScheduler::new(1)));

/// Kernel entry point - called from boot code after hardware init.
#[no_mangle]
pub fn kernel_main() -> ! {
    // Initialize the kernel
    KERNEL.init().expect("Failed to initialize kernel");

    // Register kernel globally for interrupt handlers
    unsafe {
        KERNEL.register_global();
    }

    // Spawn some test threads
    KERNEL
        .spawn(
            || {
                let mut counter = 0u64;
                loop {
                    counter = counter.wrapping_add(1);
                    if counter % 1_000_000 == 0 {
                        // In a real kernel, we'd output to UART here
                        // println!("Thread 1: {}", counter);
                    }
                    // Cooperative yield (preemption will also happen via timer)
                    preemptive_threads::yield_now();
                }
            },
            128,
        )
        .expect("Failed to spawn thread 1");

    KERNEL
        .spawn(
            || {
                let mut counter = 0u64;
                loop {
                    counter = counter.wrapping_add(1);
                    if counter % 1_000_000 == 0 {
                        // println!("Thread 2: {}", counter);
                    }
                    preemptive_threads::yield_now();
                }
            },
            128,
        )
        .expect("Failed to spawn thread 2");

    // Set up the preemption timer (1ms time slices)
    unsafe {
        preemptive_threads::arch::aarch64::setup_preemption_timer(1000)
            .expect("Failed to setup timer");
    }

    // Enable interrupts to start preemption
    DefaultArch::enable_interrupts();

    // Start running the first thread
    // This never returns - we're now running threads
    KERNEL.start_first_thread();

    // If we somehow get here, just halt
    loop {
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}

// Panic handler is provided by the library
