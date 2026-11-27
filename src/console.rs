//! Serial console (`println!`-style macros over the board's PL011).
//!
//! Output is serialised with an IRQ-safe spinlock so lines printed from
//! different cores do not interleave.
//!
//! Input is interrupt driven: [`set_rx_handler`] installs a function that
//! is called in IRQ context (on core 0) for every received byte.

use crate::platform::UART;
use crate::sync::spin::IrqSpinLock;
use core::fmt::{self, Write};

static LOCK: IrqSpinLock<()> = IrqSpinLock::new(());
static RX_HANDLER: IrqSpinLock<Option<fn(u8)>> = IrqSpinLock::new(None);

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

struct Raw;

impl Write for Raw {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        UART.puts(s);
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments<'_>) {
    let _g = LOCK.lock();
    let _ = Raw.write_fmt(args);
}

/// Write raw bytes (e.g. from a user process) as one unit, translating
/// `\n` to `\r\n`. Bytes are sent as they are; nothing is assumed about
/// their encoding.
pub fn write_bytes(bytes: &[u8]) {
    let _g = LOCK.lock();
    for &b in bytes {
        if b == b'\n' {
            UART.putc(b'\r');
        }
        UART.putc(b);
    }
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
