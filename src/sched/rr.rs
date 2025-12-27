//! Round-robin scheduler implementation with lock-free queues.

use super::trait_def::{CpuId, Scheduler};
use crate::thread_new::{ReadyRef, RunningRef, ThreadId};
use portable_atomic::{AtomicPtr, AtomicUsize, Ordering};
use core::ptr;
extern crate alloc;
use alloc::{boxed::Box, vec::Vec};

pub struct RoundRobinScheduler {
    num_cpus: usize,
    run_queues: Box<[CpuRunQueue]>,
    total_threads: AtomicUsize,
    runnable_threads: AtomicUsize,
}


pub struct FirstComeFirstServeScheduler {
    queue: LockFreeQueue,
    runnable_threads: AtomicUsize,
}

/// Per-CPU run queue with priority levels.
struct CpuRunQueue {
    high_priority: LockFreeQueue,
    normal_priority: LockFreeQueue,
    low_priority: LockFreeQueue,
    idle_priority: LockFreeQueue,
    thread_count: AtomicUsize,
}

struct LockFreeQueue {
    head: AtomicPtr<QueueNode>,
    tail: AtomicPtr<QueueNode>,
}

struct QueueNode {
    thread: Option<ReadyRef>,
    next: AtomicPtr<QueueNode>,
}

impl Scheduler for FirstComeFirstServeScheduler {
    fn enqueue(&self, thread: ReadyRef) {
        let tid = thread.id().get();
        crate::pl011_println!("[FCFS] enqueue: thread {} (queue before: {:?})", tid, self.queue.debug_list_threads());
        self.queue.push(thread);
        crate::pl011_println!("[FCFS] enqueue done: (queue after: {:?})", self.queue.debug_list_threads());
        self.runnable_threads.fetch_add(1, Ordering::AcqRel);
    }

    fn pick_next(&self, _cpu_id: CpuId) -> Option<ReadyRef> {
        crate::pl011_println!("[FCFS] pick_next: (queue before: {:?})", self.queue.debug_list_threads());
        let thread = self.queue.try_pop()?;
        let tid = thread.id().get();
        crate::pl011_println!("[FCFS] pick_next: got thread {} (queue after: {:?})", tid, self.queue.debug_list_threads());
        self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
        Some(thread)
    }

    fn on_tick(&self, _current: &RunningRef) -> Option<ReadyRef> {
        None
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
    fn set_priority(&self, _thread_id: ThreadId, _priority: u8) {
        // later
    }

}
impl FirstComeFirstServeScheduler {
    pub fn new(num_cpus: usize) -> Self {
        Self {
            num_cpus,
            queue: LockFreeQueue::new(),
            total_threads: AtomicUsize::new(0),
            runnable_threads: AtomicUsize::new(0),
        }
    }
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

    fn priority_level(priority: u8) -> PriorityLevel {
        match priority {
            0 => PriorityLevel::Idle,
            1..=63 => PriorityLevel::Low,
            64..=191 => PriorityLevel::Normal,
            192..=255 => PriorityLevel::High,
        }
    }

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

    fn try_steal_work(&self, requesting_cpu: CpuId) -> Option<ReadyRef> {
        let start_cpu = (requesting_cpu + 1) % self.num_cpus;

        for i in 0..self.num_cpus {
            let victim_cpu = (start_cpu + i) % self.num_cpus;
            if victim_cpu == requesting_cpu {
                continue; // Don't steal from ourselves
            }

            let victim_queue = &self.run_queues[victim_cpu];

            if let Some(thread) = victim_queue.normal_priority.try_pop() {
                victim_queue.thread_count.fetch_sub(1, Ordering::AcqRel);
                return Some(thread);
            }

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

        if let Some(thread) = self.try_steal_work(cpu_id) {
            self.runnable_threads.fetch_sub(1, Ordering::AcqRel);
            return Some(thread);
        }

        None
    }

    fn on_tick(&self, current: &RunningRef) -> Option<ReadyRef> {
        if current.time_slice().should_preempt() {
            let ready = current.prepare_preemption();

            let cpu_id = current.last_cpu();

            if cpu_id < self.num_cpus {
                let queue = &self.run_queues[cpu_id];
                let current_priority = current.priority();

                match Self::priority_level(current_priority) {
                    PriorityLevel::Idle => {
                        if queue.low_priority.peek().is_some()
                            || queue.normal_priority.peek().is_some()
                            || queue.high_priority.peek().is_some()
                        {
                            return Some(ready);
                        }
                    }
                    PriorityLevel::Low => {
                        if queue.normal_priority.peek().is_some()
                            || queue.high_priority.peek().is_some()
                        {
                            return Some(ready);
                        }
                    }
                    PriorityLevel::Normal => {
                        if queue.high_priority.peek().is_some() {
                            return Some(ready);
                        }
                    },
                    PriorityLevel::High => {
                        return Some(ready);
                    },
                }
            }
        }

        None
    }

    fn set_priority(&self, thread_id: ThreadId, priority: u8) {
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

impl CpuRunQueue {
    fn new() -> Self {
        Self {
            high_priority: LockFreeQueue::new(),
            normal_priority: LockFreeQueue::new(),
            low_priority: LockFreeQueue::new(),
            idle_priority: LockFreeQueue::new(),
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

    fn debug_list_threads(&self) -> alloc::vec::Vec<usize> {
        let mut ids = alloc::vec::Vec::new();
        let head = self.head.load(Ordering::Acquire);
        let mut current = unsafe { (*head).next.load(Ordering::Acquire) };
        while !current.is_null() {
            if let Some(ref thread) = unsafe { &(*current).thread } {
                ids.push(thread.id().get());
            } else {
                ids.push(0);
            }
            current = unsafe { (*current).next.load(Ordering::Acquire) };
        }
        ids
    }

    fn push(&self, thread: ReadyRef) {
        let new_node = Box::into_raw(Box::new(QueueNode {
            thread: Some(thread),
            next: AtomicPtr::new(ptr::null_mut()),
        }));

        loop {
            let tail = self.tail.load(Ordering::Acquire);
            let next = unsafe { (*tail).next.load(Ordering::Acquire) };

            //  (ABA prevention)
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
    }

    fn try_pop(&self) -> Option<ReadyRef> {
        loop {
            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);
            let next = unsafe { (*head).next.load(Ordering::Acquire) };

            // (ABA prevention)
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
                        return thread;
                    } else {
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


unsafe impl Send for FirstComeFirstServeScheduler {}
unsafe impl Sync for FirstComeFirstServeScheduler {}

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
