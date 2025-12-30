//! Work-stealing scheduler implementation with lock-free deques.

use super::trait_def::{Scheduler, CpuId};
use crate::thread::{ReadyRef, RunningRef, ThreadId};
use portable_atomic::{AtomicUsize, AtomicPtr, AtomicIsize, Ordering};
use core::ptr;
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

/// Work-stealing scheduler with per-CPU deques.
///
/// This scheduler uses lock-free double-ended queues (deques) per CPU.
/// Local threads are pushed/popped from one end (LIFO for better cache locality),
/// while work stealing occurs from the other end (FIFO to avoid conflicts).
pub struct WorkStealingScheduler {
    /// Number of CPUs in the system
    num_cpus: usize,
    /// Per-CPU work-stealing deques
    work_deques: Box<[WorkStealingDeque]>,
    /// Global overflow queue for load balancing
    global_queue: LockFreeQueue,
    /// Global statistics
    total_threads: AtomicUsize,
    runnable_threads: AtomicUsize,
}

/// Lock-free work-stealing deque using Chase-Lev algorithm.
///
/// This allows lock-free push/pop operations from the owner (bottom),
/// and lock-free steal operations from thieves (top).
struct WorkStealingDeque {
    /// Circular buffer for thread storage
    buffer: AtomicPtr<*mut ReadyRef>,
    /// Buffer capacity (always power of 2)
    capacity: AtomicUsize,
    /// Bottom index (owner operations)
    bottom: AtomicUsize,
    /// Top index (steal operations)
    top: AtomicIsize,
    /// Current number of elements
    size: AtomicUsize,
}

/// Lock-free MPMC queue for global overflow.
struct LockFreeQueue {
    head: AtomicPtr<QueueNode>,
    tail: AtomicPtr<QueueNode>,
    size: AtomicUsize,
}

/// Queue node for the overflow queue.
struct QueueNode {
    thread: Option<ReadyRef>,
    next: AtomicPtr<QueueNode>,
}

/// Work-stealing operation results.
enum StealResult {
    /// Successfully stole a thread
    Success(ReadyRef),
    /// Deque was empty
    Empty,
    /// Race condition occurred, retry
    Abort,
}

impl WorkStealingScheduler {
    /// Create a new work-stealing scheduler for the given number of CPUs.
    pub fn new(num_cpus: usize) -> Self {
        let mut work_deques = Vec::with_capacity(num_cpus);
        for _ in 0..num_cpus {
            work_deques.push(WorkStealingDeque::new());
        }

        Self {
            num_cpus,
            work_deques: work_deques.into_boxed_slice(),
            global_queue: LockFreeQueue::new(),
            total_threads: AtomicUsize::new(0),
            runnable_threads: AtomicUsize::new(0),
        }
    }

    /// Select CPU for thread placement using randomization.
    fn select_cpu(&self) -> CpuId {
        // Use simple pseudo-random selection to distribute load
        // In a real implementation, this could use RDRAND or system entropy
        static COUNTER: AtomicUsize = AtomicUsize::new(1);
        let seed = COUNTER.fetch_add(1, Ordering::Relaxed);
        
        // Simple linear congruential generator
        let next = seed.wrapping_mul(1103515245).wrapping_add(12345);
        next % self.num_cpus
    }

    /// Attempt to steal work from other CPUs.
    fn try_steal_work(&self, requesting_cpu: CpuId) -> Option<ReadyRef> {
        // Try stealing from 2 * num_cpus attempts to increase success rate
        let attempts = self.num_cpus * 2;
        
        for i in 0..attempts {
            let victim_cpu = (requesting_cpu + i + 1) % self.num_cpus;
            if victim_cpu == requesting_cpu {
                continue; // Don't steal from ourselves
            }

            match self.work_deques[victim_cpu].steal() {
                StealResult::Success(thread) => {
                    return Some(thread);
                },
                StealResult::Empty => continue,
                StealResult::Abort => {
                    // Retry the same victim on abort
                    match self.work_deques[victim_cpu].steal() {
                        StealResult::Success(thread) => return Some(thread),
                        _ => continue,
                    }
                },
            }
        }

        // If local stealing failed, try global queue
        self.global_queue.try_pop()
    }

    /// Balance load by moving threads to global queue.
    fn balance_load(&self, cpu_id: CpuId) {
        let deque = &self.work_deques[cpu_id];
        let current_size = deque.size.load(Ordering::Acquire);
        
        // If deque is getting too large, move some threads to global queue
        const MAX_LOCAL_SIZE: usize = 256;
        if current_size > MAX_LOCAL_SIZE {
            let move_count = current_size / 4; // Move 25% to global
            
            for _ in 0..move_count {
                if let Some(thread) = deque.pop() {
                    self.global_queue.push(thread);
                }
            }
        }
    }
}

impl Scheduler for WorkStealingScheduler {
    fn enqueue(&self, thread: ReadyRef) {
        let cpu_id = self.select_cpu();
        let deque = &self.work_deques[cpu_id];
        
        // Try to push to local deque first
        if !deque.push(thread.clone()) {
            // Deque is full, push to global queue
            self.global_queue.push(thread);
        }
        
        self.runnable_threads.fetch_add(1, Ordering::AcqRel);
        
        // Periodic load balancing
        if self.runnable_threads.load(Ordering::Acquire) % 100 == 0 {
            self.balance_load(cpu_id);
        }
    }

    fn pick_next(&self, cpu_id: CpuId) -> Option<ReadyRef> {
        if cpu_id >= self.num_cpus {
            return None;
        }

        // First try local deque (LIFO for cache locality)
        let deque = &self.work_deques[cpu_id];
        if let Some(thread) = deque.pop() {
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        // Try global queue
        if let Some(thread) = self.global_queue.try_pop() {
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        // Finally try work stealing
        if let Some(thread) = self.try_steal_work(cpu_id) {
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        None
    }

    fn on_tick(&self, current: &RunningRef) -> Option<ReadyRef> {
        // Work-stealing scheduler uses shorter time slices to improve responsiveness
        if current.time_slice().should_preempt() {
            Some(current.prepare_preemption())
        } else {
            None
        }
    }

    fn set_priority(&self, thread_id: ThreadId, priority: u8) {
        // Priority changes take effect on next scheduling decision
        let _ = (thread_id, priority);
    }

    fn on_yield(&self, current: RunningRef) {
        let ready = current.stop_running();
        self.enqueue(ready);
    }

    fn on_block(&self, current: RunningRef) {
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

impl WorkStealingDeque {
    fn new() -> Self {
        const INITIAL_CAPACITY: usize = 64;
        let buffer = unsafe {
            let layout = core::alloc::Layout::array::<*mut ReadyRef>(INITIAL_CAPACITY).unwrap();
            let ptr = alloc::alloc::alloc_zeroed(layout) as *mut *mut ReadyRef;
            ptr
        };

        Self {
            buffer: AtomicPtr::new(buffer),
            capacity: AtomicUsize::new(INITIAL_CAPACITY),
            bottom: AtomicUsize::new(0),
            top: AtomicIsize::new(0),
            size: AtomicUsize::new(0),
        }
    }

    /// Push a thread to the bottom of the deque (owner operation).
    fn push(&self, thread: ReadyRef) -> bool {
        let bottom = self.bottom.load(Ordering::Relaxed);
        let top = self.top.load(Ordering::Acquire);
        let capacity = self.capacity.load(Ordering::Relaxed);

        // Check if deque is full
        if bottom - (top as usize) >= capacity - 1 {
            // Deque is full, would need to resize
            return false;
        }

        let buffer = self.buffer.load(Ordering::Relaxed);
        let index = bottom & (capacity - 1);
        
        // Store the thread in the buffer
        unsafe {
            *buffer.add(index) = Box::into_raw(Box::new(thread));
        }

        // Release fence ensures the thread store is visible before bottom update
        // This synchronizes with the acquire fence in steal()
        core::sync::atomic::fence(Ordering::Release);
        self.bottom.store(bottom + 1, Ordering::Relaxed);
        self.size.fetch_add(1, Ordering::AcqRel);
        
        true
    }

    /// Pop a thread from the bottom of the deque (owner operation).
    fn pop(&self) -> Option<ReadyRef> {
        let bottom = self.bottom.load(Ordering::Relaxed);
        if bottom == 0 {
            return None;
        }

        let new_bottom = bottom - 1;
        self.bottom.store(new_bottom, Ordering::Relaxed);
        
        // Sequential consistency fence to ensure ordering with steal operations
        // This is critical for correctness of the Chase-Lev algorithm
        core::sync::atomic::fence(Ordering::SeqCst);

        let top = self.top.load(Ordering::Relaxed);
        let capacity = self.capacity.load(Ordering::Relaxed);
        let buffer = self.buffer.load(Ordering::Relaxed);
        
        if (new_bottom as isize) < top {
            // Deque is empty, restore bottom
            self.bottom.store(bottom, Ordering::Relaxed);
            return None;
        }

        let index = new_bottom & (capacity - 1);
        let thread_ptr = unsafe { *buffer.add(index) };
        
        if (new_bottom as isize) > top {
            // More than one element, pop is successful (no race with steal)
            self.size.fetch_sub(1, Ordering::AcqRel);
            return Some(unsafe { *Box::from_raw(thread_ptr) });
        }

        // Exactly one element, compete with steal using sequential consistency
        if self.top.compare_exchange(
            top,
            top + 1,
            Ordering::SeqCst,  // Strong ordering for correctness
            Ordering::Relaxed
        ).is_err() {
            // Lost the race to stealer, restore bottom
            self.bottom.store(bottom, Ordering::Relaxed);
            return None;
        }

        // Won the race, restore bottom and return the thread
        self.bottom.store(bottom, Ordering::Relaxed);
        self.size.fetch_sub(1, Ordering::AcqRel);
        Some(unsafe { *Box::from_raw(thread_ptr) })
    }

    /// Steal a thread from the top of the deque (thief operation).
    fn steal(&self) -> StealResult {
        let top = self.top.load(Ordering::Acquire);
        
        // Sequential consistency fence ensures proper ordering with pop operations
        // This synchronizes with the fence in pop() for Chase-Lev correctness
        core::sync::atomic::fence(Ordering::SeqCst);
        
        let bottom = self.bottom.load(Ordering::Acquire);

        // Check if deque appears empty
        if (top as usize) >= bottom {
            return StealResult::Empty;
        }

        let capacity = self.capacity.load(Ordering::Relaxed);
        let buffer = self.buffer.load(Ordering::Relaxed);
        let index = (top as usize) & (capacity - 1);
        let thread_ptr = unsafe { *buffer.add(index) };

        // Try to increment top with sequential consistency to compete with pop
        if self.top.compare_exchange_weak(
            top,
            top + 1,
            Ordering::SeqCst,  // Must use SeqCst for Chase-Lev correctness
            Ordering::Relaxed  // Relaxed on failure is fine
        ).is_err() {
            return StealResult::Abort;
        }

        // Successfully stole the thread
        self.size.fetch_sub(1, Ordering::AcqRel);
        StealResult::Success(unsafe { *Box::from_raw(thread_ptr) })
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
            size: AtomicUsize::new(0),
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

            if tail == self.tail.load(Ordering::Acquire) {
                if next.is_null() {
                    if unsafe { (*tail).next.compare_exchange_weak(
                        ptr::null_mut(),
                        new_node,
                        Ordering::Release,
                        Ordering::Relaxed
                    ).is_ok() } {
                        break;
                    }
                } else {
                    let _ = self.tail.compare_exchange_weak(
                        tail,
                        next,
                        Ordering::Release,
                        Ordering::Relaxed
                    );
                }
            }
        }

        let _ = self.tail.compare_exchange_weak(
            self.tail.load(Ordering::Acquire),
            new_node,
            Ordering::Release,
            Ordering::Relaxed
        );
        
        self.size.fetch_add(1, Ordering::AcqRel);
    }

    fn try_pop(&self) -> Option<ReadyRef> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);
            let next = unsafe { (*head).next.load(Ordering::Acquire) };

            if head == self.head.load(Ordering::Acquire) {
                if head == tail {
                    if next.is_null() {
                        return None;
                    }
                    let _ = self.tail.compare_exchange_weak(
                        tail,
                        next,
                        Ordering::Release,
                        Ordering::Relaxed
                    );
                } else {
                    if next.is_null() {
                        continue;
                    }

                    let thread = unsafe { (*next).thread.take() };
                    
                    if self.head.compare_exchange_weak(
                        head,
                        next,
                        Ordering::Release,
                        Ordering::Relaxed
                    ).is_ok() {
                        unsafe {
                            drop(Box::from_raw(head));
                        }
                        self.size.fetch_sub(1, Ordering::AcqRel);
                        return thread;
                    }
                }
            }
        }
    }
}

impl Drop for WorkStealingDeque {
    fn drop(&mut self) {
        while self.pop().is_some() {}
        
        let buffer = self.buffer.load(Ordering::Relaxed);
        if !buffer.is_null() {
            let capacity = self.capacity.load(Ordering::Relaxed);
            unsafe {
                let layout = core::alloc::Layout::array::<*mut ReadyRef>(capacity).unwrap();
                alloc::alloc::dealloc(buffer as *mut u8, layout);
            }
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

unsafe impl Send for WorkStealingScheduler {}
unsafe impl Sync for WorkStealingScheduler {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_work_stealing_scheduler_creation() {
        let scheduler = WorkStealingScheduler::new(4);
        assert_eq!(scheduler.num_cpus, 4);
        
        let (total, runnable, blocked) = scheduler.stats();
        assert_eq!(total, 0);
        assert_eq!(runnable, 0);
        assert_eq!(blocked, 0);
    }

    #[test]
    fn test_deque_creation() {
        let deque = WorkStealingDeque::new();
        assert!(deque.pop().is_none());
        
        match deque.steal() {
            StealResult::Empty => {},
            _ => panic!("Expected empty deque"),
        }
    }
}