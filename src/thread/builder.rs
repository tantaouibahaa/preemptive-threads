use super::{Thread, JoinHandle, ThreadId};
use crate::mem::{StackPool, StackSizeClass};
use crate::errors::SpawnError;

extern crate alloc;
use alloc::string::String;

pub struct ThreadBuilder {
    stack_size: StackSizeClass,
    priority: u8,
    name: Option<String>,
}

impl ThreadBuilder {
    pub fn new() -> Self {
        Self {
            stack_size: StackSizeClass::Medium,
            priority: 128,
            name: None,
        }
    }
    
    pub fn stack_size(mut self, size: StackSizeClass) -> Self {
        self.stack_size = size;
        self
    }
    
    pub fn priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
    
    pub fn name<T: Into<String>>(mut self, name: T) -> Self {
        self.name = Some(name.into());
        self
    }
    
    pub fn spawn<F>(self, _f: F, pool: &StackPool, next_id: ThreadId) -> Result<(Thread, JoinHandle), SpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        let stack = pool
            .allocate(self.stack_size)
            .ok_or(SpawnError::OutOfMemory)?;

        let entry_fn: fn() = || {};
        let (thread, handle) = Thread::new(next_id, stack, entry_fn, self.priority);

        if let Some(name) = self.name {
            thread.set_name(name);
        }

        Ok((thread, handle))
    }
}

impl Default for ThreadBuilder {
    fn default() -> Self {
        Self::new()
    }
}