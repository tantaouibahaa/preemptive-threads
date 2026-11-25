//! Kernel heap: a first-fit linked-list allocator over the
//! `__heap_start..__heap_end` region reserved by the linker script, guarded
//! by an IRQ-safe spinlock so it can be used from any core and from
//! interrupt context.
//!
//! The crate installs it as the `#[global_allocator]`, so `alloc`
//! collections and `Box`/`Arc` work in kernels built on this crate.

use crate::sync::spin::IrqSpinLock;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use linked_list_allocator::Heap;

struct KernelHeap {
    heap: IrqSpinLock<Heap>,
    allocs: AtomicUsize,
    frees: AtomicUsize,
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap {
    heap: IrqSpinLock::new(Heap::empty()),
    allocs: AtomicUsize::new(0),
    frees: AtomicUsize::new(0),
};

/// Hand the linker-reserved region to the allocator. Called once at boot.
pub(crate) fn init() {
    let (start, end) = crate::arch::boot::heap_region();
    // SAFETY: the region is reserved for the heap by the linker script and
    // not used by anything else.
    unsafe { HEAP.heap.lock().init(start as *mut u8, end - start) };
}

// SAFETY: allocation is serialised by the spinlock; the underlying heap
// returns properly sized and aligned blocks.
unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match self.heap.lock().allocate_first_fit(layout) {
            Ok(p) => {
                self.allocs.fetch_add(1, Ordering::Relaxed);
                p.as_ptr()
            }
            Err(()) => core::ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.frees.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `ptr` came from `alloc` with the same layout.
        unsafe { self.heap.lock().deallocate(NonNull::new_unchecked(ptr), layout) };
    }
}

/// Heap usage snapshot.
#[derive(Debug, Clone, Copy)]
pub struct HeapStats {
    /// Total heap size in bytes.
    pub size: usize,
    /// Bytes currently allocated.
    pub used: usize,
    /// Bytes currently free.
    pub free: usize,
    /// Number of successful allocations so far.
    pub allocations: usize,
    /// Number of deallocations so far.
    pub deallocations: usize,
}

/// Current heap usage.
pub fn heap_stats() -> HeapStats {
    let h = HEAP.heap.lock();
    HeapStats {
        size: h.size(),
        used: h.used(),
        free: h.free(),
        allocations: HEAP.allocs.load(Ordering::Relaxed),
        deallocations: HEAP.frees.load(Ordering::Relaxed),
    }
}
