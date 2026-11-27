//! `ulib`: the user library of pt-os.
//!
//! - Program entry: `_start` collects argc/argv and calls the function
//!   named with [`entry!`]; its return value is the exit code.
//! - System calls ([`sys`]): raw wrappers returning `-errno`, and
//!   `Result`-returning ones ([`File`], [`spawn_path`], [`wait_pid`], ...).
//! - [`print!`] / [`println!`] / [`eprint!`] / [`eprintln!`]: formatted in
//!   a stack buffer and written with one system call per line.
//! - A global allocator over `sbrk`, so `alloc` (`Vec`, `String`,
//!   `format!`) works; [`prelude`] re-exports the usual names.
//! - Files ([`File`], [`read_dir`], [`stat`]), line input
//!   ([`LineReader`]), processes ([`SpawnAttr`], [`procinfo`],
//!   [`sysinfo`]).
//!
//! ```ignore
//! #![no_std]
//! #![no_main]
//! use ulib::prelude::*;
//! ulib::entry!(main);
//! fn main(args: Args) -> i32 {
//!     println!("hello, {}", args.str(1).unwrap_or("world"));
//!     0
//! }
//! ```
//!
//! System call ABI: `svc #0`, number in x8, arguments in x0-x5, result in
//! x0 (negative errno on failure). See `os/src/syscall.rs`.

#![no_std]

extern crate alloc;

mod heap;
mod io;
mod proc;
pub mod sys;

pub use io::{
    eprint_fmt, print_fmt, read_dir, read_line, stat, DirEntry, Errno, File, Kind, LineReader, Stat, O_APPEND, O_CREATE,
    O_READ, O_TRUNC, O_WRITE,
};
pub use proc::{
    is_fault_status, procinfo, spawn_path, status_text, sysinfo, try_wait_any, wait_any, wait_pid, CpuInfo, ProcInfo,
    SpawnAttr, SysInfo, STATUS_INTERRUPTED, STATUS_KILLED,
};
pub use sys::{
    chdir, close, console_raw, cpu, dup2, errno, exit, getcwd, getpid, input_available, is_console, kill, mkdir, nr, open, pipe,
    read, sbrk, setfg, sleep_ms, sleep_ns, spawn, syscall, unlink, uptime_ns, wait, write, yield_now,
};

/// Common imports for programs.
pub mod prelude {
    pub use crate::{eprint, eprintln, print, println, Args, Errno};
    pub use alloc::boxed::Box;
    pub use alloc::format;
    pub use alloc::string::{String, ToString};
    pub use alloc::vec;
    pub use alloc::vec::Vec;
}

/// Command-line arguments (`argv`, as the kernel put them on the stack).
#[derive(Clone, Copy)]
pub struct Args {
    argc: usize,
    argv: *const *const u8,
}

impl Args {
    /// Number of arguments, including the program name.
    pub fn len(&self) -> usize {
        self.argc
    }

    /// Whether there are none (never the case for programs started by
    /// the shell).
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

    /// Argument `i` as a string (`None` if missing or not UTF-8).
    pub fn str(&self, i: usize) -> Option<&'static str> {
        self.get(i).and_then(|b| core::str::from_utf8(b).ok())
    }

    /// All arguments as strings (invalid UTF-8 becomes `?`).
    pub fn iter(&self) -> impl Iterator<Item = &'static str> + '_ {
        (0..self.argc).map(|i| self.str(i).unwrap_or("?"))
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

/// Define the program's entry point: `entry!(main)` with
/// `fn main(args: Args) -> i32`. The return value is the exit code.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[no_mangle]
        fn __ulib_main(args: $crate::Args) -> i32 {
            $main(args)
        }
    };
}

core::arch::global_asm!(
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
        fn __ulib_main(args: Args) -> i32;
    }
    // SAFETY: every program defines `__ulib_main` with `entry!`.
    let code = unsafe { __ulib_main(Args { argc, argv }) };
    exit(code)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    eprintln!("panic: {}", info);
    exit(101)
}
