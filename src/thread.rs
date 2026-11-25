//! Thread API: spawning, joining, sleeping and yielding.

use crate::errors::SpawnError;
use crate::mem::DEFAULT_STACK_SIZE;
use crate::sched::{self, priority, NewThread, Tcb, ThreadId};
use crate::sync::{IrqSpinLock, WaitQueue};
use crate::time::{Duration, Instant};
use crate::arch;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

/// Configuration for a new thread.
///
/// ```ignore
/// let h = Builder::new().name("worker").priority(priority::HIGH).cpu(2).spawn(|| 42)?;
/// assert_eq!(h.join(), 42);
/// ```
#[derive(Debug, Clone)]
pub struct Builder {
    name: Option<String>,
    priority: u8,
    stack_size: usize,
    cpu: Option<usize>,
}

impl Builder {
    /// Defaults: no name, [`priority::NORMAL`], 64 KiB stack, any core.
    pub fn new() -> Self {
        Self { name: None, priority: priority::NORMAL, stack_size: DEFAULT_STACK_SIZE, cpu: None }
    }

    /// Thread name (for diagnostics).
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Scheduling priority (0-255, higher runs first).
    pub fn priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Stack size in bytes (rounded up to 4 KiB, at least 8 KiB).
    pub fn stack_size(mut self, bytes: usize) -> Self {
        self.stack_size = bytes;
        self
    }

    /// Pin the thread to core `cpu`.
    pub fn cpu(mut self, cpu: usize) -> Self {
        self.cpu = Some(cpu);
        self
    }

    /// Spawn the thread. Its return value is delivered by
    /// [`JoinHandle::join`].
    pub fn spawn<F, T>(self, f: F) -> Result<JoinHandle<T>, SpawnError>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let packet = Arc::new(Packet { result: IrqSpinLock::new(None), done: AtomicBool::new(false), waiters: WaitQueue::new() });
        let theirs = packet.clone();
        let main = Box::new(move || {
            let value = f();
            *theirs.result.lock() = Some(value);
            theirs.done.store(true, Ordering::Release);
            theirs.waiters.wake_all();
        });
        let tcb = sched::create_thread(
            NewThread { name: self.name, priority: self.priority, stack_size: self.stack_size, cpu: self.cpu },
            main,
        )?;
        Ok(JoinHandle { thread: Thread { tcb }, packet })
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

struct Packet<T> {
    result: IrqSpinLock<Option<T>>,
    done: AtomicBool,
    waiters: WaitQueue,
}

/// Owned permission to wait for a thread and take its result. Dropping it
/// detaches the thread.
pub struct JoinHandle<T> {
    thread: Thread,
    packet: Arc<Packet<T>>,
}

impl<T> JoinHandle<T> {
    /// Block until the thread finishes and return its result.
    pub fn join(self) -> T {
        let p = &self.packet;
        p.waiters.wait_until(|| p.done.load(Ordering::Acquire));
        let value = p.result.lock().take();
        value.expect("thread result already taken")
    }

    /// Whether the thread has finished.
    pub fn is_finished(&self) -> bool {
        self.packet.done.load(Ordering::Acquire)
    }

    /// The thread being joined.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

/// A handle to a thread.
#[derive(Clone)]
pub struct Thread {
    tcb: Arc<Tcb>,
}

impl Thread {
    /// Thread id.
    pub fn id(&self) -> ThreadId {
        self.tcb.id
    }

    /// Thread name, if any.
    pub fn name(&self) -> Option<&str> {
        self.tcb.name.as_deref()
    }

    /// Effective priority (including priority-inheritance boosts).
    pub fn priority(&self) -> u8 {
        self.tcb.priority()
    }

    /// Change the thread's base priority.
    pub fn set_priority(&self, prio: u8) {
        sched::set_base_priority(&self.tcb, prio);
    }

    /// Lifecycle state.
    pub fn state(&self) -> sched::ThreadState {
        self.tcb.state()
    }

    /// Bitmask of the cores this thread has run on (bit n = core n).
    pub fn cpus_ran(&self) -> u8 {
        sched::cpus_ran(&self.tcb)
    }
}

impl core::fmt::Debug for Thread {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Thread").field("id", &self.id()).field("name", &self.name()).finish()
    }
}

/// Spawn a thread with default settings. Panics if out of memory.
pub fn spawn<F, T>(f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(f).expect("failed to spawn thread")
}

/// Handle to the calling thread.
pub fn current() -> Thread {
    Thread { tcb: sched::current() }
}

/// Id of the calling thread.
pub fn current_thread_id() -> ThreadId {
    sched::with_current(|t| t.id)
}

/// Index of the core the caller is running on (may change at any
/// preemption point unless the thread is pinned).
pub fn current_cpu() -> usize {
    let irq = arch::irq_save();
    let id = sched::percpu::this_cpu().id;
    arch::irq_restore(irq);
    id
}

/// Give up the core to other ready threads of the same or higher priority.
pub fn yield_now() {
    sched::yield_now();
}

/// Block the calling thread for at least `d`.
pub fn sleep(d: Duration) {
    if d.is_zero() {
        yield_now();
        return;
    }
    sleep_until(Instant::now() + d);
}

/// Block the calling thread until `deadline`.
pub fn sleep_until(deadline: Instant) {
    if Instant::now() >= deadline {
        return;
    }
    sched::sleep::sleep_until(deadline.ticks());
}
