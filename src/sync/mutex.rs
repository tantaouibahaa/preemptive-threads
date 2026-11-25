//! Blocking mutex with priority inheritance.

use super::spin::IrqSpinLock;
use super::waitqueue::WaitQueue;
use crate::sched::{self, Tcb};
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

struct State {
    owner: Option<Arc<Tcb>>,
    /// Owner's effective priority before a waiter boosted it.
    boosted_from: Option<u8>,
}

/// A mutual-exclusion lock that blocks (instead of spinning) when
/// contended.
///
/// - **Priority inheritance**: when a thread blocks on a mutex held by a
///   lower-priority thread, the owner runs at the waiter's priority until it
///   unlocks, so a medium-priority thread cannot starve it (no unbounded
///   priority inversion). Inheritance is one level deep (it is not
///   propagated along chains of mutexes) and boosts are undone at unlock,
///   which is exact when nested mutexes are released in reverse order.
/// - **Hand-off**: unlock passes ownership directly to the highest-priority
///   waiter, so waiters cannot be starved by threads that keep re-locking.
/// - Not reentrant: locking a mutex you already hold panics.
pub struct Mutex<T: ?Sized> {
    state: IrqSpinLock<State>,
    waiters: WaitQueue,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is serialised by the lock.
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}
// SAFETY: as above.
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// New unlocked mutex.
    pub const fn new(value: T) -> Self {
        Self {
            state: IrqSpinLock::new(State { owner: None, boosted_from: None }),
            waiters: WaitQueue::new(),
            data: UnsafeCell::new(value),
        }
    }

    /// Consume the mutex and return the data.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Acquire the mutex, blocking while another thread holds it.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let me = sched::current();
        let mut st = self.state.lock();
        match &st.owner {
            None => {
                st.owner = Some(me);
                return MutexGuard { mutex: self };
            }
            Some(owner) if Arc::ptr_eq(owner, &me) => panic!("Mutex locked twice by thread {}", me.id),
            Some(owner) => {
                let owner = owner.clone();
                if me.priority() > owner.priority() {
                    if st.boosted_from.is_none() {
                        st.boosted_from = Some(owner.priority());
                    }
                    sched::set_effective_priority(&owner, me.priority());
                }
            }
        }
        drop(me);
        self.waiters.block_releasing(st);
        // Ownership was handed to us by unlock().
        MutexGuard { mutex: self }
    }

    /// Acquire the mutex if it is free.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let mut st = self.state.lock();
        if st.owner.is_none() {
            st.owner = Some(sched::current());
            Some(MutexGuard { mutex: self })
        } else {
            None
        }
    }

    /// Whether the mutex is currently held.
    pub fn is_locked(&self) -> bool {
        self.state.lock().owner.is_some()
    }

    /// Mutable access without locking (requires `&mut self`).
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    pub(crate) fn unlock(&self) {
        {
            // Everything below runs with IRQs masked (the state lock is an
            // IrqSpinLock). In particular the next owner is woken *before*
            // our own boost is dropped: once de-boosted we may be preempted
            // by a medium-priority thread, and if the hand-off had not
            // happened yet the waiter would never be woken.
            let mut st = self.state.lock();
            let me = st.owner.take().expect("unlock of an unlocked Mutex");
            let my_boost = st.boosted_from.take();
            if let Some(next) = self.waiters.pop() {
                st.owner = Some(next.clone());
                // Remaining waiters may outrank the new owner.
                if let Some(top) = self.waiters.top_priority() {
                    if top > next.priority() {
                        st.boosted_from = Some(next.priority());
                        sched::set_effective_priority(&next, top);
                    }
                }
                sched::wake(next);
            }
            if let Some(p) = my_boost {
                sched::set_effective_priority(&me, p.max(me.base_priority()));
            }
        }
        sched::preempt_point();
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// RAII guard returned by [`Mutex::lock`].
pub struct MutexGuard<'a, T: ?Sized> {
    pub(crate) mutex: &'a Mutex<T>,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves ownership.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves ownership.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}
