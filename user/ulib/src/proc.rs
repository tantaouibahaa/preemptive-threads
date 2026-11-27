//! Processes: spawning with fd mappings, waiting, and the process and
//! system tables (`procinfo`, `sysinfo`).

use crate::io::Errno;
use crate::sys::{self, nr};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// How to start a child (see `pt_os::syscall::SpawnAttr`): which fds it
/// gets and which process group it joins.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SpawnAttr {
    flags: u32,
    pgid: u32,
    nfds: u32,
    reserved: u32,
    fds: [[i32; 2]; 8],
}

impl SpawnAttr {
    /// Inherit every fd and the process group.
    pub fn new() -> Self {
        Self::default()
    }

    /// Give the child only the fds mapped with [`fd`](Self::fd) (instead
    /// of inheriting all).
    pub fn only_mapped_fds(mut self) -> Self {
        self.flags |= 1;
        self
    }

    /// Child fd `child` is our fd `parent` (implies
    /// [`only_mapped_fds`](Self::only_mapped_fds)). At most 8.
    pub fn fd(mut self, child: i32, parent: i32) -> Self {
        self.flags |= 1;
        if let Some(slot) = self.fds.iter_mut().take(self.nfds as usize).find(|s| s[0] == child) {
            slot[1] = parent;
        } else if (self.nfds as usize) < self.fds.len() {
            self.fds[self.nfds as usize] = [child, parent];
            self.nfds += 1;
        }
        self
    }

    /// Put the child in process group `pgid` (0 = a new group it leads).
    pub fn group(mut self, pgid: u32) -> Self {
        self.flags |= 2;
        self.pgid = pgid;
        self
    }
}

/// Start `path` (a bare name is looked up in `/bin`) with arguments
/// `argv` (`argv[0]` included). Returns the child's pid.
pub fn spawn_path(path: &str, argv: &[&str], attr: Option<&SpawnAttr>) -> Result<i64, Errno> {
    let mut p = Vec::with_capacity(path.len() + 1);
    p.extend_from_slice(path.as_bytes());
    p.push(0);
    let strings: Vec<Vec<u8>> = argv
        .iter()
        .map(|a| {
            let mut v = Vec::from(a.as_bytes());
            v.push(0);
            v
        })
        .collect();
    let mut ptrs: Vec<*const u8> = strings.iter().map(|s| s.as_ptr()).collect();
    ptrs.push(core::ptr::null());
    let attr_ptr = attr.map_or(0, |a| a as *const SpawnAttr as u64);
    // SAFETY: NUL-terminated strings, a NULL-terminated pointer array and an
    // 80-byte attribute block, all alive across the call.
    Errno::check(unsafe { sys::syscall(nr::SPAWN, p.as_ptr() as u64, ptrs.as_ptr() as u64, attr_ptr) })
}

fn wait_raw(pid: i64, flags: u64) -> Result<Option<(i64, i32)>, Errno> {
    let mut status = 0i32;
    // SAFETY: `status` is a valid i32.
    let r = Errno::check(unsafe { sys::syscall(nr::WAIT, pid as u64, &mut status as *mut i32 as u64, flags) })?;
    Ok(if r == 0 { None } else { Some((r, status)) })
}

/// Wait for child `pid` to exit. Returns its exit status.
pub fn wait_pid(pid: i64) -> Result<i32, Errno> {
    wait_raw(pid, 0).map(|r| r.map_or(0, |(_, s)| s))
}

/// Wait for any child. Returns `(pid, status)`.
pub fn wait_any() -> Result<(i64, i32), Errno> {
    wait_raw(-1, 0).map(|r| r.unwrap_or((0, 0)))
}

/// Reap a child that has exited, without blocking.
pub fn try_wait_any() -> Result<Option<(i64, i32)>, Errno> {
    wait_raw(-1, 1)
}

/// Exit status of a process interrupted with Ctrl-C.
pub const STATUS_INTERRUPTED: i32 = -2;
/// Exit status of a process killed with `kill`.
pub const STATUS_KILLED: i32 = -9;

/// Whether `status` says the process was killed by a fault (segfault,
/// illegal instruction, bus error, breakpoint, floating-point exception).
/// The kernel prints a line with the fault address and pc when that
/// happens.
pub fn is_fault_status(status: i32) -> bool {
    matches!(status, -11 | -4 | -7 | -5 | -8)
}

/// Describe an exit status: `exited 3`, `killed`, `interrupted`,
/// `segfault`, ...
pub fn status_text(status: i32) -> String {
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

/// One process, as reported by [`procinfo`].
#[derive(Debug, Clone)]
pub struct ProcInfo {
    /// Process id.
    pub pid: u32,
    /// Parent (0 = the kernel).
    pub ppid: u32,
    /// Process group.
    pub pgid: u32,
    /// `R` running, `Q` ready, `S` blocked, `Z` exited.
    pub state: char,
    /// Core it runs on or last ran on.
    pub cpu: u8,
    /// Mask of the cores it has run on.
    pub cpus_ran: u8,
    /// CPU time used, in nanoseconds.
    pub cpu_time_ns: u64,
    /// User pages mapped.
    pub pages: u32,
    /// Open fds.
    pub fds: u32,
    /// Program name.
    pub name: String,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Every process in the system, by pid.
pub fn procinfo() -> Vec<ProcInfo> {
    let mut cap = 64;
    loop {
        let mut buf = vec![0u8; cap * 48];
        // SAFETY: `buf` holds `cap` 48-byte entries.
        let r = unsafe { sys::syscall(nr::PROCINFO, buf.as_mut_ptr() as u64, cap as u64, 0) };
        if r < 0 {
            return Vec::new();
        }
        let n = r as usize;
        if n > cap {
            cap = n + 16;
            continue;
        }
        return (0..n)
            .map(|i| {
                let e = &buf[i * 48..(i + 1) * 48];
                let name = &e[32..48];
                let len = name.iter().position(|&c| c == 0).unwrap_or(16);
                ProcInfo {
                    pid: u32_at(e, 0),
                    ppid: u32_at(e, 4),
                    pgid: u32_at(e, 8),
                    state: e[12] as char,
                    cpu: e[13],
                    cpus_ran: e[14],
                    cpu_time_ns: u64_at(e, 16),
                    pages: u32_at(e, 24),
                    fds: u32_at(e, 28),
                    name: String::from_utf8_lossy(&name[..len]).into_owned(),
                }
            })
            .collect();
    }
}

/// One core, as reported by [`sysinfo`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuInfo {
    /// Whether the core is up.
    pub online: bool,
    /// Timer ticks (1 kHz).
    pub ticks: u64,
    /// Ticks spent idle.
    pub idle_ticks: u64,
    /// Thread switches.
    pub switches: u64,
}

/// System-wide counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct SysInfo {
    /// Nanoseconds since boot.
    pub uptime_ns: u64,
    /// Free physical frames (4 KiB).
    pub frames_free: u64,
    /// All frames.
    pub frames_total: u64,
    /// Cores online.
    pub cpus_online: u32,
    /// Processes in the table.
    pub processes: u32,
    /// Per-core counters.
    pub cpus: [CpuInfo; 4],
}

/// Read the system counters.
pub fn sysinfo() -> SysInfo {
    let mut b = [0u8; 160];
    // SAFETY: 160-byte output buffer.
    if unsafe { sys::syscall(nr::SYSINFO, b.as_mut_ptr() as u64, 0, 0) } < 0 {
        return SysInfo::default();
    }
    let mut s = SysInfo {
        uptime_ns: u64_at(&b, 0),
        frames_free: u64_at(&b, 8),
        frames_total: u64_at(&b, 16),
        cpus_online: u32_at(&b, 24),
        processes: u32_at(&b, 28),
        cpus: [CpuInfo::default(); 4],
    };
    for (i, c) in s.cpus.iter_mut().enumerate() {
        let o = 32 + i * 32;
        *c = CpuInfo {
            online: u32_at(&b, o) != 0,
            ticks: u64_at(&b, o + 8),
            idle_ticks: u64_at(&b, o + 16),
            switches: u64_at(&b, o + 24),
        };
    }
    s
}
