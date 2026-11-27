//! System calls.
//!
//! ABI: `svc #0` with the number in x8, arguments in x0-x5, result in x0;
//! a negative result is `-errno`. Pointers are user virtual addresses and
//! are only accessed through [`crate::mm`]'s checked copies. Paths are
//! NUL-terminated, absolute or relative to the working directory.
//!
//! | nr | name | arguments | result |
//! |----|------|-----------|--------|
//! | 0 | `exit` | code | does not return |
//! | 1 | `write` | fd, buf, len | bytes written |
//! | 2 | `read` | fd, buf, len | bytes read, 0 = end of file |
//! | 3 | `open` | path, flags (read 1, write 2, create 4, truncate 8, append 16) | fd |
//! | 4 | `close` | fd | 0 |
//! | 5 | `readdir` | fd, buf, len | entry length (kind byte + name), 0 = end |
//! | 6 | `spawn` | path, argv (NULL-terminated), attr ([`SpawnAttr`], may be 0) | pid |
//! | 7 | `wait` | pid (-1 = any child), status ptr (may be 0), flags (1 = no hang) | pid, 0 = none yet |
//! | 8 | `kill` | pid (own child or self) | 0 |
//! | 9 | `getpid` | | pid |
//! | 10 | `sleep` | nanoseconds | 0 |
//! | 11 | `yield` | | 0 |
//! | 12 | `sbrk` | increment (signed) | old break |
//! | 13 | `pipe` | fds ptr (two i32: read end, write end) | 0 |
//! | 14 | `dup2` | old fd, new fd | new fd |
//! | 15 | `uptime` | | nanoseconds since boot |
//! | 16 | `procinfo` | buf ([`ProcInfo`] array), max entries | number of processes |
//! | 17 | `cpu` | | core the caller runs on |
//! | 18 | `setfg` | process group (0 = none) | 0 |
//! | 19 | `sysinfo` | buf ([`SysInfo`]) | 0 |
//! | 20 | `unlink` | path | 0 |
//! | 21 | `mkdir` | path | 0 |
//! | 22 | `chdir` | path | 0 |
//! | 23 | `getcwd` | buf, len | length |
//! | 24 | `stat` | path, buf ([`Stat`]) | 0 |
//! | 25 | `ioctl` | fd, request (1 = console raw mode, 2 = bytes available, 3 = is the console), arg | request specific |
//!
//! Blocking calls (`read`/`write` on the console and pipes, `wait`,
//! `sleep`) return `EINTR` as soon as the process is killed.

use crate::console;
use crate::errno::Errno;
use crate::process::{self, Image, Process, SpawnOptions};
use crate::vfs::{self, FdTable, FileKind};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use preemptive_threads::arch::vectors::TrapFrame;
use preemptive_threads::{thread, Duration, Instant};

#[allow(missing_docs)]
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const OPEN: u64 = 3;
    pub const CLOSE: u64 = 4;
    pub const READDIR: u64 = 5;
    pub const SPAWN: u64 = 6;
    pub const WAIT: u64 = 7;
    pub const KILL: u64 = 8;
    pub const GETPID: u64 = 9;
    pub const SLEEP: u64 = 10;
    pub const YIELD: u64 = 11;
    pub const SBRK: u64 = 12;
    pub const PIPE: u64 = 13;
    pub const DUP2: u64 = 14;
    pub const UPTIME: u64 = 15;
    pub const PROCINFO: u64 = 16;
    pub const CPU: u64 = 17;
    pub const SETFG: u64 = 18;
    pub const SYSINFO: u64 = 19;
    pub const UNLINK: u64 = 20;
    pub const MKDIR: u64 = 21;
    pub const CHDIR: u64 = 22;
    pub const GETCWD: u64 = 23;
    pub const STAT: u64 = 24;
    pub const IOCTL: u64 = 25;
}

const MAX_ARGS: usize = 32;
const MAX_ARG_LEN: usize = 256;
/// Bytes moved per step of `read`/`write` (one kernel bounce buffer).
const CHUNK: usize = 4096;

/// `spawn` attributes (all fields little-endian, 80 bytes).
///
/// ```text
///   u32 flags      bit 0: fds below replace the inherited table
///                  bit 1: pgid below is used (0 = a new group led by the child)
///   u32 pgid
///   u32 nfds       entries used in `fds` (at most 8)
///   u32 reserved
///   [i32; 2] x 8   fds: (child fd, parent fd)
/// ```
///
/// Without bit 0 the child inherits every fd of the parent; with it, the
/// child gets exactly the listed fds (so a shell's other pipe ends do not
/// leak into it).
pub struct SpawnAttr;

/// `procinfo` entry (48 bytes, little-endian).
///
/// ```text
///   u32 pid, u32 ppid, u32 pgid,
///   u8 state ('R' running, 'Q' ready, 'S' blocked, 'Z' exited),
///   u8 cpu (running on / last ran on), u8 cpus_ran (mask), u8 reserved,
///   u64 cpu_time_ns, u32 mapped_pages, u32 open_fds, [u8; 16] name (NUL padded)
/// ```
pub struct ProcInfo;

/// `sysinfo` result (160 bytes, little-endian).
///
/// ```text
///   u64 uptime_ns, u64 frames_free, u64 frames_total, u32 cpus_online, u32 processes,
///   4 x { u32 online, u32 reserved, u64 ticks, u64 idle_ticks, u64 context_switches }
/// ```
pub struct SysInfo;

/// `stat` result (16 bytes): `u32 kind` (1 file, 2 directory, 3 device),
/// `u32 reserved`, `u64 size`.
pub struct Stat;

/// What the run loop should do after a system call.
pub enum Action {
    /// Resume the process with this value in x0.
    Return(i64),
    /// The process called `exit`.
    Exit(i32),
}

/// Handle the system call in `f` for process `p`.
pub fn dispatch(p: &Arc<Process>, f: &TrapFrame) -> Action {
    let a = f.x;
    let r = match f.x[8] {
        nr::EXIT => return Action::Exit(a[0] as i32),
        nr::WRITE => sys_write(p, a[0], a[1], a[2] as usize),
        nr::READ => sys_read(p, a[0], a[1], a[2] as usize),
        nr::OPEN => sys_open(p, a[0], a[1] as u32),
        nr::CLOSE => sys_close(p, a[0]),
        nr::READDIR => sys_readdir(p, a[0], a[1], a[2] as usize),
        nr::SPAWN => sys_spawn(p, a[0], a[1], a[2]),
        nr::WAIT => sys_wait(p, a[0] as i64, a[1], a[2]),
        nr::KILL => sys_kill(p, a[0]),
        nr::GETPID => Ok(p.pid() as i64),
        nr::SLEEP => sys_sleep(a[0]),
        nr::YIELD => {
            thread::yield_now();
            Ok(0)
        }
        nr::SBRK => p.mm.lock().as_mut().ok_or(Errno::EFAULT).and_then(|m| m.sbrk(a[0] as i64)).map(|v| v as i64),
        nr::PIPE => sys_pipe(p, a[0]),
        nr::DUP2 => sys_dup2(p, a[0], a[1]),
        nr::UPTIME => Ok(preemptive_threads::time::uptime().as_nanos() as i64),
        nr::PROCINFO => sys_procinfo(p, a[0], a[1] as usize),
        nr::CPU => Ok(thread::current_cpu() as i64),
        nr::SETFG => {
            console::set_foreground(a[0] as u32);
            Ok(0)
        }
        nr::SYSINFO => sys_sysinfo(p, a[0]),
        nr::UNLINK => path_arg(p, a[0]).and_then(|path| vfs::unlink(&path)).map(|_| 0),
        nr::MKDIR => path_arg(p, a[0]).and_then(|path| vfs::mkdir(&path)).map(|_| 0),
        nr::CHDIR => sys_chdir(p, a[0]),
        nr::GETCWD => sys_getcwd(p, a[0], a[1] as usize),
        nr::STAT => sys_stat(p, a[0], a[1]),
        nr::IOCTL => sys_ioctl(p, a[0], a[1], a[2]),
        _ => Err(Errno::ENOSYS),
    };
    Action::Return(match r {
        Ok(v) => v,
        Err(e) => -(e.0 as i64),
    })
}

fn copy_in(p: &Process, va: u64, buf: &mut [u8]) -> Result<(), Errno> {
    p.mm.lock().as_ref().ok_or(Errno::EFAULT)?.copy_from_user(va, buf)
}

fn copy_out(p: &Process, va: u64, data: &[u8]) -> Result<(), Errno> {
    p.mm.lock().as_ref().ok_or(Errno::EFAULT)?.copy_to_user(va, data)
}

fn check(p: &Process, va: u64, len: usize, write: bool) -> Result<(), Errno> {
    p.mm.lock().as_ref().ok_or(Errno::EFAULT)?.check(va, len, write)
}

fn read_cstr(p: &Process, va: u64, max: usize) -> Result<Vec<u8>, Errno> {
    p.mm.lock().as_ref().ok_or(Errno::EFAULT)?.read_cstr(va, max)
}

/// Read a path argument and make it absolute.
fn path_arg(p: &Process, va: u64) -> Result<String, Errno> {
    let raw = read_cstr(p, va, vfs::PATH_MAX)?;
    let s = core::str::from_utf8(&raw).map_err(|_| Errno::EINVAL)?;
    vfs::normalize(&p.cwd.lock(), s)
}

fn fd_arg(fd: u64) -> Result<usize, Errno> {
    usize::try_from(fd).ok().filter(|&f| f < vfs::MAX_FDS).ok_or(Errno::EBADF)
}

fn sys_write(p: &Process, fd: u64, buf: u64, len: usize) -> Result<i64, Errno> {
    let file = p.fd(fd_arg(fd)?)?;
    check(p, buf, len, false)?;
    let mut chunk = vec![0u8; len.min(CHUNK)];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(chunk.len());
        copy_in(p, buf + done as u64, &mut chunk[..n])?;
        match file.write(&chunk[..n]) {
            Ok(w) => {
                done += w;
                if w < n {
                    break;
                }
            }
            Err(e) if done == 0 => return Err(e),
            Err(_) => break,
        }
    }
    Ok(done as i64)
}

fn sys_read(p: &Process, fd: u64, buf: u64, len: usize) -> Result<i64, Errno> {
    let file = p.fd(fd_arg(fd)?)?;
    check(p, buf, len, true)?;
    let mut chunk = vec![0u8; len.min(CHUNK)];
    let n = file.read(&mut chunk)?;
    copy_out(p, buf, &chunk[..n])?;
    Ok(n as i64)
}

fn sys_open(p: &Process, path: u64, flags: u32) -> Result<i64, Errno> {
    let path = path_arg(p, path)?;
    let f = vfs::open(&path, flags)?;
    p.files.lock().insert(f).map(|fd| fd as i64)
}

fn sys_close(p: &Process, fd: u64) -> Result<i64, Errno> {
    let f = p.files.lock().remove(fd_arg(fd)?)?;
    drop(f); // outside the table lock: closing a pipe end wakes its peer
    Ok(0)
}

fn sys_readdir(p: &Process, fd: u64, buf: u64, len: usize) -> Result<i64, Errno> {
    let file = p.fd(fd_arg(fd)?)?;
    match file.readdir()? {
        None => Ok(0),
        Some((kind, name)) => {
            let mut out = Vec::with_capacity(name.len() + 1);
            out.push(kind as u8);
            out.extend_from_slice(name.as_bytes());
            if out.len() > len {
                return Err(Errno::EINVAL);
            }
            copy_out(p, buf, &out)?;
            Ok(out.len() as i64)
        }
    }
}

fn sys_spawn(p: &Process, path: u64, argv: u64, attr: u64) -> Result<i64, Errno> {
    let path = read_cstr(p, path, vfs::PATH_MAX)?;
    let mut args = Vec::new();
    if argv != 0 {
        for i in 0..=MAX_ARGS {
            let mut w = [0u8; 8];
            copy_in(p, argv.checked_add(8 * i as u64).ok_or(Errno::EFAULT)?, &mut w)?;
            let ptr = u64::from_le_bytes(w);
            if ptr == 0 {
                break;
            }
            if i == MAX_ARGS {
                return Err(Errno::E2BIG);
            }
            args.push(read_cstr(p, ptr, MAX_ARG_LEN)?);
        }
    }
    let mut files = p.files.lock().clone();
    let mut pgid = None;
    if attr != 0 {
        let mut a = [0u8; 80];
        copy_in(p, attr, &mut a)?;
        let word = |i: usize| u32::from_le_bytes(a[4 * i..4 * i + 4].try_into().unwrap());
        let (flags, group, nfds) = (word(0), word(1), word(2) as usize);
        if flags & 1 != 0 {
            if nfds > 8 {
                return Err(Errno::EINVAL);
            }
            let parent = files;
            files = FdTable::new();
            for i in 0..nfds {
                let (child_fd, parent_fd) = (word(4 + 2 * i) as i32, word(5 + 2 * i) as i32);
                let f = parent.get(fd_arg(parent_fd as u64)?)?;
                files.set(fd_arg(child_fd as u64)?, f)?;
            }
        }
        if flags & 2 != 0 {
            pgid = Some(group);
        }
    }
    let name = core::str::from_utf8(&path).map_err(|_| Errno::ENOENT)?;
    let argv: Vec<&[u8]> = args.iter().map(|a| a.as_slice()).collect();
    let opts = SpawnOptions {
        parent: p.pid(),
        files: Some(files),
        pgid,
        cwd: Some(p.cwd.lock().clone()),
        ..Default::default()
    };
    process::spawn(Image::Named(name), &argv, opts).map(|pid| pid as i64)
}

fn sys_wait(p: &Process, pid: i64, status: u64, flags: u64) -> Result<i64, Errno> {
    let target = match pid {
        -1 => None,
        1.. if pid <= u32::MAX as i64 => Some(pid as u32),
        _ => return Err(Errno::EINVAL),
    };
    if status != 0 {
        check(p, status, 4, true)?;
    }
    match process::wait_child(p.pid(), target, true, flags & 1 != 0)? {
        None => Ok(0),
        Some((pid, code)) => {
            if status != 0 {
                copy_out(p, status, &code.to_le_bytes())?;
            }
            Ok(pid as i64)
        }
    }
}

fn sys_kill(p: &Process, pid: u64) -> Result<i64, Errno> {
    let pid = u32::try_from(pid).map_err(|_| Errno::ESRCH)?;
    let target = process::find(pid).ok_or(Errno::ESRCH)?;
    if pid != p.pid() && target.parent() != p.pid() {
        return Err(Errno::EPERM);
    }
    if target.status().is_some() {
        return Err(Errno::ESRCH);
    }
    process::kill(pid).map(|_| 0)
}

fn sys_sleep(ns: u64) -> Result<i64, Errno> {
    Instant::now().checked_add(Duration::from_nanos(ns)).ok_or(Errno::EINVAL)?;
    thread::sleep_interruptible(Duration::from_nanos(ns)).map(|_| 0).map_err(|_| Errno::EINTR)
}

fn sys_pipe(p: &Process, fds: u64) -> Result<i64, Errno> {
    check(p, fds, 8, true)?;
    let (r, w) = vfs::pipe();
    let (rfd, wfd) = {
        let mut t = p.files.lock();
        let rfd = t.insert(r)?;
        match t.insert(w) {
            Ok(wfd) => (rfd, wfd),
            Err(e) => {
                let r = t.remove(rfd);
                drop(t);
                drop(r);
                return Err(e);
            }
        }
    };
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(rfd as i32).to_le_bytes());
    out[4..].copy_from_slice(&(wfd as i32).to_le_bytes());
    copy_out(p, fds, &out)?;
    Ok(0)
}

fn sys_dup2(p: &Process, old: u64, new: u64) -> Result<i64, Errno> {
    let (old, new) = (fd_arg(old)?, fd_arg(new)?);
    let replaced = {
        let mut t = p.files.lock();
        let f = t.get(old)?;
        if old == new {
            return Ok(new as i64);
        }
        t.set(new, f)?
    };
    drop(replaced);
    Ok(new as i64)
}

fn sys_procinfo(p: &Process, buf: u64, max: usize) -> Result<i64, Errno> {
    let procs = process::list();
    let n = procs.len().min(max);
    check(p, buf, n * 48, true)?;
    let mut out = Vec::with_capacity(n * 48);
    for q in procs.iter().take(n) {
        out.extend_from_slice(&q.pid().to_le_bytes());
        out.extend_from_slice(&q.parent().to_le_bytes());
        out.extend_from_slice(&q.pgid().to_le_bytes());
        out.push(q.state());
        out.push(q.cpu() as u8);
        out.push(q.cpus_ran());
        out.push(0);
        out.extend_from_slice(&(q.cpu_time().as_nanos() as u64).to_le_bytes());
        out.extend_from_slice(&(q.with_mm(|m| m.mapped_pages()).unwrap_or(0) as u32).to_le_bytes());
        out.extend_from_slice(&(q.open_fds() as u32).to_le_bytes());
        let mut name = [0u8; 16];
        let b = q.name().as_bytes();
        let k = b.len().min(15);
        name[..k].copy_from_slice(&b[..k]);
        out.extend_from_slice(&name);
    }
    copy_out(p, buf, &out)?;
    Ok(procs.len() as i64)
}

fn sys_sysinfo(p: &Process, buf: u64) -> Result<i64, Errno> {
    let k = preemptive_threads::stats();
    let o = crate::stats();
    let mut out = Vec::with_capacity(160);
    out.extend_from_slice(&(preemptive_threads::time::uptime().as_nanos() as u64).to_le_bytes());
    out.extend_from_slice(&(o.frames_free as u64).to_le_bytes());
    out.extend_from_slice(&(o.frames_total as u64).to_le_bytes());
    out.extend_from_slice(&(k.cpus_online() as u32).to_le_bytes());
    out.extend_from_slice(&(o.processes as u32).to_le_bytes());
    for c in k.cpus.iter() {
        out.extend_from_slice(&(c.online as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&c.ticks.to_le_bytes());
        out.extend_from_slice(&c.idle_ticks.to_le_bytes());
        out.extend_from_slice(&c.context_switches.to_le_bytes());
    }
    copy_out(p, buf, &out)?;
    Ok(0)
}

fn sys_chdir(p: &Process, path: u64) -> Result<i64, Errno> {
    let path = path_arg(p, path)?;
    if vfs::lookup(&path)?.kind() != vfs::Kind::Dir {
        return Err(Errno::ENOTDIR);
    }
    *p.cwd.lock() = path;
    Ok(0)
}

fn sys_getcwd(p: &Process, buf: u64, len: usize) -> Result<i64, Errno> {
    let cwd = p.cwd.lock().clone();
    if cwd.len() + 1 > len {
        return Err(Errno::ERANGE);
    }
    let mut out = Vec::from(cwd.as_bytes());
    out.push(0);
    copy_out(p, buf, &out)?;
    Ok(cwd.len() as i64)
}

fn sys_stat(p: &Process, path: u64, buf: u64) -> Result<i64, Errno> {
    let path = path_arg(p, path)?;
    let node = vfs::lookup(&path)?;
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&(node.kind() as u32).to_le_bytes());
    out[8..].copy_from_slice(&(node.size() as u64).to_le_bytes());
    copy_out(p, buf, &out)?;
    Ok(0)
}

fn sys_ioctl(p: &Process, fd: u64, req: u64, arg: u64) -> Result<i64, Errno> {
    let file = p.fd(fd_arg(fd)?)?;
    match req {
        1 => match file.kind() {
            FileKind::Console => {
                console::set_raw(arg != 0);
                Ok(0)
            }
            _ => Err(Errno::ENOTTY),
        },
        2 => file.available().map(|n| n as i64),
        3 => Ok(matches!(file.kind(), FileKind::Console) as i64),
        _ => Err(Errno::EINVAL),
    }
}
