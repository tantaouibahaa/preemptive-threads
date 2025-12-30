

use super::{ThreadInner, ThreadState};
use crate::mem::ArcLite;

pub struct JoinHandle {
    pub(super) inner: ArcLite<ThreadInner>,
}

impl JoinHandle {
    pub fn join(self) -> Result<(), ()> {
        loop {
            let state = self.inner.state.load(portable_atomic::Ordering::Acquire);
            if state == ThreadState::Finished as u8 {
                break;
            }

            // Yield to scheduler to let other threads (including the one we're
            // waiting for) run
            crate::yield_now();
        }

        if let Some(join_result) = self.inner.join_result.try_lock() {
            if join_result.is_some() {
                Ok(())
            } else {
                Err(())
            }
        } else {
            Err(())
        }
    }
    
    pub fn try_join(&self) -> Option<Result<(), ()>> {
        let state = self.inner.state.load(portable_atomic::Ordering::Acquire);
        if state == ThreadState::Finished as u8 {
            if let Some(join_result) = self.inner.join_result.try_lock() {
                if join_result.is_some() {
                    Some(Ok(()))
                } else {
                    Some(Err(()))
                }
            } else {
                Some(Err(()))
            }
        } else {
            None
        }
    }
    
    pub fn thread_id(&self) -> super::ThreadId {
        self.inner.id
    }
    
    pub fn is_alive(&self) -> bool {
        let state = self.inner.state.load(portable_atomic::Ordering::Acquire);
        state != ThreadState::Finished as u8
    }
}

unsafe impl Send for JoinHandle {}
unsafe impl Sync for JoinHandle {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{Thread, ThreadId};
    use crate::mem::{StackPool, StackSizeClass};
    
    #[cfg(feature = "std-shim")]
    #[test]
    fn test_join_handle_basic() {
        let pool = StackPool::new();
        let stack = pool.allocate(StackSizeClass::Small).unwrap();
        let thread_id = unsafe { ThreadId::new_unchecked(1) };
        
        let (thread, join_handle) = Thread::new(
            thread_id,
            stack,
            || {},
            128,
        );
        
        assert_eq!(join_handle.thread_id(), thread_id);
        assert!(join_handle.is_alive());
        assert!(join_handle.try_join().is_none()); // Thread not finished
        
        // Simulate thread completion
        thread.set_state(ThreadState::Finished);
        if let Some(mut join_result) = thread.inner.join_result.try_lock() {
            *join_result = Some(());
        }
        
        assert!(!join_handle.is_alive());
        assert_eq!(join_handle.try_join(), Some(Ok(())));
    }
}