//! Mini UART driver for Raspberry Pi Zero 2 W.
//!
//! This provides basic serial output for debugging. Connect a USB-to-serial
//! adapter to GPIO 14 (TX) and GPIO 15 (RX), then use `screen /dev/tty.usbserial* 115200`
//! or similar to see output.
//!
//! # Hardware
//!
//! The BCM2837 has two UARTs:
//! - PL011 (full UART) - typically used for Bluetooth
//! - Mini UART - simpler, we use this one
//!
//! # Memory Map
//!
//! Peripheral base for BCM2837: 0x3F000000
//! - GPIO base: 0x3F200000
//! - Mini UART base: 0x3F215000

use core::fmt::{self, Write};
use core::ptr::{read_volatile, write_volatile};

// BCM2837 peripheral base address
const PERIPHERAL_BASE: usize = 0x3F00_0000;

// GPIO registers
const GPIO_BASE: usize = PERIPHERAL_BASE + 0x20_0000;
const GPFSEL1: usize = GPIO_BASE + 0x04;      // GPIO Function Select 1 (pins 10-19)
const GPPUD: usize = GPIO_BASE + 0x94;        // GPIO Pull-up/down Enable
const GPPUDCLK0: usize = GPIO_BASE + 0x98;    // GPIO Pull-up/down Clock 0

// Mini UART registers (active by AUX_ENABLES)
const AUX_BASE: usize = PERIPHERAL_BASE + 0x21_5000;
const AUX_ENABLES: usize = AUX_BASE + 0x04;     // Auxiliary enables
const AUX_MU_IO: usize = AUX_BASE + 0x40;       // Mini UART I/O Data
const AUX_MU_IER: usize = AUX_BASE + 0x44;      // Mini UART Interrupt Enable
const AUX_MU_IIR: usize = AUX_BASE + 0x48;      // Mini UART Interrupt Identify
const AUX_MU_LCR: usize = AUX_BASE + 0x4C;      // Mini UART Line Control
const AUX_MU_MCR: usize = AUX_BASE + 0x50;      // Mini UART Modem Control
const AUX_MU_LSR: usize = AUX_BASE + 0x54;      // Mini UART Line Status
const AUX_MU_CNTL: usize = AUX_BASE + 0x60;     // Mini UART Extra Control
const AUX_MU_BAUD: usize = AUX_BASE + 0x68;     // Mini UART Baudrate

/// Initialize the Mini UART for 115200 baud output.
///
/// # Safety
///
/// Must be called once during system initialization.
/// Modifies GPIO and UART hardware registers.
pub unsafe fn init() {
    unsafe {
        // Enable Mini UART (set bit 0 of AUX_ENABLES)
        write_volatile(AUX_ENABLES as *mut u32, 1);

        // Disable TX/RX while configuring
        write_volatile(AUX_MU_CNTL as *mut u32, 0);

        // Disable interrupts
        write_volatile(AUX_MU_IER as *mut u32, 0);

        // Set 8-bit mode (bit 0 and bit 1 = 1 for 8-bit)
        write_volatile(AUX_MU_LCR as *mut u32, 3);

        // Set RTS line high (not used, but good practice)
        write_volatile(AUX_MU_MCR as *mut u32, 0);

        // Clear FIFOs
        write_volatile(AUX_MU_IIR as *mut u32, 0xC6);

        // Set baud rate to 115200
        // baudrate = system_clock / (8 * (baud_reg + 1))
        // For 250MHz clock: 250000000 / (8 * (270 + 1)) = 115313 ≈ 115200
        write_volatile(AUX_MU_BAUD as *mut u32, 270);

        // Configure GPIO pins 14 and 15 for UART (ALT5 function)
        let mut gpfsel1 = read_volatile(GPFSEL1 as *const u32);
        // Clear bits 12-14 (GPIO14) and 15-17 (GPIO15)
        gpfsel1 &= !((7 << 12) | (7 << 15));
        // Set ALT5 (binary 010) for both pins
        gpfsel1 |= (2 << 12) | (2 << 15);
        write_volatile(GPFSEL1 as *mut u32, gpfsel1);

        // Disable pull-up/down for pins 14 and 15
        write_volatile(GPPUD as *mut u32, 0);
        delay_cycles(150);
        write_volatile(GPPUDCLK0 as *mut u32, (1 << 14) | (1 << 15));
        delay_cycles(150);
        write_volatile(GPPUDCLK0 as *mut u32, 0);

        // Enable TX and RX
        write_volatile(AUX_MU_CNTL as *mut u32, 3);
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
    // Bit 5 of LSR is set when transmitter can accept at least one byte
    unsafe { (read_volatile(AUX_MU_LSR as *const u32) & (1 << 5)) != 0 }
}

/// Send a single byte over UART.
pub fn send_byte(byte: u8) {
    // Wait until transmitter is ready
    while !can_transmit() {
        core::hint::spin_loop();
    }
    unsafe {
        write_volatile(AUX_MU_IO as *mut u32, byte as u32);
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

/// Print a formatted string to UART.
///
/// # Example
///
/// ```ignore
/// uart_print!("Hello, world!\n");
/// uart_print!("Counter: {}\n", counter);
/// ```
#[macro_export]
macro_rules! uart_print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::arch::uart::UartWriter, $($arg)*);
    }};
}

/// Print a formatted string to UART with a newline.
#[macro_export]
macro_rules! uart_println {
    () => {
        $crate::uart_print!("\n")
    };
    ($($arg:tt)*) => {{
        $crate::uart_print!($($arg)*);
        $crate::uart_print!("\n");
    }};
}
