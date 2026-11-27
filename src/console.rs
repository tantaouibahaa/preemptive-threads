//! Serial console (`println!`-style macros over the board's PL011).
//!
//! Output is polled, so a long write takes real time: at 115200 baud a
//! byte takes 87 us and a 4 KiB write a third of a second. It is therefore
//! sent in chunks of at most [`CHUNK`] bytes, with the console lock (and
//! the IRQ mask it implies) held for one chunk only, and a writer waiting
//! for the lock spins with its interrupts as they were, not masked.
//!
//! Each `print!`, `println!` or [`write_bytes`] call still comes out as one
//! unit: between its chunks the call keeps ownership of the console, and
//! other writers wait for it. Two exceptions keep this from ever stalling
//! the system: a writer running with IRQs masked (interrupt handlers, code
//! holding an [`IrqSpinLock`]) does not wait for an owner, and nobody waits
//! for an owner that has made no progress for [`STALL_MS`] ms (it was
//! preempted, for instance by the waiter itself on the same core). In those
//! cases lines can interleave, at a chunk boundary.
//!
//! Input is interrupt driven: [`set_rx_handler`] installs a function that
//! is called in IRQ context (on core 0) for every received byte.

use crate::arch;
use crate::platform::UART;
use crate::sync::spin::{IrqSpinLock, IrqSpinLockGuard};
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

/// Most bytes sent to the UART per hold of the console lock.
pub const CHUNK: usize = 64;
/// A console owner idle for longer than this no longer holds others up.
pub const STALL_MS: u64 = 10;

/// Who owns the console between chunks.
struct Owner {
    /// Ticket of the writer whose output is in progress (0 = none).
    ticket: u64,
    /// Counter value when it last made progress.
    last: u64,
}

static LOCK: IrqSpinLock<Owner> = IrqSpinLock::new(Owner { ticket: 0, last: 0 });
static NEXT_TICKET: AtomicU64 = AtomicU64::new(1);
static RX_HANDLER: IrqSpinLock<Option<fn(u8)>> = IrqSpinLock::new(None);

/// One `print!` (or [`write_bytes`]) call: owns the console from its first
/// chunk to its last.
struct Writer {
    ticket: u64,
    /// Whether to wait for another writer's output in progress.
    polite: bool,
}

impl Writer {
    fn new() -> Self {
        Self { ticket: NEXT_TICKET.fetch_add(1, Ordering::Relaxed), polite: arch::irqs_enabled() }
    }

    /// Take the console lock, waiting with IRQs in their current state.
    fn lock_console() -> IrqSpinLockGuard<'static, Owner> {
        loop {
            if let Some(g) = LOCK.try_lock() {
                return g;
            }
            while LOCK.is_locked() {
                core::hint::spin_loop();
            }
        }
    }

    /// Take the console lock and the ownership of the console.
    fn acquire(&self) -> IrqSpinLockGuard<'static, Owner> {
        let stall = arch::timer::ns_to_ticks(STALL_MS * 1_000_000);
        loop {
            let mut g = Self::lock_console();
            let now = arch::timer::counter();
            if g.ticket == 0 || g.ticket == self.ticket || !self.polite || now.wrapping_sub(g.last) > stall {
                g.ticket = self.ticket;
                g.last = now;
                return g;
            }
            drop(g);
            core::hint::spin_loop();
        }
    }

    /// Send `bytes`, translating `\n` to `\r\n`, [`CHUNK`] bytes at a time.
    fn write(&self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let mut g = self.acquire();
            let mut sent = 0;
            while let Some((&b, rest)) = bytes.split_first() {
                if sent + 2 > CHUNK {
                    break;
                }
                if b == b'\n' {
                    UART.putc(b'\r');
                    sent += 1;
                }
                UART.putc(b);
                sent += 1;
                bytes = rest;
            }
            g.last = arch::timer::counter();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let mut g = Self::lock_console();
        if g.ticket == self.ticket {
            g.ticket = 0;
        }
    }
}

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write(s.as_bytes());
        Ok(())
    }
}

/// Call `handler` for every byte the console UART receives, from now on.
/// It runs in IRQ context: it must not block and should be short.
pub fn set_rx_handler(handler: fn(u8)) {
    *RX_HANDLER.lock() = Some(handler);
    crate::platform::enable_uart_irq();
}

/// UART interrupt: drain the receive FIFO into the handler.
pub(crate) fn uart_irq() {
    let h = *RX_HANDLER.lock();
    while let Some(b) = UART.getc() {
        if let Some(h) = h {
            h(b);
        }
    }
    UART.ack_rx();
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    let _ = Writer::new().write_fmt(args);
}

/// Write raw bytes (e.g. from a user process) as one unit, translating
/// `\n` to `\r\n`. Bytes are sent as they are; nothing is assumed about
/// their encoding.
pub fn write_bytes(bytes: &[u8]) {
    Writer::new().write(bytes);
}

/// Unlocked writer for panic and fatal-exception paths (the console lock may
/// be held by the code that crashed).
pub struct PanicWriter;

impl Write for PanicWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        UART.puts(s);
        Ok(())
    }
}

/// Print to the serial console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::console::_print(format_args!($($arg)*)) };
}

/// Print a line to the serial console.
#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::console::_print(format_args!("{}\n", format_args!($($arg)*))) };
}
