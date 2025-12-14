//! Round-robin scheduler implementation with lock-free queues.

use super::trait_def::{CpuId, Scheduler};
use crate::thread_new::{ReadyRef, RunningRef, ThreadId};
use portable_atomic::{AtomicPtr, AtomicUsize, Ordering};
use core::ptr;
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

/// Lock-free round-robin scheduler.
///
/// This scheduler maintains per-CPU run queues using lock-free ring buffers.
/// Threads are scheduled in round-robin fashion with priority-based time slicing.
/// Load balancing occurs when CPUs become idle or overloaded.
pub struct RoundRobinScheduler {
    /// Number of CPUs in the system
    num_cpus: usize,
    /// Per-CPU run queues
    run_queues: Box<[CpuRunQueue]>,
    /// Global statistics
    total_threads: AtomicUsize,
    runnable_threads: AtomicUsize,
}

/// Per-CPU run queue with priority levels.
struct CpuRunQueue {
    /// High priority queue (192-255)
    high_priority: LockFreeQueue,
    /// Normal priority queue (64-191)
    normal_priority: LockFreeQueue,
    /// Low priority queue (1-63)
    low_priority: LockFreeQueue,
    /// Idle priority queue (0)
    idle_priority: LockFreeQueue,
    /// Current queue position for round-robin within priority level
    current_pos: AtomicUsize,
    /// Thread count for load balancing
    thread_count: AtomicUsize,
}

/// Lock-free MPMC queue implementation using Michael & Scott algorithm.
struct LockFreeQueue {
    head: AtomicPtr<QueueNode>,
    tail: AtomicPtr<QueueNode>,
}

/// Queue node for lock-free linked list.
struct QueueNode {
    thread: Option<ReadyRef>,
    next: AtomicPtr<QueueNode>,
}

impl RoundRobinScheduler {
    /// Create a new round-robin scheduler for the given number of CPUs.
    pub fn new(num_cpus: usize) -> Self {
        // Allocate per-CPU run queues
        let mut run_queues = Vec::with_capacity(num_cpus);
        for _ in 0..num_cpus {
            run_queues.push(CpuRunQueue::new());
        }

        Self {
            num_cpus,
            run_queues: run_queues.into_boxed_slice(),
            total_threads: AtomicUsize::new(0),
            runnable_threads: AtomicUsize::new(0),
        }
    }

    /// Get the priority level for a thread priority value.
    fn priority_level(priority: u8) -> PriorityLevel {
        match priority {
            0 => PriorityLevel::Idle,
            1..=63 => PriorityLevel::Low,
            64..=191 => PriorityLevel::Normal,
            192..=255 => PriorityLevel::High,
        }
    }

    /// Select the best CPU for thread placement.
    ///
    /// Uses simple load balancing - find CPU with fewest threads.
    fn select_cpu(&self) -> CpuId {
        let mut best_cpu = 0;
        let mut min_threads = self.run_queues[0].thread_count.load(Ordering::Acquire);

        for (cpu_id, queue) in self.run_queues.iter().enumerate().skip(1) {
            let thread_count = queue.thread_count.load(Ordering::Acquire);
            if thread_count < min_threads {
                min_threads = thread_count;
                best_cpu = cpu_id;
            }
        }

        best_cpu
    }

    /// Attempt work stealing from other CPUs.
    fn try_steal_work(&self, requesting_cpu: CpuId) -> Option<ReadyRef> {
        // Start from a random CPU to avoid always stealing from CPU 0
        let start_cpu = (requesting_cpu + 1) % self.num_cpus;
        
        for i in 0..self.num_cpus {
            let victim_cpu = (start_cpu + i) % self.num_cpus;
            if victim_cpu == requesting_cpu {
                continue; // Don't steal from ourselves
            }

            let victim_queue = &self.run_queues[victim_cpu];
            
            // Try to steal from normal priority first (most likely to have work)
            if let Some(thread) = victim_queue.normal_priority.try_pop() {
                victim_queue.thread_count.fetch_sub(1, Ordering::AcqRel);
                return Some(thread);
            }

            // Then try low priority
            if let Some(thread) = victim_queue.low_priority.try_pop() {
                victim_queue.thread_count.fetch_sub(1, Ordering::AcqRel);
                return Some(thread);
            }
        }

        None
    }
}

impl Scheduler for RoundRobinScheduler {
    fn enqueue(&self, thread: ReadyRef) {
        let priority = thread.priority();
        let cpu_id = self.select_cpu();
        let queue = &self.run_queues[cpu_id];
        
        let priority_queue = match Self::priority_level(priority) {
            PriorityLevel::High => &queue.high_priority,
            PriorityLevel::Normal => &queue.normal_priority,
            PriorityLevel::Low => &queue.low_priority,
            PriorityLevel::Idle => &queue.idle_priority,
        };

        priority_queue.push(thread);
        queue.thread_count.fetch_add(1, Ordering::AcqRel);
        self.runnable_threads.fetch_add(1, Ordering::AcqRel);
    }

    fn pick_next(&self, cpu_id: CpuId) -> Option<ReadyRef> {
        if cpu_id >= self.num_cpus {
            return None;
        }

        let queue = &self.run_queues[cpu_id];

        // Try priority queues in order: high -> normal -> low -> idle
        if let Some(thread) = queue.high_priority.try_pop() {
            queue.thread_count.fetch_sub(1, Ordering::AcqRel);
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        if let Some(thread) = queue.normal_priority.try_pop() {
            queue.thread_count.fetch_sub(1, Ordering::AcqRel);
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        if let Some(thread) = queue.low_priority.try_pop() {
            queue.thread_count.fetch_sub(1, Ordering::AcqRel);
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        if let Some(thread) = queue.idle_priority.try_pop() {
            queue.thread_count.fetch_sub(1, Ordering::AcqRel);
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        // No local work, try work stealing
        if let Some(thread) = self.try_steal_work(cpu_id) {
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        None
    }

    fn on_tick(&self, current: &RunningRef) -> Option<ReadyRef> {
        // Check if the current thread's time slice is expired
        if current.time_slice().should_preempt() {
            // Convert running thread back to ready
            let ready = current.prepare_preemption();
            
            // Find a higher priority thread to run instead
            let cpu_id = current.last_cpu();
            
            // Only preempt if there's higher priority work available
            if cpu_id < self.num_cpus {
                let queue = &self.run_queues[cpu_id];
                let current_priority = current.priority();
                
                // Check for higher priority threads
                match Self::priority_level(current_priority) {
                    PriorityLevel::Idle => {
                        // Idle can be preempted by anything
                        if let Some(next) = queue.low_priority.peek()
                            .or_else(|| queue.normal_priority.peek())
                            .or_else(|| queue.high_priority.peek()) {
                            return Some(ready);
                        }
                    },
                    PriorityLevel::Low => {
                        // Low can be preempted by normal/high
                        if let Some(next) = queue.normal_priority.peek()
                            .or_else(|| queue.high_priority.peek()) {
                            return Some(ready);
                        }
                    },
                    PriorityLevel::Normal => {
                        // Normal can be preempted by high
                        if queue.high_priority.peek().is_some() {
                            return Some(ready);
                        }
                    },
                    PriorityLevel::High => {
                        // High priority threads run to completion of time slice
                        // but can be preempted by other high priority threads
                        return Some(ready);
                    },
                }
            }
        }

        None
    }

    fn set_priority(&self, thread_id: ThreadId, priority: u8) {
        // Priority changes take effect on next scheduling decision
        // The thread's priority is stored in its ThreadRef, so this is a no-op
        // for the scheduler data structures. The change will be visible when
        // the thread is next enqueued.
        let _ = (thread_id, priority);
    }

    fn on_yield(&self, current: RunningRef) {
        // Yielding threads go to the back of their priority queue
        let ready = current.stop_running();
        self.enqueue(ready);
    }

    fn on_block(&self, current: RunningRef) {
        // Blocked threads are not enqueued - they wait for wake_up
        current.block();
    }

    fn wake_up(&self, thread: ReadyRef) {
        self.enqueue(thread);
    }

    fn stats(&self) -> (usize, usize, usize) {
        let total = self.total_threads.load(Ordering::Acquire);
        let runnable = self.runnable_threads.load(Ordering::Acquire);
        let blocked = total.saturating_sub(runnable);
        (total, runnable, blocked)
    }
}

impl CpuRunQueue {
    fn new() -> Self {
        Self {
            high_priority: LockFreeQueue::new(),
            normal_priority: LockFreeQueue::new(),
            low_priority: LockFreeQueue::new(),
            idle_priority: LockFreeQueue::new(),
            current_pos: AtomicUsize::new(0),
            thread_count: AtomicUsize::new(0),
        }
    }
}

impl LockFreeQueue {
    fn new() -> Self {
        let dummy = Box::into_raw(Box::new(QueueNode {
            thread: None,
            next: AtomicPtr::new(ptr::null_mut()),
        }));

        Self {
            head: AtomicPtr::new(dummy),
            tail: AtomicPtr::new(dummy),
        }
    }

    fn push(&self, thread: ReadyRef) {
        let new_node = Box::into_raw(Box::new(QueueNode {
            thread: Some(thread),
            next: AtomicPtr::new(ptr::null_mut()),
        }));

        loop {
            let tail = self.tail.load(Ordering::Acquire);
            let next = unsafe { (*tail).next.load(Ordering::Acquire) };

            // Double-check that tail hasn't changed (ABA prevention)
            if tail == self.tail.load(Ordering::Acquire) {
                if next.is_null() {
                    // Try to link new node at the end of the list
                    if unsafe { (*tail).next.compare_exchange_weak(
                        ptr::null_mut(),
                        new_node,
                        Ordering::Release,  // Success: synchronizes with acquire in try_pop
                        Ordering::Relaxed   // Failure: just retry
                    ).is_ok() } {
                        break;
                    }
                } else {
                    // Tail was lagging, try to advance it
                    let _ = self.tail.compare_exchange_weak(
                        tail,
                        next,
                        Ordering::Release,  // Success: make new tail visible
                        Ordering::Relaxed   // Failure: someone else advanced it
                    );
                }
            }
        }

        // Try to advance tail to point to the new node
        // This may fail if another thread already advanced it, which is fine
        let _ = self.tail.compare_exchange_weak(
            self.tail.load(Ordering::Acquire),
            new_node,
            Ordering::Release,  // Make the new tail visible to other threads
            Ordering::Relaxed
        );
    }

    fn try_pop(&self) -> Option<ReadyRef> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);
            let next = unsafe { (*head).next.load(Ordering::Acquire) };

            // Double-check head consistency (ABA prevention)
            if head == self.head.load(Ordering::Acquire) {
                if head == tail {
                    if next.is_null() {
                        return None; // Queue is definitely empty
                    }
                    // Queue appears empty but tail is lagging, help advance tail
                    let _ = self.tail.compare_exchange_weak(
                        tail,
                        next,
                        Ordering::Release,  // Make tail advancement visible
                        Ordering::Relaxed
                    );
                } else {
                    if next.is_null() {
                        continue; // Inconsistent state, retry
                    }

                    // Speculatively read the thread from next node
                    // This must be done before the CAS to avoid races
                    let thread = unsafe { (*next).thread.take() };
                    
                    // Try to advance head to next node
                    if self.head.compare_exchange_weak(
                        head,
                        next,
                        Ordering::Release,  // Success: make new head visible
                        Ordering::Relaxed   // Failure: retry
                    ).is_ok() {
                        // Successfully advanced head, safe to free old head
                        unsafe {
                            drop(Box::from_raw(head));
                        }
                        return thread;
                    } else {
                        // CAS failed, put thread back (if we got one)
                        if let Some(t) = thread {
                            unsafe {
                                (*next).thread = Some(t);
                            }
                        }
                    }
                }
            }
        }
    }

    fn peek(&self) -> Option<&ReadyRef> {
        let head = self.head.load(Ordering::Acquire);
        let next = unsafe { (*head).next.load(Ordering::Acquire) };
        
        if next.is_null() {
            None
        } else {
            unsafe { (*next).thread.as_ref() }
        }
    }
}

impl Drop for LockFreeQueue {
    fn drop(&mut self) {
        while self.try_pop().is_some() {}
        
        let head = self.head.load(Ordering::Acquire);
        if !head.is_null() {
            unsafe {
                drop(Box::from_raw(head));
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PriorityLevel {
    Idle,
    Low,
    Normal,
    High,
}

unsafe impl Send for RoundRobinScheduler {}
unsafe impl Sync for RoundRobinScheduler {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_level_mapping() {
        assert_eq!(RoundRobinScheduler::priority_level(0), PriorityLevel::Idle);
        assert_eq!(RoundRobinScheduler::priority_level(32), PriorityLevel::Low);
        assert_eq!(RoundRobinScheduler::priority_level(128), PriorityLevel::Normal);
        assert_eq!(RoundRobinScheduler::priority_level(255), PriorityLevel::High);
    }

    #[test]
    fn test_scheduler_creation() {
        let scheduler = RoundRobinScheduler::new(4);
        assert_eq!(scheduler.num_cpus, 4);
        
        let (total, runnable, blocked) = scheduler.stats();
        assert_eq!(total, 0);
        assert_eq!(runnable, 0);
        assert_eq!(blocked, 0);
    }

    #[test]
    fn test_lock_free_queue_basic() {
        let queue = LockFreeQueue::new();
        assert!(queue.try_pop().is_none());
        assert!(queue.peek().is_none());
    }
}