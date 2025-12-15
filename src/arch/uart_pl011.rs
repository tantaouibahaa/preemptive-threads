//! PL011 UART driver for Raspberry Pi (QEMU compatible).
//!
//! This driver uses the PL011 UART which is mapped to QEMU's `-serial stdio`.
//! The Mini UART is not emulated by QEMU's raspi3b machine.
//!
//! # Memory Map
//!
//! Peripheral base for BCM2837: 0x3F000000
//! - PL011 UART base: 0x3F201000

use core::fmt::{self, Write};
use core::ptr::{read_volatile, write_volatile};

// BCM2837 peripheral base address
const PERIPHERAL_BASE: usize = 0x3F00_0000;

// PL011 UART registers
const UART0_BASE: usize = PERIPHERAL_BASE + 0x20_1000;
const UART0_DR: usize = UART0_BASE + 0x00;     // Data Register
const UART0_FR: usize = UART0_BASE + 0x18;     // Flag Register
const UART0_IBRD: usize = UART0_BASE + 0x24;   // Integer Baud Rate Divisor
const UART0_FBRD: usize = UART0_BASE + 0x28;   // Fractional Baud Rate Divisor
const UART0_LCRH: usize = UART0_BASE + 0x2C;   // Line Control Register
const UART0_CR: usize = UART0_BASE + 0x30;     // Control Register
const UART0_ICR: usize = UART0_BASE + 0x44;    // Interrupt Clear Register

// GPIO registers for pin configuration
const GPIO_BASE: usize = PERIPHERAL_BASE + 0x20_0000;
const GPFSEL1: usize = GPIO_BASE + 0x04;       // GPIO Function Select 1 (pins 10-19)
const GPPUD: usize = GPIO_BASE + 0x94;         // GPIO Pull-up/down Enable
const GPPUDCLK0: usize = GPIO_BASE + 0x98;     // GPIO Pull-up/down Clock 0

// Flag register bits
const FR_TXFF: u32 = 1 << 5;  // Transmit FIFO full
#[allow(dead_code)] // Reserved for future RX support
const FR_RXFE: u32 = 1 << 4;  // Receive FIFO empty

/// Initialize the PL011 UART for 115200 baud output.
///
/// # Safety
///
/// Must be called once during system initialization.
/// Modifies GPIO and UART hardware registers.
pub unsafe fn init() {
    unsafe {
        // Disable UART0 while configuring
        write_volatile(UART0_CR as *mut u32, 0);

        // Configure GPIO pins 14 and 15 for UART (ALT0 function for PL011)
        let mut gpfsel1 = read_volatile(GPFSEL1 as *const u32);
        // Clear bits 12-14 (GPIO14) and 15-17 (GPIO15)
        gpfsel1 &= !((7 << 12) | (7 << 15));
        // Set ALT0 (binary 100) for both pins
        gpfsel1 |= (4 << 12) | (4 << 15);
        write_volatile(GPFSEL1 as *mut u32, gpfsel1);

        // Disable pull-up/down for pins 14 and 15
        write_volatile(GPPUD as *mut u32, 0);
        delay_cycles(150);
        write_volatile(GPPUDCLK0 as *mut u32, (1 << 14) | (1 << 15));
        delay_cycles(150);
        write_volatile(GPPUDCLK0 as *mut u32, 0);

        // Clear all pending interrupts
        write_volatile(UART0_ICR as *mut u32, 0x7FF);

        // Set baud rate to 115200
        // Divider = UART_CLOCK / (16 * baud_rate)
        // For 48MHz clock: 48000000 / (16 * 115200) = 26.041666...
        // Integer part = 26, Fractional part = 0.041666 * 64 = 2.666 ≈ 3
        //
        // Note: QEMU doesn't care about baud rate, but real hardware needs correct values
        // For 3MHz base clock (QEMU default): 3000000 / (16 * 115200) = 1.627
        // Just use values that work on QEMU
        write_volatile(UART0_IBRD as *mut u32, 1);   // Integer divisor
        write_volatile(UART0_FBRD as *mut u32, 40);  // Fractional divisor

        // 8 bits, no parity, 1 stop bit, enable FIFOs
        write_volatile(UART0_LCRH as *mut u32, (1 << 4) | (1 << 5) | (1 << 6));  // WLEN=8, FEN=1

        // Enable UART0, TX, and RX
        write_volatile(UART0_CR as *mut u32, (1 << 0) | (1 << 8) | (1 << 9));  // UARTEN, TXE, RXE
    }
}

/// Spin-wait for approximately `count` CPU cycles.
#[inline]
fn delay_cycles(count: u32) {
    for _ in 0..count {
        core::hint::spin_loop();
    }
}

/// Check if the transmit FIFO can accept data.
#[inline]
fn can_transmit() -> bool {
    // FR_TXFF is set when FIFO is full, so we can transmit when it's NOT set
    unsafe { (read_volatile(UART0_FR as *const u32) & FR_TXFF) == 0 }
}

/// Send a single byte over UART.
pub fn send_byte(byte: u8) {
    // Wait until transmitter is ready
    while !can_transmit() {
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile(UART0_DR as *mut u32, byte as u32);
    }
}

/// Send a string over UART.
pub fn send_str(s: &str) {
    for byte in s.bytes() {
        if byte == b'\n' {
            send_byte(b'\r'); // Add carriage return for terminals
        }
        send_byte(byte);
    }
}

/// Global UART writer for use with `write!` macro.
pub struct UartWriter;

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        send_str(s);
        Ok(())
    }
}

/// Print a formatted string to PL011 UART.
#[macro_export]
macro_rules! pl011_print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::arch::uart_pl011::UartWriter, $($arg)*);
    }};
}

/// Print a formatted string to PL011 UART with a newline.
#[macro_export]
macro_rules! pl011_println {
    () => {
        $crate::pl011_print!("\n")
    };
    ($($arg:tt)*) => {{
        $crate::pl011_print!($($arg)*);
        $crate::pl011_print!("\n");
    }};
}
