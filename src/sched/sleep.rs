//! Timed sleeps: a deadline-ordered queue checked on every timer tick.

use super::{mark_current_blocked, schedule, set_blocked_on, wake, BlockedOn, Tcb};
use crate::arch;
use crate::sync::spin::IrqSpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

static SLEEPERS: IrqSpinLock<Vec<(u64, Arc<Tcb>)>> = IrqSpinLock::new(Vec::new());
static EARLIEST: AtomicU64 = AtomicU64::new(u64::MAX);

/// Block the current thread until the counter reaches `deadline`.
pub(crate) fn sleep_until(deadline: u64) {
    let irq = arch::irq_save();
    {
        let mut q = SLEEPERS.lock_irqs_off();
        let me = mark_current_blocked();
        let pos = q.partition_point(|(d, _)| *d <= deadline);
        q.insert(pos, (deadline, me));
        EARLIEST.store(q[0].0, Ordering::Release);
    }
    schedule();
    arch::irq_restore(irq);
}

/// Like [`sleep_until`], but return `false` early if the thread is
/// interrupted (see [`super::interrupt`]). Returns `true` if the deadline
/// was reached.
pub(crate) fn sleep_until_interruptible(deadline: u64) -> bool {
    let me = super::current();
    set_blocked_on(&me, BlockedOn::Sleep);
    let irq = arch::irq_save();
    let queued = {
        let mut q = SLEEPERS.lock_irqs_off();
        if me.interrupt_pending.load(Ordering::SeqCst) {
            false
        } else {
            let t = mark_current_blocked();
            let pos = q.partition_point(|(d, _)| *d <= deadline);
            q.insert(pos, (deadline, t));
            EARLIEST.store(q[0].0, Ordering::Release);
            true
        }
    };
    if queued {
        schedule();
    }
    arch::irq_restore(irq);
    set_blocked_on(&me, BlockedOn::Nothing);
    !me.interrupt_pending.load(Ordering::SeqCst) || arch::timer::counter() >= deadline
}

/// Take `t` off the sleep queue. Returns whether it was there.
pub(crate) fn remove(t: &Arc<Tcb>) -> bool {
    let mut q = SLEEPERS.lock();
    match q.iter().position(|(_, x)| Arc::ptr_eq(x, t)) {
        Some(i) => {
            q.remove(i);
            EARLIEST.store(q.first().map_or(u64::MAX, |(d, _)| *d), Ordering::Release);
            true
        }
        None => false,
    }
}

/// Wake every sleeper whose deadline has passed (called from the tick).
pub(crate) fn wake_expired(now: u64) {
    if now < EARLIEST.load(Ordering::Acquire) {
        return;
    }
    let mut q = SLEEPERS.lock();
    let n = q.partition_point(|(d, _)| *d <= now);
    let expired: Vec<_> = q.drain(..n).collect();
    EARLIEST.store(q.first().map_or(u64::MAX, |(d, _)| *d), Ordering::Release);
    drop(q);
    for (_, t) in expired {
        wake(t);
    }
}
