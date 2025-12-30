//! Stack pool allocator for thread stacks.
//!
//! This module provides a pool-based allocator for thread stacks with
//! different size classes and optional guard page support.



use portable_atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use core::ptr::NonNull;

// Use Vec from alloc or std depending on features
#[cfg(feature = "std-shim")]
extern crate std;

#[cfg(feature = "std-shim")]
use std::vec::Vec;

#[cfg(not(feature = "std-shim"))]
extern crate alloc;

#[cfg(not(feature = "std-shim"))]
use alloc::vec::Vec;

/// Stack size classes for the pool allocator.
///
/// Different threads may need different stack sizes, so we provide
/// several size classes to minimize memory waste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackSizeClass {
    /// Small stack: 4 KiB
    Small = 4096,
    /// Medium stack: 16 KiB
    Medium = 16384,
    /// Large stack: 64 KiB
    Large = 65536,
    /// Extra large stack: 256 KiB
    ExtraLarge = 262144,
}

impl StackSizeClass {
    /// Get the size in bytes for this stack class.
    pub fn size(self) -> usize {
        self as usize
    }

    /// Choose the appropriate size class for a requested stack size.
    ///
    /// # Arguments
    ///
    /// * `requested_size` - The minimum stack size required
    ///
    /// # Returns
    ///
    /// The smallest size class that can accommodate the requested size.
    pub fn for_size(requested_size: usize) -> Option<Self> {
        match requested_size {
            0..=4096 => Some(Self::Small),
            4097..=16384 => Some(Self::Medium),
            16385..=65536 => Some(Self::Large),
            65537..=262144 => Some(Self::ExtraLarge),
            _ => None, // Size too large
        }
    }
}

/// A thread stack with optional guard pages.
///
/// This structure represents a single allocated stack that can be
/// used by a thread. It handles both the memory allocation and
/// optional guard page protection.
#[derive(Clone)]
pub struct Stack {
    /// Pointer to the start of the stack memory (lowest address)
    memory: NonNull<u8>,
    /// Usable stack size (excluding guard pages)
    usable_size: usize,
    /// Size class this stack belongs to
    size_class: StackSizeClass,
    /// Whether this stack has guard pages
    has_guard_pages: bool,
}

impl Stack {
    /// Get the usable stack size in bytes.
    pub fn size(&self) -> usize {
        self.usable_size
    }

    /// Get the stack size class.
    pub fn size_class(&self) -> StackSizeClass {
        self.size_class
    }

    /// Get a pointer to the bottom of the stack (highest address).
    pub fn stack_bottom(&self) -> *mut u8 {
        let mut sp = unsafe {
            self.memory.as_ptr().add(
                if self.has_guard_pages {
                    4096 + self.usable_size
                } else {
                    self.usable_size
                }
            ) as usize
        };

        sp &= !0xF;
        sp as *mut u8
    }


    /// Get a pointer to the top of the stack (lowest address).
    pub fn stack_top(&self) -> *const u8 {
        unsafe {
            if self.has_guard_pages {
                self.memory.as_ptr().add(4096) // Skip guard page
            } else {
                self.memory.as_ptr()
            }
        }
    }

    /// Get bottom pointer (alias for stack_bottom for compatibility).
    pub fn bottom(&self) -> *mut u8 {
        self.stack_bottom()
    }

    /// Get top pointer (alias for stack_top for compatibility).
    pub fn top(&self) -> *const u8 {
        self.stack_top()
    }

    pub fn has_guard_pages(&self) -> bool {
        self.has_guard_pages
    }

    /// Install a stack canary value for overflow detection.
    ///
    /// This writes a known pattern at the bottom of the usable stack
    /// that can be checked later to detect stack overflow.
    ///
    /// # Arguments
    ///
    /// * `canary` - The canary value to write
    pub fn install_canary(&self, canary: u64) {
        let canary_location = self.stack_top() as *mut u64;
        unsafe {
            canary_location.write(canary);
        }
    }

    /// Check if the stack canary is still intact.
    ///
    /// # Arguments
    ///
    /// * `expected_canary` - The expected canary value
    ///
    /// # Returns
    ///
    /// `true` if the canary is intact, `false` if it has been corrupted.
    pub fn check_canary(&self, expected_canary: u64) -> bool {
        let canary_location = self.stack_top() as *const u64;
        unsafe { canary_location.read() == expected_canary }
    }
}

/// Pool-based allocator for thread stacks.
///
/// This allocator maintains separate free lists for each stack size class
/// to minimize fragmentation and allocation overhead.
pub struct StackPool {
    /// Free stacks for each size class
    free_stacks: [Mutex<Vec<Stack>>; 4],
    /// Statistics counters
    stats: StackPoolStats,
}

#[derive(Debug, Default)]
struct StackPoolStats {
    /// Number of stacks allocated
    allocated: AtomicUsize,
    /// Number of stacks returned to the pool
    deallocated: AtomicUsize,
    /// Number of stacks currently in use
    in_use: AtomicUsize,
}

impl Default for StackPool {
    fn default() -> Self {
        Self::new()
    }
}

impl StackPool {
    pub const fn new() -> Self {
        Self {
            free_stacks: [
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
            ],
            stats: StackPoolStats {
                allocated: AtomicUsize::new(0),
                deallocated: AtomicUsize::new(0),
                in_use: AtomicUsize::new(0),
            },
        }
    }

    /// Allocate a stack of the given size class.
    ///
    /// This will first try to reuse a stack from the free list, and only
    /// allocate new memory if no suitable stack is available.
    ///
    /// # Arguments
    ///
    /// * `size_class` - The desired stack size class
    ///
    /// # Returns
    ///
    /// A new stack, or `None` if allocation fails.
    pub fn allocate(&self, size_class: StackSizeClass) -> Option<Stack> {
        let class_index = self.size_class_index(size_class);

        // Try to get a stack from the free list first
        if let Some(mut free_list) = self.free_stacks[class_index].try_lock() {
            if let Some(stack) = free_list.pop() {
                self.stats.in_use.fetch_add(1, Ordering::AcqRel);
                return Some(stack);
            }
        }

        // Need to allocate a new stack
        self.allocate_new_stack(size_class)
    }

    /// Return a stack to the pool for reuse.
    ///
    /// # Arguments
    ///
    /// * `stack` - The stack to return to the pool
    pub fn deallocate(&self, stack: Stack) {
        let class_index = self.size_class_index(stack.size_class);

        if let Some(mut free_list) = self.free_stacks[class_index].try_lock() {
            free_list.push(stack);
            self.stats.in_use.fetch_sub(1, Ordering::AcqRel);
            self.stats.deallocated.fetch_add(1, Ordering::AcqRel);
        }
        // If we can't get the lock, the stack will be dropped
    }

    /// Get statistics about the stack pool.
    pub fn stats(&self) -> (usize, usize, usize) {
        (
            self.stats.allocated.load(Ordering::Acquire),
            self.stats.deallocated.load(Ordering::Acquire),
            self.stats.in_use.load(Ordering::Acquire),
        )
    }

    /// Convert a size class to an array index.
    fn size_class_index(&self, size_class: StackSizeClass) -> usize {
        match size_class {
            StackSizeClass::Small => 0,
            StackSizeClass::Medium => 1,
            StackSizeClass::Large => 2,
            StackSizeClass::ExtraLarge => 3,
        }
    }

    fn allocate_new_stack(&self, size_class: StackSizeClass,) -> Option<Stack> {
        let usable_size = size_class.size();

        #[cfg(feature = "std-shim")]
        {
            extern crate std;
            use std::alloc::{alloc, Layout};

            let total_size = usable_size;
            let layout = Layout::from_size_align(total_size, 4096).ok()?;
            let memory = unsafe { alloc(layout) };

            if memory.is_null() {
                return None;
            }

            let memory = unsafe { NonNull::new_unchecked(memory) };

            let stack = Stack {
                memory,
                usable_size,
                size_class,
                has_guard_pages: false,
            };


            self.stats.allocated.fetch_add(1, Ordering::AcqRel);
            self.stats.in_use.fetch_add(1, Ordering::AcqRel);

            Some(stack)
        }

        #[cfg(not(feature = "std-shim"))]
        {
            // In bare-metal mode, use the global allocator (e.g., bump allocator)
            use alloc::alloc::{alloc, Layout};

            let layout = Layout::from_size_align(usable_size, 4096).ok()?;
            let memory = unsafe { alloc(layout) };

            if memory.is_null() {
                return None;
            }

            let memory = unsafe { NonNull::new_unchecked(memory) };

            let stack = Stack {
                memory,
                usable_size,
                size_class,
                has_guard_pages: false,
            };

            self.stats.allocated.fetch_add(1, Ordering::AcqRel);
            self.stats.in_use.fetch_add(1, Ordering::AcqRel);

            Some(stack)
        }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        #[cfg(feature = "std-shim")]
        {
            extern crate std;
            use std::alloc::{dealloc, Layout};

            if let Ok(layout) = Layout::from_size_align(self.usable_size, 4096) {
                unsafe {
                    dealloc(self.memory.as_ptr(), layout);
                }
            }
        }
    }
}

unsafe impl Send for Stack {}
unsafe impl Sync for Stack {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stack_size_class_for_size() {
        assert_eq!(StackSizeClass::for_size(1024), Some(StackSizeClass::Small));
        assert_eq!(StackSizeClass::for_size(4096), Some(StackSizeClass::Small));
        assert_eq!(StackSizeClass::for_size(8192), Some(StackSizeClass::Medium));
        assert_eq!(StackSizeClass::for_size(32768), Some(StackSizeClass::Large));
        assert_eq!(StackSizeClass::for_size(131072), Some(StackSizeClass::ExtraLarge));
        assert_eq!(StackSizeClass::for_size(500000), None);
    }

    #[cfg(feature = "std-shim")]
    #[test]
    fn test_stack_pool_basic() {
        let pool = StackPool::new();
        let stack = pool.allocate(StackSizeClass::Small).unwrap();

        assert_eq!(stack.size_class(), StackSizeClass::Small);
        assert_eq!(stack.size(), StackSizeClass::Small.size());

        pool.deallocate(stack);

        let (allocated, deallocated, in_use) = pool.stats();
        assert_eq!(allocated, 1);
        assert_eq!(deallocated, 1);
        assert_eq!(in_use, 0);
    }

    #[cfg(feature = "std-shim")]
    #[test]
    fn test_stack_canary() {
        let pool = StackPool::new();
        let stack = pool.allocate(StackSizeClass::Small).unwrap();

        let canary_value = 0xDEADBEEFCAFEBABE;
        stack.install_canary(canary_value);
        assert!(stack.check_canary(canary_value));
        assert!(!stack.check_canary(0x1234567890ABCDEF));

        pool.deallocate(stack);
    }
}
