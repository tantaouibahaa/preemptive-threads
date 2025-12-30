//! Property-based tests for verifying system invariants.

#[cfg(test)]
mod property_tests {
    use crate::thread::ThreadBuilder;
    use crate::sync::{Channel, Mutex, RwLock};
    use crate::mem::{StackPool, StackSizeClass, ArcLite};
    use crate::sched::{Scheduler, SchedulerType};
    use portable_atomic::{AtomicU64, AtomicUsize, Ordering};
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use alloc::collections::BTreeSet;
    
    /// Simple linear congruential generator for property testing.
    struct SimpleRng {
        state: u64,
    }
    
    impl SimpleRng {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }
        
        fn next_u64(&mut self) -> u64 {
            self.state = self.state.wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.state
        }
        
        fn gen_range(&mut self, min: u64, max: u64) -> u64 {
            min + (self.next_u64() % (max - min))
        }
        
        fn gen_bool(&mut self) -> bool {
            self.next_u64() & 1 == 0
        }
    }
    
    #[test]
    fn property_thread_ids_unique() {
        let mut rng = SimpleRng::new(0x12345678);
        let thread_count = 100;
        let mut thread_ids = BTreeSet::new();
        let mut handles = Vec::new();
        
        // Spawn threads and collect their IDs
        for _ in 0..thread_count {
            let handle = ThreadBuilder::new()
                .spawn(|| {
                    crate::thread::current_thread_id()
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Collect all thread IDs
        for handle in handles {
            let thread_id = handle.join().expect("Thread failed");
            thread_ids.insert(thread_id);
        }
        
        // Property: All thread IDs should be unique
        assert_eq!(thread_ids.len(), thread_count);
    }
    
    #[test]
    fn property_stack_allocation_deterministic() {
        let mut rng = SimpleRng::new(0x87654321);
        let pool = StackPool::new_for_testing();
        let iterations = 50;
        
        for _ in 0..iterations {
            let size_class = match rng.gen_range(0, 3) {
                0 => StackSizeClass::Small,
                1 => StackSizeClass::Medium,
                _ => StackSizeClass::Large,
            };
            
            let with_guards = rng.gen_bool();
            
            // Allocate stack
            let stack = pool.allocate(size_class, with_guards)
                .expect("Failed to allocate stack");
            
            // Property: Stack size should meet minimum requirements
            assert!(stack.size() >= size_class.size());
            
            // Property: Stack should be properly aligned
            assert_eq!(stack.base() as usize & 15, 0);
            
            // Property: Stack pointer should be within bounds
            assert!(stack.top() > stack.base());
            assert!((stack.top() as usize - stack.base() as usize) <= stack.size());
            
            pool.deallocate(stack);
        }
    }
    
    #[test]
    fn property_scheduler_fairness() {
        let scheduler = Scheduler::new(SchedulerType::RoundRobin, 1);
        let thread_count = 10;
        let iterations = 100;
        let mut threads = Vec::new();
        let execution_count = Arc::new(AtomicUsize::new(0));
        
        // Create test threads
        for i in 0..thread_count {
            let thread = Arc::new(crate::thread::Thread::new_test_thread());
            thread.set_priority(5); // Same priority for fairness test
            threads.push(thread.clone());
            scheduler.schedule(thread);
        }
        
        // Track execution counts
        let mut exec_counts = vec![0usize; thread_count];
        
        // Run scheduler for many iterations
        for _ in 0..iterations {
            if let Some(thread) = scheduler.pick_next(0) {
                // Find which thread was picked
                for (i, test_thread) in threads.iter().enumerate() {
                    if thread.id() == test_thread.id() {
                        exec_counts[i] += 1;
                        break;
                    }
                }
                
                // Re-schedule the thread
                scheduler.schedule(thread);
            }
        }
        
        // Property: In round-robin, execution should be roughly fair
        let total_executions: usize = exec_counts.iter().sum();
        assert_eq!(total_executions, iterations);
        
        let expected_per_thread = iterations / thread_count;
        let tolerance = expected_per_thread / 4; // 25% tolerance
        
        for &count in &exec_counts {
            assert!(count >= expected_per_thread.saturating_sub(tolerance));
            assert!(count <= expected_per_thread + tolerance);
        }
    }
    
    #[test]
    fn property_arclite_reference_counting() {
        let mut rng = SimpleRng::new(0xABCDEF12);
        let iterations = 1000;
        
        for _ in 0..iterations {
            let initial_value = rng.next_u64();
            let arc = ArcLite::new(initial_value);
            
            // Property: Initial reference count should be 1
            assert_eq!(arc.strong_count(), 1);
            
            // Create random number of clones
            let clone_count = rng.gen_range(1, 20) as usize;
            let mut clones = Vec::new();
            
            for _ in 0..clone_count {
                clones.push(arc.clone());
            }
            
            // Property: Reference count should be 1 + number of clones
            assert_eq!(arc.strong_count(), 1 + clone_count);
            
            // Property: All references should point to same value
            for clone in &clones {
                assert_eq!(**clone, initial_value);
            }
            
            // Drop some clones randomly
            let drop_count = rng.gen_range(0, clone_count as u64) as usize;
            for _ in 0..drop_count {
                if !clones.is_empty() {
                    let idx = (rng.next_u64() as usize) % clones.len();
                    clones.swap_remove(idx);
                }
            }
            
            // Property: Reference count should be correct after drops
            assert_eq!(arc.strong_count(), 1 + clones.len());
        }
    }
    
    #[test]
    fn property_channel_fifo_ordering() {
        let mut rng = SimpleRng::new(0x11111111);
        let iterations = 50;
        
        for _ in 0..iterations {
            let capacity = rng.gen_range(10, 100) as usize;
            let (sender, receiver) = Channel::new(capacity);
            
            // Send sequence of values
            let send_count = rng.gen_range(1, capacity as u64) as usize;
            let mut sent_values = Vec::new();
            
            for i in 0..send_count {
                let value = rng.next_u64();
                sender.send(value).expect("Failed to send");
                sent_values.push(value);
            }
            
            // Receive all values
            let mut received_values = Vec::new();
            for _ in 0..send_count {
                let value = receiver.recv().expect("Failed to receive");
                received_values.push(value);
            }
            
            // Property: Channel should preserve FIFO ordering
            assert_eq!(sent_values, received_values);
        }
    }
    
    #[test]
    fn property_mutex_exclusion() {
        let thread_count = 10;
        let iterations = 100;
        let mutex = Arc::new(Mutex::new(0u64));
        let active_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        
        for thread_id in 0..thread_count {
            let mutex_clone = mutex.clone();
            let active_clone = active_count.clone();
            let max_clone = max_concurrent.clone();
            
            let handle = ThreadBuilder::new()
                .name(format!("mutex_test_{}", thread_id))
                .spawn(move || {
                    for _ in 0..iterations {
                        let _guard = mutex_clone.lock();
                        
                        // Track concurrent access
                        let current_active = active_clone.fetch_add(1, Ordering::SeqCst) + 1;
                        
                        // Update maximum concurrent count
                        let mut max_val = max_clone.load(Ordering::Acquire);
                        while current_active > max_val {
                            match max_clone.compare_exchange_weak(
                                max_val,
                                current_active,
                                Ordering::Release,
                                Ordering::Acquire,
                            ) {
                                Ok(_) => break,
                                Err(actual) => max_val = actual,
                            }
                        }
                        
                        // Do some work
                        for _ in 0..10 {
                            core::hint::spin_loop();
                        }
                        
                        active_clone.fetch_sub(1, Ordering::SeqCst);
                    }
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Wait for all threads
        for handle in handles {
            handle.join().expect("Thread failed");
        }
        
        // Property: Mutex should ensure mutual exclusion (max 1 concurrent)
        assert_eq!(max_concurrent.load(Ordering::Acquire), 1);
    }
    
    #[test]
    fn property_rwlock_reader_writer_exclusion() {
        let reader_count = 8;
        let writer_count = 2;
        let iterations = 50;
        let rwlock = Arc::new(RwLock::new(0u64));
        let concurrent_readers = Arc::new(AtomicUsize::new(0));
        let concurrent_writers = Arc::new(AtomicUsize::new(0));
        let max_readers = Arc::new(AtomicUsize::new(0));
        let max_writers = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        
        // Spawn readers
        for reader_id in 0..reader_count {
            let rwlock_clone = rwlock.clone();
            let readers_clone = concurrent_readers.clone();
            let writers_clone = concurrent_writers.clone();
            let max_readers_clone = max_readers.clone();
            
            let handle = ThreadBuilder::new()
                .name(format!("reader_{}", reader_id))
                .spawn(move || {
                    for _ in 0..iterations {
                        let _guard = rwlock_clone.read();
                        
                        // Track concurrent readers and writers
                        let current_readers = readers_clone.fetch_add(1, Ordering::SeqCst) + 1;
                        let current_writers = writers_clone.load(Ordering::Acquire);
                        
                        // Property: No writers should be active during read
                        assert_eq!(current_writers, 0);
                        
                        // Update max readers
                        let mut max_val = max_readers_clone.load(Ordering::Acquire);
                        while current_readers > max_val {
                            match max_readers_clone.compare_exchange_weak(
                                max_val,
                                current_readers,
                                Ordering::Release,
                                Ordering::Acquire,
                            ) {
                                Ok(_) => break,
                                Err(actual) => max_val = actual,
                            }
                        }
                        
                        // Do read work
                        for _ in 0..20 {
                            core::hint::spin_loop();
                        }
                        
                        readers_clone.fetch_sub(1, Ordering::SeqCst);
                    }
                })
                .expect("Failed to spawn reader");
            handles.push(handle);
        }
        
        // Spawn writers
        for writer_id in 0..writer_count {
            let rwlock_clone = rwlock.clone();
            let readers_clone = concurrent_readers.clone();
            let writers_clone = concurrent_writers.clone();
            let max_writers_clone = max_writers.clone();
            
            let handle = ThreadBuilder::new()
                .name(format!("writer_{}", writer_id))
                .spawn(move || {
                    for _ in 0..iterations {
                        let _guard = rwlock_clone.write();
                        
                        // Track concurrent readers and writers
                        let current_readers = readers_clone.load(Ordering::Acquire);
                        let current_writers = writers_clone.fetch_add(1, Ordering::SeqCst) + 1;
                        
                        // Property: No readers should be active during write
                        assert_eq!(current_readers, 0);
                        
                        // Property: Only one writer should be active
                        assert_eq!(current_writers, 1);
                        
                        // Update max writers
                        let mut max_val = max_writers_clone.load(Ordering::Acquire);
                        while current_writers > max_val {
                            match max_writers_clone.compare_exchange_weak(
                                max_val,
                                current_writers,
                                Ordering::Release,
                                Ordering::Acquire,
                            ) {
                                Ok(_) => break,
                                Err(actual) => max_val = actual,
                            }
                        }
                        
                        // Do write work
                        for _ in 0..10 {
                            core::hint::spin_loop();
                        }
                        
                        writers_clone.fetch_sub(1, Ordering::SeqCst);
                    }
                })
                .expect("Failed to spawn writer");
            handles.push(handle);
        }
        
        // Wait for all threads
        for handle in handles {
            handle.join().expect("Thread failed");
        }
        
        // Property: Should allow multiple readers but only one writer
        assert!(max_readers.load(Ordering::Acquire) > 1);
        assert_eq!(max_writers.load(Ordering::Acquire), 1);
    }
    
    #[test]
    fn property_thread_priority_ordering() {
        let scheduler = Scheduler::new(SchedulerType::RoundRobin, 1);
        let mut rng = SimpleRng::new(0x22222222);
        let thread_count = 20;
        
        // Create threads with random priorities
        let mut expected_order = Vec::new();
        for i in 0..thread_count {
            let priority = rng.gen_range(1, 11) as u8;
            let thread = Arc::new(crate::thread::Thread::new_test_thread());
            thread.set_priority(priority);
            
            scheduler.schedule(thread.clone());
            expected_order.push((priority, thread.id()));
        }
        
        // Sort by priority (higher first)
        expected_order.sort_by(|a, b| b.0.cmp(&a.0));
        
        // Pick threads in order
        let mut actual_order = Vec::new();
        while let Some(thread) = scheduler.pick_next(0) {
            actual_order.push((thread.priority(), thread.id()));
        }
        
        // Property: Higher priority threads should be picked first
        for (i, ((expected_prio, expected_id), (actual_prio, actual_id))) in 
            expected_order.iter().zip(actual_order.iter()).enumerate() {
            
            if i < actual_order.len() - 1 {
                // Current thread should have >= priority than next thread
                assert!(actual_prio >= &actual_order[i + 1].0);
            }
        }
    }
}