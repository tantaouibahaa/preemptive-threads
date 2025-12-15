//! Kernel abstraction for managing the threading system.
//!
//! This module provides the main `Kernel` struct that coordinates all
//! threading operations for the Raspberry Pi Zero 2 W.

use crate::arch::Arch;
use crate::sched::Scheduler;
use crate::thread_new::{JoinHandle, ReadyRef, RunningRef, Thread, ThreadId};
use crate::mem::{StackPool, StackSizeClass};
use core::marker::PhantomData;
use portable_atomic::{AtomicBool, AtomicUsize, AtomicPtr, Ordering};
use alloc::boxed::Box;

/// Global kernel reference for interrupt handlers.
static GLOBAL_KERNEL: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Main kernel handle that manages the threading system.
///
/// This struct coordinates all threading operations and provides a safe
/// interface to the underlying scheduler and architecture abstractions.
///
/// # Type Parameters
///
/// * `A` - Architecture implementation
/// * `S` - Scheduler implementation
pub struct Kernel<A: Arch, S: Scheduler> {
    /// Scheduler instance
    scheduler: S,
    /// Stack pool for thread allocation
    stack_pool: StackPool,
    /// Architecture marker (zero-sized)
    _arch: PhantomData<A>,
    /// Whether the kernel has been initialized
    initialized: AtomicBool,
    /// Next thread ID to assign
    next_thread_id: AtomicUsize,
    /// Currently running thread on each CPU (simplified to single CPU for now)
    current_thread: spin::Mutex<Option<RunningRef>>,
}

impl<A: Arch, S: Scheduler> Kernel<A, S> {
    /// Create a new kernel instance.
    ///
    /// # Arguments
    ///
    /// * `scheduler` - Scheduler implementation to use
    ///
    /// # Returns
    ///
    /// A new kernel instance ready for initialization.
    pub const fn new(scheduler: S) -> Self {
        Self {
            scheduler,
            stack_pool: StackPool::new(),
            _arch: PhantomData,
            initialized: AtomicBool::new(false),
            next_thread_id: AtomicUsize::new(1), // Start from 1, never use 0
            current_thread: spin::Mutex::new(None),
        }
    }

    /// Initialize the kernel.
    ///
    /// This must be called before any threading operations can be performed.
    /// It sets up architecture-specific features and prepares the scheduler.
    ///
    /// # Returns
    ///
    /// `Ok(())` if initialization succeeds, `Err(())` if already initialized.
    pub fn init(&self) -> Result<(), ()> {
        if self
            .initialized
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // Architecture initialization is handled by boot code
            // (aarch64_boot.rs calls init() on GIC and timer)
            Ok(())
        } else {
            Err(()) // Already initialized
        }
    }

    /// Check if the kernel has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Generate a new unique thread ID.
    ///
    /// Thread IDs are never reused and are guaranteed to be unique
    /// for the lifetime of the kernel instance.
    pub fn next_thread_id(&self) -> ThreadId {
        let id = self.next_thread_id.fetch_add(1, Ordering::AcqRel);
        // Safety: We start from 1 and only increment, so this will never be zero
        unsafe { ThreadId::new_unchecked(id) }
    }

    /// Get a reference to the scheduler.
    pub fn scheduler(&self) -> &S {
        &self.scheduler
    }

    /// Spawn a new thread.
    ///
    /// # Arguments
    ///
    /// * `entry_point` - Closure to run in the new thread
    /// * `priority` - Thread priority (0-255, higher = more important)
    ///
    /// # Returns
    ///
    /// JoinHandle for the newly created thread, or an error if creation fails.
    pub fn spawn<F>(&self, entry_point: F, priority: u8) -> Result<JoinHandle, SpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        // Debug: entering spawn
        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] entering spawn()");

        if !self.is_initialized() {
            return Err(SpawnError::NotInitialized);
        }

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] allocating stack...");

        // Allocate stack
        let stack = self
            .stack_pool
            .allocate(StackSizeClass::Medium)
            .ok_or(SpawnError::OutOfMemory)?;

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] stack allocated, getting thread ID...");

        // Generate unique thread ID
        let thread_id = self.next_thread_id();

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] boxing closure...");

        // Box the closure to move it to the heap
        let closure_box = Box::new(entry_point);
        let closure_ptr = Box::into_raw(closure_box);

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] closure boxed");

        // Create trampoline that will call the closure
        // The trampoline is a fn() that we'll set up in the context
        fn thread_trampoline<F: FnOnce() + Send + 'static>(closure_ptr: *mut F) {
            // Reconstruct the boxed closure and call it
            let closure = unsafe { Box::from_raw(closure_ptr) };
            closure();

            // Thread finished - just loop forever with interrupts enabled
            // Preemption will handle scheduling other threads
            #[allow(clippy::empty_loop)]
            loop {
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    core::arch::asm!("wfe", options(nomem, nostack));
                }
                // On non-aarch64 (std-shim testing), hint to pause
                #[cfg(not(target_arch = "aarch64"))]
                core::hint::spin_loop();
            }
        }

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] getting stack bottom...");

        // Get the stack bottom (top of stack memory, since stack grows down)
        let stack_bottom = stack.stack_bottom();

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] creating Thread::new...");

        // Create thread with the trampoline as entry point
        // We'll pass closure_ptr via the thread's context (x0 register on ARM64)
        let entry_fn: fn() = || {};  // Placeholder - actual entry is set up in context

        let (thread, join_handle) = Thread::new(thread_id, stack, entry_fn, priority);

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] setting up initial context...");

        // Set up the initial context to start at the trampoline with closure_ptr as arg
        thread.setup_initial_context(
            thread_trampoline::<F> as *const () as usize,
            stack_bottom as usize,
            closure_ptr as usize,
        );

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] enqueueing in scheduler...");

        // Convert to ReadyRef and enqueue in scheduler
        let ready_ref = ReadyRef(thread);
        self.scheduler.enqueue(ready_ref);

        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[spawn] done!");

        Ok(join_handle)
    }

    /// Spawn a thread with a simple function pointer (no closure).
    ///
    /// This is simpler than spawn() and useful for threads that don't capture state.
    pub fn spawn_fn(&self, entry_point: fn(), priority: u8) -> Result<JoinHandle, SpawnError> {
        if !self.is_initialized() {
            return Err(SpawnError::NotInitialized);
        }

        let stack = self
            .stack_pool
            .allocate(StackSizeClass::Small)
            .ok_or(SpawnError::OutOfMemory)?;

        let thread_id = self.next_thread_id();
        let stack_bottom = stack.stack_bottom();

        let (thread, join_handle) = Thread::new(thread_id, stack, entry_point, priority);

        // Set up context with entry point (no argument needed for fn())
        thread.setup_initial_context(entry_point as usize, stack_bottom as usize, 0);

        let ready_ref = ReadyRef(thread);
        self.scheduler.enqueue(ready_ref);

        Ok(join_handle)
    }

    /// Yield the current thread, allowing other threads to run.
    pub fn yield_now(&self) {
        if !self.is_initialized() {
            return;
        }

        // Disable interrupts during scheduling decision
        A::disable_interrupts();

        let mut current_guard = self.current_thread.lock();

        if let Some(current) = current_guard.take() {
            // Get current thread's context pointer before converting
            let prev_ctx = current.0.context_ptr();

            // Current thread is yielding voluntarily - convert back to ready
            let ready = current.stop_running();
            self.scheduler.enqueue(ready);

            // Try to pick next thread to run
            if let Some(next) = self.scheduler.pick_next(0) {
                let next_ctx = next.0.context_ptr();
                let running = next.start_running();
                *current_guard = Some(running);
                drop(current_guard); // Release lock before context switch

                // Re-enable interrupts and perform context switch
                A::enable_interrupts();

                if !prev_ctx.is_null() && !next_ctx.is_null() {
                    unsafe {
                        // Cast to the correct type - safe because we only target RPi Zero 2 W
                        A::context_switch(
                            prev_ctx as *mut A::SavedContext,
                            next_ctx as *const A::SavedContext,
                        );
                    }
                }
            } else {
                // No other threads, re-enable interrupts
                A::enable_interrupts();
            }
        } else {
            drop(current_guard);
            A::enable_interrupts();
        }
    }

    /// Start the first thread (bootstrap the scheduler).
    ///
    /// This picks the first thread from the scheduler and starts running it.
    /// Called once during kernel initialization.
    pub fn start_first_thread(&self) {
        if !self.is_initialized() {
            return;
        }

        let mut current_guard = self.current_thread.lock();

        if current_guard.is_some() {
            // Already have a running thread
            return;
        }

        if let Some(next) = self.scheduler.pick_next(0) {
            let next_ctx = next.0.context_ptr();
            let running = next.start_running();
            *current_guard = Some(running);
            drop(current_guard);

            // Set up IRQ context pointers so interrupts save/restore to this thread
            #[cfg(target_arch = "aarch64")]
            unsafe {
                crate::arch::aarch64::set_current_irq_context(
                    next_ctx as *mut crate::arch::aarch64::Aarch64Context
                );
            }

            // Jump to the first thread (no previous context to save)
            if !next_ctx.is_null() {
                unsafe {
                    // Create a dummy context for "from" since we're not returning
                    let mut dummy_ctx = A::SavedContext::default();
                    A::context_switch(
                        &mut dummy_ctx as *mut A::SavedContext,
                        next_ctx as *const A::SavedContext,
                    );
                }
            }
        }
    }

    /// Handle a timer interrupt for preemptive scheduling (legacy - uses context_switch).
    ///
    /// This should be called from the architecture-specific timer interrupt handler.
    ///
    /// # Safety
    ///
    /// Must be called from an interrupt context with interrupts disabled.
    #[allow(dead_code)]
    pub unsafe fn handle_timer_interrupt(&self) {
        if !self.is_initialized() {
            return;
        }

        // Try to get the lock - if we can't, another CPU is scheduling
        let mut current_guard = match self.current_thread.try_lock() {
            Some(guard) => guard,
            None => return, // Lock contention, skip this tick
        };

        if let Some(ref current) = *current_guard {
            // Always preempt on timer interrupt for now (force round-robin)
            // TODO: Restore time slice checking once debugging is complete
            let should_switch = true; // current.should_preempt();
            if should_switch {
                // Take current thread out
                if let Some(current) = current_guard.take() {
                    let prev_ctx = current.0.context_ptr();

                    // Convert to ready and enqueue
                    let ready = current.stop_running();
                    self.scheduler.enqueue(ready);

                    // Pick next thread (could be same thread if no others)
                    if let Some(next) = self.scheduler.pick_next(0) {
                        let next_ctx = next.0.context_ptr();
                        let running = next.start_running();
                        *current_guard = Some(running);
                        drop(current_guard);

                        // Context switch to next thread
                        if !prev_ctx.is_null() && !next_ctx.is_null() {
                            unsafe {
                                A::context_switch(
                                    prev_ctx as *mut A::SavedContext,
                                    next_ctx as *const A::SavedContext,
                                );
                            }
                        }
                    }
                }
            }
        } else {
            // No current thread, try to schedule one
            if let Some(next) = self.scheduler.pick_next(0) {
                let next_ctx = next.0.context_ptr();
                let running = next.start_running();
                *current_guard = Some(running);
                drop(current_guard);

                // Jump to the thread
                if !next_ctx.is_null() {
                    unsafe {
                        let mut dummy_ctx = A::SavedContext::default();
                        A::context_switch(
                            &mut dummy_ctx as *mut A::SavedContext,
                            next_ctx as *const A::SavedContext,
                        );
                    }
                }
            }
        }
    }

    /// Handle preemption from an IRQ context.
    ///
    /// This method is called from the timer interrupt handler. Instead of doing
    /// a context_switch (which doesn't work from interrupt context), it updates
    /// the IRQ_LOAD_CTX pointer so that the IRQ handler's return sequence
    /// restores the new thread's context.
    ///
    /// # Safety
    ///
    /// Must be called from an IRQ handler with interrupts disabled.
    /// The IRQ handler must have saved the current context to IRQ_SAVE_CTX.
    #[cfg(target_arch = "aarch64")]
    pub fn handle_irq_preemption(&self) {
        if !self.is_initialized() {
            return;
        }

        // Try to get the lock - if we can't, another CPU is scheduling
        let mut current_guard = match self.current_thread.try_lock() {
            Some(guard) => guard,
            None => return, // Lock contention, skip this tick
        };

        // Debug output
        #[cfg(target_arch = "aarch64")]
        crate::pl011_println!("[IRQ] handle_irq_preemption called");

        if let Some(ref _current) = *current_guard {
            // Always preempt on timer interrupt for now (force round-robin)
            let should_switch = true;

            if should_switch {
                // Take current thread out
                if let Some(current) = current_guard.take() {
                    // Current thread's context was saved by IRQ handler to IRQ_SAVE_CTX
                    // which points to this thread's context structure

                    // Convert to ready and enqueue
                    let ready = current.stop_running();
                    self.scheduler.enqueue(ready);

                    // Pick next thread
                    if let Some(next) = self.scheduler.pick_next(0) {
                        let next_ctx = next.0.context_ptr();

                        #[cfg(target_arch = "aarch64")]
                        crate::pl011_println!("[IRQ] Switching to thread {}", next.id().get());

                        let running = next.start_running();
                        *current_guard = Some(running);
                        drop(current_guard);

                        // Update IRQ context pointers for the new thread
                        // - IRQ_LOAD_CTX: IRQ handler will load from here when returning
                        // - IRQ_SAVE_CTX: Next interrupt will save here
                        if !next_ctx.is_null() {
                            crate::arch::aarch64::set_irq_load_context(
                                next_ctx as *mut crate::arch::aarch64::Aarch64Context
                            );
                            // Also update SAVE for next interrupt
                            unsafe {
                                crate::arch::aarch64::set_current_irq_context(
                                    next_ctx as *mut crate::arch::aarch64::Aarch64Context
                                );
                            }
                        }
                    } else {
                        // No other thread to run, stay with current (re-enqueue it)
                        drop(current_guard);
                    }
                }
            }
        } else {
            #[cfg(target_arch = "aarch64")]
            crate::pl011_println!("[IRQ] No current thread!");
            drop(current_guard);
        }
    }

    /// Get current thread statistics.
    pub fn thread_stats(&self) -> (usize, usize, usize) {
        self.scheduler.stats()
    }

    /// Register this kernel as the global kernel for interrupt handlers.
    ///
    /// # Safety
    ///
    /// The kernel must outlive all interrupt handling (i.e., for the lifetime
    /// of the system).
    pub unsafe fn register_global(&'static self) {
        GLOBAL_KERNEL.store(self as *const _ as *mut (), Ordering::Release);
    }
}

/// Errors that can occur when spawning threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// Kernel has not been initialized
    NotInitialized,
    /// Out of memory for stack allocation
    OutOfMemory,
    /// Maximum number of threads reached
    TooManyThreads,
    /// Invalid stack size
    InvalidStackSize,
}

// Safety: Kernel can be shared between threads as long as the scheduler is thread-safe
unsafe impl<A: Arch, S: Scheduler> Send for Kernel<A, S> {}
unsafe impl<A: Arch, S: Scheduler> Sync for Kernel<A, S> {}

/// Get the global kernel reference (for interrupt handlers).
///
/// Returns None if no kernel has been registered.
pub fn get_global_kernel<A: Arch, S: Scheduler>() -> Option<&'static Kernel<A, S>> {
    let ptr = GLOBAL_KERNEL.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*(ptr as *const Kernel<A, S>) })
    }
}

/// Yield the current thread (convenience function).
///
/// This uses the global kernel if registered, otherwise does nothing.
pub fn yield_current() {
    use crate::arch::DefaultArch;
    use crate::sched::RoundRobinScheduler;

    if let Some(kernel) = get_global_kernel::<DefaultArch, RoundRobinScheduler>() {
        kernel.yield_now();
    }
}
