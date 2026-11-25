//! Synchronisation primitives.
//!
//! - [`SpinLock`] / [`IrqSpinLock`]: busy-waiting locks for short critical
//!   sections (the latter also masks IRQs).
//! - [`WaitQueue`]: the blocking building block (`block` / `wake_one` /
//!   `wake_all`) used by everything below.
//! - [`Mutex`]: blocking mutex with priority inheritance and direct
//!   hand-off to the highest-priority waiter.
//! - [`Semaphore`], [`Condvar`] and bounded MPMC [`channel()`]s.

pub mod channel;
mod condvar;
mod mutex;
mod semaphore;
pub mod spin;
mod waitqueue;

pub use channel::{channel, Receiver, RecvError, SendError, Sender, TryRecvError, TrySendError};
pub use condvar::Condvar;
pub use mutex::{Mutex, MutexGuard};
pub use semaphore::Semaphore;
pub use spin::{IrqSpinLock, IrqSpinLockGuard, SpinLock, SpinLockGuard};
pub use waitqueue::WaitQueue;
