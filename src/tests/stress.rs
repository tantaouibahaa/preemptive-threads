//! Stress tests for concurrency and high-load scenarios.

#[cfg(test)]
mod stress_tests {
    use crate::thread::ThreadBuilder;
    use crate::sync::{Channel, Mutex, RwLock};
    use crate::tests::TEST_CONFIG;
    use portable_atomic::{AtomicU64, AtomicBool, AtomicUsize, Ordering};
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    
    #[test]
    fn test_massive_thread_creation() {
        let config = TEST_CONFIG.lock();
        let thread_count = config.stress_thread_count;
        drop(config);
        
        let counter = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::with_capacity(thread_count);
        
        // Spawn many threads simultaneously
        for i in 0..thread_count {
            let counter_clone = counter.clone();
            let handle = ThreadBuilder::new()
                .name(format!("stress_{}", i))
                .spawn(move || {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    i
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Join all threads
        for (expected, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("Thread failed");
            assert_eq!(result, expected);
        }
        
        // Verify all threads executed
        assert_eq!(counter.load(Ordering::SeqCst), thread_count as u64);
    }
    
    #[test]
    fn test_high_contention_mutex() {
        let thread_count = 20;
        let iterations = 1000;
        let mutex = Arc::new(Mutex::new(0u64));
        let mut handles = Vec::new();
        
        for thread_id in 0..thread_count {
            let mutex_clone = mutex.clone();
            let handle = ThreadBuilder::new()
                .name(format!("contention_{}", thread_id))
                .spawn(move || {
                    for _ in 0..iterations {
                        let mut guard = mutex_clone.lock();
                        *guard += 1;
                        // Small delay to increase contention
                        for _ in 0..10 {
                            core::hint::spin_loop();
                        }
                    }
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Wait for all threads
        for handle in handles {
            handle.join().expect("Thread failed");
        }
        
        // Verify final value
        let final_value = *mutex.lock();
        assert_eq!(final_value, (thread_count * iterations) as u64);
    }
    
    #[test]
    fn test_reader_writer_stress() {
        let reader_count = 10;
        let writer_count = 3;
        let iterations = 500;
        let rwlock = Arc::new(RwLock::new(0u64));
        let mut handles = Vec::new();
        
        // Spawn readers
        for reader_id in 0..reader_count {
            let rwlock_clone = rwlock.clone();
            let handle = ThreadBuilder::new()
                .name(format!("reader_{}", reader_id))
                .spawn(move || {
                    let mut reads = 0;
                    for _ in 0..iterations {
                        let _guard = rwlock_clone.read();
                        reads += 1;
                        // Simulate read work
                        for _ in 0..50 {
                            core::hint::spin_loop();
                        }
                    }
                    reads
                })
                .expect("Failed to spawn reader");
            handles.push(handle);
        }
        
        // Spawn writers
        for writer_id in 0..writer_count {
            let rwlock_clone = rwlock.clone();
            let handle = ThreadBuilder::new()
                .name(format!("writer_{}", writer_id))
                .spawn(move || {
                    let mut writes = 0;
                    for i in 0..iterations {
                        let mut guard = rwlock_clone.write();
                        *guard = writer_id * iterations + i;
                        writes += 1;
                        // Simulate write work
                        for _ in 0..20 {
                            core::hint::spin_loop();
                        }
                    }
                    writes
                })
                .expect("Failed to spawn writer");
            handles.push(handle);
        }
        
        // Wait for all threads
        let mut total_reads = 0;
        let mut total_writes = 0;
        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("Thread failed");
            if i < reader_count {
                total_reads += result;
            } else {
                total_writes += result;
            }
        }
        
        assert_eq!(total_reads, (reader_count * iterations) as u64);
        assert_eq!(total_writes, (writer_count * iterations) as u64);
    }
    
    #[test]
    fn test_channel_stress() {
        let producer_count = 5;
        let consumer_count = 3;
        let messages_per_producer = 1000;
        let total_messages = producer_count * messages_per_producer;
        
        let (sender, receiver) = Channel::new(100);
        let received_count = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        
        // Spawn producers
        for producer_id in 0..producer_count {
            let sender_clone = sender.clone();
            let handle = ThreadBuilder::new()
                .name(format!("producer_{}", producer_id))
                .spawn(move || {
                    for i in 0..messages_per_producer {
                        let message = producer_id * messages_per_producer + i;
                        sender_clone.send(message).expect("Failed to send");
                    }
                })
                .expect("Failed to spawn producer");
            handles.push(handle);
        }
        
        // Drop original sender
        drop(sender);
        
        // Spawn consumers
        for consumer_id in 0..consumer_count {
            let receiver_clone = receiver.clone();
            let count_clone = received_count.clone();
            let handle = ThreadBuilder::new()
                .name(format!("consumer_{}", consumer_id))
                .spawn(move || {
                    let mut local_count = 0;
                    while let Ok(_message) = receiver_clone.recv() {
                        local_count += 1;
                    }
                    count_clone.fetch_add(local_count, Ordering::SeqCst);
                    local_count
                })
                .expect("Failed to spawn consumer");
            handles.push(handle);
        }
        
        // Wait for all threads
        for handle in handles {
            handle.join().expect("Thread failed");
        }
        
        // Verify all messages were received
        assert_eq!(received_count.load(Ordering::SeqCst), total_messages as u64);
    }
    
    #[test]
    fn test_memory_pressure() {
        use crate::mem::{StackPool, StackSizeClass};
        
        let thread_count = 50;
        let allocations_per_thread = 100;
        let pool = Arc::new(StackPool::new_for_testing());
        let mut handles = Vec::new();
        
        for thread_id in 0..thread_count {
            let pool_clone = pool.clone();
            let handle = ThreadBuilder::new()
                .name(format!("memory_{}", thread_id))
                .spawn(move || {
                    let mut stacks = Vec::new();
                    
                    // Allocate many stacks
                    for _ in 0..allocations_per_thread {
                        match pool_clone.allocate(StackSizeClass::Small, false) {
                            Ok(stack) => stacks.push(stack),
                            Err(_) => break, // Out of memory
                        }
                    }
                    
                    // Return all stacks
                    for stack in stacks {
                        pool_clone.deallocate(stack);
                    }
                    
                    allocations_per_thread
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Wait for all threads
        for handle in handles {
            handle.join().expect("Thread failed");
        }
        
        // Pool should be empty after cleanup
        assert_eq!(pool.allocated_count(), 0);
    }
    
    #[test]
    fn test_scheduler_thrashing() {
        let thread_count = 100;
        let yield_count = 50;
        let completion_counter = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        
        for thread_id in 0..thread_count {
            let counter_clone = completion_counter.clone();
            let handle = ThreadBuilder::new()
                .name(format!("thrasher_{}", thread_id))
                .spawn(move || {
                    for _ in 0..yield_count {
                        // Yield frequently to stress scheduler
                        crate::yield_now();
                        
                        // Do some minimal work
                        for _ in 0..10 {
                            core::hint::spin_loop();
                        }
                    }
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    thread_id
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Wait for all threads
        for (expected, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("Thread failed");
            assert_eq!(result, expected);
        }
        
        // Verify all threads completed
        assert_eq!(completion_counter.load(Ordering::SeqCst), thread_count as u64);
    }
    
    #[test]
    #[ignore] // Long-running test
    fn test_endurance() {
        let config = TEST_CONFIG.lock();
        let duration_secs = config.stress_duration_secs;
        let thread_count = config.stress_thread_count.min(20);
        drop(config);
        
        let start_time = crate::time::get_monotonic_time();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let iteration_count = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        
        // Spawn worker threads
        for thread_id in 0..thread_count {
            let stop_clone = stop_flag.clone();
            let count_clone = iteration_count.clone();
            
            let handle = ThreadBuilder::new()
                .name(format!("endurance_{}", thread_id))
                .spawn(move || {
                    let mut local_iterations = 0;
                    while !stop_clone.load(Ordering::Relaxed) {
                        // Simulate various workloads
                        match thread_id % 4 {
                            0 => {
                                // CPU-intensive
                                for _ in 0..1000 {
                                    core::hint::spin_loop();
                                }
                            }
                            1 => {
                                // Memory allocation
                                let _vec: Vec<u8> = Vec::with_capacity(1024);
                            }
                            2 => {
                                // Yielding
                                crate::yield_now();
                            }
                            _ => {
                                // Mixed workload
                                for _ in 0..500 {
                                    core::hint::spin_loop();
                                }
                                crate::yield_now();
                            }
                        }
                        
                        local_iterations += 1;
                        if local_iterations % 100 == 0 {
                            count_clone.fetch_add(100, Ordering::Relaxed);
                        }
                    }
                    local_iterations
                })
                .expect("Failed to spawn endurance thread");
            handles.push(handle);
        }
        
        // Run for specified duration
        let target_duration = crate::time::Duration::from_secs(duration_secs);
        while crate::time::get_monotonic_time().duration_since(start_time) < target_duration {
            crate::kernel::sleep_for(crate::time::Duration::from_millis(100));
        }
        
        // Signal stop and wait for threads
        stop_flag.store(true, Ordering::Relaxed);
        for handle in handles {
            handle.join().expect("Endurance thread failed");
        }
        
        let total_iterations = iteration_count.load(Ordering::Relaxed);
        println!("Endurance test completed: {} iterations in {} seconds", 
                total_iterations, duration_secs);
        
        // Should have completed significant work
        assert!(total_iterations > 1000);
    }
}