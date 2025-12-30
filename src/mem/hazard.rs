

use portable_atomic::{AtomicPtr, AtomicUsize, Ordering};
use core::ptr::{self, NonNull};
use core::sync::atomic::AtomicBool;
use core::marker::PhantomData;
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

/// Maximum number of hazard pointers per thread.
const HAZARDS_PER_THREAD: usize = 8;

/// Maximum number of threads that can use hazard pointers.
const MAX_THREADS: usize = 64;

/// Maximum number of retired pointers before attempting reclamation.
const RETIRE_THRESHOLD: usize = 64;

/// Global hazard pointer registry.
static mut HAZARD_REGISTRY: HazardRegistry = HazardRegistry::new();

/// Global registry of hazard pointers for all threads.
struct HazardRegistry {
    thread_records: [ThreadRecord; MAX_THREADS],
    next_thread_id: AtomicUsize,
}

impl HazardRegistry {
    const fn new() -> Self {
        Self {
            thread_records: [const { ThreadRecord::new() }; MAX_THREADS],
            next_thread_id: AtomicUsize::new(0),
        }
    }
    
    /// Acquire a thread record for the current thread.
    fn acquire_thread_record(&self) -> Option<&ThreadRecord> {
        let thread_id = self.next_thread_id.fetch_add(1, Ordering::AcqRel);
        if thread_id < MAX_THREADS {
            let record = &self.thread_records[thread_id];
            record.thread_id.store(thread_id, Ordering::Release);
            record.active.store(true, Ordering::Release);
            Some(record)
        } else {
            None
        }
    }
    
    /// Release a thread record.
    fn release_thread_record(&self, thread_id: usize) {
        if thread_id < MAX_THREADS {
            let record = &self.thread_records[thread_id];
            
            // Clear all hazard pointers
            for hazard in &record.hazards {
                hazard.store(ptr::null_mut(), Ordering::Release);
            }
            
            // Process any remaining retired pointers
            record.process_retired_list();
            
            record.active.store(false, Ordering::Release);
        }
    }
    
    /// Check if a pointer is protected by any hazard pointer.
    fn is_protected(&self, ptr: *mut u8) -> bool {
        for record in &self.thread_records {
            if !record.active.load(Ordering::Acquire) {
                continue;
            }
            
            for hazard in &record.hazards {
                if hazard.load(Ordering::Acquire) == ptr {
                    return true;
                }
            }
        }
        false
    }
}

/// Per-thread hazard pointer record.
struct ThreadRecord {
    thread_id: AtomicUsize,
    active: AtomicBool,
    hazards: [AtomicPtr<u8>; HAZARDS_PER_THREAD],
    retired_list: spin::Mutex<Vec<RetiredPointer>>,
}

impl ThreadRecord {
    const fn new() -> Self {
        const INIT_HAZARD: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
        Self {
            thread_id: AtomicUsize::new(usize::MAX),
            active: AtomicBool::new(false),
            hazards: [INIT_HAZARD; HAZARDS_PER_THREAD],
            retired_list: spin::Mutex::new(Vec::new()),
        }
    }
    
    /// Retire a pointer for later reclamation.
    fn retire_pointer(&self, ptr: *mut u8, size: usize, align: usize) {
        let retired = RetiredPointer {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
            size,
            align,
        };
        
        if let Some(mut retired_list) = self.retired_list.try_lock() {
            retired_list.push(retired);
            
            // If we have too many retired pointers, try to reclaim some
            if retired_list.len() >= RETIRE_THRESHOLD {
                self.try_reclaim(&mut retired_list);
            }
        }
    }
    
    /// Process the retired list and reclaim safe pointers.
    fn process_retired_list(&self) {
        if let Some(mut retired_list) = self.retired_list.try_lock() {
            self.try_reclaim(&mut retired_list);
        }
    }
    
    /// Try to reclaim pointers that are not protected by hazard pointers.
    fn try_reclaim(&self, retired_list: &mut Vec<RetiredPointer>) {
        let registry = unsafe { &HAZARD_REGISTRY };
        
        retired_list.retain(|retired| {
            if !registry.is_protected(retired.ptr.as_ptr()) {
                // Safe to reclaim this pointer
                unsafe {
                    let layout = core::alloc::Layout::from_size_align_unchecked(
                        retired.size,
                        retired.align,
                    );
                    alloc::alloc::dealloc(retired.ptr.as_ptr(), layout);
                }
                false // Remove from list
            } else {
                true // Keep in list
            }
        });
    }
}

/// A retired pointer waiting for reclamation.
struct RetiredPointer {
    ptr: NonNull<u8>,
    size: usize,
    align: usize,
}

unsafe impl Send for RetiredPointer {}
unsafe impl Sync for RetiredPointer {}

/// A hazard pointer that protects a specific memory location.
///
/// While this hazard pointer is active, the protected memory location
/// will not be reclaimed by other threads.
pub struct HazardPointer {
    thread_record: &'static ThreadRecord,
    hazard_index: usize,
}

impl HazardPointer {
    /// Create a new hazard pointer for the current thread.
    ///
    /// Returns `None` if no hazard pointer slots are available.
    pub fn new() -> Option<Self> {
        let registry = unsafe { &HAZARD_REGISTRY };
        let thread_record = registry.acquire_thread_record()?;
        
        // Find an available hazard pointer slot
        for (index, hazard) in thread_record.hazards.iter().enumerate() {
            if hazard.load(Ordering::Acquire).is_null() {
                return Some(Self {
                    thread_record,
                    hazard_index: index,
                });
            }
        }
        
        None // No available slots
    }
    
    /// Protect a pointer with this hazard pointer.
    ///
    /// The protected pointer will not be reclaimed while this hazard pointer
    /// is protecting it.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid pointer that could
    /// potentially be reclaimed by other threads.
    pub unsafe fn protect<T>(&self, ptr: *mut T) -> *mut T {
        let byte_ptr = ptr as *mut u8;
        self.thread_record.hazards[self.hazard_index].store(byte_ptr, Ordering::Release);
        
        // Memory fence to ensure the hazard pointer is visible before any loads
        core::sync::atomic::fence(Ordering::SeqCst);
        
        ptr
    }
    
    /// Clear the protection provided by this hazard pointer.
    pub fn clear(&self) {
        self.thread_record.hazards[self.hazard_index].store(ptr::null_mut(), Ordering::Release);
    }
    
    /// Retire a pointer for safe reclamation.
    ///
    /// The pointer will be reclaimed when it's no longer protected by any
    /// hazard pointer.
    ///
    /// # Safety
    ///
    /// - `ptr` must be a valid pointer that was allocated with the global allocator
    /// - `ptr` must not be accessed after this call
    /// - The caller must ensure that `size` and `align` match the original allocation
    pub unsafe fn retire<T>(&self, ptr: *mut T) {
        if ptr.is_null() {
            return;
        }
        
        self.thread_record.retire_pointer(
            ptr as *mut u8,
            core::mem::size_of::<T>(),
            core::mem::align_of::<T>(),
        );
    }
}

impl Drop for HazardPointer {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Initialize hazard pointers for the current thread.
pub fn init_thread() -> Result<(), &'static str> {
    let registry = unsafe { &HAZARD_REGISTRY };
    
    if registry.acquire_thread_record().is_some() {
        Ok(())
    } else {
        Err("Too many threads using hazard pointers")
    }
}

/// Clean up hazard pointers for the current thread.
pub fn cleanup_thread(thread_id: usize) {
    let registry = unsafe { &HAZARD_REGISTRY };
    registry.release_thread_record(thread_id);
}

/// Atomic pointer with hazard pointer support.
///
/// This provides a safe way to perform atomic operations on pointers
/// while using hazard pointers for memory reclamation.
pub struct HazardAtomic<T> {
    ptr: AtomicPtr<T>,
    _marker: PhantomData<T>,
}

impl<T> HazardAtomic<T> {
    /// Create a new atomic pointer.
    pub const fn new(ptr: *mut T) -> Self {
        Self {
            ptr: AtomicPtr::new(ptr),
            _marker: PhantomData,
        }
    }
    
    /// Load the pointer with hazard pointer protection.
    ///
    /// This ensures that the returned pointer is protected from reclamation
    /// while the hazard pointer is active.
    pub fn load_protected(&self, order: Ordering, hazard: &HazardPointer) -> *mut T {
        loop {
            let ptr = self.ptr.load(order);
            
            // Protect the pointer with hazard pointer
            let protected_ptr = unsafe { hazard.protect(ptr) };
            
            // Verify that the pointer hasn't changed
            if self.ptr.load(order) == protected_ptr {
                return protected_ptr;
            }
            
            // Pointer changed, clear protection and retry
            hazard.clear();
        }
    }
    
    /// Store a new pointer value.
    pub fn store(&self, ptr: *mut T, order: Ordering) {
        self.ptr.store(ptr, order);
    }
    
    /// Compare and swap the pointer with hazard pointer support.
    pub fn compare_exchange_weak(
        &self,
        current: *mut T,
        new: *mut T,
        success: Ordering,
        failure: Ordering,
        hazard: &HazardPointer,
    ) -> Result<*mut T, *mut T> {
        match self.ptr.compare_exchange_weak(current, new, success, failure) {
            Ok(old_ptr) => {
                if !old_ptr.is_null() && old_ptr != new {
                    unsafe {
                        hazard.retire(old_ptr);
                    }
                }
                Ok(old_ptr)
            }
            Err(actual) => Err(actual),
        }
    }
}

unsafe impl<T: Send> Send for HazardAtomic<T> {}
unsafe impl<T: Send + Sync> Sync for HazardAtomic<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_hazard_pointer_creation() {
        let _result = init_thread();
        let hazard = HazardPointer::new();
        assert!(hazard.is_some());
    }
    
    #[test]
    fn test_hazard_protection() {
        let _result = init_thread();
        let hazard = HazardPointer::new().unwrap();
        
        let ptr = 0x1000 as *mut i32;
        let protected = unsafe { hazard.protect(ptr) };
        
        assert_eq!(ptr, protected);
    }
    
    #[test]
    fn test_hazard_atomic_operations() {
        let _result = init_thread();
        let atomic = HazardAtomic::new(ptr::null_mut());
        let hazard = HazardPointer::new().unwrap();
        
        // Test basic load/store operations
        atomic.store(0x1000 as *mut i32, Ordering::SeqCst);
        let loaded = atomic.load_protected(Ordering::SeqCst, &hazard);
        assert_eq!(loaded, 0x1000 as *mut i32);
    }
    
    #[test]
    fn test_pointer_retirement() {
        let _result = init_thread();
        let hazard = HazardPointer::new().unwrap();
        
        // Create a test allocation (in a real scenario this would be actual allocated memory)
        let ptr = Box::into_raw(Box::new(42i32));
        
        // Retire the pointer
        unsafe {
            hazard.retire(ptr);
        }
        
        // The pointer should eventually be reclaimed when it's not protected
    }
}