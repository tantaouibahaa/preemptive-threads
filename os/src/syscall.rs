//! System calls.
//!
//! ABI: `svc #0` with the number in x8, arguments in x0-x5, result in x0;
//! a negative result is `-errno`. Pointers are user virtual addresses and
//! are only accessed through [`crate::mm`]'s checked copies.
//!
//! | nr | name | arguments | result |
//! |----|------|-----------|--------|
//! | 0 | `exit` | code | does not return |
//! | 1 | `write` | fd (1 or 2), buf, len | bytes written |
//! | 6 | `spawn` | path (C string), argv (NULL-terminated array of C strings) | pid |
//! | 7 | `wait` | pid (-1 = any child), status ptr (may be 0) | pid |
//! | 8 | `kill` | pid (own child or self) | 0 |
//! | 9 | `getpid` | | pid |
//! | 10 | `sleep` | nanoseconds | 0 |
//! | 11 | `yield` | | 0 |
//! | 12 | `sbrk` | increment (signed) | old break |
//! | 15 | `uptime` | | nanoseconds since boot |
//! | 17 | `cpu` | | core the caller runs on |
//!
//! Numbers 2-5, 13, 14 and 16 are reserved for `read`, `open`, `close`,
//! `readdir`, `pipe`, `dup2` and `procinfo` and return `ENOSYS` for now.

use crate::errno::Errno;
use crate::process::{self, Image, Process, SpawnOptions};
use alloc::sync::Arc;
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
}

const MAX_ARGS: usize = 32;
const MAX_ARG_LEN: usize = 256;
const MAX_PATH: usize = 64;

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
        nr::SPAWN => sys_spawn(p, a[0], a[1]),
        nr::WAIT => sys_wait(p, a[0] as i64, a[1]),
        nr::KILL => sys_kill(p, a[0]),
        nr::GETPID => Ok(p.pid() as i64),
        nr::SLEEP => sys_sleep(p, a[0]),
        nr::YIELD => {
            thread::yield_now();
            Ok(0)
        }
        nr::SBRK => p.mm.lock().as_mut().ok_or(Errno::EFAULT).and_then(|m| m.sbrk(a[0] as i64)).map(|v| v as i64),
        nr::UPTIME => Ok(preemptive_threads::time::uptime().as_nanos() as i64),
        nr::CPU => Ok(thread::current_cpu() as i64),
        _ => Err(Errno::ENOSYS),
    };
    Action::Return(match r {
        Ok(v) => v,
        Err(e) => -(e.0 as i64),
    })
}

fn sys_write(p: &Process, fd: u64, buf: u64, len: usize) -> Result<i64, Errno> {
    if fd != 1 && fd != 2 {
        return Err(Errno::EBADF);
    }
    let mut chunk = [0u8; 512];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(chunk.len());
        {
            let mm = p.mm.lock();
            let mm = mm.as_ref().ok_or(Errno::EFAULT)?;
            if done == 0 {
                mm.check(buf, len, false)?;
            }
            mm.copy_from_user(buf + done as u64, &mut chunk[..n])?;
        }
        p.output.write(&chunk[..n]);
        done += n;
    }
    Ok(done as i64)
}

fn sys_spawn(p: &Process, path: u64, argv: u64) -> Result<i64, Errno> {
    let (path, args) = {
        let mm = p.mm.lock();
        let mm = mm.as_ref().ok_or(Errno::EFAULT)?;
        let path = mm.read_cstr(path, MAX_PATH)?;
        let mut args = Vec::new();
        if argv != 0 {
            for i in 0..=MAX_ARGS {
                let ptr = mm.read_u64(argv.checked_add(8 * i as u64).ok_or(Errno::EFAULT)?)?;
                if ptr == 0 {
                    break;
                }
                if i == MAX_ARGS {
                    return Err(Errno::E2BIG);
                }
                args.push(mm.read_cstr(ptr, MAX_ARG_LEN)?);
            }
        }
        (path, args)
    };
    let name = core::str::from_utf8(&path).map_err(|_| Errno::ENOENT)?;
    let argv: Vec<&[u8]> = args.iter().map(|a| a.as_slice()).collect();
    let opts = SpawnOptions { parent: p.pid(), cpu: None, output: p.output.clone() };
    process::spawn(Image::Named(name), &argv, opts).map(|pid| pid as i64)
}

fn sys_wait(p: &Process, pid: i64, status: u64) -> Result<i64, Errno> {
    let target = match pid {
        -1 => None,
        1.. if pid <= u32::MAX as i64 => Some(pid as u32),
        _ => return Err(Errno::EINVAL),
    };
    if status != 0 {
        p.mm.lock().as_ref().ok_or(Errno::EFAULT)?.check(status, 4, true)?;
    }
    let (pid, code) = process::wait_child(p.pid(), target, Some(p.killed_flag()))?;
    if status != 0 {
        p.mm.lock().as_ref().ok_or(Errno::EFAULT)?.copy_to_user(status, &code.to_le_bytes())?;
    }
    Ok(pid as i64)
}

fn sys_kill(p: &Process, pid: u64) -> Result<i64, Errno> {
    let pid = u32::try_from(pid).map_err(|_| Errno::ESRCH)?;
    let target = process::find(pid).ok_or(Errno::ESRCH)?;
    if pid != p.pid() && target.parent() != p.pid() {
        return Err(Errno::EPERM);
    }
    process::kill(pid).map(|_| 0)
}

fn sys_sleep(p: &Process, ns: u64) -> Result<i64, Errno> {
    let deadline = Instant::now().checked_add(Duration::from_nanos(ns)).ok_or(Errno::EINVAL)?;
    // Sleep in slices so a kill is noticed quickly.
    loop {
        if p.killed() {
            return Err(Errno::EINTR);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(0);
        }
        thread::sleep((deadline - now).min(Duration::from_millis(10)));
    }
}
