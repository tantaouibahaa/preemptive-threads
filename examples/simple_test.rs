//! Simplified test kernel - just 2 threads that print and yield

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

mod allocator {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;
    use core::ptr::null_mut;
    use core::sync::atomic::{AtomicUsize, Ordering};

    const HEAP_SIZE: usize = 16 * 1024 * 1024;

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

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
    }

    #[global_allocator]
    static ALLOCATOR: BumpAllocator = BumpAllocator;
}

static KERNEL: Lazy<Kernel<DefaultArch, FirstComeFirstServeScheduler>> =
    Lazy::new(|| Kernel::new(FirstComeFirstServeScheduler::new()));

#[no_mangle]
pub fn kernel_main() -> ! {
    unsafe {
        preemptive_threads::arch::uart_pl011::init();
    }

    pl011_println!("=== Simple Test Kernel ===");
    
    KERNEL.init().expect("Init failed");
    unsafe {
        KERNEL.register_global();
    }

    pl011_println!("Spawning thread 1...");
    KERNEL.spawn(
        || {
            pl011_println!("[T1] Hello!");
            for i in 0..5 {
                pl011_println!("[T1] Count: {}", i);
                preemptive_threads::yield_now();
            }
            pl011_println!("[T1] Done!");
        },
        128,
    ).expect("Spawn 1 failed");

    pl011_println!("Spawning thread 2...");
    KERNEL.spawn(
        || {
            pl011_println!("[T2] Hello!");
            for i in 0..5 {
                pl011_println!("[T2] Count: {}", i);
                preemptive_threads::yield_now();
            }
            pl011_println!("[T2] Done!");
        },
        128,
    ).expect("Spawn 2 failed");

    pl011_println!("Starting scheduler...");
    KERNEL.start_first_thread();

    pl011_println!("ERROR: Should never reach here!");
    loop {
        unsafe { core::arch::asm!("wfe"); }
    }
}
