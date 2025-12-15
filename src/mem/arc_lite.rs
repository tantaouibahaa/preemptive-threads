//! Lightweight atomic reference counting for no_std environments.
//!
//! This provides an Arc-like abstraction using portable atomics that works
//! in no_std environments and supports manual reference count management.

use core::alloc::Layout;
use core::ops::Deref;
use core::ptr::NonNull;
use portable_atomic::{AtomicUsize, Ordering};

/// A lightweight atomic reference counter similar to Arc but with manual control.
///
/// This provides thread-safe reference counting without requiring std::sync::Arc.
/// Unlike standard Arc, this allows manual increment/decrement operations which
/// can be useful for intrusive data structures.
pub struct ArcLite<T> {
    ptr: NonNull<ArcLiteInner<T>>,
}

struct ArcLiteInner<T> {
    count: AtomicUsize,
    data: T,
}

impl<T> ArcLite<T> {
    /// Create a new ArcLite with the given data.
    ///
    /// # Arguments
    ///
    /// * `data` - The data to store in the ArcLite
    ///
    /// # Returns
    ///
    /// A new ArcLite instance with reference count of 1.
    #[allow(unused_variables)]
    pub fn new(data: T) -> Self {
        // For now, we'll use a simple Box-like allocation approach
        // In a real implementation, we'd need a proper allocator
        let layout = Layout::new::<ArcLiteInner<T>>();
        
        // TODO: Replace with proper no_std allocator
        // For now, this will only work with std-shim feature
        #[cfg(feature = "std-shim")]
        {
            extern crate std;
            use core::alloc::GlobalAlloc;
            use std::alloc::System;
            let alloc_ptr = unsafe { GlobalAlloc::alloc(&System, layout) as *mut ArcLiteInner<T> };
            if alloc_ptr.is_null() {
                panic!("Failed to allocate memory for ArcLite");
            }

            unsafe {
                core::ptr::write(alloc_ptr, ArcLiteInner {
                    count: AtomicUsize::new(1),
                    data,
                });
            }

            Self {
                ptr: unsafe { NonNull::new_unchecked(alloc_ptr) },
            }
        }
        
        #[cfg(not(feature = "std-shim"))]
        {
            // Use the global allocator in bare-metal environments
            extern crate alloc;
            use alloc::alloc::alloc;

            let alloc_ptr = unsafe { alloc(layout) as *mut ArcLiteInner<T> };
            if alloc_ptr.is_null() {
                panic!("Failed to allocate memory for ArcLite");
            }

            unsafe {
                core::ptr::write(alloc_ptr, ArcLiteInner {
                    count: AtomicUsize::new(1),
                    data,
                });
            }

            Self {
                ptr: unsafe { NonNull::new_unchecked(alloc_ptr) },
            }
        }
    }
    
    /// Increment the reference count.
    ///
    /// This is useful for intrusive data structures where you need manual
    /// control over the reference count.
    ///
    /// # Returns
    ///
    /// `true` if the increment succeeded, `false` if the object was being destroyed.
    pub fn try_inc(&self) -> bool {
        let inner = unsafe { self.ptr.as_ref() };
        let mut current = inner.count.load(Ordering::Acquire);
        
        loop {
            if current == 0 {
                return false; // Object is being destroyed
            }
            
            match inner.count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }
    
    /// Decrement the reference count.
    ///
    /// If the count reaches zero, the object will be deallocated.
    ///
    /// # Returns
    ///
    /// The previous reference count value.
    pub fn dec(&self) -> usize {
        let inner = unsafe { self.ptr.as_ref() };
        let prev_count = inner.count.fetch_sub(1, Ordering::AcqRel);
        
        if prev_count == 1 {
            // We were the last reference, deallocate
            unsafe {
                self.deallocate();
            }
        }
        
        prev_count
    }
    
    /// Get the current reference count.
    ///
    /// Note that this value may change immediately after being read in
    /// multi-threaded environments.
    pub fn ref_count(&self) -> usize {
        let inner = unsafe { self.ptr.as_ref() };
        inner.count.load(Ordering::Acquire)
    }
    
    /// Deallocate the ArcLite.
    ///
    /// # Safety
    ///
    /// This must only be called when the reference count has reached zero.
    unsafe fn deallocate(&self) {
        #[cfg(feature = "std-shim")]
        {
            extern crate std;
            use core::alloc::GlobalAlloc;
            use std::alloc::System;
            let layout = Layout::new::<ArcLiteInner<T>>();

            // Drop the data
            unsafe {
                core::ptr::drop_in_place(&mut self.ptr.as_ptr().as_mut().unwrap().data);

                // Deallocate the memory
                GlobalAlloc::dealloc(&System, self.ptr.as_ptr() as *mut u8, layout);
            }
        }
        
        #[cfg(not(feature = "std-shim"))]
        {
            // In a real no_std environment, we'd use a custom allocator
            unimplemented!("ArcLite deallocation requires a custom allocator in no_std environments")
        }
    }
}

impl<T> Clone for ArcLite<T> {
    fn clone(&self) -> Self {
        let inner = unsafe { self.ptr.as_ref() };
        let _prev_count = inner.count.fetch_add(1, Ordering::AcqRel);
        
        Self { ptr: self.ptr }
    }
}

impl<T> Drop for ArcLite<T> {
    fn drop(&mut self) {
        self.dec();
    }
}

impl<T> Deref for ArcLite<T> {
    type Target = T;
    
    fn deref(&self) -> &Self::Target {
        let inner = unsafe { self.ptr.as_ref() };
        &inner.data
    }
}

unsafe impl<T: Send + Sync> Send for ArcLite<T> {}
unsafe impl<T: Send + Sync> Sync for ArcLite<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_arc_lite_basic() {
        let arc = ArcLite::new(42);
        assert_eq!(*arc, 42);
        assert_eq!(arc.ref_count(), 1);
    }
    
    #[test]
    fn test_arc_lite_clone() {
        let arc1 = ArcLite::new(42);
        let arc2 = arc1.clone();
        
        assert_eq!(*arc1, 42);
        assert_eq!(*arc2, 42);
        assert_eq!(arc1.ref_count(), 2);
        assert_eq!(arc2.ref_count(), 2);
    }
    
    #[test] 
    fn test_arc_lite_try_inc() {
        let arc = ArcLite::new(42);
        assert_eq!(arc.ref_count(), 1);
        
        assert!(arc.try_inc());
        assert_eq!(arc.ref_count(), 2);
        
        arc.dec();
        assert_eq!(arc.ref_count(), 1);
    }
}