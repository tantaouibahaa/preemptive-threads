//! Join handle implementation for waiting on thread completion.

use super::{ThreadInner, ThreadState};
use crate::mem::ArcLite;

/// A handle that can be used to wait for a thread to complete.
///
/// This handle allows the caller to wait for a thread to finish execution
/// and retrieve any result. When dropped, it does not affect the thread's
/// execution - only the ability to join it.
pub struct JoinHandle {
    /// Reference to the thread's internal data
    pub(super) inner: ArcLite<ThreadInner>,
}

impl JoinHandle {
    /// Wait for the thread to complete.
    ///
    /// This function blocks until the associated thread has finished
    /// execution. If the thread has already finished, this returns
    /// immediately.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the thread completes successfully, or `Err(())` 
    /// if the thread panicked or could not be joined.
    pub fn join(self) -> Result<(), ()> {
        // Wait for the thread to finish
        loop {
            let state = self.inner.state.load(portable_atomic::Ordering::Acquire);
            if state == ThreadState::Finished as u8 {
                break;
            }

            // Yield to scheduler to let other threads (including the one we're
            // waiting for) run
            crate::yield_now();
        }

        // Check if we have a result
        if let Some(join_result) = self.inner.join_result.try_lock() {
            if join_result.is_some() {
                Ok(())
            } else {
                Err(()) // Thread panicked or failed
            }
        } else {
            Err(()) // Could not acquire lock
        }
    }
    
    /// Check if the thread has finished without blocking.
    ///
    /// # Returns
    ///
    /// `Some(Ok(()))` if the thread has finished successfully,
    /// `Some(Err(()))` if the thread panicked, or `None` if the
    /// thread is still running.
    pub fn try_join(&self) -> Option<Result<(), ()>> {
        let state = self.inner.state.load(portable_atomic::Ordering::Acquire);
        if state == ThreadState::Finished as u8 {
            // Thread has finished, check the result
            if let Some(join_result) = self.inner.join_result.try_lock() {
                if join_result.is_some() {
                    Some(Ok(()))
                } else {
                    Some(Err(())) // Thread panicked or failed
                }
            } else {
                Some(Err(())) // Could not acquire lock
            }
        } else {
            None // Thread still running
        }
    }
    
    /// Get the ID of the thread this handle refers to.
    pub fn thread_id(&self) -> super::ThreadId {
        self.inner.id
    }
    
    /// Check if the associated thread is still alive.
    ///
    /// # Returns
    ///
    /// `true` if the thread is still running or ready to run,
    /// `false` if it has finished or been terminated.
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
    use crate::thread_new::{Thread, ThreadId};
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