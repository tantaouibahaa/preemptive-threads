//! Minimal bare-metal kernel example for QEMU virt machine.
//!
//! This is a simpler setup that works with QEMU's standard virt machine.
//! The virt machine has a cleaner emulation environment than raspi3b.
//!
//! # Building
//!
//! ```bash
//! cargo build --release --example qemu_virt --target aarch64-unknown-none
//! ```
//!
//! # Running
//!
//! ```bash
//! qemu-system-aarch64 \
//!     -M virt \
//!     -cpu cortex-a72 \
//!     -m 512M \
//!     -kernel target/aarch64-unknown-none/release/examples/qemu_virt \
//!     -nographic
//! ```
//!
//! Press Ctrl-A X to exit QEMU.

#![no_std]
#![no_main]

use core::fmt::Write;
use core::ptr::write_volatile;

// QEMU virt machine PL011 UART is at 0x09000000
const UART0_BASE: usize = 0x0900_0000;
const UART0_DR: usize = UART0_BASE;       // Data Register
const UART0_FR: usize = UART0_BASE + 0x18; // Flag Register

/// Send a byte to the QEMU virt PL011 UART.
fn uart_putc(c: u8) {
    unsafe {
        // Wait for transmit FIFO to have space
        while (core::ptr::read_volatile(UART0_FR as *const u32) & (1 << 5)) != 0 {
            core::hint::spin_loop();
        }
        write_volatile(UART0_DR as *mut u32, c as u32);
    }
}

/// Send a string to UART.
fn uart_puts(s: &str) {
    for c in s.bytes() {
        if c == b'\n' {
            uart_putc(b'\r');
        }
        uart_putc(c);
    }
}

/// Simple UART writer for formatting.
struct UartWriter;

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        uart_puts(s);
        Ok(())
    }
}

/// Macro for printing.
macro_rules! print {
    ($($arg:tt)*) => {
        let _ = write!(UartWriter, $($arg)*);
    };
}

macro_rules! println {
    () => { print!("\n") };
    ($($arg:tt)*) => {
        print!($($arg)*);
        print!("\n");
    };
}

/// Entry point - QEMU virt loads kernel and jumps here at EL1.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    println!();
    println!("========================================");
    println!("  Preemptive Threads - QEMU virt test");
    println!("========================================");
    println!();
    println!("[BOOT] Hello from bare-metal ARM64!");
    println!("[BOOT] UART output working!");
    println!();

    // Show we're alive with a simple counter
    let mut counter = 0u64;
    loop {
        counter = counter.wrapping_add(1);
        if counter % 100_000_000 == 0 {
            println!("[IDLE] Counter: {}", counter / 100_000_000);
        }
        core::hint::spin_loop();
    }
}

/// Panic handler.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!();
    println!("!!! KERNEL PANIC !!!");
    if let Some(location) = info.location() {
        println!("at {}:{}", location.file(), location.line());
    }
    loop {
        core::hint::spin_loop();
    }
}
