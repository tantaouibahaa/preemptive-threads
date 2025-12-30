//! Performance regression tests and benchmarks.

#[cfg(test)]
mod performance_tests {
    use crate::thread::ThreadBuilder;
    use crate::sync::{Channel, Mutex};
    use crate::mem::{StackPool, StackSizeClass};
    use crate::time::{get_monotonic_time, Duration};
    use crate::tests::TEST_CONFIG;
    use portable_atomic::{AtomicU64, AtomicBool, Ordering};
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    
    /// Performance measurement utilities
    struct PerfCounter {
        start_time: crate::time::Instant,
        samples: Vec<u64>,
        label: &'static str,
    }
    
    impl PerfCounter {
        fn new(label: &'static str) -> Self {
            Self {
                start_time: get_monotonic_time(),
                samples: Vec::new(),
                label,
            }
        }
        
        fn start_sample(&mut self) {
            self.start_time = get_monotonic_time();
        }
        
        fn end_sample(&mut self) {
            let elapsed = get_monotonic_time().duration_since(self.start_time);
            self.samples.push(elapsed.as_nanos() as u64);
        }
        
        fn report(&self) {
            if self.samples.is_empty() {
                return;
            }
            
            let sum: u64 = self.samples.iter().sum();
            let count = self.samples.len() as u64;
            let avg_ns = sum / count;
            
            let min_ns = *self.samples.iter().min().unwrap();
            let max_ns = *self.samples.iter().max().unwrap();
            
            // Calculate percentiles
            let mut sorted = self.samples.clone();
            sorted.sort_unstable();
            let p50 = sorted[sorted.len() / 2];
            let p90 = sorted[(sorted.len() * 9) / 10];
            let p99 = sorted[(sorted.len() * 99) / 100];
            
            println!("{}: {} samples", self.label, count);
            println!("  Avg: {}ns ({}μs)", avg_ns, avg_ns / 1000);
            println!("  Min: {}ns ({}μs)", min_ns, min_ns / 1000);
            println!("  Max: {}ns ({}μs)", max_ns, max_ns / 1000);
            println!("  P50: {}ns ({}μs)", p50, p50 / 1000);
            println!("  P90: {}ns ({}μs)", p90, p90 / 1000);
            println!("  P99: {}ns ({}μs)", p99, p99 / 1000);
        }
    }
    
    #[test]
    fn perf_thread_creation() {
        let config = TEST_CONFIG.lock();
        let iterations = config.perf_iterations.min(1000);
        drop(config);
        
        let mut perf = PerfCounter::new("Thread Creation");
        
        for _ in 0..iterations {
            perf.start_sample();
            
            let handle = ThreadBuilder::new()
                .spawn(|| 42)
                .expect("Failed to spawn thread");
            
            perf.end_sample();
            
            let _result = handle.join().expect("Failed to join thread");
        }
        
        perf.report();
        
        // Performance regression check: thread creation should be < 100μs on average
        let avg_ns = perf.samples.iter().sum::<u64>() / perf.samples.len() as u64;
        assert!(avg_ns < 100_000, "Thread creation too slow: {}ns", avg_ns);
    }
    
    #[test]
    fn perf_context_switch() {
        let iterations = 1000;
        let switch_count = Arc::new(AtomicU64::new(0));
        let barrier = Arc::new(AtomicBool::new(false));
        let mut perf = PerfCounter::new("Context Switch");
        
        let switch_clone = switch_count.clone();
        let barrier_clone = barrier.clone();
        
        // Spawn a thread that will yield repeatedly
        let handle = ThreadBuilder::new()
            .name("yielder".into())
            .spawn(move || {
                barrier_clone.store(true, Ordering::Release);
                
                for _ in 0..iterations {
                    switch_clone.fetch_add(1, Ordering::SeqCst);
                    crate::yield_now();
                }
            })
            .expect("Failed to spawn thread");
        
        // Wait for thread to start
        while !barrier.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        
        perf.start_sample();
        
        // Main thread also yields to create context switches
        for _ in 0..iterations {
            crate::yield_now();
        }
        
        perf.end_sample();
        
        handle.join().expect("Failed to join thread");
        perf.report();
        
        // Should have completed both thread cycles
        assert!(switch_count.load(Ordering::SeqCst) >= iterations as u64);
        
        // Performance check: context switching should be efficient
        let total_ns = perf.samples[0];
        let avg_switch_ns = total_ns / (iterations as u64 * 2);
        assert!(avg_switch_ns < 10_000, "Context switch too slow: {}ns", avg_switch_ns);
    }
    
    #[test]
    fn perf_stack_allocation() {
        let iterations = 10000;
        let pool = StackPool::new_for_testing();
        let mut perf = PerfCounter::new("Stack Allocation");
        
        for _ in 0..iterations {
            perf.start_sample();
            
            let stack = pool.allocate(StackSizeClass::Small, false)
                .expect("Failed to allocate stack");
            
            perf.end_sample();
            
            pool.deallocate(stack);
        }
        
        perf.report();
        
        // Performance check: stack allocation should be fast
        let avg_ns = perf.samples.iter().sum::<u64>() / perf.samples.len() as u64;
        assert!(avg_ns < 1000, "Stack allocation too slow: {}ns", avg_ns);
    }
    
    #[test]
    fn perf_channel_throughput() {
        let message_count = 100000;
        let (sender, receiver) = Channel::new(1000);
        let mut perf = PerfCounter::new("Channel Throughput");
        
        let handle = ThreadBuilder::new()
            .name("receiver".into())
            .spawn(move || {
                for _ in 0..message_count {
                    let _msg = receiver.recv().expect("Failed to receive");
                }
            })
            .expect("Failed to spawn receiver");
        
        perf.start_sample();
        
        // Send messages as fast as possible
        for i in 0..message_count {
            sender.send(i).expect("Failed to send");
        }
        
        perf.end_sample();
        
        handle.join().expect("Failed to join receiver");
        perf.report();
        
        // Performance check: should achieve high throughput
        let total_ns = perf.samples[0];
        let ns_per_message = total_ns / message_count as u64;
        assert!(ns_per_message < 1000, "Channel throughput too low: {}ns per message", ns_per_message);
    }
    
    #[test]
    fn perf_mutex_contention() {
        let thread_count = 8;
        let iterations = 10000;
        let mutex = Arc::new(Mutex::new(0u64));
        let mut perf = PerfCounter::new("Mutex Contention");
        let mut handles = Vec::new();
        
        perf.start_sample();
        
        for thread_id in 0..thread_count {
            let mutex_clone = mutex.clone();
            let handle = ThreadBuilder::new()
                .name(format!("contender_{}", thread_id))
                .spawn(move || {
                    for _ in 0..iterations {
                        let mut guard = mutex_clone.lock();
                        *guard += 1;
                        // Minimal critical section
                    }
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        for handle in handles {
            handle.join().expect("Thread failed");
        }
        
        perf.end_sample();
        perf.report();
        
        // Verify correctness
        let final_value = *mutex.lock();
        assert_eq!(final_value, (thread_count * iterations) as u64);
        
        // Performance check: mutex operations should be reasonably fast under contention
        let total_ns = perf.samples[0];
        let ns_per_operation = total_ns / (thread_count * iterations) as u64;
        assert!(ns_per_operation < 5000, "Mutex contention too slow: {}ns per op", ns_per_operation);
    }
    
    #[test]
    fn perf_scheduler_overhead() {
        let thread_count = 50;
        let yield_count = 100;
        let mut perf = PerfCounter::new("Scheduler Overhead");
        let mut handles = Vec::new();
        
        perf.start_sample();
        
        for thread_id in 0..thread_count {
            let handle = ThreadBuilder::new()
                .name(format!("scheduler_test_{}", thread_id))
                .spawn(move || {
                    for _ in 0..yield_count {
                        crate::yield_now();
                    }
                    thread_id
                })
                .expect("Failed to spawn thread");
            handles.push(handle);
        }
        
        for (expected, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("Thread failed");
            assert_eq!(result, expected);
        }
        
        perf.end_sample();
        perf.report();
        
        // Performance check: scheduler should handle many threads efficiently
        let total_ns = perf.samples[0];
        let ns_per_yield = total_ns / (thread_count * yield_count) as u64;
        assert!(ns_per_yield < 20000, "Scheduler overhead too high: {}ns per yield", ns_per_yield);
    }
    
    #[test]
    fn perf_memory_allocation() {
        use crate::mem::ArcLite;
        
        let iterations = 50000;
        let mut perf = PerfCounter::new("Memory Allocation");
        
        for _ in 0..iterations {
            perf.start_sample();
            
            let arc = ArcLite::new(42u64);
            let _clone1 = arc.clone();
            let _clone2 = arc.clone();
            
            perf.end_sample();
        }
        
        perf.report();
        
        // Performance check: memory allocation should be fast
        let avg_ns = perf.samples.iter().sum::<u64>() / perf.samples.len() as u64;
        assert!(avg_ns < 500, "Memory allocation too slow: {}ns", avg_ns);
    }
    
    #[test]
    fn perf_atomic_operations() {
        let iterations = 1000000;
        let counter = AtomicU64::new(0);
        let mut perf = PerfCounter::new("Atomic Operations");
        
        perf.start_sample();
        
        for _ in 0..iterations {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        
        perf.end_sample();
        
        assert_eq!(counter.load(Ordering::SeqCst), iterations as u64);
        perf.report();
        
        // Performance check: atomic operations should be very fast
        let total_ns = perf.samples[0];
        let ns_per_op = total_ns / iterations as u64;
        assert!(ns_per_op < 100, "Atomic operations too slow: {}ns per op", ns_per_op);
    }
    
    #[test]
    #[ignore] // Long-running benchmark
    fn benchmark_comprehensive_workload() {
        let duration_secs = 30;
        let producer_count = 4;
        let consumer_count = 2;
        let processor_count = 4;
        
        let (work_sender, work_receiver) = Channel::new(10000);
        let (result_sender, result_receiver) = Channel::new(10000);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let work_generated = Arc::new(AtomicU64::new(0));
        let work_processed = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        
        // Spawn work producers
        for producer_id in 0..producer_count {
            let sender_clone = work_sender.clone();
            let stop_clone = stop_flag.clone();
            let gen_count_clone = work_generated.clone();
            
            let handle = ThreadBuilder::new()
                .name(format!("producer_{}", producer_id))
                .spawn(move || {
                    let mut local_count = 0;
                    while !stop_clone.load(Ordering::Relaxed) {
                        let work_item = producer_id * 1000000 + local_count;
                        if sender_clone.try_send(work_item).is_ok() {
                            local_count += 1;
                            if local_count % 1000 == 0 {
                                gen_count_clone.fetch_add(1000, Ordering::Relaxed);
                            }
                        }
                    }
                    local_count
                })
                .expect("Failed to spawn producer");
            handles.push(handle);
        }
        
        // Spawn work processors
        for processor_id in 0..processor_count {
            let receiver_clone = work_receiver.clone();
            let sender_clone = result_sender.clone();
            let stop_clone = stop_flag.clone();
            
            let handle = ThreadBuilder::new()
                .name(format!("processor_{}", processor_id))
                .spawn(move || {
                    let mut processed = 0;
                    while !stop_clone.load(Ordering::Relaxed) {
                        if let Ok(work_item) = receiver_clone.try_recv() {
                            // Simulate work processing
                            let result = work_item * 2 + 1;
                            for _ in 0..100 {
                                core::hint::spin_loop();
                            }
                            
                            if sender_clone.try_send(result).is_ok() {
                                processed += 1;
                            }
                        }
                    }
                    processed
                })
                .expect("Failed to spawn processor");
            handles.push(handle);
        }
        
        // Spawn result consumers
        for consumer_id in 0..consumer_count {
            let receiver_clone = result_receiver.clone();
            let stop_clone = stop_flag.clone();
            let proc_count_clone = work_processed.clone();
            
            let handle = ThreadBuilder::new()
                .name(format!("consumer_{}", consumer_id))
                .spawn(move || {
                    let mut local_count = 0;
                    while !stop_clone.load(Ordering::Relaxed) {
                        if let Ok(_result) = receiver_clone.try_recv() {
                            local_count += 1;
                            if local_count % 1000 == 0 {
                                proc_count_clone.fetch_add(1000, Ordering::Relaxed);
                            }
                        }
                    }
                    local_count
                })
                .expect("Failed to spawn consumer");
            handles.push(handle);
        }
        
        // Run benchmark for specified duration
        let start_time = get_monotonic_time();
        let target_duration = Duration::from_secs(duration_secs);
        
        while get_monotonic_time().duration_since(start_time) < target_duration {
            crate::kernel::sleep_for(Duration::from_millis(100));
        }
        
        // Signal stop and collect results
        stop_flag.store(true, Ordering::Relaxed);
        
        let mut total_generated = 0;
        let mut total_processed = 0;
        
        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.join().expect("Thread failed");
            if i < producer_count {
                total_generated += result;
            } else if i < producer_count + processor_count {
                // Processor results
            } else {
                total_processed += result;
            }
        }
        
        let generated = work_generated.load(Ordering::Relaxed) + total_generated as u64;
        let processed = work_processed.load(Ordering::Relaxed) + total_processed as u64;
        
        let throughput = generated / duration_secs;
        let efficiency = (processed as f64 / generated as f64) * 100.0;
        
        println!("Comprehensive Benchmark Results:");
        println!("  Duration: {}s", duration_secs);
        println!("  Work Generated: {}", generated);
        println!("  Work Processed: {}", processed);
        println!("  Throughput: {} items/sec", throughput);
        println!("  Efficiency: {:.1}%", efficiency);
        
        // Performance assertions
        assert!(throughput > 10000, "Throughput too low: {} items/sec", throughput);
        assert!(efficiency > 80.0, "Efficiency too low: {:.1}%", efficiency);
    }
}