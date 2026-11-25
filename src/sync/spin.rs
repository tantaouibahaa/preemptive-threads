//! Spinlocks.
//!
//! [`SpinLock`] is a plain test-and-test-and-set lock. [`IrqSpinLock`] also
//! masks IRQs on the local core while held, which is required for any data
//! touched from interrupt context or by the scheduler (otherwise a timer
//! interrupt could preempt the holder and deadlock the core).

use crate::arch::{self, IrqState};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// A spinlock that does not touch the interrupt mask.
pub struct SpinLock<T: ?Sized> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: the lock serialises all access to `data`.
unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}
// SAFETY: moving the lock moves the data.
unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// Create an unlocked spinlock.
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), data: UnsafeCell::new(value) }
    }
}

impl<T: ?Sized> SpinLock<T> {
    /// Acquire the lock, spinning until it is available.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return SpinLockGuard { lock: self };
            }
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    /// Try to acquire the lock without spinning.
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| SpinLockGuard { lock: self })
    }

    /// Whether the lock is currently held (racy, for diagnostics).
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    /// Release the lock without a guard.
    ///
    /// # Safety
    ///
    /// The caller must own the lock (e.g. after `mem::forget` of a guard).
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

/// Guard for [`SpinLock`].
pub struct SpinLockGuard<'a, T: ?Sized> {
    lock: &'a SpinLock<T>,
}

impl<T: ?Sized> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves exclusive access.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves exclusive access.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// A spinlock that masks IRQs on the local core while held.
pub struct IrqSpinLock<T: ?Sized> {
    inner: SpinLock<T>,
}

impl<T> IrqSpinLock<T> {
    /// Create an unlocked lock.
    pub const fn new(value: T) -> Self {
        Self { inner: SpinLock::new(value) }
    }
}

impl<T: ?Sized> IrqSpinLock<T> {
    /// Mask IRQs and acquire the lock. The previous mask is restored when
    /// the guard is dropped.
    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let irq = arch::irq_save();
        let guard = self.inner.lock();
        IrqSpinLockGuard { guard: Some(guard), irq }
    }

    /// Try to acquire without spinning.
    pub fn try_lock(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let irq = arch::irq_save();
        match self.inner.try_lock() {
            Some(guard) => Some(IrqSpinLockGuard { guard: Some(guard), irq }),
            None => {
                arch::irq_restore(irq);
                None
            }
        }
    }

    /// Acquire the lock when IRQs are already known to be masked.
    pub fn lock_irqs_off(&self) -> SpinLockGuard<'_, T> {
        debug_assert!(!arch::irqs_enabled());
        self.inner.lock()
    }

    /// Whether the lock is currently held (racy, for diagnostics).
    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }
}

/// Guard for [`IrqSpinLock`].
pub struct IrqSpinLockGuard<'a, T: ?Sized> {
    guard: Option<SpinLockGuard<'a, T>>,
    irq: IrqState,
}

impl<'a, T: ?Sized> IrqSpinLockGuard<'a, T> {
    /// Release the lock but keep IRQs masked; returns the mask to restore
    /// later with [`arch::irq_restore`]. Used by blocking primitives that
    /// must drop a lock and switch threads atomically.
    pub fn unlock_keep_irqs_masked(mut self) -> IrqState {
        drop(self.guard.take());
        self.irq
    }
}

impl<T: ?Sized> Deref for IrqSpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().unwrap()
    }
}

impl<T: ?Sized> DerefMut for IrqSpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().unwrap()
    }
}

impl<T: ?Sized> Drop for IrqSpinLockGuard<'_, T> {
    fn drop(&mut self) {
        if let Some(g) = self.guard.take() {
            drop(g);
            arch::irq_restore(self.irq);
        }
    }
}
