//! PL011 UART driver (transmit only).

use core::ptr::{read_volatile, write_volatile};

const DR: usize = 0x00;
const FR: usize = 0x18;
const IBRD: usize = 0x24;
const FBRD: usize = 0x28;
const LCRH: usize = 0x2c;
const CR: usize = 0x30;
const ICR: usize = 0x44;
const FR_TXFF: u32 = 1 << 5;
const FR_BUSY: u32 = 1 << 3;

/// A PL011 at a fixed MMIO base address.
pub struct Pl011 {
    base: usize,
}

impl Pl011 {
    /// Create a handle for the UART at `base`.
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    fn reg(&self, off: usize) -> *mut u32 {
        (self.base + off) as *mut u32
    }

    /// Configure 8N1 with FIFOs. `divisor` is `(IBRD, FBRD)`:
    /// `uart_clk / (16 * baud)` as integer part and `round(frac * 64)`.
    ///
    /// # Safety
    ///
    /// `base` must be the MMIO address of a PL011.
    pub unsafe fn init(&self, divisor: (u32, u32)) {
        // SAFETY: MMIO accesses to the UART owned by this driver.
        unsafe {
            write_volatile(self.reg(CR), 0);
            while read_volatile(self.reg(FR)) & FR_BUSY != 0 {}
            write_volatile(self.reg(ICR), 0x7ff);
            write_volatile(self.reg(IBRD), divisor.0);
            write_volatile(self.reg(FBRD), divisor.1);
            write_volatile(self.reg(LCRH), (0b11 << 5) | (1 << 4)); // 8 bit, FIFO on
            write_volatile(self.reg(CR), (1 << 0) | (1 << 8) | (1 << 9)); // UARTEN, TXE, RXE
        }
    }

    /// Blocking write of one byte.
    pub fn putc(&self, b: u8) {
        // SAFETY: MMIO accesses to the UART owned by this driver.
        unsafe {
            while read_volatile(self.reg(FR)) & FR_TXFF != 0 {
                core::hint::spin_loop();
            }
            write_volatile(self.reg(DR), b as u32);
        }
    }

    /// Write a string, translating `\n` to `\r\n`.
    pub fn puts(&self, s: &str) {
        for b in s.bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
    }
}
