//! Scheduler trait definition for the new lock-free scheduler architecture.

use crate::thread::{ReadyRef, RunningRef, ThreadId};

/// CPU identifier type.
pub type CpuId = usize;

/// New scheduler trait for lock-free implementations.
///
/// This trait defines the interface that all scheduler implementations must
/// provide. It's designed to support lock-free operation and per-CPU scheduling.
pub trait Scheduler: Send + Sync {
    /// Enqueue a thread that is ready to run.
    ///
    /// This is called when a thread becomes ready to run (either newly created,
    /// woken up from blocking, or preempted and should continue running).
    ///
    /// # Arguments
    ///
    /// * `thread` - Ready thread to enqueue
    fn enqueue(&self, thread: ReadyRef);
    
    /// Pick the next thread to run on the given CPU.
    ///
    /// This is called by the scheduler when a CPU needs a new thread to run.
    /// It should return the thread with the highest priority or best scheduling
    /// characteristics according to the algorithm.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - ID of the CPU requesting the next thread
    ///
    /// # Returns
    ///
    /// The next thread to run, or `None` if no threads are ready.
    fn pick_next(&self, cpu_id: CpuId) -> Option<ReadyRef>;
    
    /// Handle a scheduler tick for the currently running thread.
    ///
    /// This is called periodically from timer interrupts to allow the scheduler
    /// to make preemption decisions. The scheduler should check if the current
    /// thread should be preempted and handle time slice accounting.
    ///
    /// # Arguments
    ///
    /// * `current` - Reference to the currently running thread
    ///
    /// # Returns
    ///
    /// `Some(ready_thread)` if the current thread should be preempted and
    /// replaced with the returned thread. `None` if the current thread should
    /// continue running.
    fn on_tick(&self, current: &RunningRef) -> Option<ReadyRef>;
    
    /// Set the priority of a thread.
    ///
    /// This updates the scheduling priority of the given thread. The scheduler
    /// may need to reorder queues or adjust scheduling parameters.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - ID of the thread to modify
    /// * `priority` - New priority value (0-255, higher = more important)
    fn set_priority(&self, thread_id: ThreadId, priority: u8);
    
    /// Handle a thread yielding the CPU voluntarily.
    ///
    /// This is called when a thread explicitly yields (e.g., via yield_now()).
    /// The scheduler may treat this differently from preemption.
    ///
    /// # Arguments
    ///
    /// * `current` - The thread that is yielding
    fn on_yield(&self, current: RunningRef) {
        // Default implementation: treat yield like normal preemption
        let ready = current.stop_running();
        self.enqueue(ready);
    }
    
    /// Handle a thread blocking (going to sleep).
    ///
    /// This is called when a thread blocks on I/O, synchronization primitives,
    /// or other blocking operations. The thread is removed from scheduling
    /// queues until it's explicitly woken up.
    ///
    /// # Arguments
    ///
    /// * `current` - The thread that is blocking
    fn on_block(&self, current: RunningRef) {
        // When a thread blocks, it's not put back in the ready queue
        current.block();
    }
    
    /// Wake up a blocked thread.
    ///
    /// This is called when a blocked thread should become ready to run again
    /// (e.g., I/O completed, lock acquired, condition signaled).
    ///
    /// # Arguments
    ///
    /// * `thread` - The thread to wake up
    fn wake_up(&self, thread: ReadyRef) {
        self.enqueue(thread);
    }
    
    /// Get scheduler statistics.
    ///
    /// Returns various metrics about the scheduler state for monitoring
    /// and debugging purposes.
    ///
    /// # Returns
    ///
    /// A tuple of (total_threads, runnable_threads, blocked_threads).
    fn stats(&self) -> (usize, usize, usize) {
        // Default implementation returns zeros
        (0, 0, 0)
    }
}

/// Priority levels for threads.
///
/// These are convenience constants for common priority levels.
pub mod priority {
    /// Idle priority - only runs when nothing else is ready
    pub const IDLE: u8 = 0;
    
    /// Low priority - background tasks
    pub const LOW: u8 = 64;
    
    /// Normal priority - default for most threads
    pub const NORMAL: u8 = 128;
    
    /// High priority - important system tasks
    pub const HIGH: u8 = 192;
    
    /// Real-time priority - critical system operations
    pub const REALTIME: u8 = 255;
}