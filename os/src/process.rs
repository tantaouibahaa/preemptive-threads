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
//! alignment, `-5` breakpoint, `-8` FP), by `kill` (`-9`) or by Ctrl-C
//! (`-2`). See [`describe_status`].

use crate::console::STATUS_INTERRUPTED;
use crate::errno::Errno;
use crate::mm::Mm;
use crate::syscall::{self, Action};
use crate::vfs::{self, FdTable, File, Inode};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{fence, AtomicBool, AtomicI32, AtomicU32, Ordering};
use preemptive_threads::arch::mmu::USER_ASIDS;
use preemptive_threads::arch::user::{classify, run_user, FaultKind, UserExit, UserTrap};
use preemptive_threads::arch::vectors::TrapFrame;
use preemptive_threads::sync::{IrqSpinLock, Mutex, WaitQueue};
use preemptive_threads::{sched, thread, Builder, Duration, Thread, ThreadState};

/// Process id. 0 is the kernel.
pub type Pid = u32;

/// Status of a process killed with [`kill`].
pub const STATUS_KILLED: i32 = -9;
/// Most processes alive at once (one ASID each).
pub const MAX_PROCS: usize = USER_ASIDS;
/// Kernel stack of a process thread.
const KERNEL_STACK: usize = 32 * 1024;

/// Where a kernel-spawned process's standard output (fds 1 and 2) goes.
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

    /// An open file that writes here.
    pub fn file(&self) -> Arc<File> {
        match self {
            Output::Console => File::console(),
            Output::Capture(b) => File::capture(b.clone()),
        }
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
    pgid: AtomicU32,
    name: String,
    pub(crate) mm: Mutex<Option<Mm>>,
    pub(crate) files: Mutex<FdTable>,
    pub(crate) cwd: Mutex<String>,
    status: IrqSpinLock<Option<i32>>,
    /// Exit status `kill` asked for; 0 = not killed (kill statuses are
    /// negative). One atomic, so whoever sees the process killed also sees
    /// the status it was killed with.
    kill_status: AtomicI32,
    detached: AtomicBool,
    thread: IrqSpinLock<Option<Thread>>,
    fault: IrqSpinLock<Option<Fault>>,
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

    /// Process group (what Ctrl-C kills as a unit).
    pub fn pgid(&self) -> Pid {
        self.pgid.load(Ordering::Acquire)
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
        self.kill_status.load(Ordering::SeqCst) != 0
    }

    /// The fault that killed it, if any.
    pub fn fault(&self) -> Option<Fault> {
        *self.fault.lock()
    }

    fn thread(&self) -> Option<Thread> {
        self.thread.lock().clone()
    }

    /// Bitmask of the cores its thread has run on.
    pub fn cpus_ran(&self) -> u8 {
        self.thread().map_or(0, |t| t.cpus_ran())
    }

    /// CPU time used so far.
    pub fn cpu_time(&self) -> Duration {
        self.thread().map_or(Duration::ZERO, |t| t.cpu_time())
    }

    /// The core it runs on, last ran on, or is queued on.
    pub fn cpu(&self) -> usize {
        self.thread().map_or(0, |t| t.cpu())
    }

    /// Scheduling state: `Z` exited (zombie), `R` running on a core, `Q`
    /// ready (queued), `S` blocked (sleeping or waiting).
    pub fn state(&self) -> u8 {
        if self.status().is_some() {
            return b'Z';
        }
        match self.thread().map(|t| t.state()) {
            Some(ThreadState::Running) => b'R',
            Some(ThreadState::Blocked) => b'S',
            Some(ThreadState::Dead) => b'Z',
            _ => b'Q',
        }
    }

    /// Run `f` on its memory map (`None` once it has exited).
    pub fn with_mm<R>(&self, f: impl FnOnce(&Mm) -> R) -> Option<R> {
        self.mm.lock().as_ref().map(f)
    }

    /// Number of open file descriptors.
    pub fn open_fds(&self) -> usize {
        self.files.lock().count()
    }

    /// The file behind one of its fds.
    pub fn fd(&self, fd: usize) -> Result<Arc<File>, Errno> {
        self.files.lock().get(fd)
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
    /// A program in the file system: a path, or a bare name looked up in
    /// `/bin`.
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
    /// Standard output and error, when `files` is `None` (stdin is then
    /// the console).
    pub output: Output,
    /// The child's whole fd table.
    pub files: Option<FdTable>,
    /// Process group: `None` = the parent's (a new group for kernel-spawned
    /// processes), `Some(0)` = a new group led by the child, `Some(g)` = g.
    pub pgid: Option<Pid>,
    /// Working directory (default `/`).
    pub cwd: Option<String>,
}

/// Find the program for `path`: a bare name is looked up in `/bin`,
/// anything else relative to `cwd`. Returns its absolute path and node.
pub fn resolve_program(cwd: &str, path: &str) -> Result<(String, Arc<Inode>), Errno> {
    let abs = if path.contains('/') { vfs::normalize(cwd, path)? } else { vfs::normalize("/bin", path)? };
    let node = vfs::lookup(&abs)?;
    match node.kind() {
        vfs::Kind::File => Ok((abs, node)),
        vfs::Kind::Dir => Err(Errno::EISDIR),
        vfs::Kind::Device => Err(Errno::ENOEXEC),
    }
}

/// Create a process running `image` with arguments `argv` and make it
/// runnable. Returns its pid.
pub fn spawn(image: Image<'_>, argv: &[&[u8]], opts: SpawnOptions) -> Result<Pid, Errno> {
    if count() >= MAX_PROCS {
        return Err(Errno::EAGAIN);
    }
    let cwd = opts.cwd.clone().unwrap_or_else(|| String::from("/"));
    let mut mm = Mm::new()?;
    let (name, entry) = match image {
        Image::Named(path) => {
            let (abs, node) = resolve_program(&cwd, path)?;
            let entry = node.with_data(|d| crate::elf::load(&mut mm, d))??;
            (String::from(abs.rsplit('/').next().unwrap_or("?")), entry)
        }
        Image::Elf(e) => (String::from("<elf>"), crate::elf::load(&mut mm, e)?),
    };
    let (sp, argv_ptr) = mm.setup_stack(argv)?;
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let files = opts.files.clone().unwrap_or_else(|| {
        let mut t = FdTable::new();
        let out = opts.output.file();
        let _ = t.set(0, File::console());
        let _ = t.set(1, out.clone());
        let _ = t.set(2, out);
        t
    });
    let pgid = match opts.pgid {
        Some(0) => pid,
        Some(g) => g,
        None if opts.parent == 0 => pid,
        None => find(opts.parent).map_or(pid, |p| p.pgid()),
    };
    let p = Arc::new(Process {
        pid,
        parent: AtomicU32::new(opts.parent),
        pgid: AtomicU32::new(pgid),
        name,
        mm: Mutex::new(Some(mm)),
        files: Mutex::new(files),
        cwd: Mutex::new(cwd),
        status: IrqSpinLock::new(None),
        kill_status: AtomicI32::new(0),
        detached: AtomicBool::new(false),
        thread: IrqSpinLock::new(None),
        fault: IrqSpinLock::new(None),
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
            let mut t = p.thread.lock();
            if t.is_none() {
                *t = Some(h.thread().clone());
            }
            Ok(pid)
        }
        Err(_) => {
            let gone = TABLE.lock().remove(&pid);
            if let Some(p) = gone {
                drop(p.files.lock().clear());
            }
            Err(Errno::ENOMEM)
        }
    }
}

/// Body of a process's kernel thread.
fn process_main(p: Arc<Process>, mut frame: TrapFrame) {
    // Publish our thread before the first kill check, so a `kill` either
    // sees the thread (and interrupts it) or is seen by the check below.
    *p.thread.lock() = Some(thread::current());
    fence(Ordering::SeqCst);
    let ttbr0 = p.mm.lock().as_ref().map(|m| m.ttbr0()).expect("process without memory");
    // SAFETY: the address space copies the kernel's level-1 entries, and it
    // stays alive until `exit_process` has switched back to the kernel
    // tables (nobody else drops it).
    unsafe { sched::set_address_space(ttbr0) };
    let status = loop {
        if p.killed() {
            break p.kill_status.load(Ordering::SeqCst);
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
                // The one report of a fault (the shell does not add its own):
                // through the console, so it lands in order with the
                // process's output.
                let line = format!("[pid {}] {} at {:#x} (pc {:#x})\n", p.pid, kind, addr, frame.elr);
                crate::console::write(line.as_bytes());
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
    // Close every fd: pipe ends going away wake their peers.
    let files = p.files.lock().clear();
    drop(files);
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
/// and reap it. Returns `(pid, status)`, or `None` if `nohang` and no
/// child has exited yet. An `interruptible` wait (a process's) gives up
/// with `EINTR` when the calling thread is interrupted (killed).
pub fn wait_child(parent: Pid, target: Option<Pid>, interruptible: bool, nohang: bool) -> Result<Option<(Pid, i32)>, Errno> {
    let mut out = Err(Errno::ECHILD);
    let mut reaped = None;
    let mut check = || {
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
                out = Ok(Some((pid, s)));
                true
            }
            None if any && nohang => {
                out = Ok(None);
                true
            }
            None => !any,
        }
    };
    if interruptible {
        EXITED.wait_until_interruptible(&mut check).map_err(|_| Errno::EINTR)?;
    } else {
        EXITED.wait_until(&mut check);
    }
    drop(reaped);
    out
}

/// Wait for kernel-spawned process `pid` (or any, with `None`) and reap it.
pub fn wait(pid: Option<Pid>) -> Result<(Pid, i32), Errno> {
    wait_child(0, pid, false, false).map(|r| r.expect("blocking wait"))
}

/// Kill a process (exit status -9).
pub fn kill(pid: Pid) -> Result<(), Errno> {
    kill_with(pid, STATUS_KILLED)
}

/// Kill a process with exit status `status`: it stops right away if it is
/// running user code (an IPI pulls it out of EL0), and any interruptible
/// wait it is blocked in (console or pipe I/O, `wait`, `sleep`) returns
/// `EINTR` at once.
pub fn kill_with(pid: Pid, status: i32) -> Result<(), Errno> {
    let p = find(pid).ok_or(Errno::ESRCH)?;
    debug_assert!(status < 0);
    if p.status().is_none() {
        // The first kill wins.
        let _ = p.kill_status.compare_exchange(0, status, Ordering::SeqCst, Ordering::SeqCst);
    }
    fence(Ordering::SeqCst);
    if let Some(t) = p.thread() {
        t.interrupt();
    }
    EXITED.wake_all();
    Ok(())
}

/// Kill every live process in group `pgid`. Returns how many.
pub fn kill_group(pgid: Pid, status: i32) -> usize {
    let victims: Vec<Pid> =
        list().iter().filter(|p| p.pgid() == pgid && p.status().is_none()).map(|p| p.pid()).collect();
    for &pid in &victims {
        let _ = kill_with(pid, status);
    }
    victims.len()
}

/// A one-line description of an exit status: `exited 3`, `killed`,
/// `segfault`, ...
pub fn describe_status(status: i32) -> String {
    match status {
        s if s >= 0 => format!("exited {}", s),
        STATUS_KILLED => String::from("killed"),
        STATUS_INTERRUPTED => String::from("interrupted"),
        -11 => String::from("segfault"),
        -4 => String::from("illegal instruction"),
        -7 => String::from("bus error"),
        -5 => String::from("breakpoint"),
        -8 => String::from("floating-point exception"),
        s => format!("signal {}", -s),
    }
}
