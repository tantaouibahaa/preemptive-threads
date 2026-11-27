//! Raw system calls. Each returns the kernel's result: a value, or
//! `-errno` (see [`errno`]).

use core::arch::asm;

/// System call numbers (must match `pt_os::syscall::nr`).
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

/// Error numbers (positive; system calls return them negated).
#[allow(missing_docs)]
pub mod errno {
    pub const EPERM: i64 = 1;
    pub const ENOENT: i64 = 2;
    pub const ESRCH: i64 = 3;
    pub const EINTR: i64 = 4;
    pub const E2BIG: i64 = 7;
    pub const ENOEXEC: i64 = 8;
    pub const EBADF: i64 = 9;
    pub const ECHILD: i64 = 10;
    pub const EAGAIN: i64 = 11;
    pub const ENOMEM: i64 = 12;
    pub const EFAULT: i64 = 14;
    pub const EEXIST: i64 = 17;
    pub const ENOTDIR: i64 = 20;
    pub const EISDIR: i64 = 21;
    pub const EINVAL: i64 = 22;
    pub const EMFILE: i64 = 24;
    pub const ENOTTY: i64 = 25;
    pub const EFBIG: i64 = 27;
    pub const EPIPE: i64 = 32;
    pub const ERANGE: i64 = 34;
    pub const ENAMETOOLONG: i64 = 36;
    pub const ENOSYS: i64 = 38;
    pub const ENOTEMPTY: i64 = 39;
}

/// Raw system call with three arguments.
///
/// # Safety
///
/// Pointer arguments must be valid for what the call does with them (the
/// kernel validates them, but a bad pointer may still clobber our memory).
#[inline(always)]
pub unsafe fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let r: i64;
    // SAFETY: the kernel preserves every register except x0.
    unsafe { asm!("svc #0", inlateout("x0") a0 as i64 => r, in("x1") a1, in("x2") a2, in("x8") n, options(nostack)) };
    r
}

/// Terminate the process with exit code `code`.
pub fn exit(code: i32) -> ! {
    // SAFETY: exit takes no pointers.
    unsafe { syscall(nr::EXIT, code as u64, 0, 0) };
    // exit never returns; if it somehow did, stop here.
    loop {
        // SAFETY: `brk` only traps into the kernel, which kills us.
        unsafe { asm!("brk #1") };
    }
}

/// Write `buf` to `fd`.
pub fn write(fd: u64, buf: &[u8]) -> i64 {
    // SAFETY: `buf` is a valid slice.
    unsafe { syscall(nr::WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) }
}

/// Read into `buf` from `fd`; 0 = end of file.
pub fn read(fd: u64, buf: &mut [u8]) -> i64 {
    // SAFETY: `buf` is a valid, writable slice.
    unsafe { syscall(nr::READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Copy `s` into `buf` with a NUL terminator (truncating long paths, which
/// the kernel then rejects or fails to find).
fn cstr<'a>(s: &str, buf: &'a mut [u8; 260]) -> &'a [u8] {
    let n = s.len().min(buf.len() - 1);
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
    buf[n] = 0;
    &buf[..=n]
}

fn path_call(n: u64, path: &str, a1: u64) -> i64 {
    let mut b = [0u8; 260];
    let p = cstr(path, &mut b);
    // SAFETY: `p` is NUL terminated; the call's other argument is not a
    // pointer for the calls that use this helper (except `stat`, see there).
    unsafe { syscall(n, p.as_ptr() as u64, a1, 0) }
}

/// Open `path` with `O_*` flags. Returns the fd.
pub fn open(path: &str, flags: u32) -> i64 {
    path_call(nr::OPEN, path, flags as u64)
}

/// Close `fd`.
pub fn close(fd: u64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::CLOSE, fd, 0, 0) }
}

/// Create a pipe: `[read end, write end]`.
pub fn pipe(fds: &mut [i32; 2]) -> i64 {
    // SAFETY: `fds` is valid for 8 bytes.
    unsafe { syscall(nr::PIPE, fds.as_mut_ptr() as u64, 0, 0) }
}

/// Make `new` refer to the same open file as `old`.
pub fn dup2(old: u64, new: u64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::DUP2, old, new, 0) }
}

/// Remove a file or empty directory.
pub fn unlink(path: &str) -> i64 {
    path_call(nr::UNLINK, path, 0)
}

/// Create a directory.
pub fn mkdir(path: &str) -> i64 {
    path_call(nr::MKDIR, path, 0)
}

/// Change the working directory.
pub fn chdir(path: &str) -> i64 {
    path_call(nr::CHDIR, path, 0)
}

/// Write the working directory into `buf` (NUL terminated). Returns its
/// length.
pub fn getcwd(buf: &mut [u8]) -> i64 {
    // SAFETY: `buf` is a valid, writable slice.
    unsafe { syscall(nr::GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64, 0) }
}

/// Our process id.
pub fn getpid() -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::GETPID, 0, 0, 0) }
}

/// Give up the core.
pub fn yield_now() -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::YIELD, 0, 0, 0) }
}

/// Sleep for `ns` nanoseconds (`-EINTR` if killed meanwhile).
pub fn sleep_ns(ns: u64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::SLEEP, ns, 0, 0) }
}

/// Sleep for `ms` milliseconds.
pub fn sleep_ms(ms: u64) -> i64 {
    sleep_ns(ms * 1_000_000)
}

/// Nanoseconds since boot.
pub fn uptime_ns() -> u64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::UPTIME, 0, 0, 0) as u64 }
}

/// The core we are running on (may change at any time).
pub fn cpu() -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::CPU, 0, 0, 0) }
}

/// Move the heap break; returns the old break.
pub fn sbrk(incr: i64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::SBRK, incr as u64, 0, 0) }
}

/// Spawn a program, inheriting every fd. `path` and every argument must
/// end with a NUL byte (at most 8 arguments).
pub fn spawn(path: &[u8], args: &[&[u8]]) -> i64 {
    let mut argv = [core::ptr::null::<u8>(); 9];
    for (slot, a) in argv.iter_mut().zip(args.iter().take(8)) {
        *slot = a.as_ptr();
    }
    // SAFETY: NUL-terminated strings and a NULL-terminated array.
    unsafe { syscall(nr::SPAWN, path.as_ptr() as u64, argv.as_ptr() as u64, 0) }
}

/// Wait for a child (`pid` = -1: any). Returns `(pid or -errno, status)`.
pub fn wait(pid: i64) -> (i64, i32) {
    let mut status = 0i32;
    // SAFETY: `status` is a valid i32.
    let r = unsafe { syscall(nr::WAIT, pid as u64, &mut status as *mut i32 as u64, 0) };
    (r, status)
}

/// Kill a child (or ourselves).
pub fn kill(pid: i64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::KILL, pid as u64, 0, 0) }
}

/// Make process group `pgid` the console's foreground job (Ctrl-C kills
/// it); 0 = none.
pub fn setfg(pgid: u32) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::SETFG, pgid as u64, 0, 0) }
}

/// Switch the console (on `fd`) between raw and line mode.
pub fn console_raw(fd: u64, raw: bool) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::IOCTL, fd, 1, raw as u64) }
}

/// Bytes that can be read from `fd` without blocking.
pub fn input_available(fd: u64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::IOCTL, fd, 2, 0) }
}

/// Whether `fd` is the console.
pub fn is_console(fd: u64) -> bool {
    // SAFETY: no pointers.
    unsafe { syscall(nr::IOCTL, fd, 3, 0) == 1 }
}
