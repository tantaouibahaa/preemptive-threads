//! Thread stacks.
//!
//! Stacks are allocated from the heap and recycled through a small pool so
//! that spawning and reaping threads in a loop does not churn the
//! allocator. Each stack carries a canary word at its lowest address that
//! the scheduler checks on every switch-out to detect overflow.

use crate::sync::spin::IrqSpinLock;
use alloc::alloc::{alloc, dealloc, Layout};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Default thread stack size (64 KiB).
pub const DEFAULT_STACK_SIZE: usize = 64 * 1024;
/// Smallest allowed stack (the IRQ trap frame alone needs 800 bytes).
pub const MIN_STACK_SIZE: usize = 8 * 1024;
const CANARY: u64 = 0x57ac_6ca7_57ac_6ca7;
const STACK_ALIGN: usize = 16;
/// Free stacks kept per size for reuse.
const POOL_LIMIT: usize = 32;

/// An owned thread stack.
pub struct Stack {
    base: *mut u8,
    size: usize,
}

// SAFETY: a stack is plain memory owned by exactly one thread at a time.
unsafe impl Send for Stack {}
// SAFETY: shared references only expose addresses.
unsafe impl Sync for Stack {}

impl Stack {
    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size, STACK_ALIGN).unwrap()
    }

    /// Lowest address of the stack.
    pub fn bottom(&self) -> usize {
        self.base as usize
    }

    /// One past the highest address (initial stack pointer).
    pub fn top(&self) -> usize {
        self.base as usize + self.size
    }

    /// Size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    fn write_canary(&self) {
        // SAFETY: the base is 16-byte aligned and owned by this stack.
        unsafe { core::ptr::write_volatile(self.base as *mut u64, CANARY) };
    }

    /// Whether the overflow canary at the bottom of the stack is intact.
    pub fn canary_ok(&self) -> bool {
        // SAFETY: as above.
        unsafe { core::ptr::read_volatile(self.base as *const u64) == CANARY }
    }
}

/// Stack pool statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct StackPoolStats {
    /// Stacks handed out and not yet returned.
    pub in_use: usize,
    /// Stacks cached for reuse.
    pub cached: usize,
    /// Stacks ever allocated from the heap.
    pub allocated: usize,
}

/// A pool of reusable stacks, bucketed by size.
pub struct StackPool {
    free: IrqSpinLock<Vec<Stack>>,
    in_use: AtomicUsize,
    allocated: AtomicUsize,
}

impl StackPool {
    /// Create an empty pool.
    pub const fn new() -> Self {
        Self { free: IrqSpinLock::new(Vec::new()), in_use: AtomicUsize::new(0), allocated: AtomicUsize::new(0) }
    }

    /// Get a stack of at least `size` bytes (rounded up to 4 KiB).
    pub fn allocate(&self, size: usize) -> Option<Stack> {
        let size = size.max(MIN_STACK_SIZE).next_multiple_of(4096);
        let reused = {
            let mut free = self.free.lock();
            free.iter().position(|s| s.size == size).map(|i| free.swap_remove(i))
        };
        let stack = match reused {
            Some(s) => s,
            None => {
                // SAFETY: the layout has a non-zero size.
                let base = unsafe { alloc(Stack::layout(size)) };
                if base.is_null() {
                    return None;
                }
                self.allocated.fetch_add(1, Ordering::Relaxed);
                Stack { base, size }
            }
        };
        stack.write_canary();
        self.in_use.fetch_add(1, Ordering::Relaxed);
        Some(stack)
    }

    /// Return a stack to the pool (or to the heap if the pool is full).
    pub fn release(&self, stack: Stack) {
        self.in_use.fetch_sub(1, Ordering::Relaxed);
        let mut free = self.free.lock();
        if free.len() < POOL_LIMIT {
            free.push(stack);
        } else {
            drop(free);
            // SAFETY: allocated in `allocate` with the same layout.
            unsafe { dealloc(stack.base, Stack::layout(stack.size)) };
        }
    }

    /// Current statistics.
    pub fn stats(&self) -> StackPoolStats {
        StackPoolStats {
            in_use: self.in_use.load(Ordering::Relaxed),
            cached: self.free.lock().len(),
            allocated: self.allocated.load(Ordering::Relaxed),
        }
    }
}

impl Default for StackPool {
    fn default() -> Self {
        Self::new()
    }
}
