//! Minimal bare-metal kernel example for QEMU raspi3b emulation.
//!
//! This example uses the PL011 UART which QEMU maps to `-serial stdio`.
//!
//! # Building
//!
//! ```bash
//! cargo build --release --example qemu_kernel --target aarch64-unknown-none
//! ```
//!
//! # Running
//!
//! ```bash
//! qemu-system-aarch64 \
//!     -M raspi3b \
//!     -kernel target/aarch64-unknown-none/release/examples/qemu_kernel \
//!     -serial stdio \
//!     -display none
//! ```
//!
//! Press Ctrl-A X to exit QEMU.

#![no_std]
#![no_main]

extern crate alloc;

use preemptive_threads::{
    arch::DefaultArch,
    sched::RoundRobinScheduler,
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
static KERNEL: Lazy<Kernel<DefaultArch, RoundRobinScheduler>> =
    Lazy::new(|| Kernel::new(RoundRobinScheduler::new(1)));

/// Kernel entry point - called from boot code after hardware init.
#[no_mangle]
pub fn kernel_main() -> ! {
    // Initialize PL011 UART first so we can see output in QEMU
    unsafe {
        preemptive_threads::arch::uart_pl011::init();
    }

    pl011_println!("");
    pl011_println!("========================================");
    pl011_println!("  Preemptive Threads Kernel v0.6.0");
    pl011_println!("  QEMU raspi3b emulation");
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
    pl011_println!("[BOOT] Spawning Thread 1...");
    KERNEL
        .spawn(
            || {
                let mut counter = 0u64;
                loop {
                    counter = counter.wrapping_add(1);
                    if counter % 100_000 == 0 {
                        pl011_println!("[Thread 1] counter = {}", counter);
                    }
                    // Small busy loop to simulate work
                    for _ in 0..100 {
                        core::hint::spin_loop();
                    }
                }
            },
            128, // Normal priority
        )
        .expect("Failed to spawn thread 1");
    pl011_println!("[BOOT] Thread 1 spawned!");

    // Spawn Thread 2
    pl011_println!("[BOOT] Spawning Thread 2...");
    KERNEL
        .spawn(
            || {
                let mut counter = 0u64;
                loop {
                    counter = counter.wrapping_add(1);
                    if counter % 100_000 == 0 {
                        pl011_println!("[Thread 2] counter = {}", counter);
                    }
                    for _ in 0..100 {
                        core::hint::spin_loop();
                    }
                }
            },
            128, // Normal priority
        )
        .expect("Failed to spawn thread 2");
    pl011_println!("[BOOT] Thread 2 spawned!");

    pl011_println!("");
    pl011_println!("[BOOT] Setup complete!");
    pl011_println!("[BOOT] NOTE: Timer interrupts not enabled in QEMU example");
    pl011_println!("[BOOT] Threads will run cooperatively");
    pl011_println!("========================================");
    pl011_println!("");

    // For QEMU testing, we just loop here showing we're alive
    // Full preemption requires GIC timer setup which is complex in QEMU
    let mut tick = 0u64;
    loop {
        tick = tick.wrapping_add(1);
        if tick % 10_000_000 == 0 {
            pl011_println!("[IDLE] Main loop tick = {}", tick / 10_000_000);
        }
        core::hint::spin_loop();
    }
}

