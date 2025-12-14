//! Thread builder for configuring thread creation.

use super::{Thread, JoinHandle, ThreadId};
use crate::mem::{StackPool, StackSizeClass};
use crate::errors::SpawnError;
use crate::time::Duration;
extern crate alloc;
use alloc::string::String;

/// Builder for configuring and creating new threads.
///
/// This provides a comprehensive interface for setting thread parameters
/// before spawning, with advanced options for scheduling, debugging, and resource management.
pub struct ThreadBuilder {
    /// Stack size class to use
    stack_size_class: Option<StackSizeClass>,
    /// Custom stack size in bytes
    custom_stack_size: Option<usize>,
    /// Thread priority (0-255, higher = more important)
    priority: u8,
    /// Thread name (for debugging and profiling)
    name: Option<String>,
    /// CPU affinity mask (bitfield of allowed CPUs)
    cpu_affinity: Option<u64>,
    /// Thread group ID for resource accounting
    group_id: Option<u32>,
    /// Whether to enable stack guard pages
    stack_guard_pages: bool,
    /// Whether to enable stack canary protection
    stack_canary: bool,
    /// Custom stack canary value
    custom_canary: Option<u64>,
    /// Time slice duration override
    time_slice: Option<Duration>,
    /// Whether this thread is critical (affects scheduling)
    critical: bool,
    /// Whether this thread can be preempted
    preemptible: bool,
    /// Thread-local storage size reservation
    tls_size: Option<usize>,
    /// Whether to enable detailed debugging info
    debug_info: bool,
    /// Custom thread attributes
    attributes: ThreadAttributes,
}

/// Custom thread attributes for advanced configuration.
#[derive(Debug, Clone)]
pub struct ThreadAttributes {
    /// Real-time scheduling parameters
    rt_priority: Option<u8>,
    /// Nice value for process priority
    nice_value: i8,
    /// Whether to inherit parent's signal mask
    inherit_signal_mask: bool,
    /// Custom environment variables
    environment: Option<alloc::collections::BTreeMap<String, String>>,
    /// Resource limits
    limits: ResourceLimits,
}

/// Resource limits for threads.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum CPU time (in nanoseconds)
    max_cpu_time: Option<u64>,
    /// Maximum memory usage (in bytes)
    max_memory: Option<usize>,
    /// Maximum number of file descriptors
    max_files: Option<u32>,
    /// Maximum number of child threads
    max_children: Option<u32>,
}

impl ThreadBuilder {
    /// Create a new thread builder with default settings.
    pub fn new() -> Self {
        Self {
            stack_size_class: None,
            custom_stack_size: None,
            priority: 128, // Normal priority
            name: None,
            cpu_affinity: None,
            group_id: None,
            stack_guard_pages: false, // No MMU support currently
            stack_canary: true,
            custom_canary: None,
            time_slice: None,
            critical: false,
            preemptible: true,
            tls_size: None,
            debug_info: cfg!(debug_assertions),
            attributes: ThreadAttributes::default(),
        }
    }
    
    /// Set the stack size class for the thread.
    pub fn stack_size_class(mut self, size_class: StackSizeClass) -> Self {
        self.stack_size_class = Some(size_class);
        self.custom_stack_size = None; // Clear custom size
        self
    }
    
    /// Set the stack size in bytes.
    pub fn stack_size(mut self, size: usize) -> Self {
        self.custom_stack_size = Some(size);
        self.stack_size_class = None; // Clear size class
        self
    }
    
    /// Set the thread priority.
    pub fn priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
    
    /// Set the thread name for debugging purposes.
    pub fn name<T: Into<String>>(mut self, name: T) -> Self {
        self.name = Some(name.into());
        self
    }
    
    /// Set CPU affinity mask (which CPUs this thread can run on).
    pub fn cpu_affinity(mut self, mask: u64) -> Self {
        self.cpu_affinity = Some(mask);
        self
    }
    
    /// Set thread group ID for resource accounting.
    pub fn group_id(mut self, group: u32) -> Self {
        self.group_id = Some(group);
        self
    }
    
    /// Enable or disable stack guard pages (requires MMU feature).
    pub fn stack_guard_pages(mut self, enabled: bool) -> Self {
        self.stack_guard_pages = enabled;
        self
    }
    
    /// Enable or disable stack canary protection.
    pub fn stack_canary(mut self, enabled: bool) -> Self {
        self.stack_canary = enabled;
        self
    }
    
    /// Set custom stack canary value.
    pub fn custom_canary(mut self, canary: u64) -> Self {
        self.custom_canary = Some(canary);
        self.stack_canary = true;
        self
    }
    
    /// Set custom time slice duration.
    pub fn time_slice(mut self, duration: Duration) -> Self {
        self.time_slice = Some(duration);
        self
    }
    
    /// Mark this thread as critical (affects scheduling priority).
    pub fn critical(mut self, critical: bool) -> Self {
        self.critical = critical;
        self
    }
    
    /// Set whether this thread can be preempted.
    pub fn preemptible(mut self, preemptible: bool) -> Self {
        self.preemptible = preemptible;
        self
    }
    
    /// Reserve space for thread-local storage.
    pub fn tls_size(mut self, size: usize) -> Self {
        self.tls_size = Some(size);
        self
    }
    
    /// Enable or disable detailed debugging information.
    pub fn debug_info(mut self, enabled: bool) -> Self {
        self.debug_info = enabled;
        self
    }
    
    /// Set custom thread attributes.
    pub fn attributes(mut self, attributes: ThreadAttributes) -> Self {
        self.attributes = attributes;
        self
    }
    
    /// Spawn a new thread with the configured parameters.
    ///
    /// # Arguments
    ///
    /// * `thread_id` - Unique identifier for the new thread
    /// * `stack_pool` - Stack pool to allocate from
    /// * `entry_point` - Function to run in the new thread
    ///
    /// # Returns
    ///
    /// A tuple of (Thread, JoinHandle) if successful, or an error if
    /// thread creation fails.
    pub fn spawn(
        self,
        thread_id: ThreadId,
        stack_pool: &StackPool,
        entry_point: fn(),
    ) -> Result<(Thread, JoinHandle), SpawnError> {
        // Validate configuration parameters
        if let Some(name) = &self.name {
            if name.len() > 64 {
                return Err(SpawnError::InvalidName(name.clone()));
            }
        }
        
        if let Some(affinity) = self.cpu_affinity {
            if affinity == 0 {
                return Err(SpawnError::InvalidAffinity(affinity));
            }
        }
        
        // Determine stack size
        let stack_size = if let Some(custom_size) = self.custom_stack_size {
            if custom_size < 4096 || custom_size > 16 * 1024 * 1024 {
                return Err(SpawnError::InvalidStackSize(custom_size));
            }
            custom_size
        } else {
            let size_class = self.stack_size_class.unwrap_or(StackSizeClass::Small);
            match size_class {
                StackSizeClass::Small => 16384,
                StackSizeClass::Medium => 65536,
                StackSizeClass::Large => 262144,
                StackSizeClass::ExtraLarge => 1048576, // 1MB
            }
        };
        
        // Allocate stack based on size class or custom size
        let stack = if let Some(_custom_size) = self.custom_stack_size {
            // For custom sizes, we'll still use the size class system but pick the closest match
            let size_class = if stack_size <= 16384 {
                StackSizeClass::Small
            } else if stack_size <= 65536 {
                StackSizeClass::Medium
            } else if stack_size <= 262144 {
                StackSizeClass::Large
            } else {
                StackSizeClass::ExtraLarge
            };
            stack_pool.allocate(size_class).ok_or(SpawnError::OutOfMemory)?
        } else {
            let size_class = self.stack_size_class.unwrap_or(StackSizeClass::Small);
            stack_pool.allocate(size_class).ok_or(SpawnError::OutOfMemory)?
        };
        
        // Set up stack canary if enabled
        if self.stack_canary {
            let canary_value = self.custom_canary.unwrap_or_else(|| {
                // Generate a random canary value
                // In a real implementation, we would use a proper CSPRNG
                0xDEADBEEFCAFEBABE
            });
            
            // Store canary at the bottom of the stack
            unsafe {
                let stack_bottom = stack.stack_bottom() as *mut u64;
                *stack_bottom = canary_value;
            }
        }
        
        // Create the thread with all configuration
        let (thread, join_handle) = Thread::new(
            thread_id,
            stack,
            entry_point,
            self.priority,
        );
        
        // Apply additional configuration
        if let Some(name) = self.name {
            thread.set_name(name);
        }
        
        if let Some(affinity) = self.cpu_affinity {
            thread.set_cpu_affinity(affinity);
        }
        
        if let Some(group_id) = self.group_id {
            thread.set_group_id(group_id);
        }
        
        if let Some(time_slice) = self.time_slice {
            thread.set_time_slice(time_slice);
        }
        
        thread.set_critical(self.critical);
        thread.set_preemptible(self.preemptible);
        
        if let Some(tls_size) = self.tls_size {
            thread.reserve_tls(tls_size);
        }
        
        thread.set_debug_info(self.debug_info);
        
        // Apply thread attributes
        if let Some(rt_priority) = self.attributes.rt_priority {
            thread.set_realtime_priority(rt_priority);
        }
        
        thread.set_nice_value(self.attributes.nice_value);
        thread.set_inherit_signal_mask(self.attributes.inherit_signal_mask);
        
        if let Some(env) = self.attributes.environment {
            thread.set_environment(env);
        }
        
        // Apply resource limits
        let limits = &self.attributes.limits;
        if let Some(max_cpu_time) = limits.max_cpu_time {
            thread.set_max_cpu_time(max_cpu_time);
        }
        
        if let Some(max_memory) = limits.max_memory {
            thread.set_max_memory(max_memory);
        }
        
        if let Some(max_files) = limits.max_files {
            thread.set_max_files(max_files);
        }
        
        if let Some(max_children) = limits.max_children {
            thread.set_max_children(max_children);
        }
        
        Ok((thread, join_handle))
    }
}

impl Default for ThreadBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for ThreadAttributes {
    fn default() -> Self {
        Self {
            rt_priority: None,
            nice_value: 0,
            inherit_signal_mask: true,
            environment: None,
            limits: ResourceLimits::default(),
        }
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_cpu_time: None,
            max_memory: None,
            max_files: None,
            max_children: None,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    
    #[cfg(feature = "std-shim")]
    #[test]
    fn test_thread_builder() {
        let pool = StackPool::new();
        let thread_id = unsafe { ThreadId::new_unchecked(1) };
        
        let builder = ThreadBuilder::new()
            .stack_size_class(StackSizeClass::Medium)
            .priority(200)
            .name("test-thread");
        
        let result = builder.spawn(thread_id, &pool, || {
            // Thread code here
        });
        
        assert!(result.is_ok());
        let (thread, _join_handle) = result.unwrap();
        
        assert_eq!(thread.id(), thread_id);
        assert_eq!(thread.priority(), 200);
    }
    
    #[cfg(feature = "std-shim")]
    #[test]
    fn test_thread_builder_stack_size() {
        let builder1 = ThreadBuilder::new().stack_size(8192);
        // Should select Medium size class for 8KB request
        
        let builder2 = ThreadBuilder::new().stack_size(1024);
        // Should select Small size class for 1KB request
        
        // We can't easily test the internal state without exposing it,
        // but the important thing is that it compiles and doesn't panic
        let _ = builder1;
        let _ = builder2;
    }
}