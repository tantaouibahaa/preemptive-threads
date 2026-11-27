//! The global allocator: a linked-list heap that grows with `sbrk`.

use crate::sys::sbrk;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::{self, NonNull};
use linked_list_allocator::Heap;

/// Smallest amount the heap grows by.
const GROW_MIN: usize = 64 * 1024;

struct SbrkHeap {
    heap: UnsafeCell<Heap>,
}

// SAFETY: a pt-os process has exactly one thread, so the heap is never
// accessed concurrently.
unsafe impl Sync for SbrkHeap {}

#[global_allocator]
static HEAP: SbrkHeap = SbrkHeap { heap: UnsafeCell::new(Heap::empty()) };

impl SbrkHeap {
    /// Grow the heap by at least `need` bytes. The heap must stay
    /// contiguous, so this fails if something else moved the break.
    ///
    /// # Safety
    ///
    /// Caller has exclusive access to the heap.
    unsafe fn grow(&self, h: &mut Heap, need: usize) -> bool {
        let by = (need.max(GROW_MIN) + 4095) & !4095;
        let old = sbrk(by as i64);
        if old < 0 {
            return false;
        }
        // SAFETY: [old, old + by) was just given to us by the kernel and is
        // not used by anything else.
        unsafe {
            if h.size() == 0 {
                h.init(old as *mut u8, by);
            } else if h.top() as i64 == old {
                h.extend(by);
            } else {
                sbrk(-(by as i64));
                return false;
            }
        }
        true
    }
}

// SAFETY: blocks come from `linked_list_allocator`, which returns properly
// sized and aligned memory from the region we gave it.
unsafe impl GlobalAlloc for SbrkHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: single-threaded (see the Sync impl).
        let h = unsafe { &mut *self.heap.get() };
        if let Ok(p) = h.allocate_first_fit(layout) {
            return p.as_ptr();
        }
        // SAFETY: as above.
        if unsafe { self.grow(h, layout.size() + layout.align() + 64) } {
            if let Ok(p) = h.allocate_first_fit(layout) {
                return p.as_ptr();
            }
        }
        ptr::null_mut()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: single-threaded; `ptr` came from `alloc` with `layout`.
        unsafe { (*self.heap.get()).deallocate(NonNull::new_unchecked(ptr), layout) };
    }
}
