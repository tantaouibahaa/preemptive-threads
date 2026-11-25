//! Per-core run queues and load balancing.
//!
//! A run queue holds four FIFO bands; the scheduler always serves the
//! highest non-empty band, round-robin within a band.

use super::Tcb;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

/// Number of priority bands.
pub const BANDS: usize = 4;

/// Band of a priority: 0 = idle (0), 1 = low (1-63), 2 = normal (64-191),
/// 3 = high (192-255).
pub const fn band(prio: u8) -> usize {
    match prio {
        0 => 0,
        1..=63 => 1,
        64..=191 => 2,
        _ => 3,
    }
}

/// Ready threads of one core.
pub struct RunQueue {
    bands: [VecDeque<Arc<Tcb>>; BANDS],
}

impl RunQueue {
    /// Empty run queue.
    pub const fn new() -> Self {
        Self { bands: [VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new()] }
    }

    /// Append to the back of the thread's band.
    pub fn push(&mut self, t: Arc<Tcb>) {
        let b = band(t.priority());
        self.bands[b].push_back(t);
    }

    /// Remove and return the first thread, highest band first, for which
    /// `ok` holds.
    pub fn pop_where(&mut self, mut ok: impl FnMut(&Arc<Tcb>) -> bool) -> Option<Arc<Tcb>> {
        for b in (0..BANDS).rev() {
            if let Some(i) = self.bands[b].iter().position(&mut ok) {
                return self.bands[b].remove(i);
            }
        }
        None
    }

    /// Remove a specific thread. Returns whether it was queued here.
    pub fn remove(&mut self, t: &Arc<Tcb>) -> bool {
        for q in self.bands.iter_mut() {
            if let Some(i) = q.iter().position(|x| Arc::ptr_eq(x, t)) {
                q.remove(i);
                return true;
            }
        }
        false
    }

    /// Visit every queued thread, highest band first.
    pub fn for_each(&self, mut f: impl FnMut(&Arc<Tcb>)) {
        for q in self.bands.iter().rev() {
            q.iter().for_each(&mut f);
        }
    }

    /// Highest band that has a queued thread.
    pub fn top_band(&self) -> Option<usize> {
        (0..BANDS).rev().find(|&b| !self.bands[b].is_empty())
    }

    /// Number of queued threads.
    pub fn len(&self) -> usize {
        self.bands.iter().map(|q| q.len()).sum()
    }

    /// Whether no thread is queued.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for RunQueue {
    fn default() -> Self {
        Self::new()
    }
}
