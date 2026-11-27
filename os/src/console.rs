//! The console: serial output for processes, and input through a line
//! discipline.
//!
//! ```text
//!  PL011 RX IRQ (core 0) ──► rx_byte(): 256-byte ring ──► wake ─┐
//!  inject() (tests) ───────────────────────────────────────────┤
//!                                                               ▼
//!  "console" kernel thread (high priority): line discipline
//!     canonical mode: echo, backspace/DEL erase, Enter completes the line,
//!                     Ctrl-D: end of file (or flushes a partial line),
//!                     Ctrl-U: erase the line
//!     raw mode:       bytes go straight through, no echo
//!     both:           Ctrl-C kills the foreground process group (status -2);
//!                     with no foreground job it abandons the line
//!        │
//!        ▼ completed lines / raw bytes / EOF markers
//!  input queue ──► read(0): blocks (interruptibly) until data or EOF;
//!                  canonical reads return at most one line
//! ```
//!
//! The IRQ handler only queues bytes; everything else (echo, which takes
//! the console lock, and killing processes, which takes kernel locks and
//! may free memory) runs on the console thread.

use crate::errno::Errno;
use crate::process::{self, Pid};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use preemptive_threads::sync::{IrqSpinLock, WaitQueue};
use preemptive_threads::{priority, Builder};

/// Exit status of a process killed with Ctrl-C (`-SIGINT`).
pub const STATUS_INTERRUPTED: i32 = -2;
/// Longest line the line discipline buffers.
pub const LINE_MAX: usize = 1024;

const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const CTRL_U: u8 = 0x15;
const BS: u8 = 0x08;
const DEL: u8 = 0x7f;
/// Marks an end of file in the input queue.
const EOF: u16 = 0x100;

struct Ring {
    buf: [u8; 256],
    head: usize,
    len: usize,
}

/// Bytes received but not yet processed (filled in IRQ context).
static RX: IrqSpinLock<Ring> = IrqSpinLock::new(Ring { buf: [0; 256], head: 0, len: 0 });
static RX_READY: WaitQueue = WaitQueue::new();

struct Input {
    /// Processed input waiting for `read` (bytes, or `EOF`).
    queue: VecDeque<u16>,
    /// The line being edited (canonical mode).
    line: Vec<u8>,
    raw: bool,
    last_cr: bool,
}

static INPUT: IrqSpinLock<Input> =
    IrqSpinLock::new(Input { queue: VecDeque::new(), line: Vec::new(), raw: false, last_cr: false });
static READABLE: WaitQueue = WaitQueue::new();
/// Foreground process group (0 = none): what Ctrl-C kills.
static FOREGROUND: AtomicU32 = AtomicU32::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
/// Everything written to the console by processes and echo (tests).
static CAPTURE: IrqSpinLock<Option<Vec<u8>>> = IrqSpinLock::new(None);

/// Start the console thread and enable the UART receive interrupt.
pub fn init() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    Builder::new()
        .name("console")
        .priority(priority::HIGH)
        .spawn(console_thread)
        .expect("console thread");
    preemptive_threads::console::set_rx_handler(rx_byte);
}

/// Called in IRQ context for every received byte.
fn rx_byte(b: u8) {
    {
        let mut r = RX.lock();
        if r.len < r.buf.len() {
            let i = (r.head + r.len) % r.buf.len();
            r.buf[i] = b;
            r.len += 1;
        }
    }
    RX_READY.wake_one();
}

/// Feed bytes into the input path as if they had been typed (tests).
pub fn inject(bytes: &[u8]) {
    for &b in bytes {
        // Do not overrun the ring: wait for the console thread to catch up.
        while RX.lock().len >= 200 {
            preemptive_threads::thread::sleep(preemptive_threads::Duration::from_millis(1));
        }
        rx_byte(b);
    }
}

fn console_thread() {
    loop {
        let mut chunk = [0u8; 64];
        let mut n = 0;
        RX_READY.wait_until(|| {
            let mut r = RX.lock();
            while r.len > 0 && n < chunk.len() {
                chunk[n] = r.buf[r.head];
                r.head = (r.head + 1) % r.buf.len();
                r.len -= 1;
                n += 1;
            }
            n > 0
        });
        for &b in &chunk[..n] {
            discipline(b);
        }
    }
}

/// Handle one input byte.
fn discipline(b: u8) {
    if b == CTRL_C {
        interrupt_foreground();
        return;
    }
    let mut echo: Vec<u8> = Vec::new();
    let mut wake = false;
    {
        let mut inp = INPUT.lock();
        let cr = inp.last_cr;
        inp.last_cr = b == b'\r';
        if inp.raw {
            inp.queue.push_back(b as u16);
            wake = true;
        } else {
            match b {
                b'\n' if cr => {} // "\r\n": already handled at the \r
                b'\r' | b'\n' => {
                    echo.push(b'\n');
                    let mut line = core::mem::take(&mut inp.line);
                    line.push(b'\n');
                    inp.queue.extend(line.iter().map(|&c| c as u16));
                    wake = true;
                }
                CTRL_D => {
                    let line = core::mem::take(&mut inp.line);
                    if line.is_empty() {
                        inp.queue.push_back(EOF);
                    } else {
                        inp.queue.extend(line.iter().map(|&c| c as u16));
                    }
                    wake = true;
                }
                BS | DEL => {
                    if inp.line.pop().is_some() {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                CTRL_U => {
                    for _ in 0..inp.line.len() {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                    inp.line.clear();
                }
                0x20..=0x7e | b'\t' | 0x80.. if inp.line.len() < LINE_MAX => {
                    inp.line.push(b);
                    echo.push(b);
                }
                _ => {} // other control characters, or the line is full
            }
        }
    }
    if !echo.is_empty() {
        write(&echo);
    }
    if wake {
        READABLE.wake_all();
    }
}

/// Ctrl-C: kill the foreground job, or abandon the line being typed.
fn interrupt_foreground() {
    write(b"^C\n");
    let fg = FOREGROUND.load(Ordering::Acquire);
    let killed = fg != 0 && process::kill_group(fg, STATUS_INTERRUPTED) > 0;
    let mut inp = INPUT.lock();
    inp.line.clear();
    if !killed && !inp.raw {
        // Nobody to kill: hand the reader an empty line, so a shell
        // prints a fresh prompt.
        inp.queue.push_back(b'\n' as u16);
        drop(inp);
        READABLE.wake_all();
    }
}

/// Read console input (see the module docs). Returns 0 at end of file.
pub fn read(buf: &mut [u8]) -> Result<usize, Errno> {
    if buf.is_empty() {
        return Ok(0);
    }
    READABLE.wait_until_interruptible(|| !INPUT.lock().queue.is_empty()).map_err(|_| Errno::EINTR)?;
    let mut inp = INPUT.lock();
    if inp.queue.front() == Some(&EOF) {
        inp.queue.pop_front();
        return Ok(0);
    }
    let raw = inp.raw;
    let mut n = 0;
    while n < buf.len() {
        match inp.queue.front() {
            Some(&c) if c != EOF => {
                inp.queue.pop_front();
                buf[n] = c as u8;
                n += 1;
                if c == b'\n' as u16 && !raw {
                    break;
                }
            }
            _ => break,
        }
    }
    Ok(n)
}

/// Bytes of input ready to be read.
pub fn available() -> usize {
    INPUT.lock().queue.iter().take_while(|&&c| c != EOF).count()
}

/// Write process output (or echo) to the serial console.
pub fn write(bytes: &[u8]) {
    if let Some(c) = CAPTURE.lock().as_mut() {
        c.extend_from_slice(bytes);
    }
    preemptive_threads::console::write_bytes(bytes);
}

/// Switch between canonical (line) mode and raw mode. The partial line is
/// dropped.
pub fn set_raw(raw: bool) {
    let mut inp = INPUT.lock();
    inp.raw = raw;
    inp.line.clear();
}

/// Make process group `pgid` the foreground job (0 = none; this also puts
/// the console back into canonical mode, in case the job left it raw).
pub fn set_foreground(pgid: Pid) {
    FOREGROUND.store(pgid, Ordering::Release);
    if pgid == 0 {
        set_raw(false);
    }
}

/// The foreground process group.
pub fn foreground() -> Pid {
    FOREGROUND.load(Ordering::Acquire)
}

/// Start recording console output (tests).
pub fn capture_start() {
    *CAPTURE.lock() = Some(Vec::new());
}

/// Recorded console output so far.
pub fn captured() -> Vec<u8> {
    CAPTURE.lock().clone().unwrap_or_default()
}

/// Stop recording and return what was recorded.
pub fn capture_stop() -> Vec<u8> {
    CAPTURE.lock().take().unwrap_or_default()
}

/// Drop any pending input (tests).
pub fn flush_input() {
    let mut inp = INPUT.lock();
    inp.queue.clear();
    inp.line.clear();
}
