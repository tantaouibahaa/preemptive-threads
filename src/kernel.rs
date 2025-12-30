

use crate::arch::Arch;
use crate::sched::Scheduler;
use crate::thread::{JoinHandle, ReadyRef, RunningRef, Thread, ThreadId};
use crate::mem::{StackPool, StackSizeClass};
use crate::errors::SpawnError;
use core::marker::PhantomData;
use portable_atomic::{AtomicBool, AtomicUsize, AtomicPtr, Ordering};
use alloc::boxed::Box;

static GLOBAL_KERNEL: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

pub struct Kernel<A: Arch, S: Scheduler> {
    scheduler: S,
    stack_pool: StackPool,
    _arch: PhantomData<A>,
    initialized: AtomicBool,
    next_thread_id: AtomicUsize,
    current_thread: spin::Mutex<Option<RunningRef>>,
}

impl<A: Arch, S: Scheduler> Kernel<A, S> {
    pub const fn new(scheduler: S) -> Self {
        Self {
            scheduler,
            stack_pool: StackPool::new(),
            _arch: PhantomData,
            initialized: AtomicBool::new(false),
            next_thread_id: AtomicUsize::new(1),
            current_thread: spin::Mutex::new(None),
        }
    }

    pub fn init(&self) -> Result<(), ()> {
        if self
            .initialized
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    pub fn next_thread_id(&self) -> ThreadId {
        let id = self.next_thread_id.fetch_add(1, Ordering::AcqRel);
        unsafe { ThreadId::new_unchecked(id) }
    }

    /// Get a reference to the scheduler.
    pub fn scheduler(&self) -> &S {
        &self.scheduler
    }


    pub fn spawn<F>(&self, entry_point: F, priority: u8) -> Result<JoinHandle, SpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        if !self.is_initialized() {
            return Err(SpawnError::NotInitialized);
        }

        let stack = self
            .stack_pool
            .allocate(StackSizeClass::Medium)
            .ok_or(SpawnError::OutOfMemory)?;

        let thread_id = self.next_thread_id();

        let closure_box = Box::new(entry_point);
        let closure_ptr = Box::into_raw(closure_box);

        fn thread_trampoline<F: FnOnce() + Send + 'static>(closure_ptr: *mut F) {
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!(
                    "msr daifclr, #2",
                    options(nomem, nostack)
                );
            }

            let closure = unsafe { Box::from_raw(closure_ptr) };
            closure();

            // Preemption will handle scheduling other threads
            #[allow(clippy::empty_loop)]
            loop {
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    core::arch::asm!("wfe", options(nomem, nostack));
                }
                #[cfg(not(target_arch = "aarch64"))]
                core::hint::spin_loop();
            }
        }

        let stack_bottom = stack.stack_bottom();

        let entry_fn: fn() = || {};
        let (thread, join_handle) = Thread::new(thread_id, stack, entry_fn, priority);

        thread.setup_initial_context(
            thread_trampoline::<F> as *const () as usize,
            stack_bottom as usize,
            closure_ptr as usize,
        );

        let ready_ref = ReadyRef(thread);
        self.scheduler.enqueue(ready_ref);

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

        thread.setup_initial_context(entry_point as usize, stack_bottom as usize, 0);

        let ready_ref = ReadyRef(thread);
        self.scheduler.enqueue(ready_ref);

        Ok(join_handle)
    }

    #[inline(never)]
    pub fn yield_now(&self) {
        if !self.is_initialized() {
            return;
        }

        A::disable_interrupts();

        let mut current_guard = self.current_thread.lock();

        if let Some(current) = current_guard.take() {
            let prev_id = current.id().get();
            let prev_ctx = current.0.context_ptr();

            let current_sp: u64;
            unsafe { core::arch::asm!("mov {}, sp", out(reg) current_sp); }
            crate::pl011_println!("[DEBUG] T{} yielding, actual SP={:#x}, ctx_addr={:#x}",
                prev_id, current_sp, prev_ctx as usize);

            let ready = current.stop_running();
            self.scheduler.enqueue(ready);

            if let Some(next) = self.scheduler.pick_next(0) {
                let next_id = next.id().get();
                let next_ctx = next.0.context_ptr();
                crate::pl011_println!("[YIELD] {} -> {}: next_ctx_addr={:#x}",
                    prev_id, next_id, next_ctx as usize);
                let next_pc = unsafe { (*next_ctx).pc };
                let next_sp = unsafe { (*next_ctx).sp };
                let next_x30 = unsafe { (*next_ctx).x[30] };
                crate::pl011_println!("        next_pc={:#x}, next_sp={:#x}, next_x30={:#x}",
                    next_pc, next_sp, next_x30);
                let running = next.start_running();
                *current_guard = Some(running);
                drop(current_guard);


                if !prev_ctx.is_null() && !next_ctx.is_null() {
                    unsafe {
                        A::context_switch(
                            prev_ctx as *mut A::SavedContext,
                            next_ctx as *const A::SavedContext,
                        );
                    }
                    A::enable_interrupts();
                    let my_saved_sp = unsafe { (*prev_ctx).sp };
                    crate::pl011_println!("[RESUMED] saved_sp in my ctx = {:#x}", my_saved_sp);
                } else {
                    A::enable_interrupts();
                }
            } else {
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
    ///
    /// Note: This function handles interrupt enabling internally - do NOT enable
    /// interrupts before calling this function.
    #[inline(never)]
    pub fn start_first_thread(&self) {
        if !self.is_initialized() {
            return;
        }

        A::disable_interrupts();

        let mut current_guard = self.current_thread.lock();

        if current_guard.is_some() {
            A::enable_interrupts();
            return;
        }

        if let Some(next) = self.scheduler.pick_next(0) {
            let next_ctx = next.0.context_ptr();

            let running = next.start_running();
            *current_guard = Some(running);
            drop(current_guard);

            #[cfg(target_arch = "aarch64")]
            unsafe {
                crate::arch::aarch64::set_current_irq_context(
                    next_ctx
                );
            }


            if !next_ctx.is_null() {
                unsafe {
                    let mut dummy_ctx = A::SavedContext::default();
                    A::context_switch(
                        &mut dummy_ctx as *mut A::SavedContext,
                        next_ctx as *const A::SavedContext,
                    );
                }
            }
        } else {
            A::enable_interrupts();
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

        let mut current_guard = match self.current_thread.try_lock() {
            Some(guard) => guard,
            None => return,
        };

        if let Some(ref _current) = *current_guard {
            // TODO: Restore time slice checking once debugging is complete
            let should_switch = true; // current.should_preempt();
            if should_switch {
                if let Some(current) = current_guard.take() {
                    let prev_ctx = current.0.context_ptr();

                    let ready = current.stop_running();
                    self.scheduler.enqueue(ready);

                    if let Some(next) = self.scheduler.pick_next(0) {
                        let next_ctx = next.0.context_ptr();
                        let running = next.start_running();
                        *current_guard = Some(running);
                        drop(current_guard);

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
            if let Some(next) = self.scheduler.pick_next(0) {
                let next_ctx = next.0.context_ptr();
                let running = next.start_running();
                *current_guard = Some(running);
                drop(current_guard);

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

        let mut current_guard = match self.current_thread.try_lock() {
            Some(guard) => guard,
            None => return,
        };

        if let Some(ref _current) = *current_guard {
            let should_switch = true;

            if should_switch {
                if let Some(current) = current_guard.take() {


                    let old_id = current.id().get();

                    let ready = current.stop_running();
                    self.scheduler.enqueue(ready);

                    if let Some(next) = self.scheduler.pick_next(0) {
                        let next_ctx = next.0.context_ptr();
                        let _old_id = old_id; // Suppress unused warning
                        let _new_id = next.id().get();

                        let running = next.start_running();
                        *current_guard = Some(running);
                        drop(current_guard);

                        if !next_ctx.is_null() {
                            crate::arch::aarch64::set_irq_load_context(
                                next_ctx
                            );
                            unsafe {
                                crate::arch::aarch64::set_current_irq_context(
                                    next_ctx
                                );
                            }
                        }
                    } else {
                        drop(current_guard);
                    }
                }
            }
        } else {
            drop(current_guard);
        }
    }

    pub fn thread_stats(&self) -> (usize, usize, usize) {
        self.scheduler.stats()
    }
    /// # Safety
    ///
    /// This function stores a raw pointer to `self` in a global `AtomicPtr`.
    /// TODO:  try to find another way
    pub unsafe fn register_global(&'static self) {
        GLOBAL_KERNEL.store(self as *const _ as *mut (), Ordering::Release);
    }
}



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
