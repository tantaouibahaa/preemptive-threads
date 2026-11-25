//! Bounded multi-producer multi-consumer channels.

use super::spin::IrqSpinLock;
use super::waitqueue::WaitQueue;
use crate::sched;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

struct State<T> {
    buf: VecDeque<T>,
    cap: usize,
    senders: usize,
    receivers: usize,
}

struct Chan<T> {
    state: IrqSpinLock<State<T>>,
    not_empty: WaitQueue,
    not_full: WaitQueue,
}

/// Create a bounded channel holding at most `capacity` messages
/// (`capacity` must be at least 1). Senders block while it is full,
/// receivers while it is empty. Messages from one sender arrive in order.
pub fn channel<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    assert!(capacity > 0, "channel capacity must be at least 1");
    let chan = Arc::new(Chan {
        state: IrqSpinLock::new(State { buf: VecDeque::with_capacity(capacity), cap: capacity, senders: 1, receivers: 1 }),
        not_empty: WaitQueue::new(),
        not_full: WaitQueue::new(),
    });
    (Sender { chan: chan.clone() }, Receiver { chan })
}

/// Sending half of a channel. Clone it for more producers.
pub struct Sender<T> {
    chan: Arc<Chan<T>>,
}

/// Receiving half of a channel. Clone it for more consumers.
pub struct Receiver<T> {
    chan: Arc<Chan<T>>,
}

/// The message could not be sent because every receiver is gone.
#[derive(Debug, PartialEq, Eq)]
pub struct SendError<T>(pub T);

/// Error from [`Sender::try_send`].
#[derive(Debug, PartialEq, Eq)]
pub enum TrySendError<T> {
    /// The channel is full.
    Full(T),
    /// Every receiver is gone.
    Disconnected(T),
}

/// Every sender is gone and the channel is empty.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct RecvError;

/// Error from [`Receiver::try_recv`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TryRecvError {
    /// The channel is empty.
    Empty,
    /// Every sender is gone and the channel is empty.
    Disconnected,
}

impl<T: Send> Sender<T> {
    /// Send a message, blocking while the channel is full.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        loop {
            let mut st = self.chan.state.lock();
            if st.receivers == 0 {
                return Err(SendError(value));
            }
            if st.buf.len() < st.cap {
                st.buf.push_back(value);
                drop(st);
                self.chan.not_empty.wake_one();
                sched::preempt_point();
                return Ok(());
            }
            self.chan.not_full.block_releasing(st);
        }
    }

    /// Send without blocking.
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut st = self.chan.state.lock();
        if st.receivers == 0 {
            return Err(TrySendError::Disconnected(value));
        }
        if st.buf.len() >= st.cap {
            return Err(TrySendError::Full(value));
        }
        st.buf.push_back(value);
        drop(st);
        self.chan.not_empty.wake_one();
        Ok(())
    }
}

impl<T: Send> Receiver<T> {
    /// Receive a message, blocking while the channel is empty.
    pub fn recv(&self) -> Result<T, RecvError> {
        loop {
            let mut st = self.chan.state.lock();
            if let Some(v) = st.buf.pop_front() {
                drop(st);
                self.chan.not_full.wake_one();
                sched::preempt_point();
                return Ok(v);
            }
            if st.senders == 0 {
                return Err(RecvError);
            }
            self.chan.not_empty.block_releasing(st);
        }
    }

    /// Receive without blocking.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut st = self.chan.state.lock();
        match st.buf.pop_front() {
            Some(v) => {
                drop(st);
                self.chan.not_full.wake_one();
                Ok(v)
            }
            None if st.senders == 0 => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.chan.state.lock().senders += 1;
        Self { chan: self.chan.clone() }
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        self.chan.state.lock().receivers += 1;
        Self { chan: self.chan.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let last = {
            let mut st = self.chan.state.lock();
            st.senders -= 1;
            st.senders == 0
        };
        if last {
            self.chan.not_empty.wake_all();
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let last = {
            let mut st = self.chan.state.lock();
            st.receivers -= 1;
            st.receivers == 0
        };
        if last {
            self.chan.not_full.wake_all();
        }
    }
}
