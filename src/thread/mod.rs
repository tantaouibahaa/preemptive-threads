

use crate::arch::Arch;
use crate::mem::{ArcLite, Stack};
use crate::time::{Instant, TimeSlice};
use portable_atomic::{AtomicU8, Ordering};

extern crate alloc;
use alloc::string::String;

pub mod handle;
pub mod builder;

pub use handle::JoinHandle;
pub use builder::ThreadBuilder;

static CURRENT_THREAD_ID: portable_atomic::AtomicU64 = portable_atomic::AtomicU64::new(1);

pub fn current_thread_id() -> ThreadId {
    let id = CURRENT_THREAD_ID.load(portable_atomic::Ordering::Relaxed);
    ThreadId::new(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(core::num::NonZeroUsize);

impl core::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ThreadId {
    /// Create a new thread ID from a u64.
    pub fn new(id: u64) -> Self {
        let id_usize = id as usize;
        if id_usize == 0 {
            Self(unsafe { core::num::NonZeroUsize::new_unchecked(1) })
        } else {
            Self(unsafe { core::num::NonZeroUsize::new_unchecked(id_usize) })
        }
    }

    /// Create a new thread ID.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `id` is non-zero and unique.
    pub unsafe fn new_unchecked(id: usize) -> Self {
        Self(unsafe { core::num::NonZeroUsize::new_unchecked(id) })
    }

    /// Get the raw ID value.
    pub fn get(self) -> usize {
        self.0.get()
    }

    /// Get the ID as u64.
    pub fn as_u64(self) -> u64 {
        self.0.get() as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreadState {
    Ready = 0,
    Running = 1,
    Blocked = 2,
    Finished = 3,
}

pub struct Thread {
    inner: ArcLite<ThreadInner>,
}

/// Internal thread data shared between Thread and JoinHandle.
pub struct ThreadInner {
    pub id: ThreadId,
    pub state: AtomicU8,
    pub priority: AtomicU8,
    pub stack: Option<Stack>,
    pub context: spin::Mutex<<crate::arch::DefaultArch as Arch>::SavedContext>,
    pub entry_point: Option<fn()>,
    pub join_result: spin::Mutex<Option<()>>,
    pub time_slice: TimeSlice,
    pub name: spin::Mutex<Option<String>>,
}

impl Thread {
    /// Create a new thread with the given parameters.
    ///
    /// # Arguments
    ///
    /// * `id` - Unique identifier for this thread
    /// * `stack` - Stack allocated for this thread
    /// * `entry_point` - Function to execute in this thread
    /// * `priority` - Thread priority (0-255, higher = more important)
    ///
    /// # Returns
    ///
    /// A new Thread instance and corresponding JoinHandle.
    pub fn new(
        id: ThreadId,
        stack: Stack,
        entry_point: fn(),
        priority: u8,
    ) -> (Self, JoinHandle) {
        let inner = ThreadInner {
            id,
            state: AtomicU8::new(ThreadState::Ready as u8),
            priority: AtomicU8::new(priority),
            stack: Some(stack),
            context: spin::Mutex::new(Default::default()),
            entry_point: Some(entry_point),
            join_result: spin::Mutex::new(None),
            time_slice: TimeSlice::new(priority),
            name: spin::Mutex::new(None),
        };

        let inner_arc = ArcLite::new(inner);

        let thread = Self { inner: inner_arc.clone() };

        if let Some(stack_bottom) = thread.stack_bottom() {
            let entry = entry_point as usize;
            let stack_top = stack_bottom as usize;

            thread.setup_initial_context(entry, stack_top, 0);
        }


        let join_handle = JoinHandle {
            inner: inner_arc,
        };

        (thread, join_handle)
    }

    /// Get the thread's unique identifier.
    pub fn id(&self) -> ThreadId {
        self.inner.id
    }

    /// Get the thread's current state.
    pub fn state(&self) -> ThreadState {
        let state_val = self.inner.state.load(Ordering::Acquire);
        match state_val {
            0 => ThreadState::Ready,
            1 => ThreadState::Running,
            2 => ThreadState::Blocked,
            3 => ThreadState::Finished,
            _ => ThreadState::Ready, // Default fallback
        }
    }

    /// Set the thread's state.
    ///
    /// # Arguments
    ///
    /// * `new_state` - The new state to set
    pub fn set_state(&self, new_state: ThreadState) {
        self.inner.state.store(new_state as u8, Ordering::Release);
    }

    /// Get the thread's priority.
    pub fn priority(&self) -> u8 {
        self.inner.priority.load(Ordering::Acquire)
    }

    /// Set the thread's priority.
    ///
    /// # Arguments
    ///
    /// * `new_priority` - The new priority (0-255, higher = more important)
    pub fn set_priority(&self, new_priority: u8) {
        self.inner.priority.store(new_priority, Ordering::Release);
        self.inner.time_slice.set_priority(new_priority);
    }

    /// Check if this thread is runnable (ready or running).
    pub fn is_runnable(&self) -> bool {
        matches!(self.state(), ThreadState::Ready | ThreadState::Running)
    }

    /// Get a pointer to the thread's saved context.
    ///
    /// # Safety
    ///
    /// The returned pointer is only valid while the context mutex is not locked
    /// by another thread. Caller must ensure proper synchronization.
    ///
    /// # Returns
    ///
    /// A pointer to the saved context.
    pub fn context_ptr(&self) -> *mut <crate::arch::DefaultArch as Arch>::SavedContext {
        // Get a pointer to the context inside the mutex
        // This is safe because ArcLite ensures the ThreadInner stays alive
        let ctx_guard = self.inner.context.lock();
        // Convert the reference to a raw pointer
        // The mutex guard will be dropped, but the pointer remains valid
        // because ThreadInner (and thus the context) is kept alive by ArcLite
        let ptr = &*ctx_guard as *const _ as *mut _;
        drop(ctx_guard);
        ptr
    }

    /// Set up the initial context for a new thread.
    ///
    /// This configures the context so that when context-switched to, the thread
    /// will begin execution at the specified entry point with the given argument.
    ///
    /// # Arguments
    ///
    /// * `entry_point` - Address of the function to start executing
    /// * `stack_top` - Top of the stack (initial SP value)
    /// * `arg` - Argument to pass to the entry point (in x0 on ARM64)
    #[allow(unused_variables, unused_mut)]
    pub fn setup_initial_context(&self, entry_point: usize, stack_top: usize, arg: usize) {
        let mut ctx_guard = self.inner.context.lock();

        // Set up ARM64 context
        #[cfg(target_arch = "aarch64")]
        {
            // Clear all registers
            ctx_guard.x = [0; 31];
            // Set argument in x0
            ctx_guard.x[0] = arg as u64;
            // Set stack pointer
            ctx_guard.sp = stack_top as u64;
            // Set program counter to entry point
            ctx_guard.pc = entry_point as u64;
            // Set PSTATE: EL1h mode, interrupts enabled
            ctx_guard.pstate = 0x3c5;

            // Initialize FPU state if enabled
            #[cfg(feature = "full-fpu")]
            {
                ctx_guard.neon_state = [0; 32];
                ctx_guard.fpcr = 0;
                ctx_guard.fpsr = 0;
            }
        }

        // Fallback for non-ARM64 (testing)
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = (entry_point, stack_top, arg);
            // NoOp context doesn't have registers
        }
    }

    /// Get the thread's stack bottom (initial stack pointer).
    pub fn stack_bottom(&self) -> Option<*mut u8> {
        self.inner.stack.as_ref().map(|stack| stack.stack_bottom())
    }

    /// Check if the thread's stack canary is intact (stack overflow detection).
    pub fn check_stack_integrity(&self) -> bool {
        if let Some(ref stack) = self.inner.stack {
            // Use a fixed canary value for now
            let canary = 0xDEADBEEFCAFEBABE;
            stack.check_canary(canary)
        } else {
            false
        }
    }

    /// Start a new time slice for this thread.
    ///
    /// This should be called when the thread is scheduled to run.
    pub fn start_time_slice(&self) {
        let current_time = Instant::now();
        self.inner.time_slice.start_slice(current_time);
    }

    /// Update the thread's virtual runtime and check if preemption is needed.
    ///
    /// # Returns
    ///
    /// `true` if the thread's time slice has expired and it should be preempted.
    pub fn should_preempt(&self) -> bool {
        let current_time = Instant::now();
        self.inner.time_slice.update_vruntime(current_time)
    }

    /// Get the thread's current virtual runtime.
    ///
    /// This is used by the scheduler for fair scheduling decisions.
    pub fn vruntime(&self) -> u64 {
        self.inner.time_slice.vruntime()
    }

    /// Set the thread name for debugging purposes.
    pub fn set_name(&self, name: String) {
        if let Some(mut thread_name) = self.inner.name.try_lock() {
            *thread_name = Some(name);
        }
    }

    /// Get the thread name.
    pub fn name(&self) -> Option<String> {
        self.inner.name.try_lock().and_then(|name| name.clone())
    }
}

impl Clone for Thread {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

unsafe impl Send for ThreadInner {}
unsafe impl Sync for ThreadInner {}

/// A reference to a thread that is currently ready to run.
///
/// This type represents a thread that is in the scheduler's ready queue
/// and can be selected to run on a CPU.
#[derive(Clone)]
pub struct ReadyRef(pub Thread);

/// A reference to a thread that is currently running on a CPU.
///
/// This type represents a thread that is actively executing on a CPU.
#[derive(Clone)]
pub struct RunningRef(pub Thread);

impl ReadyRef {
    /// Convert this ready reference to a running reference.
    ///
    /// This should be called when the scheduler selects this thread to run.
    pub fn start_running(self) -> RunningRef {
        self.0.set_state(ThreadState::Running);
        self.0.start_time_slice();
        RunningRef(self.0)
    }

    /// Get the thread's priority.
    pub fn priority(&self) -> u8 {
        self.0.priority()
    }

    /// Get the thread's unique identifier.
    pub fn id(&self) -> ThreadId {
        self.0.id()
    }
}

impl RunningRef {
    /// Convert this running reference back to a ready reference.
    ///
    /// This should be called when the thread is preempted or yields.
    pub fn stop_running(self) -> ReadyRef {
        self.0.set_state(ThreadState::Ready);
        ReadyRef(self.0)
    }

    /// Check if this thread should be preempted.
    ///
    /// This updates the thread's virtual runtime and returns true if
    /// the time slice has expired.
    pub fn should_preempt(&self) -> bool {
        self.0.should_preempt()
    }

    /// Mark this thread as blocked.
    ///
    /// This should be called when the thread blocks on I/O or synchronization.
    pub fn block(self) {
        self.0.set_state(ThreadState::Blocked);
    }

    /// Mark this thread as finished.
    ///
    /// This should be called when the thread's entry point returns.
    pub fn finish(self) {
        self.0.set_state(ThreadState::Finished);

        // Signal any joiners that we're done
        if let Some(mut join_result) = self.0.inner.join_result.try_lock() {
            *join_result = Some(());
        }
    }

    /// Prepare this thread for preemption.
    ///
    /// This saves the current state and returns a ReadyRef that can be re-enqueued.
    pub fn prepare_preemption(&self) -> ReadyRef {
        let ready = ReadyRef(self.0.clone());
        ready.0.set_state(ThreadState::Ready);
        ready
    }

    /// Get the thread's priority.
    pub fn priority(&self) -> u8 {
        self.0.priority()
    }

    /// Get the thread's unique identifier.
    pub fn id(&self) -> ThreadId {
        self.0.id()
    }

    /// Get the CPU this thread last ran on.
    ///
    /// For now, return 0 as a placeholder. In a real implementation,
    /// this would track the actual CPU assignment.
    pub fn last_cpu(&self) -> usize {
        0 // TODO: Track actual CPU assignment
    }

    /// Get access to the thread's time slice for scheduler decisions.
    pub fn time_slice(&self) -> &TimeSlice {
        &self.0.inner.time_slice
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::{StackPool, StackSizeClass};

    #[cfg(feature = "std-shim")]
    #[test]
    fn test_thread_creation() {
        use std::println;
        let pool = StackPool::new();
        let stack = pool.allocate(StackSizeClass::Small).unwrap();
        let thread_id = unsafe { ThreadId::new_unchecked(1) };

        let (thread, _join_handle) = Thread::new(
            thread_id,
            stack,
            || { println!("Hello from thread!"); },
            128,
        );

        assert_eq!(thread.id(), thread_id);
        assert_eq!(thread.state(), ThreadState::Ready);
        assert_eq!(thread.priority(), 128);
        assert!(thread.is_runnable());
    }

    #[cfg(feature = "std-shim")]
    #[test]
    fn test_thread_state_transitions() {
        let pool = StackPool::new();
        let stack = pool.allocate(StackSizeClass::Small).unwrap();
        let thread_id = unsafe { ThreadId::new_unchecked(1) };

        let (thread, _join_handle) = Thread::new(
            thread_id,
            stack,
            || {},
            128,
        );

        // Test state transitions
        assert_eq!(thread.state(), ThreadState::Ready);

        thread.set_state(ThreadState::Running);
        assert_eq!(thread.state(), ThreadState::Running);

        thread.set_state(ThreadState::Blocked);
        assert_eq!(thread.state(), ThreadState::Blocked);
        assert!(!thread.is_runnable());

        thread.set_state(ThreadState::Finished);
        assert_eq!(thread.state(), ThreadState::Finished);
        assert!(!thread.is_runnable());
    }
}
