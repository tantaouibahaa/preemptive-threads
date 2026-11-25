//! Condition variable for use with [`Mutex`](super::Mutex).

use super::mutex::MutexGuard;
use super::waitqueue::WaitQueue;
use crate::{arch, sched};

/// A condition variable.
///
/// [`wait`](Condvar::wait) atomically releases the mutex and blocks; it
/// re-acquires the mutex before returning. As with any condition variable,
/// re-check the predicate in a loop (or use
/// [`wait_while`](Condvar::wait_while)).
pub struct Condvar {
    waiters: WaitQueue,
}

impl Condvar {
    /// New condition variable.
    pub const fn new() -> Self {
        Self { waiters: WaitQueue::new() }
    }

    /// Release `guard`'s mutex, block until notified, then re-lock.
    pub fn wait<'a, T: ?Sized>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        let mutex = guard.mutex;
        let irq = arch::irq_save();
        // Queue ourselves before the mutex is released so a notify issued
        // by the next owner cannot be missed.
        self.waiters.enqueue_current();
        core::mem::forget(guard);
        mutex.unlock();
        sched::schedule();
        arch::irq_restore(irq);
        mutex.lock()
    }

    /// Wait until `cond` returns false.
    pub fn wait_while<'a, T: ?Sized>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut cond: impl FnMut(&mut T) -> bool,
    ) -> MutexGuard<'a, T> {
        while cond(&mut *guard) {
            guard = self.wait(guard);
        }
        guard
    }

    /// Wake one waiter.
    pub fn notify_one(&self) {
        self.waiters.wake_one();
        sched::preempt_point();
    }

    /// Wake all waiters.
    pub fn notify_all(&self) {
        self.waiters.wake_all();
        sched::preempt_point();
    }
}

impl Default for Condvar {
    fn default() -> Self {
        Self::new()
    }
}
