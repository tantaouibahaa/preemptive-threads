//! The virtual file system: a writable in-memory tree (ramfs), open files,
//! pipes and the console device.
//!
//! ```text
//!   process fd table          open files (shared by dup2 / spawn)        objects
//!   ┌────┐
//!   │ 0  │──────┐            ┌──────────────────────────┐
//!   │ 1  │──────┼──────────► │ File { Console }         │ ─────────► line discipline (console.rs)
//!   │ 2  │──────┘            └──────────────────────────┘
//!   │ 3  │─────────────────► │ File { Inode, pos, mode }│ ─────────► Inode::File(bytes) in the tree
//!   │ 4  │─────────────────► │ File { PipeRead }        │ ──┐
//!   └────┘                   │ File { PipeWrite }       │ ──┴──────► Pipe (4 KiB ring, wait queues)
//! ```
//!
//! An fd table entry is an `Arc<File>`: `dup2` and `spawn` share the
//! `File` (and so its offset), as in POSIX. The pipe's reader and writer
//! counts follow the `File`s: when the last `File` for one end is dropped
//! (every fd closed, every process holding it exited), readers see EOF or
//! writers get `EPIPE`.
//!
//! Paths are absolute or relative to the process's working directory; `.`
//! and `..` are resolved lexically. Directory entries are kept sorted.
//! Inode contents are guarded by blocking mutexes, so every operation here
//! must run in thread context (never in an interrupt handler).

use crate::console;
use crate::errno::Errno;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use preemptive_threads::sync::{IrqSpinLock, Mutex, WaitQueue};

/// Longest path accepted.
pub const PATH_MAX: usize = 256;
/// Longest file name.
pub const NAME_MAX: usize = 64;
/// Bytes a pipe buffers before writers block.
pub const PIPE_CAPACITY: usize = 4096;

/// `open` flags.
pub mod flags {
    /// Open for reading.
    pub const READ: u32 = 1;
    /// Open for writing.
    pub const WRITE: u32 = 2;
    /// Create the file if it does not exist.
    pub const CREATE: u32 = 4;
    /// Truncate it to zero length.
    pub const TRUNC: u32 = 8;
    /// Every write goes to the end.
    pub const APPEND: u32 = 16;
}

/// A device node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// The serial console (`/dev/console`).
    Console,
    /// Discards writes, reads give EOF (`/dev/null`).
    Null,
}

/// A node in the tree.
pub enum Inode {
    /// A regular file.
    File(Mutex<Vec<u8>>),
    /// A directory: name -> node.
    Dir(Mutex<BTreeMap<String, Arc<Inode>>>),
    /// A device.
    Device(Device),
}

/// Kind of a node, as reported by `stat` and `readdir`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Regular file.
    File = 1,
    /// Directory.
    Dir = 2,
    /// Device.
    Device = 3,
}

impl Inode {
    fn new_dir() -> Arc<Inode> {
        Arc::new(Inode::Dir(Mutex::new(BTreeMap::new())))
    }

    /// Kind of the node.
    pub fn kind(&self) -> Kind {
        match self {
            Inode::File(_) => Kind::File,
            Inode::Dir(_) => Kind::Dir,
            Inode::Device(_) => Kind::Device,
        }
    }

    /// Size in bytes (files), number of entries (directories) or 0.
    pub fn size(&self) -> usize {
        match self {
            Inode::File(d) => d.lock().len(),
            Inode::Dir(e) => e.lock().len(),
            Inode::Device(_) => 0,
        }
    }

    /// Run `f` on a file's contents.
    pub fn with_data<R>(&self, f: impl FnOnce(&[u8]) -> R) -> Result<R, Errno> {
        match self {
            Inode::File(d) => Ok(f(&d.lock())),
            Inode::Dir(_) => Err(Errno::EISDIR),
            Inode::Device(_) => Err(Errno::EINVAL),
        }
    }

    fn entries(&self) -> Result<&Mutex<BTreeMap<String, Arc<Inode>>>, Errno> {
        match self {
            Inode::Dir(e) => Ok(e),
            _ => Err(Errno::ENOTDIR),
        }
    }
}

static ROOT: IrqSpinLock<Option<Arc<Inode>>> = IrqSpinLock::new(None);

/// The root directory (created on first use, with `/dev`).
pub fn root() -> Arc<Inode> {
    let mut r = ROOT.lock();
    if let Some(root) = r.as_ref() {
        return root.clone();
    }
    let root = Inode::new_dir();
    let dev = Inode::new_dir();
    if let (Inode::Dir(rd), Inode::Dir(dd)) = (&*root, &*dev) {
        let mut d = dd.lock();
        d.insert(String::from("console"), Arc::new(Inode::Device(Device::Console)));
        d.insert(String::from("null"), Arc::new(Inode::Device(Device::Null)));
        rd.lock().insert(String::from("dev"), dev.clone());
    }
    *r = Some(root.clone());
    root
}

/// Make `path` absolute (relative to `cwd`) and resolve `.` and `..`.
/// The result starts with `/` and has no trailing `/` (except the root).
pub fn normalize(cwd: &str, path: &str) -> Result<String, Errno> {
    if path.is_empty() {
        return Err(Errno::ENOENT);
    }
    if path.len() > PATH_MAX {
        return Err(Errno::ENAMETOOLONG);
    }
    let mut parts: Vec<&str> = Vec::new();
    let joined = if path.starts_with('/') { [path, ""] } else { [cwd, path] };
    for piece in joined {
        for c in piece.split('/') {
            match c {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                name => {
                    if name.len() > NAME_MAX {
                        return Err(Errno::ENAMETOOLONG);
                    }
                    parts.push(name);
                }
            }
        }
    }
    let mut out = String::new();
    for p in &parts {
        out.push('/');
        out.push_str(p);
    }
    if out.is_empty() {
        out.push('/');
    }
    Ok(out)
}

/// Look up an absolute, normalized path.
pub fn lookup(abs: &str) -> Result<Arc<Inode>, Errno> {
    let mut node = root();
    for c in abs.split('/').filter(|c| !c.is_empty()) {
        let next = node.entries()?.lock().get(c).cloned().ok_or(Errno::ENOENT)?;
        node = next;
    }
    Ok(node)
}

/// Split an absolute, normalized path into its parent directory and last
/// component.
fn parent_of(abs: &str) -> Result<(Arc<Inode>, &str), Errno> {
    let i = abs.rfind('/').ok_or(Errno::EINVAL)?;
    let name = &abs[i + 1..];
    if name.is_empty() {
        return Err(Errno::EEXIST); // the root
    }
    let dir = lookup(if i == 0 { "/" } else { &abs[..i] })?;
    dir.entries()?;
    Ok((dir, name))
}

/// Create a directory.
pub fn mkdir(abs: &str) -> Result<Arc<Inode>, Errno> {
    let (dir, name) = parent_of(abs)?;
    let mut e = dir.entries()?.lock();
    if e.contains_key(name) {
        return Err(Errno::EEXIST);
    }
    let d = Inode::new_dir();
    e.insert(String::from(name), d.clone());
    Ok(d)
}

/// Create every missing directory along `abs`.
pub fn mkdir_all(abs: &str) -> Result<Arc<Inode>, Errno> {
    let mut node = root();
    for c in abs.split('/').filter(|c| !c.is_empty()) {
        let next = {
            let mut e = node.entries()?.lock();
            e.entry(String::from(c)).or_insert_with(Inode::new_dir).clone()
        };
        node = next;
    }
    node.entries()?;
    Ok(node)
}

/// Create (or replace the contents of) a regular file.
pub fn write_file(abs: &str, data: &[u8]) -> Result<(), Errno> {
    let (dir, name) = parent_of(abs)?;
    let mut e = dir.entries()?.lock();
    match e.get(name) {
        Some(n) => match &**n {
            Inode::File(d) => {
                let mut d = d.lock();
                d.clear();
                d.extend_from_slice(data);
                Ok(())
            }
            _ => Err(Errno::EISDIR),
        },
        None => {
            e.insert(String::from(name), Arc::new(Inode::File(Mutex::new(data.to_vec()))));
            Ok(())
        }
    }
}

/// Read a whole regular file.
pub fn read_file(abs: &str) -> Result<Vec<u8>, Errno> {
    lookup(abs)?.with_data(|d| d.to_vec())
}

/// Remove a file or an empty directory.
pub fn unlink(abs: &str) -> Result<(), Errno> {
    let (dir, name) = parent_of(abs)?;
    let mut e = dir.entries()?.lock();
    let n = e.get(name).ok_or(Errno::ENOENT)?;
    if let Inode::Dir(d) = &**n {
        if !d.lock().is_empty() {
            return Err(Errno::ENOTEMPTY);
        }
    }
    e.remove(name);
    Ok(())
}

/// Open `abs` with [`flags`]. Directories open read-only (for
/// `readdir`).
pub fn open(abs: &str, fl: u32) -> Result<Arc<File>, Errno> {
    let readable = fl & flags::READ != 0;
    let writable = fl & (flags::WRITE | flags::APPEND) != 0;
    if !readable && !writable {
        return Err(Errno::EINVAL);
    }
    let node = match lookup(abs) {
        Ok(n) => n,
        Err(Errno::ENOENT) if fl & flags::CREATE != 0 => {
            let (dir, name) = parent_of(abs)?;
            let mut e = dir.entries()?.lock();
            e.entry(String::from(name)).or_insert_with(|| Arc::new(Inode::File(Mutex::new(Vec::new())))).clone()
        }
        Err(e) => return Err(e),
    };
    let kind = match &*node {
        Inode::Dir(_) if writable => return Err(Errno::EISDIR),
        Inode::Device(Device::Console) => FileKind::Console,
        Inode::Device(Device::Null) => FileKind::Null,
        Inode::File(d) => {
            if fl & flags::TRUNC != 0 && writable {
                d.lock().clear();
            }
            FileKind::Inode(node.clone())
        }
        Inode::Dir(_) => FileKind::Inode(node.clone()),
    };
    Ok(Arc::new(File { kind, pos: IrqSpinLock::new(0), readable, writable, append: fl & flags::APPEND != 0 }))
}

// --- open files --------------------------------------------------------------

/// What an open file refers to.
pub enum FileKind {
    /// A file or directory in the tree.
    Inode(Arc<Inode>),
    /// The console (line discipline input, serial output).
    Console,
    /// `/dev/null`.
    Null,
    /// Read end of a pipe.
    PipeRead(Arc<Pipe>),
    /// Write end of a pipe.
    PipeWrite(Arc<Pipe>),
    /// Console output that is also recorded (kernel tests).
    Capture(Arc<IrqSpinLock<Vec<u8>>>),
}

/// An open file: shared by every fd that refers to it.
pub struct File {
    kind: FileKind,
    pos: IrqSpinLock<u64>,
    readable: bool,
    writable: bool,
    append: bool,
}

impl File {
    fn new(kind: FileKind, readable: bool, writable: bool) -> Arc<File> {
        Arc::new(File { kind, pos: IrqSpinLock::new(0), readable, writable, append: false })
    }

    /// The console, for reading and writing.
    pub fn console() -> Arc<File> {
        File::new(FileKind::Console, true, true)
    }

    /// Console output that is also appended to `buf`.
    pub fn capture(buf: Arc<IrqSpinLock<Vec<u8>>>) -> Arc<File> {
        File::new(FileKind::Capture(buf), false, true)
    }

    /// What it refers to.
    pub fn kind(&self) -> &FileKind {
        &self.kind
    }

    /// Read up to `buf.len()` bytes. Blocks (interruptibly) on the console
    /// and pipes; 0 means end of file.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        if !self.readable {
            return Err(Errno::EBADF);
        }
        match &self.kind {
            FileKind::Inode(n) => match &**n {
                Inode::File(d) => {
                    let d = d.lock();
                    let mut pos = self.pos.lock();
                    let start = (*pos as usize).min(d.len());
                    let k = buf.len().min(d.len() - start);
                    buf[..k].copy_from_slice(&d[start..start + k]);
                    *pos += k as u64;
                    Ok(k)
                }
                Inode::Dir(_) => Err(Errno::EISDIR),
                Inode::Device(_) => Err(Errno::EINVAL),
            },
            FileKind::Console => console::read(buf),
            FileKind::Null => Ok(0),
            FileKind::PipeRead(p) => p.read(buf),
            FileKind::PipeWrite(_) | FileKind::Capture(_) => Err(Errno::EBADF),
        }
    }

    /// Write `data`. Returns how much was written (pipes: all of it unless
    /// interrupted or the readers went away).
    pub fn write(&self, data: &[u8]) -> Result<usize, Errno> {
        if !self.writable {
            return Err(Errno::EBADF);
        }
        match &self.kind {
            FileKind::Inode(n) => match &**n {
                Inode::File(d) => {
                    let mut d = d.lock();
                    let mut pos = self.pos.lock();
                    if self.append {
                        *pos = d.len() as u64;
                    }
                    let start = *pos as usize;
                    let end = start.checked_add(data.len()).ok_or(Errno::EFBIG)?;
                    if end > MAX_FILE {
                        return Err(Errno::EFBIG);
                    }
                    if d.len() < end {
                        d.resize(end, 0);
                    }
                    d[start..end].copy_from_slice(data);
                    *pos = end as u64;
                    Ok(data.len())
                }
                Inode::Dir(_) => Err(Errno::EISDIR),
                Inode::Device(_) => Err(Errno::EINVAL),
            },
            FileKind::Console => {
                console::write(data);
                Ok(data.len())
            }
            FileKind::Capture(b) => {
                b.lock().extend_from_slice(data);
                console::write(data);
                Ok(data.len())
            }
            FileKind::Null => Ok(data.len()),
            FileKind::PipeWrite(p) => p.write(data),
            FileKind::PipeRead(_) => Err(Errno::EBADF),
        }
    }

    /// Next directory entry: `(kind, name)`, or `None` at the end.
    pub fn readdir(&self) -> Result<Option<(Kind, String)>, Errno> {
        let FileKind::Inode(n) = &self.kind else { return Err(Errno::ENOTDIR) };
        let e = n.entries()?.lock();
        let mut pos = self.pos.lock();
        let r = e.iter().nth(*pos as usize).map(|(k, v)| (v.kind(), k.clone()));
        if r.is_some() {
            *pos += 1;
        }
        Ok(r)
    }

    /// Bytes that can be read without blocking (console and pipes).
    pub fn available(&self) -> Result<usize, Errno> {
        match &self.kind {
            FileKind::Console => Ok(console::available()),
            FileKind::PipeRead(p) => Ok(p.state.lock().buf.len()),
            FileKind::Inode(n) => {
                let size = n.size() as u64;
                Ok(size.saturating_sub(*self.pos.lock()) as usize)
            }
            _ => Ok(0),
        }
    }
}

/// Largest regular file (keeps a runaway writer from eating the heap).
pub const MAX_FILE: usize = 8 * 1024 * 1024;

impl Drop for File {
    fn drop(&mut self) {
        match &self.kind {
            FileKind::PipeRead(p) => p.close_end(false),
            FileKind::PipeWrite(p) => p.close_end(true),
            _ => {}
        }
    }
}

// --- pipes -------------------------------------------------------------------

struct PipeState {
    buf: VecDeque<u8>,
    readers: usize,
    writers: usize,
}

/// A bounded byte pipe.
pub struct Pipe {
    state: IrqSpinLock<PipeState>,
    readable: WaitQueue,
    writable: WaitQueue,
}

/// Create a pipe: `(read end, write end)`.
pub fn pipe() -> (Arc<File>, Arc<File>) {
    let p = Arc::new(Pipe {
        state: IrqSpinLock::new(PipeState { buf: VecDeque::with_capacity(PIPE_CAPACITY), readers: 1, writers: 1 }),
        readable: WaitQueue::new(),
        writable: WaitQueue::new(),
    });
    (File::new(FileKind::PipeRead(p.clone()), true, false), File::new(FileKind::PipeWrite(p), false, true))
}

impl Pipe {
    fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            self.readable
                .wait_until_interruptible(|| {
                    let st = self.state.lock();
                    !st.buf.is_empty() || st.writers == 0
                })
                .map_err(|_| Errno::EINTR)?;
            let n = {
                let mut st = self.state.lock();
                if st.buf.is_empty() {
                    if st.writers == 0 {
                        return Ok(0);
                    }
                    continue; // another reader got there first
                }
                let n = buf.len().min(st.buf.len());
                for (dst, src) in buf.iter_mut().zip(st.buf.drain(..n)) {
                    *dst = src;
                }
                n
            };
            self.writable.wake_all();
            return Ok(n);
        }
    }

    fn write(&self, data: &[u8]) -> Result<usize, Errno> {
        let mut done = 0;
        while done < data.len() {
            let waited = self.writable.wait_until_interruptible(|| {
                let st = self.state.lock();
                st.readers == 0 || st.buf.len() < PIPE_CAPACITY
            });
            if waited.is_err() {
                return if done > 0 { Ok(done) } else { Err(Errno::EINTR) };
            }
            {
                let mut st = self.state.lock();
                if st.readers == 0 {
                    return if done > 0 { Ok(done) } else { Err(Errno::EPIPE) };
                }
                let n = (PIPE_CAPACITY - st.buf.len()).min(data.len() - done);
                st.buf.extend(&data[done..done + n]);
                done += n;
            }
            self.readable.wake_all();
        }
        Ok(done)
    }

    fn close_end(&self, writer: bool) {
        {
            let mut st = self.state.lock();
            if writer {
                st.writers -= 1;
            } else {
                st.readers -= 1;
            }
        }
        self.readable.wake_all();
        self.writable.wake_all();
    }
}

// --- fd tables ---------------------------------------------------------------

/// Most open fds per process.
pub const MAX_FDS: usize = 32;

/// A process's file descriptors.
#[derive(Clone, Default)]
pub struct FdTable {
    fds: Vec<Option<Arc<File>>>,
}

impl FdTable {
    /// Empty table.
    pub fn new() -> Self {
        Self { fds: Vec::new() }
    }

    /// Table with the console as fds 0, 1 and 2.
    pub fn console() -> Self {
        let c = File::console();
        Self { fds: alloc::vec![Some(c.clone()), Some(c.clone()), Some(c)] }
    }

    /// The file behind `fd`.
    pub fn get(&self, fd: usize) -> Result<Arc<File>, Errno> {
        self.fds.get(fd).and_then(|f| f.clone()).ok_or(Errno::EBADF)
    }

    /// Put `f` at `fd` (closing what was there).
    pub fn set(&mut self, fd: usize, f: Arc<File>) -> Result<Option<Arc<File>>, Errno> {
        if fd >= MAX_FDS {
            return Err(Errno::EBADF);
        }
        if self.fds.len() <= fd {
            self.fds.resize(fd + 1, None);
        }
        Ok(self.fds[fd].replace(f))
    }

    /// Put `f` in the lowest free slot.
    pub fn insert(&mut self, f: Arc<File>) -> Result<usize, Errno> {
        let fd = match self.fds.iter().position(|s| s.is_none()) {
            Some(i) => i,
            None if self.fds.len() < MAX_FDS => {
                self.fds.push(None);
                self.fds.len() - 1
            }
            None => return Err(Errno::EMFILE),
        };
        self.fds[fd] = Some(f);
        Ok(fd)
    }

    /// Remove `fd`, returning its file (drop it outside any lock).
    pub fn remove(&mut self, fd: usize) -> Result<Arc<File>, Errno> {
        self.fds.get_mut(fd).and_then(|s| s.take()).ok_or(Errno::EBADF)
    }

    /// Close everything.
    pub fn clear(&mut self) -> Vec<Arc<File>> {
        self.fds.drain(..).flatten().collect()
    }

    /// Number of open fds.
    pub fn count(&self) -> usize {
        self.fds.iter().filter(|f| f.is_some()).count()
    }
}
