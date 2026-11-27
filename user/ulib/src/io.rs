//! Output macros, errors, files and line input.

use crate::sys::{self, errno, nr};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Open for reading.
pub const O_READ: u32 = 1;
/// Open for writing.
pub const O_WRITE: u32 = 2;
/// Create if missing.
pub const O_CREATE: u32 = 4;
/// Truncate to zero length.
pub const O_TRUNC: u32 = 8;
/// Append every write.
pub const O_APPEND: u32 = 16;

/// A failed system call's error number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Errno(pub i64);

impl Errno {
    /// `Ok(v)` for a non-negative result, the error otherwise.
    pub fn check(r: i64) -> Result<i64, Errno> {
        if r < 0 {
            Err(Errno(-r))
        } else {
            Ok(r)
        }
    }

    /// Short description, like `strerror`.
    pub fn text(self) -> &'static str {
        match self.0 {
            errno::EPERM => "operation not permitted",
            errno::ENOENT => "no such file or directory",
            errno::ESRCH => "no such process",
            errno::EINTR => "interrupted",
            errno::E2BIG => "argument list too long",
            errno::ENOEXEC => "not an executable",
            errno::EBADF => "bad file descriptor",
            errno::ECHILD => "no child processes",
            errno::EAGAIN => "too many processes",
            errno::ENOMEM => "out of memory",
            errno::EFAULT => "bad address",
            errno::EEXIST => "file exists",
            errno::ENOTDIR => "not a directory",
            errno::EISDIR => "is a directory",
            errno::EINVAL => "invalid argument",
            errno::EMFILE => "too many open files",
            errno::ENOTTY => "not a terminal",
            errno::EFBIG => "file too large",
            errno::EPIPE => "broken pipe",
            errno::ERANGE => "result too large",
            errno::ENAMETOOLONG => "name too long",
            errno::ENOSYS => "not implemented",
            errno::ENOTEMPTY => "directory not empty",
            _ => "error",
        }
    }
}

impl fmt::Display for Errno {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text())
    }
}

/// Write all of `data` to `fd`. A process whose output pipe has no reader
/// left exits quietly (status 141), like a Unix process killed by SIGPIPE.
fn write_all_fd(fd: u64, mut data: &[u8]) {
    while !data.is_empty() {
        let r = sys::write(fd, data);
        if r == -errno::EPIPE {
            sys::exit(141);
        }
        if r <= 0 {
            return;
        }
        data = &data[r as usize..];
    }
}

struct LineBuf {
    fd: u64,
    b: [u8; 256],
    n: usize,
}

impl fmt::Write for LineBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &c in s.as_bytes() {
            if self.n == self.b.len() {
                write_all_fd(self.fd, &self.b);
                self.n = 0;
            }
            self.b[self.n] = c;
            self.n += 1;
        }
        Ok(())
    }
}

fn fmt_to(fd: u64, args: fmt::Arguments<'_>) {
    let mut buf = LineBuf { fd, b: [0; 256], n: 0 };
    let _ = fmt::Write::write_fmt(&mut buf, args);
    write_all_fd(fd, &buf.b[..buf.n]);
}

/// Format to standard output with one system call per 256 bytes, so lines
/// from different processes do not interleave.
pub fn print_fmt(args: fmt::Arguments<'_>) {
    fmt_to(1, args);
}

/// Format to standard error.
pub fn eprint_fmt(args: fmt::Arguments<'_>) {
    fmt_to(2, args);
}

/// Print to standard output.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::print_fmt(format_args!($($arg)*)) };
}

/// Print a line to standard output.
#[macro_export]
macro_rules! println {
    () => { $crate::print_fmt(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::print_fmt(format_args!("{}\n", format_args!($($arg)*))) };
}

/// Print to standard error.
#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => { $crate::eprint_fmt(format_args!($($arg)*)) };
}

/// Print a line to standard error.
#[macro_export]
macro_rules! eprintln {
    () => { $crate::eprint_fmt(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::eprint_fmt(format_args!("{}\n", format_args!($($arg)*))) };
}

/// An open file descriptor, closed on drop.
pub struct File {
    fd: i32,
}

impl File {
    /// Open `path` with `O_*` flags.
    pub fn open(path: &str, flags: u32) -> Result<File, Errno> {
        Errno::check(sys::open(path, flags)).map(|fd| File { fd: fd as i32 })
    }

    /// Open for reading.
    pub fn read_only(path: &str) -> Result<File, Errno> {
        File::open(path, O_READ)
    }

    /// Create (or truncate) for writing.
    pub fn create(path: &str) -> Result<File, Errno> {
        File::open(path, O_WRITE | O_CREATE | O_TRUNC)
    }

    /// Take ownership of an open fd (it is closed when the `File` drops).
    pub fn from_fd(fd: i32) -> File {
        File { fd }
    }

    /// The fd number.
    pub fn fd(&self) -> i32 {
        self.fd
    }

    /// Give up ownership: the fd stays open.
    pub fn into_fd(self) -> i32 {
        let fd = self.fd;
        core::mem::forget(self);
        fd
    }

    /// Read some bytes; 0 = end of file.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        Errno::check(sys::read(self.fd as u64, buf)).map(|n| n as usize)
    }

    /// Read everything that is left.
    pub fn read_to_end(&self) -> Result<Vec<u8>, Errno> {
        read_all_fd(self.fd as u64)
    }

    /// Read everything that is left as text (invalid UTF-8 is replaced).
    pub fn read_to_string(&self) -> Result<String, Errno> {
        self.read_to_end().map(|v| String::from_utf8_lossy(&v).into_owned())
    }

    /// Write all of `data`.
    pub fn write_all(&self, mut data: &[u8]) -> Result<(), Errno> {
        while !data.is_empty() {
            let n = Errno::check(sys::write(self.fd as u64, data))? as usize;
            if n == 0 {
                return Err(Errno(errno::EPIPE));
            }
            data = &data[n..];
        }
        Ok(())
    }
}

impl Drop for File {
    fn drop(&mut self) {
        sys::close(self.fd as u64);
    }
}

/// Read `fd` to its end.
pub fn read_all_fd(fd: u64) -> Result<Vec<u8>, Errno> {
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = Errno::check(sys::read(fd, &mut buf))? as usize;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// Reads an fd line by line.
pub struct LineReader {
    fd: u64,
    buf: Vec<u8>,
    eof: bool,
}

impl LineReader {
    /// Read lines from `fd`.
    pub fn new(fd: i32) -> Self {
        LineReader { fd: fd as u64, buf: Vec::new(), eof: false }
    }

    /// The next line without its `\n`; `None` at end of file. A read error
    /// (e.g. `EINTR`) is reported as end of file.
    pub fn read_line(&mut self) -> Option<String> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let rest = self.buf.split_off(i + 1);
                let mut line = core::mem::replace(&mut self.buf, rest);
                line.pop();
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            if self.eof {
                if self.buf.is_empty() {
                    return None;
                }
                let line = core::mem::take(&mut self.buf);
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            let mut chunk = [0u8; 512];
            let n = sys::read(self.fd, &mut chunk);
            if n <= 0 {
                self.eof = true;
            } else {
                self.buf.extend_from_slice(&chunk[..n as usize]);
            }
        }
    }
}

/// Read one line from standard input (the console delivers one line per
/// read). `None` at end of file (Ctrl-D).
pub fn read_line() -> Option<String> {
    let mut buf = [0u8; 1024];
    let n = sys::read(0, &mut buf);
    if n <= 0 {
        return None;
    }
    let mut s = &buf[..n as usize];
    if s.last() == Some(&b'\n') {
        s = &s[..s.len() - 1];
    }
    Some(String::from_utf8_lossy(s).into_owned())
}

/// Kind of a directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Regular file.
    File,
    /// Directory.
    Dir,
    /// Device.
    Device,
}

impl Kind {
    fn from_raw(k: u32) -> Kind {
        match k {
            2 => Kind::Dir,
            3 => Kind::Device,
            _ => Kind::File,
        }
    }
}

/// One entry of [`read_dir`].
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Its kind.
    pub kind: Kind,
    /// Its name.
    pub name: String,
}

/// List a directory (sorted by name).
pub fn read_dir(path: &str) -> Result<Vec<DirEntry>, Errno> {
    let dir = File::read_only(path)?;
    let mut out = Vec::new();
    let mut buf = [0u8; 128];
    loop {
        // SAFETY: `buf` is a valid, writable buffer.
        let n = Errno::check(unsafe {
            sys::syscall(nr::READDIR, dir.fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64)
        })? as usize;
        if n == 0 {
            return Ok(out);
        }
        out.push(DirEntry {
            kind: Kind::from_raw(buf[0] as u32),
            name: String::from_utf8_lossy(&buf[1..n]).into_owned(),
        });
    }
}

/// What [`stat`] reports.
#[derive(Debug, Clone, Copy)]
pub struct Stat {
    /// File, directory or device.
    pub kind: Kind,
    /// Bytes (files) or entries (directories).
    pub size: u64,
}

/// Information about `path`.
pub fn stat(path: &str) -> Result<Stat, Errno> {
    let mut p = [0u8; 260];
    let n = path.len().min(259);
    p[..n].copy_from_slice(&path.as_bytes()[..n]);
    let mut out = [0u8; 16];
    // SAFETY: NUL-terminated path, 16-byte output buffer.
    Errno::check(unsafe { sys::syscall(nr::STAT, p.as_ptr() as u64, out.as_mut_ptr() as u64, 0) })?;
    Ok(Stat {
        kind: Kind::from_raw(u32::from_le_bytes(out[..4].try_into().unwrap())),
        size: u64::from_le_bytes(out[8..].try_into().unwrap()),
    })
}
