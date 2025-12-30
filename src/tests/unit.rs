//! Unit tests for core threading components.

#[cfg(test)]
mod thread_tests {
    use crate::thread::{Thread, ThreadBuilder, ThreadState};
    use crate::errors::ThreadError;
    use crate::mem::{StackSizeClass, StackPool};
    use portable_atomic::{AtomicU64, Ordering};
    use alloc::sync::Arc;
    
    #[test]
    fn test_thread_creation() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        
        let builder = ThreadBuilder::new()
            .name("test_thread".into())
            .stack_size_class(StackSizeClass::Small)
            .priority(5);
            
        let handle = builder.spawn(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            42
        }).expect("Failed to spawn thread");
        
        let result = handle.join().expect("Failed to join thread");
        assert_eq!(result, 42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
    
    #[test]
    fn test_thread_state_transitions() {
        let thread = Thread::new_test_thread();
        
        // Initial state should be Ready
        assert_eq!(thread.state(), ThreadState::Ready);
        
        // Transition to Running
        thread.set_state(ThreadState::Running);
        assert_eq!(thread.state(), ThreadState::Running);
        
        // Transition to Blocked
        thread.set_state(ThreadState::Blocked);
        assert_eq!(thread.state(), ThreadState::Blocked);
        
        // Transition to Terminated
        thread.set_state(ThreadState::Terminated);
        assert_eq!(thread.state(), ThreadState::Terminated);
    }
    
    #[test]
    fn test_thread_priority() {
        let thread = Thread::new_test_thread();
        
        // Default priority
        assert_eq!(thread.priority(), 5);
        
        // Set new priority
        thread.set_priority(10);
        assert_eq!(thread.priority(), 10);
        
        // Priority should be clamped to MAX_PRIORITY
        thread.set_priority(255);
        assert_eq!(thread.priority(), crate::thread::MAX_PRIORITY);
    }
    
    #[test]
    fn test_thread_name() {
        let builder = ThreadBuilder::new()
            .name("named_thread".into());
            
        let handle = builder.spawn(|| {}).expect("Failed to spawn thread");
        let thread = handle.thread();
        
        assert_eq!(thread.name(), Some("named_thread"));
    }
    
    #[test]
    fn test_multiple_threads() {
        let counter = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        
        for i in 0..10 {
            let counter_clone = counter.clone();
            let handle = ThreadBuilder::new()
                .name(format!("thread_{}", i))
                .spawn(move || {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    i
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("Failed to join thread");
            assert_eq!(result, i);
        }
        
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }
}

#[cfg(test)]
mod stack_tests {
    use crate::mem::{StackPool, StackSizeClass, Stack};
    use crate::errors::MemoryError;
    
    #[test]
    fn test_stack_allocation() {
        let pool = StackPool::new_for_testing();
        
        // Allocate a small stack
        let stack = pool.allocate(StackSizeClass::Small, false)
            .expect("Failed to allocate stack");
        
        assert!(stack.size() >= StackSizeClass::Small.size());
        assert!(!stack.base().is_null());
        
        // Stack should be properly aligned
        assert_eq!(stack.base() as usize & 15, 0);
    }
    
    #[test]
    fn test_stack_pool_reuse() {
        let pool = StackPool::new_for_testing();
        
        // Allocate and track the base address
        let stack1 = pool.allocate(StackSizeClass::Small, false)
            .expect("Failed to allocate stack");
        let base1 = stack1.base();
        
        // Return stack to pool
        pool.deallocate(stack1);
        
        // Allocate again - should get the same stack back
        let stack2 = pool.allocate(StackSizeClass::Small, false)
            .expect("Failed to allocate stack");
        let base2 = stack2.base();
        
        assert_eq!(base1, base2, "Stack was not reused from pool");
    }
    
    #[test]
    fn test_stack_size_classes() {
        let pool = StackPool::new_for_testing();
        
        for size_class in &[
            StackSizeClass::Small,
            StackSizeClass::Medium,
            StackSizeClass::Large,
        ] {
            let stack = pool.allocate(*size_class, false)
                .expect("Failed to allocate stack");
            
            assert!(stack.size() >= size_class.size());
            pool.deallocate(stack);
        }
    }
    
    #[test]
    fn test_custom_stack_size() {
        let pool = StackPool::new_for_testing();
        
        let custom_size = 1024 * 1024; // 1MB
        let stack = pool.allocate_custom(custom_size, false)
            .expect("Failed to allocate custom stack");
        
        assert!(stack.size() >= custom_size);
        pool.deallocate(stack);
    }
}

#[cfg(test)]
mod scheduler_tests {
    use crate::sched::{Scheduler, SchedulerType};
    use crate::thread::{Thread, ThreadState};
    use portable_atomic::{AtomicU64, AtomicBool, Ordering};
    use alloc::sync::Arc;
    
    #[test]
    fn test_scheduler_creation() {
        let scheduler = Scheduler::new(SchedulerType::RoundRobin, 4);
        assert_eq!(scheduler.scheduler_type(), SchedulerType::RoundRobin);
    }
    
    #[test]
    fn test_thread_scheduling() {
        let scheduler = Scheduler::new(SchedulerType::RoundRobin, 1);
        let thread = Arc::new(Thread::new_test_thread());
        
        // Schedule the thread
        scheduler.schedule(thread.clone());
        
        // Pick next thread should return our thread
        let next = scheduler.pick_next(0).expect("No thread picked");
        assert_eq!(next.id(), thread.id());
    }
    
    #[test]
    fn test_priority_scheduling() {
        let scheduler = Scheduler::new(SchedulerType::RoundRobin, 1);
        
        // Create threads with different priorities
        let low_prio = Arc::new(Thread::new_test_thread());
        low_prio.set_priority(1);
        
        let high_prio = Arc::new(Thread::new_test_thread());
        high_prio.set_priority(10);
        
        // Schedule both threads
        scheduler.schedule(low_prio.clone());
        scheduler.schedule(high_prio.clone());
        
        // High priority thread should be picked first
        let next = scheduler.pick_next(0).expect("No thread picked");
        assert_eq!(next.id(), high_prio.id());
    }
    
    #[test]
    fn test_round_robin_fairness() {
        let scheduler = Scheduler::new(SchedulerType::RoundRobin, 1);
        let mut threads = Vec::new();
        
        // Create and schedule multiple threads
        for _ in 0..5 {
            let thread = Arc::new(Thread::new_test_thread());
            scheduler.schedule(thread.clone());
            threads.push(thread);
        }
        
        // Each thread should be picked in round-robin order
        for expected_thread in &threads {
            let picked = scheduler.pick_next(0).expect("No thread picked");
            assert_eq!(picked.id(), expected_thread.id());
            
            // Re-schedule for next round
            scheduler.schedule(picked);
        }
    }
    
    #[test]
    fn test_cpu_affinity() {
        let scheduler = Scheduler::new(SchedulerType::RoundRobin, 4);
        let thread = Arc::new(Thread::new_test_thread());
        
        // Set CPU affinity to CPU 2
        thread.set_cpu_affinity(0b0100); // Bit 2 set
        
        scheduler.schedule(thread.clone());
        
        // Thread should not be picked on CPU 0
        assert!(scheduler.pick_next(0).is_none());
        
        // Thread should be picked on CPU 2
        let picked = scheduler.pick_next(2).expect("Thread not picked on CPU 2");
        assert_eq!(picked.id(), thread.id());
    }
}

#[cfg(test)]
mod memory_tests {
    use crate::mem::{ArcLite, EpochGc, HazardPointer};
    use portable_atomic::{AtomicU64, Ordering};
    use alloc::sync::Arc;
    
    #[test]
    fn test_arclite_basic() {
        let value = ArcLite::new(42);
        let clone = value.clone();
        
        assert_eq!(*value, 42);
        assert_eq!(*clone, 42);
        assert_eq!(value.strong_count(), 2);
    }
    
    #[test]
    fn test_arclite_drop() {
        let counter = Arc::new(AtomicU64::new(0));
        
        {
            let value = ArcLite::new_with_drop(100, {
                let counter = counter.clone();
                move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            });
            
            let _clone1 = value.clone();
            let _clone2 = value.clone();
            assert_eq!(value.strong_count(), 3);
        } // All references dropped here
        
        // Destructor should have been called
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
    
    #[test]
    fn test_epoch_gc() {
        let gc = EpochGc::new();
        let thread_id = 0;
        
        // Enter epoch
        gc.pin(thread_id);
        
        // Defer some work
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        gc.defer(thread_id, Box::new(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }));
        
        // Work shouldn't be executed yet
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        
        // Unpin and advance epoch
        gc.unpin(thread_id);
        gc.try_advance();
        gc.collect(thread_id);
        
        // Deferred work should now be executed
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
    
    #[test]
    fn test_hazard_pointer() {
        let hp = HazardPointer::<u64>::new(10);
        let thread_id = 0;
        let value = 42u64;
        let ptr = &value as *const u64;
        
        // Protect the pointer
        hp.protect(thread_id, 0, ptr);
        
        // Check if protected
        assert!(hp.is_protected(ptr));
        
        // Clear protection
        hp.clear(thread_id, 0);
        
        // Should no longer be protected
        assert!(!hp.is_protected(ptr));
    }
}