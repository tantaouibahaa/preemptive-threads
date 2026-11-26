//! Per-core scheduler state, reached through TPIDR_EL1.

use super::runqueue::RunQueue;
use super::Tcb;
use crate::platform::MAX_CPUS;
use crate::sync::spin::IrqSpinLock;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};

/// Size of each core's IRQ stack.
pub const IRQ_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct IrqStacks([[u8; IRQ_STACK_SIZE]; MAX_CPUS]);

static mut IRQ_STACKS: IrqStacks = IrqStacks([[0; IRQ_STACK_SIZE]; MAX_CPUS]);

/// State owned by one core.
pub struct PerCpu {
    /// Core index.
    pub id: usize,
    /// Set once the core has finished booting.
    pub online: AtomicBool,
    /// Thread currently running on this core. Only touched by this core,
    /// with IRQs masked.
    pub(crate) current: UnsafeCell<Option<Arc<Tcb>>>,
    /// This core's idle thread.
    pub(crate) idle: UnsafeCell<Option<Arc<Tcb>>>,
    /// Ready threads assigned to this core.
    pub(crate) rq: IrqSpinLock<RunQueue>,
    /// Number of threads in `rq` (read without the lock by `select_cpu`).
    pub(crate) nr_queued: AtomicUsize,
    /// Run the scheduler at the next opportunity (IRQ exit or idle loop).
    pub(crate) need_resched: AtomicBool,
    /// Whether the idle thread is running.
    pub(crate) is_idle: AtomicBool,
    /// Effective priority of the running thread.
    pub(crate) current_prio: AtomicU8,
    /// Id of the running thread (for diagnostics).
    pub(crate) current_tid: AtomicU64,
    /// Absolute counter value of the next tick.
    pub(crate) next_tick: AtomicU64,
    /// TTBR0_EL1 value of the running thread (0 = the kernel tables).
    pub(crate) ttbr0: AtomicU64,
    pub(crate) ticks: AtomicU64,
    pub(crate) idle_ticks: AtomicU64,
    pub(crate) switches: AtomicU64,
    pub(crate) preemptions: AtomicU64,
    pub(crate) steals: AtomicU64,
    pub(crate) ipis: AtomicU64,
}

// SAFETY: the UnsafeCell fields are only accessed by the owning core with
// IRQs masked; everything else is atomic or lock protected.
unsafe impl Sync for PerCpu {}

impl PerCpu {
    const fn new(id: usize) -> Self {
        Self {
            id,
            online: AtomicBool::new(false),
            current: UnsafeCell::new(None),
            idle: UnsafeCell::new(None),
            rq: IrqSpinLock::new(RunQueue::new()),
            nr_queued: AtomicUsize::new(0),
            need_resched: AtomicBool::new(false),
            is_idle: AtomicBool::new(true),
            current_prio: AtomicU8::new(0),
            current_tid: AtomicU64::new(0),
            next_tick: AtomicU64::new(0),
            ttbr0: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
            idle_ticks: AtomicU64::new(0),
            switches: AtomicU64::new(0),
            preemptions: AtomicU64::new(0),
            steals: AtomicU64::new(0),
            ipis: AtomicU64::new(0),
        }
    }
}

/// Per-core state of every core.
pub static PERCPU: [PerCpu; MAX_CPUS] = [PerCpu::new(0), PerCpu::new(1), PerCpu::new(2), PerCpu::new(3)];

/// Point TPIDR_EL1 at this core's [`PerCpu`]. Called once per core at boot.
pub(crate) fn init_this_cpu(cpu: usize) {
    let p = &PERCPU[cpu] as *const PerCpu as u64;
    // SAFETY: TPIDR_EL1 is reserved for this pointer.
    unsafe { core::arch::asm!("msr tpidr_el1, {}", in(reg) p, options(nostack, preserves_flags)) };
}

/// The calling core's [`PerCpu`].
#[inline(always)]
pub fn this_cpu() -> &'static PerCpu {
    let p: u64;
    // SAFETY: TPIDR_EL1 is set to a `&'static PerCpu` at boot.
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) p, options(nomem, nostack, preserves_flags));
        &*(p as *const PerCpu)
    }
}

/// Top of the IRQ stack of the core whose `PerCpu` is `pcpu`
/// (called from the IRQ vector).
pub extern "C" fn irq_stack_top(pcpu: &PerCpu) -> usize {
    // SAFETY: only the address is taken; each core uses its own slot.
    unsafe { core::ptr::addr_of!(IRQ_STACKS.0[pcpu.id]) as usize + IRQ_STACK_SIZE }
}

/// Number of cores that have come online.
pub fn online_cpus() -> usize {
    PERCPU.iter().filter(|c| c.online.load(Ordering::Acquire)).count()
}
