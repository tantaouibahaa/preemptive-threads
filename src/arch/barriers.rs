//! Cross-platform memory barriers and atomic operation helpers.
//!
//! This module provides unified memory barrier operations, primarily for
//! ARM64 (AArch64) architecture used in Raspberry Pi Zero 2 W.

use portable_atomic::{AtomicU64, AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierType {
    Full,
    Acquire,
    Release,
    LoadOnly,
    StoreOnly,
}

pub struct MemoryBarriers;

impl MemoryBarriers {
    pub fn barrier(barrier_type: BarrierType) {
        match barrier_type {
            BarrierType::Full => Self::full_barrier(),
            BarrierType::Acquire => Self::acquire_barrier(),
            BarrierType::Release => Self::release_barrier(),
            BarrierType::LoadOnly => Self::load_barrier(),
            BarrierType::StoreOnly => Self::store_barrier(),
        }
    }

    #[inline(always)]
    pub fn full_barrier() {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("dmb sy", options(nostack, preserves_flags));
        }

        #[cfg(not(target_arch = "aarch64"))]
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    #[inline(always)]
    pub fn acquire_barrier() {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("dmb ld", options(nostack, preserves_flags));
        }

        #[cfg(not(target_arch = "aarch64"))]
        core::sync::atomic::fence(Ordering::Acquire);
    }

    #[inline(always)]
    pub fn release_barrier() {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("dmb st", options(nostack, preserves_flags));
        }

        #[cfg(not(target_arch = "aarch64"))]
        core::sync::atomic::fence(Ordering::Release);
    }

    #[inline(always)]
    pub fn load_barrier() {
        Self::acquire_barrier();
    }

    #[inline(always)]
    pub fn store_barrier() {
        Self::release_barrier();
    }
}

/// Cross-platform atomic operation extensions.
pub trait AtomicExt<T> {
    /// Atomic compare-and-swap with explicit memory ordering.
    fn compare_exchange_explicit(
        &self,
        current: T,
        new: T,
        success_order: Ordering,
        failure_order: Ordering,
    ) -> Result<T, T>;

    fn fetch_add_explicit(&self, val: T, order: Ordering) -> T;

    fn fetch_sub_explicit(&self, val: T, order: Ordering) -> T;

    fn load_with_barrier(&self, barrier: BarrierType) -> T;

    fn store_with_barrier(&self, val: T, barrier: BarrierType);
}

impl AtomicExt<u64> for AtomicU64 {
    fn compare_exchange_explicit(
        &self,
        current: u64,
        new: u64,
        success_order: Ordering,
        failure_order: Ordering,
    ) -> Result<u64, u64> {
        self.compare_exchange(current, new, success_order, failure_order)
    }

    fn fetch_add_explicit(&self, val: u64, order: Ordering) -> u64 {
        self.fetch_add(val, order)
    }

    fn fetch_sub_explicit(&self, val: u64, order: Ordering) -> u64 {
        self.fetch_sub(val, order)
    }

    fn load_with_barrier(&self, barrier: BarrierType) -> u64 {
        match barrier {
            BarrierType::Full => {
                MemoryBarriers::full_barrier();
                let val = self.load(Ordering::Relaxed);
                MemoryBarriers::full_barrier();
                val
            }
            BarrierType::Acquire => self.load(Ordering::Acquire),
            BarrierType::Release => {
                MemoryBarriers::release_barrier();
                self.load(Ordering::Relaxed)
            }
            _ => self.load(Ordering::SeqCst),
        }
    }

    fn store_with_barrier(&self, val: u64, barrier: BarrierType) {
        match barrier {
            BarrierType::Full => {
                MemoryBarriers::full_barrier();
                self.store(val, Ordering::Relaxed);
                MemoryBarriers::full_barrier();
            }
            BarrierType::Acquire => {
                self.store(val, Ordering::Relaxed);
                MemoryBarriers::acquire_barrier();
            }
            BarrierType::Release => self.store(val, Ordering::Release),
            _ => self.store(val, Ordering::SeqCst),
        }
    }
}

impl AtomicExt<usize> for AtomicUsize {
    fn compare_exchange_explicit(
        &self,
        current: usize,
        new: usize,
        success_order: Ordering,
        failure_order: Ordering,
    ) -> Result<usize, usize> {
        self.compare_exchange(current, new, success_order, failure_order)
    }

    fn fetch_add_explicit(&self, val: usize, order: Ordering) -> usize {
        self.fetch_add(val, order)
    }

    fn fetch_sub_explicit(&self, val: usize, order: Ordering) -> usize {
        self.fetch_sub(val, order)
    }

    fn load_with_barrier(&self, barrier: BarrierType) -> usize {
        match barrier {
            BarrierType::Full => {
                MemoryBarriers::full_barrier();
                let val = self.load(Ordering::Relaxed);
                MemoryBarriers::full_barrier();
                val
            }
            BarrierType::Acquire => self.load(Ordering::Acquire),
            BarrierType::Release => {
                MemoryBarriers::release_barrier();
                self.load(Ordering::Relaxed)
            }
            _ => self.load(Ordering::SeqCst),
        }
    }

    fn store_with_barrier(&self, val: usize, barrier: BarrierType) {
        match barrier {
            BarrierType::Full => {
                MemoryBarriers::full_barrier();
                self.store(val, Ordering::Relaxed);
                MemoryBarriers::full_barrier();
            }
            BarrierType::Acquire => {
                self.store(val, Ordering::Relaxed);
                MemoryBarriers::acquire_barrier();
            }
            BarrierType::Release => self.store(val, Ordering::Release),
            _ => self.store(val, Ordering::SeqCst),
        }
    }
}

pub struct LockFreeUtils;

impl LockFreeUtils {
    /// Perform an atomic read-modify-write operation with retry.
    pub fn atomic_update<F>(atomic: &AtomicU64, mut updater: F) -> u64
    where
        F: FnMut(u64) -> u64,
    {
        let mut current = atomic.load(Ordering::Acquire);
        loop {
            let new_value = updater(current);
            match atomic.compare_exchange_weak(
                current,
                new_value,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return new_value,
                Err(actual) => current = actual,
            }
        }
    }

    pub fn atomic_increment_bounded(atomic: &AtomicU64, max_value: u64) -> Result<u64, u64> {
        let final_val = Self::atomic_update(atomic, |current| {
            if current >= max_value {
                current
            } else {
                current + 1
            }
        });

        if final_val > max_value {
            Err(final_val)
        } else {
            Ok(final_val)
        }
    }

    pub fn atomic_decrement_bounded(atomic: &AtomicU64, min_value: u64) -> Result<u64, u64> {
        let final_val = Self::atomic_update(atomic, |current| {
            if current <= min_value {
                current
            } else {
                current - 1
            }
        });

        if final_val < min_value {
            Err(final_val)
        } else {
            Ok(final_val)
        }
    }

    pub fn double_checked_init<T, F>(atomic_flag: &AtomicUsize, initializer: F) -> bool
    where
        F: FnOnce() -> T,
    {
        if atomic_flag.load(Ordering::Acquire) != 0 {
            return true;
        }

        MemoryBarriers::full_barrier();

        if atomic_flag.load(Ordering::Acquire) == 0 {
            if atomic_flag
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let _result = initializer();

                atomic_flag.store(2, Ordering::Release);
                return true;
            }
        }

        while atomic_flag.load(Ordering::Acquire) == 1 {
            core::hint::spin_loop();
        }

        atomic_flag.load(Ordering::Acquire) == 2
    }
}

pub struct CacheInfo;

impl CacheInfo {
    pub const fn cache_line_size() -> usize {
        64
    }

    pub const fn align_to_cache_line(size: usize) -> usize {
        let cache_size = Self::cache_line_size();
        (size + cache_size - 1) & !(cache_size - 1)
    }

    pub fn is_cache_line_aligned(addr: *const u8) -> bool {
        (addr as usize) & (Self::cache_line_size() - 1) == 0
    }
}

/// Padding structure to prevent false sharing.
#[repr(align(64))]
#[derive(Debug, Default)]
pub struct CacheLinePadded<T> {
    pub value: T,
    _padding: [u8; 0],
}

impl<T> CacheLinePadded<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            _padding: [],
        }
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }
}
