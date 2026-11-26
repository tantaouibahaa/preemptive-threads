//! Processes: the process table, `spawn`, `wait`, `kill` and the run loop.
//!
//! A process is one kernel thread plus an [`Mm`]. The thread loads the
//! process's TTBR0, then loops: [`run_user`] until the user code traps,
//! handle the system call or fault, repeat. Interrupts at EL0 preempt it
//! like any other thread.
//!
//! ```text
//!   spawn ──► Running ──► exit(code) / fault / kill ──► Zombie(status) ──► wait() reaps it
//! ```
//!
//! On exit the thread switches back to the kernel tables, drops the `Mm`
//! (every frame, the page tables and the ASID go back right away), marks the
//! process a zombie with its status and wakes waiters. `wait` removes the
//! zombie from the table. Children of an exiting process are handed to the
//! kernel (pid 0) and reaped automatically when they exit.
//!
//! Exit status: the value passed to `exit`, or `-signal` for a process
//! killed by a fault (`-11` segfault, `-4` illegal instruction, `-7`
//! alignment, `-5` breakpoint, `-8` FP) or by `kill` (`-9`).

use crate::errno::Errno;
use crate::mm::Mm;
use crate::syscall::{self, Action};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use preemptive_threads::arch::mmu::USER_ASIDS;
use preemptive_threads::arch::user::{classify, run_user, FaultKind, UserExit, UserTrap};
use preemptive_threads::arch::vectors::TrapFrame;
use preemptive_threads::sync::{IrqSpinLock, Mutex, WaitQueue};
use preemptive_threads::{println, sched, thread, Builder, Thread};

/// Process id. 0 is the kernel.
pub type Pid = u32;

/// Status of a process killed with [`kill`].
pub const STATUS_KILLED: i32 = -9;
/// Most processes alive at once (one ASID each).
pub const MAX_PROCS: usize = USER_ASIDS;
/// Kernel stack of a process thread.
const KERNEL_STACK: usize = 32 * 1024;

/// Where a process's standard output (fds 1 and 2) goes.
#[derive(Clone, Default)]
pub enum Output {
    /// The serial console.
    #[default]
    Console,
    /// The console, and also a buffer the kernel can inspect (tests).
    Capture(Arc<IrqSpinLock<Vec<u8>>>),
}

impl Output {
    /// A fresh capture buffer.
    pub fn capture() -> Self {
        Output::Capture(Arc::new(IrqSpinLock::new(Vec::new())))
    }

    /// Bytes captured so far (empty for the console).
    pub fn captured(&self) -> Vec<u8> {
        match self {
            Output::Console => Vec::new(),
            Output::Capture(b) => b.lock().clone(),
        }
    }

    pub(crate) fn write(&self, bytes: &[u8]) {
        if let Output::Capture(b) = self {
            b.lock().extend_from_slice(bytes);
        }
        preemptive_threads::console::write_bytes(bytes);
    }
}

/// A fault that killed a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    /// What happened.
    pub kind: FaultKind,
    /// Faulting address.
    pub addr: u64,
    /// User PC.
    pub pc: u64,
}

/// A process.
pub struct Process {
    pid: Pid,
    parent: AtomicU32,
    name: String,
    pub(crate) mm: Mutex<Option<Mm>>,
    status: IrqSpinLock<Option<i32>>,
    killed: AtomicBool,
    detached: AtomicBool,
    thread: IrqSpinLock<Option<Thread>>,
    fault: IrqSpinLock<Option<Fault>>,
    pub(crate) output: Output,
}

impl Process {
    /// Process id.
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Parent pid (0 = the kernel).
    pub fn parent(&self) -> Pid {
        self.parent.load(Ordering::Acquire)
    }

    /// Program name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exit status once the process has exited.
    pub fn status(&self) -> Option<i32> {
        *self.status.lock()
    }

    /// Whether `kill` was called on it.
    pub fn killed(&self) -> bool {
        self.killed.load(Ordering::Acquire)
    }

    pub(crate) fn killed_flag(&self) -> &AtomicBool {
        &self.killed
    }

    /// The fault that killed it, if any.
    pub fn fault(&self) -> Option<Fault> {
        *self.fault.lock()
    }

    /// Its standard output.
    pub fn output(&self) -> &Output {
        &self.output
    }

    /// Bitmask of the cores its thread has run on.
    pub fn cpus_ran(&self) -> u8 {
        self.thread.lock().as_ref().map_or(0, |t| t.cpus_ran())
    }

    /// Run `f` on its memory map (`None` once it has exited).
    pub fn with_mm<R>(&self, f: impl FnOnce(&Mm) -> R) -> Option<R> {
        self.mm.lock().as_ref().map(f)
    }
}

static TABLE: IrqSpinLock<BTreeMap<Pid, Arc<Process>>> = IrqSpinLock::new(BTreeMap::new());
static NEXT_PID: AtomicU32 = AtomicU32::new(1);
/// Woken whenever a process exits or is killed.
static EXITED: WaitQueue = WaitQueue::new();

/// Number of processes in the table.
pub fn count() -> usize {
    TABLE.lock().len()
}

/// Look up a live or zombie process.
pub fn find(pid: Pid) -> Option<Arc<Process>> {
    TABLE.lock().get(&pid).cloned()
}

/// All processes in the table, by pid.
pub fn list() -> Vec<Arc<Process>> {
    TABLE.lock().values().cloned().collect()
}

/// The program to run.
#[derive(Clone, Copy)]
pub enum Image<'a> {
    /// An embedded program, by name (see [`crate::programs`]).
    Named(&'a str),
    /// An ELF image.
    Elf(&'a [u8]),
}

/// Options for [`spawn`].
#[derive(Clone, Default)]
pub struct SpawnOptions {
    /// Parent pid (0 = the kernel).
    pub parent: Pid,
    /// Pin the process to a core.
    pub cpu: Option<usize>,
    /// Standard output.
    pub output: Output,
}

/// Create a process running `image` with arguments `argv` and make it
/// runnable. Returns its pid.
pub fn spawn(image: Image<'_>, argv: &[&[u8]], opts: SpawnOptions) -> Result<Pid, Errno> {
    let (name, elf) = match image {
        Image::Named(n) => (String::from(n), crate::programs::find(n).ok_or(Errno::ENOENT)?),
        Image::Elf(e) => (String::from("<elf>"), e),
    };
    if count() >= MAX_PROCS {
        return Err(Errno::EAGAIN);
    }
    let mut mm = Mm::new()?;
    let entry = crate::elf::load(&mut mm, elf)?;
    let (sp, argv_ptr) = mm.setup_stack(argv)?;
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let p = Arc::new(Process {
        pid,
        parent: AtomicU32::new(opts.parent),
        name,
        mm: Mutex::new(Some(mm)),
        status: IrqSpinLock::new(None),
        killed: AtomicBool::new(false),
        detached: AtomicBool::new(false),
        thread: IrqSpinLock::new(None),
        fault: IrqSpinLock::new(None),
        output: opts.output,
    });
    TABLE.lock().insert(pid, p.clone());
    let mut frame = TrapFrame::zeroed();
    frame.elr = entry;
    frame.sp_el0 = sp;
    frame.x[0] = argv.len() as u64;
    frame.x[1] = argv_ptr;
    let theirs = p.clone();
    let mut b = Builder::new().name(format!("pid{}", pid)).stack_size(KERNEL_STACK);
    if let Some(cpu) = opts.cpu {
        b = b.cpu(cpu);
    }
    match b.spawn(move || process_main(theirs, frame)) {
        Ok(h) => {
            *p.thread.lock() = Some(h.thread().clone());
            Ok(pid)
        }
        Err(_) => {
            TABLE.lock().remove(&pid);
            Err(Errno::ENOMEM)
        }
    }
}

/// Body of a process's kernel thread.
fn process_main(p: Arc<Process>, mut frame: TrapFrame) {
    let ttbr0 = p.mm.lock().as_ref().map(|m| m.ttbr0()).expect("process without memory");
    // SAFETY: the address space copies the kernel's level-1 entries, and it
    // stays alive until `exit_process` has switched back to the kernel
    // tables (nobody else drops it).
    unsafe { sched::set_address_space(ttbr0) };
    let status = loop {
        if p.killed() {
            break STATUS_KILLED;
        }
        let (esr, far) = match run_user(&mut frame) {
            UserExit::Interrupted => continue,
            UserExit::Exception { esr, far } => (esr, far),
        };
        match classify(esr, far, frame.elr) {
            UserTrap::Syscall(0) => match syscall::dispatch(&p, &frame) {
                Action::Return(v) => frame.x[0] = v as u64,
                Action::Exit(code) => break code,
            },
            UserTrap::Syscall(_) => frame.x[0] = -(Errno::ENOSYS.0 as i64) as u64,
            UserTrap::WaitForInterrupt => {
                frame.elr += 4;
                thread::yield_now();
            }
            UserTrap::Fault { kind, addr } => {
                println!("[pid {}] {} at {:#x} (pc {:#x})", p.pid, kind, addr, frame.elr);
                *p.fault.lock() = Some(Fault { kind, addr, pc: frame.elr });
                break -kind.signal();
            }
        }
    };
    exit_process(&p, status);
}

fn exit_process(p: &Arc<Process>, status: i32) {
    // SAFETY: back to the kernel tables before our own are freed.
    unsafe { sched::set_address_space(0) };
    // Every frame, the page tables and the ASID go back now.
    drop(p.mm.lock().take());
    let mut reaped = Vec::new();
    {
        let mut t = TABLE.lock();
        for c in t.values() {
            if c.parent() == p.pid {
                c.parent.store(0, Ordering::Release);
                c.detached.store(true, Ordering::Release);
            }
        }
        *p.status.lock() = Some(status);
        // Detached zombies have nobody left to wait for them.
        let gone: Vec<Pid> =
            t.values().filter(|c| c.detached.load(Ordering::Acquire) && c.status().is_some()).map(|c| c.pid).collect();
        for pid in gone {
            reaped.extend(t.remove(&pid));
        }
    }
    drop(reaped);
    EXITED.wake_all();
}

/// Wait for a child of `parent` to exit (`target` = a pid, or any child)
/// and reap it. Returns `(pid, status)`. `interrupt`, if given, aborts the
/// wait with `EINTR` when it becomes true.
pub fn wait_child(parent: Pid, target: Option<Pid>, interrupt: Option<&AtomicBool>) -> Result<(Pid, i32), Errno> {
    let mut out = Err(Errno::ECHILD);
    let mut reaped = None;
    EXITED.wait_until(|| {
        if interrupt.is_some_and(|k| k.load(Ordering::Acquire)) {
            out = Err(Errno::EINTR);
            return true;
        }
        let mut t = TABLE.lock();
        let mut any = false;
        let mut found = None;
        for (pid, c) in t.iter() {
            if c.parent() == parent && target.is_none_or(|x| x == *pid) {
                any = true;
                if let Some(s) = c.status() {
                    found = Some((*pid, s));
                    break;
                }
            }
        }
        match found {
            Some((pid, s)) => {
                reaped = t.remove(&pid);
                out = Ok((pid, s));
                true
            }
            None => !any,
        }
    });
    drop(reaped);
    out
}

/// Wait for kernel-spawned process `pid` (or any, with `None`) and reap it.
pub fn wait(pid: Option<Pid>) -> Result<(Pid, i32), Errno> {
    wait_child(0, pid, None)
}

/// Kill a process: it stops at its next exit from user mode (right away
/// if it is running user code; a sleeping process notices within 10 ms).
pub fn kill(pid: Pid) -> Result<(), Errno> {
    let p = find(pid).ok_or(Errno::ESRCH)?;
    if p.status().is_none() {
        p.killed.store(true, Ordering::Release);
        if let Some(t) = p.thread.lock().clone() {
            t.interrupt_user();
        }
        EXITED.wake_all();
    }
    Ok(())
}
