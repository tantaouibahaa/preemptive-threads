//! Serial console (`println!`-style macros over the board's PL011).
//!
//! Output is serialised with an IRQ-safe spinlock so lines printed from
//! different cores do not interleave.

use crate::platform::UART;
use crate::sync::spin::IrqSpinLock;
use core::fmt::{self, Write};

static LOCK: IrqSpinLock<()> = IrqSpinLock::new(());

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
