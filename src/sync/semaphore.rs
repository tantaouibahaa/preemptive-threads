//! Counting semaphore.

use super::spin::IrqSpinLock;
use super::waitqueue::WaitQueue;
use crate::sched;

/// A counting semaphore. Permits released while threads wait are handed
/// directly to the highest-priority waiter.
pub struct Semaphore {
    count: IrqSpinLock<usize>,
    waiters: WaitQueue,
}

impl Semaphore {
    /// Semaphore with `permits` initial permits.
    pub const fn new(permits: usize) -> Self {
        Self { count: IrqSpinLock::new(permits), waiters: WaitQueue::new() }
    }

    /// Take a permit, blocking until one is available.
    pub fn acquire(&self) {
        let mut c = self.count.lock();
        if *c > 0 {
            *c -= 1;
            return;
        }
        // A releaser hands its permit to us directly.
        self.waiters.block_releasing(c);
    }

    /// Take a permit if one is available.
    pub fn try_acquire(&self) -> bool {
        let mut c = self.count.lock();
        if *c > 0 {
            *c -= 1;
            true
        } else {
            false
        }
    }

    /// Return a permit.
    pub fn release(&self) {
        {
            let mut c = self.count.lock();
            match self.waiters.pop() {
                Some(t) => {
                    drop(c);
                    sched::wake(t);
                }
                None => *c += 1,
            }
        }
        sched::preempt_point();
    }

    /// Permits currently available.
    pub fn available(&self) -> usize {
        *self.count.lock()
    }
}
