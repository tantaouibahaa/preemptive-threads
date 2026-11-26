//! Tiny runtime for the EL0 test programs: `_start`, raw system calls and
//! `println!`. The real user library (`ulib`) comes later; this is only what
//! the kernel tests need.
//!
//! System call ABI: `svc #0`, number in x8, arguments in x0-x5, result in
//! x0 (negative errno on failure).

#![no_std]

use core::arch::{asm, global_asm};
use core::fmt;

/// System call numbers (must match `pt_os::syscall`).
pub mod nr {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const SPAWN: u64 = 6;
    pub const WAIT: u64 = 7;
    pub const KILL: u64 = 8;
    pub const GETPID: u64 = 9;
    pub const SLEEP: u64 = 10;
    pub const YIELD: u64 = 11;
    pub const SBRK: u64 = 12;
    pub const UPTIME: u64 = 15;
    pub const CPU: u64 = 17;
}

/// errno values used by the tests.
pub mod errno {
    pub const ESRCH: i64 = 3;
    pub const EBADF: i64 = 9;
    pub const ECHILD: i64 = 10;
    pub const ENOMEM: i64 = 12;
    pub const EFAULT: i64 = 14;
    pub const ENOENT: i64 = 2;
    pub const ENOSYS: i64 = 38;
}

/// Raw system call.
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

pub fn exit(code: i32) -> ! {
    // SAFETY: exit takes no pointers.
    unsafe { syscall(nr::EXIT, code as u64, 0, 0) };
    // exit never returns; if it somehow did, stop here.
    loop {
        // SAFETY: `brk` only traps into the kernel, which kills us.
        unsafe { asm!("brk #1") };
    }
}

pub fn write(fd: u64, buf: &[u8]) -> i64 {
    // SAFETY: `buf` is a valid slice.
    unsafe { syscall(nr::WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) }
}

pub fn getpid() -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::GETPID, 0, 0, 0) }
}

pub fn yield_now() -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::YIELD, 0, 0, 0) }
}

pub fn sleep_ns(ns: u64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::SLEEP, ns, 0, 0) }
}

pub fn uptime_ns() -> u64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::UPTIME, 0, 0, 0) as u64 }
}

pub fn cpu() -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::CPU, 0, 0, 0) }
}

pub fn sbrk(incr: i64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::SBRK, incr as u64, 0, 0) }
}

/// Spawn a program. `path` and every argument must end with a NUL byte.
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

pub fn kill(pid: i64) -> i64 {
    // SAFETY: no pointers.
    unsafe { syscall(nr::KILL, pid as u64, 0, 0) }
}

/// Command-line arguments.
#[derive(Clone, Copy)]
pub struct Args {
    argc: usize,
    argv: *const *const u8,
}

impl Args {
    pub fn len(&self) -> usize {
        self.argc
    }

    pub fn is_empty(&self) -> bool {
        self.argc == 0
    }

    /// Argument `i` without its NUL terminator.
    pub fn get(&self, i: usize) -> Option<&'static [u8]> {
        if i >= self.argc {
            return None;
        }
        // SAFETY: the kernel put argc valid NUL-terminated strings on our
        // stack, which lives for the whole program.
        unsafe {
            let p = *self.argv.add(i);
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            Some(core::slice::from_raw_parts(p, n))
        }
    }
}

/// Parse a decimal or `0x` hexadecimal number.
pub fn parse_u64(s: &[u8]) -> Option<u64> {
    let (digits, radix) = match s {
        [b'0', b'x', rest @ ..] => (rest, 16),
        _ => (s, 10),
    };
    if digits.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for &c in digits {
        let d = (c as char).to_digit(radix)? as u64;
        v = v.checked_mul(radix as u64)?.checked_add(d)?;
    }
    Some(v)
}

pub struct Stdout;

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write(1, s.as_bytes());
        Ok(())
    }
}

/// Format into a stack buffer and write it with one system call, so lines
/// from different processes do not interleave.
pub fn print_fmt(args: fmt::Arguments<'_>) {
    struct Buf {
        b: [u8; 256],
        n: usize,
    }
    impl fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for &c in s.as_bytes() {
                if self.n == self.b.len() {
                    write(1, &self.b);
                    self.n = 0;
                }
                self.b[self.n] = c;
                self.n += 1;
            }
            Ok(())
        }
    }
    let mut buf = Buf { b: [0; 256], n: 0 };
    let _ = fmt::Write::write_fmt(&mut buf, args);
    write(1, &buf.b[..buf.n]);
}

#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => { $crate::print_fmt(format_args!("{}\n", format_args!($($arg)*))) };
}

global_asm!(
    r#"
.section .text._start, "ax"
.global _start
_start:
    // x0 = argc, x1 = argv, sp = 16-byte aligned (set up by the kernel)
    mov     x29, #0
    mov     x30, #0
    bl      {start}
    brk     #0
"#,
    start = sym rust_start,
);

extern "C" fn rust_start(argc: usize, argv: *const *const u8) -> ! {
    extern "Rust" {
        fn ut_main(args: Args) -> i32;
    }
    // SAFETY: every program defines `ut_main`.
    let code = unsafe { ut_main(Args { argc, argv }) };
    exit(code)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    println!("user panic: {}", info);
    exit(101)
}
