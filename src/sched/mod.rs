//! The scheduler.
//!
//! # Model
//!
//! Every core has a run queue with four priority bands ([`runqueue::band`]),
//! an idle thread (its boot context) and a `current` thread. Threads move
//! between these states:
//!
//! ```text
//!            spawn / wake                pick
//!   Blocked ─────────────► Ready ───────────────► Running
//!      ▲                    ▲  ◄───────────────────  │ │
//!      │                    │   preempt / yield      │ │
//!      └────────────────────┼────────────────────────┘ │ block (wait queue, sleep, join)
//!                           │                          │
//!                                                      ▼ exit
//!                                                     Dead ──► reaped (stack back to the pool)
//! ```
//!
//! [`schedule`] is the only place a switch happens. It runs with IRQs
//! masked and no locks held, re-queues the previous thread if it is still
//! `Running`, picks the next one (local queue, then work stealing, then the
//! idle thread) and calls [`crate::arch::context::switch_context`].
//!
//! A thread that has just been switched away from keeps `on_cpu = true`
//! until the next thread on that core has finished the switch
//! ([`finish_switch`]). Pick and steal skip threads that are still on a
//! core, so a thread is never resumed on two cores at once and cores never
//! spin waiting on each other.
//!
//! # Policies
//!
//! - [`SchedPolicy::RoundRobin`]: threads in the same band share the core in
//!   time slices of `Config::time_slice_ticks` timer ticks.
//! - [`SchedPolicy::Fifo`]: a thread runs until it blocks, yields or a
//!   higher band becomes ready.
//!
//! In both policies a higher band always preempts a lower one (on the next
//! IRQ exit, triggered immediately with an IPI when the target is another
//! core).

pub mod percpu;
pub mod runqueue;
pub mod sleep;

use crate::arch::{self, context};
use crate::mem::{Stack, StackPool};
use crate::platform::{self, MAX_CPUS};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use percpu::{this_cpu, PerCpu, PERCPU};
use runqueue::band;

/// Common priority values (0-255, higher runs first).
pub mod priority {
    /// Only runs when nothing else is ready.
    pub const IDLE: u8 = 0;
    /// Background work.
    pub const LOW: u8 = 32;
    /// Default for new threads.
    pub const NORMAL: u8 = 128;
    /// Latency-sensitive work.
    pub const HIGH: u8 = 224;
}

/// Scheduling policy within a priority band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedPolicy {
    /// Time-sliced round-robin.
    RoundRobin,
    /// First come, first served: no time slicing.
    Fifo,
}

/// Unique thread identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub(crate) u64);

impl ThreadId {
    /// Numeric value.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl core::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Thread lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThreadState {
    /// In a run queue.
    Ready = 0,
    /// Executing on a core.
    Running = 1,
    /// Waiting on a wait queue, the sleep queue or a join.
    Blocked = 2,
    /// Finished; resources are reclaimed after the switch away.
    Dead = 3,
}

const READY: u8 = ThreadState::Ready as u8;
const RUNNING: u8 = ThreadState::Running as u8;
const BLOCKED: u8 = ThreadState::Blocked as u8;
const DEAD: u8 = ThreadState::Dead as u8;

type StartFn = Box<dyn FnOnce() + Send + 'static>;

/// Thread control block.
pub struct Tcb {
    pub(crate) id: ThreadId,
    pub(crate) name: Option<String>,
    base_prio: AtomicU8,
    prio: AtomicU8,
    state: AtomicU8,
    pub(crate) on_cpu: AtomicBool,
    saved_sp: UnsafeCell<usize>,
    stack: UnsafeCell<Option<Stack>>,
    affinity: Option<usize>,
    pub(crate) last_cpu: AtomicUsize,
    pub(crate) rq_cpu: AtomicUsize,
    slice_left: AtomicU32,
    pub(crate) cpus_ran: AtomicU8,
    pub(crate) is_idle: bool,
    start: UnsafeCell<Option<StartFn>>,
}

// SAFETY: the UnsafeCell fields are only touched by the scheduler under the
// on_cpu / state protocol described in the module docs.
unsafe impl Sync for Tcb {}
// SAFETY: as above.
unsafe impl Send for Tcb {}

impl Tcb {
    /// Effective priority (including priority inheritance boosts).
    pub fn priority(&self) -> u8 {
        self.prio.load(Ordering::Relaxed)
    }

    /// Priority the thread was created with (or last set to).
    pub fn base_priority(&self) -> u8 {
        self.base_prio.load(Ordering::Relaxed)
    }

    /// Current lifecycle state.
    pub fn state(&self) -> ThreadState {
        match self.state.load(Ordering::Acquire) {
            READY => ThreadState::Ready,
            RUNNING => ThreadState::Running,
            BLOCKED => ThreadState::Blocked,
            _ => ThreadState::Dead,
        }
    }

    fn set_state(&self, s: u8) {
        self.state.store(s, Ordering::Release);
    }

    fn allowed_on(&self, cpu: usize) -> bool {
        self.affinity.is_none_or(|c| c == cpu)
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

pub(crate) static STACKS: StackPool = StackPool::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) static LIVE_THREADS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SPAWNED: AtomicU64 = AtomicU64::new(0);
pub(crate) static REAPED: AtomicU64 = AtomicU64::new(0);
static POLICY_FIFO: AtomicBool = AtomicBool::new(false);
static TIME_SLICE: AtomicU32 = AtomicU32::new(10);
static TICK_TICKS: AtomicU64 = AtomicU64::new(0);

/// Configure policy, slice length and tick period (counter ticks).
pub(crate) fn configure(policy: SchedPolicy, slice_ticks: u32, tick_period: u64) {
    POLICY_FIFO.store(policy == SchedPolicy::Fifo, Ordering::Relaxed);
    TIME_SLICE.store(slice_ticks.max(1), Ordering::Relaxed);
    TICK_TICKS.store(tick_period, Ordering::Relaxed);
}

/// Change the scheduling policy at run time (takes effect at the next tick).
pub fn set_policy(policy: SchedPolicy) {
    POLICY_FIFO.store(policy == SchedPolicy::Fifo, Ordering::Relaxed);
}

/// The current scheduling policy.
pub fn policy() -> SchedPolicy {
    if POLICY_FIFO.load(Ordering::Relaxed) {
        SchedPolicy::Fifo
    } else {
        SchedPolicy::RoundRobin
    }
}

// ---------------------------------------------------------------------------
// Thread creation
// ---------------------------------------------------------------------------

/// Parameters for [`create_thread`].
pub(crate) struct NewThread {
    pub name: Option<String>,
    pub priority: u8,
    pub stack_size: usize,
    pub cpu: Option<usize>,
}

/// Allocate a thread and make it ready. `f` runs on the new thread.
pub(crate) fn create_thread(p: NewThread, f: StartFn) -> Result<Arc<Tcb>, crate::errors::SpawnError> {
    use crate::errors::SpawnError;
    if let Some(c) = p.cpu {
        if c >= MAX_CPUS {
            return Err(SpawnError::InvalidCpu);
        }
    }
    let stack = STACKS.allocate(p.stack_size).ok_or(SpawnError::OutOfMemory)?;
    let tcb = Arc::new(Tcb {
        id: ThreadId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
        name: p.name,
        base_prio: AtomicU8::new(p.priority),
        prio: AtomicU8::new(p.priority),
        state: AtomicU8::new(READY),
        on_cpu: AtomicBool::new(false),
        saved_sp: UnsafeCell::new(0),
        stack: UnsafeCell::new(None),
        affinity: p.cpu,
        last_cpu: AtomicUsize::new(p.cpu.unwrap_or(0)),
        rq_cpu: AtomicUsize::new(0),
        slice_left: AtomicU32::new(0),
        cpus_ran: AtomicU8::new(0),
        is_idle: false,
        start: UnsafeCell::new(Some(f)),
    });
    // SAFETY: the stack is fresh and exclusively ours; the TCB is not yet
    // visible to any other core.
    unsafe {
        *tcb.saved_sp.get() = context::init_stack(stack.top(), Arc::as_ptr(&tcb) as usize);
        *tcb.stack.get() = Some(stack);
    }
    LIVE_THREADS.fetch_add(1, Ordering::Relaxed);
    SPAWNED.fetch_add(1, Ordering::Relaxed);
    make_ready(tcb.clone());
    Ok(tcb)
}

/// Turn the calling core's boot context into its idle thread.
pub(crate) fn init_idle_thread(cpu: &PerCpu) {
    let idle = Arc::new(Tcb {
        id: ThreadId(u64::MAX - cpu.id as u64),
        name: None,
        base_prio: AtomicU8::new(0),
        prio: AtomicU8::new(0),
        state: AtomicU8::new(RUNNING),
        on_cpu: AtomicBool::new(true),
        saved_sp: UnsafeCell::new(0),
        stack: UnsafeCell::new(None),
        affinity: Some(cpu.id),
        last_cpu: AtomicUsize::new(cpu.id),
        rq_cpu: AtomicUsize::new(cpu.id),
        slice_left: AtomicU32::new(0),
        cpus_ran: AtomicU8::new(1 << cpu.id),
        is_idle: true,
        start: UnsafeCell::new(None),
    });
    // SAFETY: called once per core during boot, IRQs masked.
    unsafe {
        *cpu.idle.get() = Some(idle.clone());
        *cpu.current.get() = Some(idle);
    }
}

/// First Rust code on a new thread (called from `__pt_thread_entry`).
pub(crate) extern "C" fn thread_start(prev: usize, tcb: *const Tcb) -> ! {
    finish_switch(prev);
    // SAFETY: `tcb` is the running thread (kept alive by PerCpu::current);
    // `start` is only taken here, once.
    let f = unsafe { (*(*tcb).start.get()).take() };
    arch::irq_enable();
    if let Some(f) = f {
        f();
    }
    exit_current()
}

// ---------------------------------------------------------------------------
// Current thread
// ---------------------------------------------------------------------------

/// The thread running on the calling core.
pub fn current() -> Arc<Tcb> {
    let irq = arch::irq_save();
    // SAFETY: own core, IRQs masked.
    let t = unsafe { (*this_cpu().current.get()).as_ref().expect("no current thread").clone() };
    arch::irq_restore(irq);
    t
}

/// Run `f` on the current thread's TCB without touching its refcount.
pub(crate) fn with_current<R>(f: impl FnOnce(&Arc<Tcb>) -> R) -> R {
    let irq = arch::irq_save();
    // SAFETY: own core, IRQs masked.
    let r = f(unsafe { (*this_cpu().current.get()).as_ref().expect("no current thread") });
    arch::irq_restore(irq);
    r
}

// ---------------------------------------------------------------------------
// Ready / wake / priority
// ---------------------------------------------------------------------------

/// Pick a core for a thread that just became ready.
fn select_cpu(t: &Tcb) -> usize {
    if let Some(c) = t.affinity {
        return c;
    }
    let last = t.last_cpu.load(Ordering::Relaxed);
    let online = |c: &&PerCpu| c.online.load(Ordering::Acquire);
    if PERCPU[last].online.load(Ordering::Acquire)
        && PERCPU[last].is_idle.load(Ordering::Relaxed)
        && PERCPU[last].nr_queued.load(Ordering::Relaxed) == 0
    {
        return last;
    }
    if let Some(c) = PERCPU
        .iter()
        .filter(online)
        .find(|c| c.is_idle.load(Ordering::Relaxed) && c.nr_queued.load(Ordering::Relaxed) == 0)
    {
        return c.id;
    }
    PERCPU
        .iter()
        .filter(online)
        .min_by_key(|c| (c.nr_queued.load(Ordering::Relaxed), c.id != last))
        .map(|c| c.id)
        .unwrap_or(0)
}

/// Ask `cpu` to reschedule if a thread of priority `prio` should run
/// there before what it is running now.
fn maybe_preempt(cpu: usize, prio: u8) {
    let c = &PERCPU[cpu];
    if c.is_idle.load(Ordering::Relaxed) || band(prio) > band(c.current_prio.load(Ordering::Relaxed)) {
        kick(cpu);
    }
}

/// Make `cpu` run its scheduler soon.
pub(crate) fn kick(cpu: usize) {
    let c = &PERCPU[cpu];
    c.need_resched.store(true, Ordering::Release);
    let irq = arch::irq_save();
    let me = this_cpu().id;
    arch::irq_restore(irq);
    if cpu != me && c.online.load(Ordering::Acquire) {
        platform::send_resched_ipi(cpu);
    }
}

/// Queue a `Ready` thread on a suitable core.
fn make_ready(t: Arc<Tcb>) {
    let cpu = select_cpu(&t);
    let prio = t.priority();
    {
        let mut rq = PERCPU[cpu].rq.lock();
        t.rq_cpu.store(cpu, Ordering::Relaxed);
        rq.push(t);
        PERCPU[cpu].nr_queued.fetch_add(1, Ordering::Relaxed);
    }
    maybe_preempt(cpu, prio);
}

/// Wake a blocked thread. Returns `false` if it was not blocked.
pub(crate) fn wake(t: Arc<Tcb>) -> bool {
    if t.state.compare_exchange(BLOCKED, READY, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        make_ready(t);
        true
    } else {
        false
    }
}

/// Change a thread's effective priority, moving it between bands if it is
/// queued (used by priority inheritance).
pub(crate) fn set_effective_priority(t: &Arc<Tcb>, prio: u8) {
    let old = t.prio.swap(prio, Ordering::Relaxed);
    if old == prio {
        return;
    }
    match t.state.load(Ordering::Acquire) {
        READY => {
            let cpu = t.rq_cpu.load(Ordering::Relaxed);
            let moved = {
                let mut rq = PERCPU[cpu].rq.lock();
                if rq.remove(t) {
                    rq.push(t.clone());
                    true
                } else {
                    false
                }
            };
            if moved {
                maybe_preempt(cpu, prio);
            }
        }
        RUNNING => {
            let cpu = t.last_cpu.load(Ordering::Relaxed);
            PERCPU[cpu].current_prio.store(prio, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Set a thread's base priority (and effective priority unless boosted).
pub(crate) fn set_base_priority(t: &Arc<Tcb>, prio: u8) {
    let old_base = t.base_prio.swap(prio, Ordering::Relaxed);
    if t.priority() == old_base || prio > t.priority() {
        set_effective_priority(t, prio);
    }
}

// ---------------------------------------------------------------------------
// Switching
// ---------------------------------------------------------------------------

fn pick_next(cpu: &PerCpu, prev: &Arc<Tcb>) -> Option<Arc<Tcb>> {
    // A thread that is still on another core cannot be resumed yet.
    let runnable = |t: &Arc<Tcb>| !t.on_cpu.load(Ordering::Acquire) || Arc::ptr_eq(t, prev);
    {
        let mut rq = cpu.rq.lock_irqs_off();
        if let Some(t) = rq.pop_where(runnable) {
            cpu.nr_queued.fetch_sub(1, Ordering::Relaxed);
            return Some(t);
        }
    }
    try_steal_work(cpu)
}

/// Take a ready thread from another core's queue.
fn try_steal_work(cpu: &PerCpu) -> Option<Arc<Tcb>> {
    for i in 1..MAX_CPUS {
        let victim = &PERCPU[(cpu.id + i) % MAX_CPUS];
        if !victim.online.load(Ordering::Acquire) || victim.nr_queued.load(Ordering::Relaxed) == 0 {
            continue;
        }
        let mut rq = victim.rq.lock_irqs_off();
        if let Some(t) = rq.pop_where(|t| t.allowed_on(cpu.id) && !t.on_cpu.load(Ordering::Acquire)) {
            victim.nr_queued.fetch_sub(1, Ordering::Relaxed);
            cpu.steals.fetch_add(1, Ordering::Relaxed);
            return Some(t);
        }
    }
    None
}

/// Run the scheduler on the calling core.
///
/// Must be called with IRQs masked and no spinlocks held. Returns when the
/// calling thread is scheduled again (immediately if it is still the best
/// choice). A `Dead` caller never returns.
pub(crate) fn schedule() {
    schedule_impl(false);
}

fn schedule_impl(preempting: bool) {
    debug_assert!(!arch::irqs_enabled());
    let cpu = this_cpu();
    cpu.need_resched.store(false, Ordering::Relaxed);
    // SAFETY: own core, IRQs masked.
    let prev = unsafe { (*cpu.current.get()).take().expect("no current thread") };
    let prev_state = prev.state.load(Ordering::Acquire);

    if !prev.is_idle {
        // SAFETY: the stack of a thread that is on this core is not touched
        // by anyone else.
        if let Some(stack) = unsafe { &*prev.stack.get() } {
            if !stack.canary_ok() {
                panic!("stack overflow in thread {}", prev.id);
            }
        }
        if prev_state == RUNNING {
            prev.set_state(READY);
            let mut rq = cpu.rq.lock_irqs_off();
            prev.rq_cpu.store(cpu.id, Ordering::Relaxed);
            rq.push(prev.clone());
            cpu.nr_queued.fetch_add(1, Ordering::Relaxed);
        }
    }

    let next = match pick_next(cpu, &prev) {
        Some(t) => t,
        // SAFETY: own core, IRQs masked.
        None => unsafe { (*cpu.idle.get()).clone().expect("no idle thread") },
    };

    if Arc::ptr_eq(&next, &prev) {
        prev.set_state(RUNNING);
        // SAFETY: own core, IRQs masked.
        unsafe { *cpu.current.get() = Some(prev) };
        return;
    }

    next.on_cpu.store(true, Ordering::Relaxed);
    next.set_state(RUNNING);
    next.last_cpu.store(cpu.id, Ordering::Relaxed);
    next.cpus_ran.fetch_or(1 << cpu.id, Ordering::Relaxed);
    next.slice_left.store(TIME_SLICE.load(Ordering::Relaxed), Ordering::Relaxed);
    cpu.current_prio.store(next.priority(), Ordering::Relaxed);
    cpu.is_idle.store(next.is_idle, Ordering::Relaxed);
    cpu.switches.fetch_add(1, Ordering::Relaxed);
    if preempting && prev_state == RUNNING && !prev.is_idle {
        cpu.preemptions.fetch_add(1, Ordering::Relaxed);
    }

    // SAFETY: `next` is not on any core (checked by pick) so its saved SP
    // is stable; `prev`'s saved_sp slot stays alive because we hand a
    // strong reference to the next thread, which drops it in
    // finish_switch only after the switch completed.
    unsafe {
        let next_sp = *next.saved_sp.get();
        let prev_sp_slot = prev.saved_sp.get();
        *cpu.current.get() = Some(next);
        let handoff = Arc::into_raw(prev) as usize;
        let from = context::switch_context(prev_sp_slot, next_sp, handoff);
        finish_switch(from);
    }
}

/// Bookkeeping after a switch, run by the thread that was switched in.
/// `prev` is the raw `Arc<Tcb>` of the thread that was switched out.
fn finish_switch(prev: usize) {
    // SAFETY: produced by Arc::into_raw in schedule().
    let prev = unsafe { Arc::from_raw(prev as *const Tcb) };
    let state = prev.state.load(Ordering::Acquire);
    prev.on_cpu.store(false, Ordering::Release);
    match state {
        DEAD => reap(prev),
        READY => {
            // It may be queued on an idle core that skipped it while it was
            // still on this one.
            let cpu = prev.rq_cpu.load(Ordering::Relaxed);
            if cpu != this_cpu().id && PERCPU[cpu].is_idle.load(Ordering::Relaxed) {
                kick(cpu);
            }
        }
        _ => {}
    }
}

fn reap(t: Arc<Tcb>) {
    // SAFETY: the thread is dead and no longer on any core.
    if let Some(stack) = unsafe { (*t.stack.get()).take() } {
        STACKS.release(stack);
    }
    LIVE_THREADS.fetch_sub(1, Ordering::Relaxed);
    REAPED.fetch_add(1, Ordering::Relaxed);
}

/// Mark the current thread blocked. The caller must already have published
/// it somewhere a waker will find it, and must call [`schedule`] next.
/// IRQs must be masked.
pub(crate) fn mark_current_blocked() -> Arc<Tcb> {
    debug_assert!(!arch::irqs_enabled());
    // SAFETY: own core, IRQs masked.
    let t = unsafe { (*this_cpu().current.get()).as_ref().expect("no current thread").clone() };
    t.set_state(BLOCKED);
    t
}

/// Voluntarily give up the core.
pub fn yield_now() {
    let irq = arch::irq_save();
    schedule();
    arch::irq_restore(irq);
}

/// Reschedule now if a higher-priority thread became ready on this core.
pub(crate) fn preempt_point() {
    if arch::irqs_enabled() && this_cpu().need_resched.load(Ordering::Relaxed) {
        yield_now();
    }
}

/// Terminate the current thread.
pub(crate) fn exit_current() -> ! {
    arch::irq_disable();
    // SAFETY: own core, IRQs masked.
    let t = unsafe { (*this_cpu().current.get()).as_ref().expect("no current thread").clone() };
    assert!(!t.is_idle, "idle thread cannot exit");
    t.set_state(DEAD);
    drop(t);
    schedule();
    unreachable!("dead thread was rescheduled");
}

// ---------------------------------------------------------------------------
// Timer tick and idle loop
// ---------------------------------------------------------------------------

/// Program the first tick on the calling core.
pub(crate) fn start_tick() {
    let cpu = this_cpu();
    let next = arch::timer::counter() + TICK_TICKS.load(Ordering::Relaxed);
    cpu.next_tick.store(next, Ordering::Relaxed);
    arch::timer::set_deadline(next);
}

/// Timer interrupt (IRQ context).
pub(crate) fn on_tick() {
    let cpu = this_cpu();
    let period = TICK_TICKS.load(Ordering::Relaxed);
    let now = arch::timer::counter();
    let mut next = cpu.next_tick.load(Ordering::Relaxed) + period;
    if next <= now {
        next = now + period;
    }
    cpu.next_tick.store(next, Ordering::Relaxed);
    arch::timer::set_deadline(next);
    cpu.ticks.fetch_add(1, Ordering::Relaxed);

    sleep::wake_expired(now);

    // SAFETY: own core, IRQs masked (IRQ context).
    let cur = unsafe { (*cpu.current.get()).as_ref().expect("no current thread") };
    if cur.is_idle {
        cpu.idle_ticks.fetch_add(1, Ordering::Relaxed);
        // Look for work to steal.
        if PERCPU.iter().any(|c| c.nr_queued.load(Ordering::Relaxed) > 0) {
            cpu.need_resched.store(true, Ordering::Relaxed);
        }
    } else if !POLICY_FIFO.load(Ordering::Relaxed) {
        let left = cur.slice_left.load(Ordering::Relaxed);
        if left <= 1 {
            cur.slice_left.store(TIME_SLICE.load(Ordering::Relaxed), Ordering::Relaxed);
            let competing = cpu.nr_queued.load(Ordering::Relaxed) > 0
                || PERCPU.iter().any(|c| c.id != cpu.id && c.nr_queued.load(Ordering::Relaxed) > 1);
            if competing {
                cpu.need_resched.store(true, Ordering::Relaxed);
            }
        } else {
            cur.slice_left.store(left - 1, Ordering::Relaxed);
        }
    }
}

/// Called on IRQ exit with IRQs masked, on the interrupted thread's stack.
pub(crate) fn irq_exit() {
    if this_cpu().need_resched.load(Ordering::Relaxed) {
        schedule_impl(true);
    }
}

/// The idle loop each core's boot context ends up in.
pub(crate) fn idle_loop() -> ! {
    loop {
        arch::irq_disable();
        let cpu = this_cpu();
        if cpu.need_resched.load(Ordering::Relaxed) || cpu.nr_queued.load(Ordering::Relaxed) > 0 {
            schedule();
        }
        // WFI with IRQs masked still wakes on a pending interrupt, which is
        // then taken as soon as IRQs are unmasked: no lost wake-ups.
        arch::wfi();
        arch::irq_enable();
    }
}

/// Bitmask of cores a thread has run on.
pub(crate) fn cpus_ran(t: &Tcb) -> u8 {
    t.cpus_ran.load(Ordering::Relaxed)
}
