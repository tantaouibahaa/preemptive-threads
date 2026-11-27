//! Wait queues: the primitive every blocking operation is built on.

use crate::arch;
use crate::sched::{self, BlockedOn, Tcb};
use crate::sync::spin::{IrqSpinLock, IrqSpinLockGuard};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::Ordering;

/// An interruptible wait was cut short by
/// [`crate::thread::Thread::interrupt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interrupted;

/// A queue of blocked threads, ordered by priority (FIFO within a
/// priority).
///
/// Lost wake-ups are avoided by always registering the waiter while the
/// state it waits on is still locked: either the queue's own lock
/// ([`WaitQueue::wait_until`]) or a caller-provided lock that is released
/// only after the waiter is queued ([`WaitQueue::block_releasing`]).
pub struct WaitQueue {
    waiters: IrqSpinLock<VecDeque<Arc<Tcb>>>,
}

impl WaitQueue {
    /// Empty wait queue.
    pub const fn new() -> Self {
        Self { waiters: IrqSpinLock::new(VecDeque::new()) }
    }

    fn push_current(q: &mut VecDeque<Arc<Tcb>>) {
        let me = sched::mark_current_blocked();
        let prio = me.priority();
        let pos = q.iter().position(|t| t.priority() < prio).unwrap_or(q.len());
        q.insert(pos, me);
    }

    /// Block the calling thread until `cond()` returns true.
    ///
    /// `cond` is evaluated with the queue locked; whoever makes it true must
    /// call [`wake_one`](Self::wake_one)/[`wake_all`](Self::wake_all)
    /// afterwards.
    pub fn wait_until(&self, mut cond: impl FnMut() -> bool) {
        loop {
            let irq = arch::irq_save();
            {
                let mut q = self.waiters.lock_irqs_off();
                if cond() {
                    drop(q);
                    arch::irq_restore(irq);
                    return;
                }
                Self::push_current(&mut q);
            }
            sched::schedule();
            arch::irq_restore(irq);
        }
    }

    /// Like [`wait_until`](Self::wait_until), but give up with
    /// [`Interrupted`] once the thread is interrupted
    /// ([`crate::thread::Thread::interrupt`]), even while it is blocked.
    ///
    /// `cond` is checked first, so a condition that is already true wins
    /// over a pending interrupt.
    pub fn wait_until_interruptible(&self, mut cond: impl FnMut() -> bool) -> Result<(), Interrupted> {
        let me = sched::current();
        sched::set_blocked_on(&me, BlockedOn::Queue(self as *const Self as usize));
        let mut queued_once = false;
        let r = loop {
            let irq = arch::irq_save();
            {
                let mut q = self.waiters.lock_irqs_off();
                if cond() {
                    drop(q);
                    arch::irq_restore(irq);
                    break Ok(());
                }
                if me.interrupt_pending.load(Ordering::SeqCst) {
                    drop(q);
                    arch::irq_restore(irq);
                    break Err(Interrupted);
                }
                Self::push_current(&mut q);
            }
            queued_once = true;
            sched::schedule();
            arch::irq_restore(irq);
        };
        sched::set_blocked_on(&me, BlockedOn::Nothing);
        if r.is_err() && queued_once {
            // A `wake_one` may have picked us just as we were interrupted:
            // pass it on so no other waiter misses it.
            self.wake_one();
        }
        r
    }

    /// Take `t` off the queue. Returns whether it was queued.
    pub(crate) fn remove(&self, t: &Arc<Tcb>) -> bool {
        let mut q = self.waiters.lock();
        match q.iter().position(|x| Arc::ptr_eq(x, t)) {
            Some(i) => {
                q.remove(i);
                true
            }
            None => false,
        }
    }

    /// Queue the calling thread, then release `guard` and block until woken.
    ///
    /// The caller decided to wait while holding `guard`; a waker that takes
    /// the same lock afterwards is guaranteed to find this thread queued.
    pub fn block_releasing<T: ?Sized>(&self, guard: IrqSpinLockGuard<'_, T>) {
        // `guard` holds IRQs masked.
        {
            let mut q = self.waiters.lock_irqs_off();
            Self::push_current(&mut q);
        }
        let irq = guard.unlock_keep_irqs_masked();
        sched::schedule();
        arch::irq_restore(irq);
    }

    /// Queue the calling thread as blocked; the caller must call
    /// `sched::schedule()` next (IRQs must be masked).
    pub(crate) fn enqueue_current(&self) {
        let mut q = self.waiters.lock_irqs_off();
        Self::push_current(&mut q);
    }

    /// Remove the highest-priority waiter without waking it.
    pub(crate) fn pop(&self) -> Option<Arc<Tcb>> {
        self.waiters.lock().pop_front()
    }

    /// Highest priority among the waiters.
    pub(crate) fn top_priority(&self) -> Option<u8> {
        self.waiters.lock().front().map(|t| t.priority())
    }

    /// Wake the highest-priority waiter. Returns whether one was woken.
    pub fn wake_one(&self) -> bool {
        match self.pop() {
            Some(t) => {
                sched::wake(t);
                true
            }
            None => false,
        }
    }

    /// Wake every waiter. Returns how many were woken.
    pub fn wake_all(&self) -> usize {
        let all: VecDeque<_> = core::mem::take(&mut *self.waiters.lock());
        let n = all.len();
        for t in all {
            sched::wake(t);
        }
        n
    }

    /// Number of queued waiters.
    pub fn len(&self) -> usize {
        self.waiters.lock().len()
    }

    /// Whether nobody is waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
