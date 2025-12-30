//! Integration tests for complete thread lifecycle and system interactions.

#[cfg(test)]
mod lifecycle_tests {
    use crate::thread::{ThreadBuilder, ThreadState};
    use crate::kernel::ThreadingKernel;
    use crate::observability::GLOBAL_METRICS;
    use portable_atomic::{AtomicU64, AtomicBool, Ordering};
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    
    #[test]
    fn test_complete_thread_lifecycle() {
        let kernel = ThreadingKernel::new_for_testing();
        let executed = Arc::new(AtomicBool::new(false));
        let executed_clone = executed.clone();
        
        let handle = ThreadBuilder::new()
            .name("lifecycle_test".into())
            .spawn_on_kernel(&kernel, move || {
                executed_clone.store(true, Ordering::SeqCst);
                "completed"
            })
            .expect("Failed to spawn thread");
        
        // Thread should be running
        assert_eq!(handle.thread().state(), ThreadState::Running);
        
        // Wait for completion
        let result = handle.join().expect("Failed to join thread");
        assert_eq!(result, "completed");
        assert!(executed.load(Ordering::SeqCst));
        
        // Thread should be terminated
        assert_eq!(handle.thread().state(), ThreadState::Terminated);
    }
    
    #[test]
    fn test_thread_communication() {
        use crate::sync::{Channel, Mutex};
        
        let (sender, receiver) = Channel::new(10);
        let mutex = Arc::new(Mutex::new(0u64));
        
        let mutex_clone = mutex.clone();
        let handle = ThreadBuilder::new()
            .name("communicator".into())
            .spawn(move || {
                // Send some values
                sender.send(1).expect("Failed to send");
                sender.send(2).expect("Failed to send");
                sender.send(3).expect("Failed to send");
                
                // Update shared state
                let mut guard = mutex_clone.lock();
                *guard = 42;
            })
            .expect("Failed to spawn thread");
        
        // Receive values
        assert_eq!(receiver.recv().expect("Failed to receive"), 1);
        assert_eq!(receiver.recv().expect("Failed to receive"), 2);
        assert_eq!(receiver.recv().expect("Failed to receive"), 3);
        
        handle.join().expect("Failed to join thread");
        
        // Check shared state
        let guard = mutex.lock();
        assert_eq!(*guard, 42);
    }
    
    #[test]
    fn test_producer_consumer() {
        use crate::sync::Channel;
        
        let (sender, receiver) = Channel::new(100);
        let producer_count = 3;
        let items_per_producer = 10;
        let total_items = producer_count * items_per_producer;
        
        // Spawn producers
        let mut producers = Vec::new();
        for producer_id in 0..producer_count {
            let sender_clone = sender.clone();
            let handle = ThreadBuilder::new()
                .name(format!("producer_{}", producer_id))
                .spawn(move || {
                    for i in 0..items_per_producer {
                        let value = producer_id * items_per_producer + i;
                        sender_clone.send(value).expect("Failed to send");
                    }
                })
                .expect("Failed to spawn producer");
            producers.push(handle);
        }
        
        // Drop original sender so consumer knows when to stop
        drop(sender);
        
        // Spawn consumer
        let received = Arc::new(AtomicU64::new(0));
        let received_clone = received.clone();
        let consumer = ThreadBuilder::new()
            .name("consumer".into())
            .spawn(move || {
                while let Ok(_value) = receiver.recv() {
                    received_clone.fetch_add(1, Ordering::SeqCst);
                }
            })
            .expect("Failed to spawn consumer");
        
        // Wait for all to complete
        for producer in producers {
            producer.join().expect("Failed to join producer");
        }
        consumer.join().expect("Failed to join consumer");
        
        // Verify all items were received
        assert_eq!(received.load(Ordering::SeqCst), total_items as u64);
    }
    
    #[test]
    fn test_thread_priorities() {
        let kernel = ThreadingKernel::new_for_testing();
        let execution_order = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        
        // Spawn threads with different priorities
        for priority in [1, 5, 10].iter() {
            let order_clone = execution_order.clone();
            let priority = *priority;
            
            let handle = ThreadBuilder::new()
                .name(format!("priority_{}", priority))
                .priority(priority)
                .spawn_on_kernel(&kernel, move || {
                    let mut guard = order_clone.lock();
                    guard.push(priority);
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Wait for all to complete
        for handle in handles {
            handle.join().expect("Failed to join thread");
        }
        
        // Higher priority threads should execute first
        let order = execution_order.lock();
        assert_eq!(order[0], 10); // Highest priority first
        assert!(order.len() == 3);
    }
    
    #[test]
    fn test_thread_local_storage() {
        use crate::tls::ThreadLocal;
        
        let tls = ThreadLocal::new(|| 0u64);
        let mut handles = Vec::new();
        
        for thread_id in 0..5 {
            let tls_clone = tls.clone();
            let handle = ThreadBuilder::new()
                .name(format!("tls_test_{}", thread_id))
                .spawn(move || {
                    // Each thread should get its own value
                    *tls_clone.get_mut() = thread_id;
                    
                    // Verify the value is isolated per thread
                    assert_eq!(*tls_clone.get(), thread_id);
                    thread_id
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        // Verify each thread returned its own ID
        for (expected_id, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("Failed to join thread");
            assert_eq!(result, expected_id as u64);
        }
    }
    
    #[test]
    fn test_resource_cleanup() {
        let kernel = ThreadingKernel::new_for_testing();
        let initial_thread_count = kernel.active_thread_count();
        let initial_stack_count = kernel.stack_pool().allocated_count();
        
        // Create and join many threads
        for i in 0..20 {
            let handle = ThreadBuilder::new()
                .name(format!("cleanup_test_{}", i))
                .spawn_on_kernel(&kernel, move || i)
                .expect("Failed to spawn thread");
            
            let result = handle.join().expect("Failed to join thread");
            assert_eq!(result, i);
        }
        
        // Force cleanup
        kernel.force_cleanup();
        
        // Verify resources were cleaned up
        assert_eq!(kernel.active_thread_count(), initial_thread_count);
        assert_eq!(kernel.stack_pool().allocated_count(), initial_stack_count);
    }
    
    #[test]
    fn test_observability_integration() {
        let kernel = ThreadingKernel::new_for_testing();
        
        // Clear existing metrics
        GLOBAL_METRICS.reset();
        
        let handle = ThreadBuilder::new()
            .name("observed_thread".into())
            .spawn_on_kernel(&kernel, || {
                // Perform some work that should be observed
                for _ in 0..1000 {
                    core::hint::spin_loop();
                }
                42
            })
            .expect("Failed to spawn thread");
        
        let result = handle.join().expect("Failed to join thread");
        assert_eq!(result, 42);
        
        // Verify metrics were collected
        let metrics = GLOBAL_METRICS.snapshot();
        assert!(metrics.total_threads_created > 0);
        assert!(metrics.total_threads_completed > 0);
        assert!(metrics.total_context_switches >= 0);
    }
}

#[cfg(test)]
mod error_handling_tests {
    use crate::thread::ThreadBuilder;
    use crate::errors::{ThreadError, SpawnError};
    use crate::mem::StackSizeClass;
    
    #[test]
    fn test_stack_overflow_detection() {
        // This test requires stack guards to be enabled
        let builder = ThreadBuilder::new()
            .stack_size_class(StackSizeClass::Small)
            .enable_stack_guards(true);
        
        // Attempt to spawn thread that would overflow stack
        let result = builder.spawn(|| {
            // Recursive function to cause stack overflow
            fn recursive_overflow(depth: usize) -> usize {
                if depth > 0 {
                    let buffer = [0u8; 1024]; // Allocate stack space
                    buffer[0]; // Use the buffer
                    recursive_overflow(depth - 1) + 1
                } else {
                    0
                }
            }
            
            // This should cause stack overflow
            recursive_overflow(10000)
        });
        
        match result {
            Ok(handle) => {
                // If thread was created, it should terminate with error
                match handle.join() {
                    Err(ThreadError::Memory(_)) => {
                        // Expected error due to stack overflow
                    }
                    other => panic!("Expected memory error, got: {:?}", other),
                }
            }
            Err(ThreadError::Spawn(SpawnError::StackOverflow)) => {
                // Stack overflow detected during spawn
            }
            other => panic!("Unexpected spawn result: {:?}", other),
        }
    }
    
    #[test]
    fn test_resource_exhaustion() {
        use crate::mem::StackPool;
        
        // Create pool with limited capacity
        let pool = StackPool::new_with_capacity(5);
        let mut stacks = Vec::new();
        
        // Exhaust the pool
        for _ in 0..5 {
            let stack = pool.allocate(StackSizeClass::Small, false)
                .expect("Failed to allocate stack");
            stacks.push(stack);
        }
        
        // Next allocation should fail
        let result = pool.allocate(StackSizeClass::Small, false);
        assert!(matches!(result, Err(_)));
        
        // Return one stack
        pool.deallocate(stacks.pop().unwrap());
        
        // Now allocation should succeed
        let _stack = pool.allocate(StackSizeClass::Small, false)
            .expect("Failed to allocate after return");
    }
    
    #[test]
    fn test_scheduler_overload() {
        use crate::kernel::ThreadingKernel;
        
        let kernel = ThreadingKernel::new_with_limits(1000); // Limited capacity
        let mut handles = Vec::new();
        
        // Try to spawn more threads than capacity
        for i in 0..1200 {
            match ThreadBuilder::new()
                .name(format!("overload_{}", i))
                .spawn_on_kernel(&kernel, move || i)
            {
                Ok(handle) => handles.push(handle),
                Err(ThreadError::Spawn(SpawnError::ResourceLimit)) => {
                    // Expected when hitting limits
                    break;
                }
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }
        
        // Should have hit the limit before 1200 threads
        assert!(handles.len() < 1200);
        
        // All spawned threads should complete successfully
        for handle in handles {
            handle.join().expect("Thread failed unexpectedly");
        }
    }
}