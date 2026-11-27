//! Kernel bring-up: boot entry points, configuration, interrupt dispatch
//! and statistics.

use crate::arch::{self, boot};
use crate::platform::{self, Irq, MAX_CPUS};
use crate::sched::percpu::{self, this_cpu, PERCPU};
use crate::sched::{self, SchedPolicy};
use crate::thread::Builder;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};

/// Kernel configuration passed to [`start`].
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Scheduling policy within a priority band.
    pub policy: SchedPolicy,
    /// Timer interrupt frequency per core.
    pub tick_hz: u32,
    /// Round-robin time slice, in ticks.
    pub time_slice_ticks: u32,
    /// Number of cores to bring up (1-4).
    pub cpus: usize,
}

impl Default for Config {
    /// Round-robin, 1 kHz tick, 10 ms slices, 4 cores.
    fn default() -> Self {
        Self { policy: SchedPolicy::RoundRobin, tick_hz: 1000, time_slice_ticks: 10, cpus: MAX_CPUS }
    }
}

static BOOT_EL: AtomicU8 = AtomicU8::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static PANICKED: AtomicBool = AtomicBool::new(false);
static CONFIGURED_CPUS: AtomicUsize = AtomicUsize::new(1);
static STRAY_IRQS: AtomicU64 = AtomicU64::new(0);

/// Interrupts from sources the kernel never enabled (each one is disabled
/// by the board code when it first fires). Should stay 0.
pub fn stray_irqs() -> u64 {
    STRAY_IRQS.load(Ordering::Relaxed)
}

/// Primary-core Rust entry, called from `_start` with the exception level
/// the image was entered at.
pub(crate) extern "C" fn boot_primary(boot_el: u64) -> ! {
    BOOT_EL.store(boot_el as u8, Ordering::Relaxed);
    percpu::init_this_cpu(0);
    platform::early_init(boot_el as u8);
    crate::mem::heap::init();
    crate::mem::frame::init();
    platform::irq_init_global();
    platform::irq_init_cpu(0);
    sched::init_idle_thread(&PERCPU[0]);
    PERCPU[0].online.store(true, Ordering::Release);

    extern "Rust" {
        fn kernel_main() -> !;
    }
    // SAFETY: provided by the application (see the crate docs).
    unsafe { kernel_main() }
}

/// Secondary-core Rust entry, called from `__pt_secondary_entry`.
pub(crate) extern "C" fn boot_secondary(cpu: u64) -> ! {
    let cpu = cpu as usize;
    percpu::init_this_cpu(cpu);
    platform::irq_init_cpu(cpu);
    sched::init_idle_thread(&PERCPU[cpu]);
    PERCPU[cpu].online.store(true, Ordering::Release);
    sched::start_tick();
    sched::idle_loop()
}

/// Exception level the kernel image was entered at (3, 2 or 1).
pub fn boot_el() -> u8 {
    BOOT_EL.load(Ordering::Relaxed)
}

/// Start the scheduler: bring up the secondary cores, spawn `main` as the
/// first thread and turn the boot core into an idle core. Never returns.
///
/// Call it exactly once, from `kernel_main` on the boot core.
pub fn start(config: Config, main: fn()) -> ! {
    assert!(!STARTED.swap(true, Ordering::AcqRel), "kernel already started");
    let cpus = config.cpus.clamp(1, MAX_CPUS);
    let tick_period = (arch::timer::frequency() / config.tick_hz.max(1) as u64).max(1);
    sched::configure(config.policy, config.time_slice_ticks, tick_period);
    CONFIGURED_CPUS.store(cpus, Ordering::Relaxed);

    for (cpu, pc) in PERCPU.iter().enumerate().take(cpus).skip(1) {
        if !platform::start_cpu(cpu, boot::secondary_entry_addr()) {
            crate::println!("[kernel] cpu{}: start request failed", cpu);
            continue;
        }
        let deadline = arch::timer::counter() + arch::timer::frequency() / 2;
        while !pc.online.load(Ordering::Acquire) && arch::timer::counter() < deadline {
            core::hint::spin_loop();
        }
        if !pc.online.load(Ordering::Acquire) {
            crate::println!("[kernel] cpu{}: did not come online", cpu);
        }
    }

    // Spawned only now, so `main` sees every core that came up.
    Builder::new().name("main").spawn(main).expect("failed to spawn main thread");
    sched::start_tick();
    sched::idle_loop()
}

/// IRQ dispatch, called from the IRQ vector on the per-core IRQ stack.
pub(crate) extern "C" fn irq_dispatch() {
    if PANICKED.load(Ordering::Relaxed) {
        arch::halt();
    }
    platform::dispatch_irqs(|irq| match irq {
        Irq::Timer => sched::on_tick(),
        Irq::Reschedule => {
            let cpu = this_cpu();
            cpu.ipis.fetch_add(1, Ordering::Relaxed);
            cpu.need_resched.store(true, Ordering::Relaxed);
        }
        Irq::Uart => crate::console::uart_irq(),
        Irq::Other(_) => {
            STRAY_IRQS.fetch_add(1, Ordering::Relaxed);
        }
    });
    if PANICKED.load(Ordering::Relaxed) {
        arch::halt();
    }
}

/// Called from the IRQ vector after [`irq_dispatch`], on the interrupted
/// thread's stack; may switch threads.
pub(crate) extern "C" fn irq_exit() {
    sched::irq_exit();
}

/// Stop the other cores after a panic.
pub(crate) fn stop_other_cpus() {
    PANICKED.store(true, Ordering::SeqCst);
    let me = arch::cpu_id();
    for c in PERCPU.iter() {
        if c.id != me && c.online.load(Ordering::Acquire) {
            platform::send_resched_ipi(c.id);
        }
    }
}

/// Counters for one core.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuStats {
    /// Whether the core is online.
    pub online: bool,
    /// Timer ticks handled.
    pub ticks: u64,
    /// Ticks during which the core was idle.
    pub idle_ticks: u64,
    /// Thread switches.
    pub context_switches: u64,
    /// Switches forced by an interrupt (time slice or higher priority).
    pub preemptions: u64,
    /// Threads taken from other cores' run queues.
    pub steals: u64,
    /// Reschedule IPIs received.
    pub ipis: u64,
    /// Threads waiting in this core's run queue.
    pub queued: usize,
}

/// Kernel-wide counters.
#[derive(Debug, Clone, Copy)]
pub struct KernelStats {
    /// Per-core counters.
    pub cpus: [CpuStats; MAX_CPUS],
    /// Threads spawned since boot.
    pub threads_spawned: u64,
    /// Threads that exited and were reclaimed.
    pub threads_reaped: u64,
    /// Threads alive now (excluding idle threads).
    pub threads_live: usize,
    /// Heap usage.
    pub heap: crate::mem::HeapStats,
    /// Stack pool usage.
    pub stacks: crate::mem::StackPoolStats,
}

impl KernelStats {
    /// Number of online cores.
    pub fn cpus_online(&self) -> usize {
        self.cpus.iter().filter(|c| c.online).count()
    }

    /// Sum of context switches over all cores.
    pub fn context_switches(&self) -> u64 {
        self.cpus.iter().map(|c| c.context_switches).sum()
    }

    /// Sum of preemptions over all cores.
    pub fn preemptions(&self) -> u64 {
        self.cpus.iter().map(|c| c.preemptions).sum()
    }
}

/// Snapshot of the kernel counters.
pub fn stats() -> KernelStats {
    let mut cpus = [CpuStats::default(); MAX_CPUS];
    for (s, c) in cpus.iter_mut().zip(PERCPU.iter()) {
        *s = CpuStats {
            online: c.online.load(Ordering::Acquire),
            ticks: c.ticks.load(Ordering::Relaxed),
            idle_ticks: c.idle_ticks.load(Ordering::Relaxed),
            context_switches: c.switches.load(Ordering::Relaxed),
            preemptions: c.preemptions.load(Ordering::Relaxed),
            steals: c.steals.load(Ordering::Relaxed),
            ipis: c.ipis.load(Ordering::Relaxed),
            queued: c.nr_queued.load(Ordering::Relaxed),
        };
    }
    KernelStats {
        cpus,
        threads_spawned: sched::SPAWNED.load(Ordering::Relaxed),
        threads_reaped: sched::REAPED.load(Ordering::Relaxed),
        threads_live: sched::LIVE_THREADS.load(Ordering::Relaxed),
        heap: crate::mem::heap_stats(),
        stacks: sched::STACKS.stats(),
    }
}
