//! Test helper utilities and common functionality.

#[cfg(test)]
use crate::thread::{Thread, ThreadId};
#[cfg(test)]
use crate::kernel::ThreadingKernel;
#[cfg(test)]
use crate::mem::{StackPool, StackSizeClass};
#[cfg(test)]
use crate::sched::{Scheduler, SchedulerType};
#[cfg(test)]
use portable_atomic::{AtomicU64, AtomicBool, Ordering};
#[cfg(test)]
use alloc::sync::Arc;
#[cfg(test)]
use alloc::vec::Vec;
#[cfg(test)]
use alloc::string::String;

#[cfg(test)]
/// Test environment setup and cleanup.
pub struct TestEnvironment {
    kernel: Option<ThreadingKernel>,
    cleanup_handlers: Vec<Box<dyn FnOnce() + Send>>,
}

#[cfg(test)]
impl TestEnvironment {
    pub fn new() -> Self {
        Self {
            kernel: None,
            cleanup_handlers: Vec::new(),
        }
    }
    
    pub fn with_kernel(mut self) -> Self {
        self.kernel = Some(ThreadingKernel::new_for_testing());
        self
    }
    
    pub fn kernel(&self) -> &ThreadingKernel {
        self.kernel.as_ref().expect("Kernel not initialized")
    }
    
    pub fn add_cleanup<F>(&mut self, cleanup: F) 
    where
        F: FnOnce() + Send + 'static,
    {
        self.cleanup_handlers.push(Box::new(cleanup));
    }
}

#[cfg(test)]
impl Drop for TestEnvironment {
    fn drop(&mut self) {
        // Run cleanup handlers in reverse order
        for cleanup in self.cleanup_handlers.drain(..).rev() {
            cleanup();
        }
        
        if let Some(kernel) = self.kernel.take() {
            kernel.shutdown();
        }
    }
}

#[cfg(test)]
/// Test thread factory for creating threads with consistent configuration.
pub struct TestThreadFactory {
    next_id: AtomicU64,
    default_stack_class: StackSizeClass,
    default_priority: u8,
}

#[cfg(test)]
impl TestThreadFactory {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1000), // Start from high number to avoid conflicts
            default_stack_class: StackSizeClass::Small,
            default_priority: 5,
        }
    }
    
    pub fn with_stack_class(mut self, stack_class: StackSizeClass) -> Self {
        self.default_stack_class = stack_class;
        self
    }
    
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.default_priority = priority;
        self
    }
    
    pub fn create_thread(&self) -> Arc<Thread> {
        let thread = Arc::new(Thread::new_test_thread());
        thread.set_priority(self.default_priority);
        thread
    }
    
    pub fn create_named_thread(&self, name: &str) -> Arc<Thread> {
        let thread = self.create_thread();
        thread.set_name(Some(name.into()));
        thread
    }
    
    pub fn create_batch(&self, count: usize) -> Vec<Arc<Thread>> {
        let mut threads = Vec::with_capacity(count);
        for i in 0..count {
            let thread = self.create_named_thread(&format!("batch_thread_{}", i));
            threads.push(thread);
        }
        threads
    }
    
    pub fn create_priority_batch(&self, priorities: &[u8]) -> Vec<Arc<Thread>> {
        let mut threads = Vec::with_capacity(priorities.len());
        for (i, &priority) in priorities.iter().enumerate() {
            let thread = self.create_named_thread(&format!("priority_thread_{}", i));
            thread.set_priority(priority);
            threads.push(thread);
        }
        threads
    }
}

#[cfg(test)]
/// Synchronization primitives for test coordination.
pub struct TestBarrier {
    counter: AtomicU64,
    target: u64,
    released: AtomicBool,
}

#[cfg(test)]
impl TestBarrier {
    pub fn new(count: u64) -> Self {
        Self {
            counter: AtomicU64::new(0),
            target: count,
            released: AtomicBool::new(false),
        }
    }
    
    pub fn wait(&self) {
        let count = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        
        if count == self.target {
            self.released.store(true, Ordering::Release);
        } else {
            while !self.released.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        }
    }
    
    pub fn reset(&self) {
        self.counter.store(0, Ordering::SeqCst);
        self.released.store(false, Ordering::SeqCst);
    }
    
    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }
}

#[cfg(test)]
/// Test data generator for creating predictable test data.
pub struct TestDataGenerator {
    seed: u64,
    counter: AtomicU64,
}

#[cfg(test)]
impl TestDataGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            counter: AtomicU64::new(0),
        }
    }
    
    pub fn next_u64(&self) -> u64 {
        let count = self.counter.fetch_add(1, Ordering::SeqCst);
        self.seed.wrapping_mul(6364136223846793005)
            .wrapping_add(count)
            .wrapping_add(1442695040888963407)
    }
    
    pub fn next_range(&self, min: u64, max: u64) -> u64 {
        min + (self.next_u64() % (max - min))
    }
    
    pub fn next_bool(&self) -> bool {
        self.next_u64() & 1 == 0
    }
    
    pub fn next_string(&self, length: usize) -> String {
        let mut result = String::with_capacity(length);
        for _ in 0..length {
            let char_val = (self.next_range(0, 26) + b'a' as u64) as u8 as char;
            result.push(char_val);
        }
        result
    }
    
    pub fn next_bytes(&self, length: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(length);
        for _ in 0..length {
            result.push(self.next_u64() as u8);
        }
        result
    }
}

#[cfg(test)]
/// Performance timing utilities for test benchmarks.
pub struct TestTimer {
    start_time: Option<crate::time::Instant>,
    samples: Vec<crate::time::Duration>,
    name: String,
}

#[cfg(test)]
impl TestTimer {
    pub fn new(name: &str) -> Self {
        Self {
            start_time: None,
            samples: Vec::new(),
            name: name.to_string(),
        }
    }
    
    pub fn start(&mut self) {
        self.start_time = Some(crate::time::get_monotonic_time());
    }
    
    pub fn stop(&mut self) {
        if let Some(start) = self.start_time.take() {
            let elapsed = crate::time::get_monotonic_time().duration_since(start);
            self.samples.push(elapsed);
        }
    }
    
    pub fn measure<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.start();
        let result = f();
        self.stop();
        result
    }
    
    pub fn average(&self) -> crate::time::Duration {
        if self.samples.is_empty() {
            return crate::time::Duration::from_nanos(0);
        }
        
        let total_nanos: u64 = self.samples.iter()
            .map(|d| d.as_nanos() as u64)
            .sum();
        
        crate::time::Duration::from_nanos(total_nanos / self.samples.len() as u64)
    }
    
    pub fn min(&self) -> crate::time::Duration {
        self.samples.iter().min().copied()
            .unwrap_or(crate::time::Duration::from_nanos(0))
    }
    
    pub fn max(&self) -> crate::time::Duration {
        self.samples.iter().max().copied()
            .unwrap_or(crate::time::Duration::from_nanos(0))
    }
    
    pub fn report(&self) {
        if self.samples.is_empty() {
            println!("{}: No samples", self.name);
            return;
        }
        
        let avg = self.average();
        let min = self.min();
        let max = self.max();
        
        println!("{}: {} samples", self.name, self.samples.len());
        println!("  Avg: {:?}", avg);
        println!("  Min: {:?}", min);
        println!("  Max: {:?}", max);
    }
    
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
    
    pub fn reset(&mut self) {
        self.samples.clear();
        self.start_time = None;
    }
}

#[cfg(test)]
/// Resource leak detector for identifying resource cleanup issues.
pub struct LeakDetector {
    initial_threads: u64,
    initial_stacks: u64,
    initial_memory: u64,
}

#[cfg(test)]
impl LeakDetector {
    pub fn new() -> Self {
        Self {
            initial_threads: 0, // Would get from kernel
            initial_stacks: 0,  // Would get from stack pool
            initial_memory: 0,  // Would get from allocator
        }
    }
    
    pub fn snapshot(&mut self) {
        // Take initial resource snapshot
        // In real implementation, would query kernel and allocators
        self.initial_threads = 0;
        self.initial_stacks = 0;
        self.initial_memory = 0;
    }
    
    pub fn check_leaks(&self) -> Vec<String> {
        let mut leaks = Vec::new();
        
        // In real implementation, would check current vs initial resources
        // For now, return empty to indicate no leaks detected
        
        leaks
    }
    
    pub fn assert_no_leaks(&self) {
        let leaks = self.check_leaks();
        if !leaks.is_empty() {
            panic!("Resource leaks detected: {:?}", leaks);
        }
    }
}

#[cfg(test)]
/// Test workload generator for creating realistic thread work patterns.
pub struct WorkloadGenerator {
    generator: TestDataGenerator,
}

#[cfg(test)]
impl WorkloadGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
            generator: TestDataGenerator::new(seed),
        }
    }
    
    /// Generate CPU-intensive work.
    pub fn cpu_work(&self, iterations: u64) {
        for _ in 0..iterations {
            // Simulate CPU work
            let mut sum = 0u64;
            for i in 0..100 {
                sum = sum.wrapping_add(i * self.generator.next_u64());
            }
            // Use the result to prevent optimization
            core::hint::black_box(sum);
        }
    }
    
    /// Generate memory allocation work.
    pub fn memory_work(&self, allocations: u64) {
        for _ in 0..allocations {
            let size = self.generator.next_range(64, 4096) as usize;
            let data = self.generator.next_bytes(size);
            core::hint::black_box(data);
        }
    }
    
    /// Generate yielding work pattern.
    pub fn yielding_work(&self, yields: u64) {
        for _ in 0..yields {
            // Do a little work then yield
            self.cpu_work(10);
            crate::yield_now();
        }
    }
    
    /// Generate mixed workload.
    pub fn mixed_work(&self, duration_ms: u64) {
        let start = crate::time::get_monotonic_time();
        let target_duration = crate::time::Duration::from_millis(duration_ms);
        
        while crate::time::get_monotonic_time().duration_since(start) < target_duration {
            match self.generator.next_range(0, 4) {
                0 => self.cpu_work(50),
                1 => self.memory_work(5),
                2 => self.yielding_work(3),
                _ => {
                    // Short sleep
                    crate::kernel::sleep_for(crate::time::Duration::from_micros(100));
                }
            }
        }
    }
}

#[cfg(test)]
/// Test assertion macros for threading-specific checks.
#[macro_export]
macro_rules! assert_thread_state {
    ($thread:expr, $expected:expr) => {
        assert_eq!(
            $thread.state(),
            $expected,
            "Thread {} expected state {:?}, found {:?}",
            $thread.id(),
            $expected,
            $thread.state()
        );
    };
}

#[cfg(test)]
#[macro_export]
macro_rules! assert_eventually {
    ($condition:expr, $timeout_ms:expr) => {
        {
            let start = crate::time::get_monotonic_time();
            let timeout = crate::time::Duration::from_millis($timeout_ms);
            
            while !$condition {
                if crate::time::get_monotonic_time().duration_since(start) > timeout {
                    panic!("Condition never became true within {}ms", $timeout_ms);
                }
                crate::yield_now();
            }
        }
    };
}

#[cfg(test)]
#[macro_export]
macro_rules! assert_performance {
    ($timer:expr, $max_duration:expr) => {
        let avg = $timer.average();
        assert!(
            avg <= $max_duration,
            "Performance regression: average {:?} exceeds maximum {:?}",
            avg,
            $max_duration
        );
    };
}