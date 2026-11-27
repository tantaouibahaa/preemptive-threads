//! PL011 UART driver: polled transmit, interrupt-driven receive.

use core::ptr::{read_volatile, write_volatile};

const DR: usize = 0x00;
const FR: usize = 0x18;
const IBRD: usize = 0x24;
const FBRD: usize = 0x28;
const LCRH: usize = 0x2c;
const CR: usize = 0x30;
const IMSC: usize = 0x38;
const ICR: usize = 0x44;
const FR_TXFF: u32 = 1 << 5;
const FR_RXFE: u32 = 1 << 4;
/// Receive and receive-timeout interrupts: the first fires when the RX FIFO
/// reaches its trigger level, the second when bytes have sat in it for 32
/// bit periods, so single keystrokes are delivered right away.
const RX_IRQS: u32 = (1 << 4) | (1 << 6);
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

    /// Whether the transmit FIFO has room for a byte.
    pub fn tx_ready(&self) -> bool {
        // SAFETY: MMIO read of the UART owned by this driver.
        let room = unsafe { read_volatile(self.reg(FR)) } & FR_TXFF == 0;
        #[cfg(feature = "slow-uart")]
        let room = room && slow::has_room();
        room
    }

    /// Queue one byte if the transmit FIFO has room; never waits.
    pub fn try_putc(&self, b: u8) -> bool {
        if !self.tx_ready() {
            return false;
        }
        #[cfg(feature = "slow-uart")]
        slow::push();
        // SAFETY: MMIO write to the UART owned by this driver.
        unsafe { write_volatile(self.reg(DR), b as u32) };
        true
    }

    /// Blocking write of one byte.
    pub fn putc(&self, b: u8) {
        while !self.try_putc(b) {
            core::hint::spin_loop();
        }
    }

    /// Read one received byte, if any.
    pub fn getc(&self) -> Option<u8> {
        // SAFETY: MMIO accesses to the UART owned by this driver.
        unsafe {
            if read_volatile(self.reg(FR)) & FR_RXFE != 0 {
                None
            } else {
                Some(read_volatile(self.reg(DR)) as u8)
            }
        }
    }

    /// Raise the UART's interrupt line when bytes arrive.
    pub fn enable_rx_irq(&self) {
        // SAFETY: MMIO accesses to the UART owned by this driver.
        unsafe {
            write_volatile(self.reg(ICR), RX_IRQS);
            let m = read_volatile(self.reg(IMSC));
            write_volatile(self.reg(IMSC), m | RX_IRQS);
        }
    }

    /// Clear the receive interrupts (after draining the FIFO with
    /// [`getc`](Self::getc)). The clear has reached the UART (its IRQ line
    /// is down) when this returns, so the interrupt does not fire again
    /// right after `eret`.
    pub fn ack_rx(&self) {
        // SAFETY: MMIO accesses to the UART owned by this driver.
        unsafe {
            write_volatile(self.reg(ICR), RX_IRQS);
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
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

/// `slow-uart`: make QEMU's instant UART as slow as a real one at 115200
/// baud 8N1 (10 bits, 86.8 us per byte) with a 16-byte transmit FIFO, timed
/// on the generic counter: bytes go into a virtual FIFO that drains one
/// byte per 86.8 us, and the UART reports itself full while it holds 16.
#[cfg(feature = "slow-uart")]
mod slow {
    use crate::arch::timer;
    use core::sync::atomic::{AtomicU64, Ordering};

    const BYTE_NS: u64 = 86_806;
    const FIFO: u64 = 16;

    /// Counter value at which the virtual FIFO is empty.
    static DRAINED_AT: AtomicU64 = AtomicU64::new(0);

    pub(super) fn has_room() -> bool {
        let now = timer::counter();
        DRAINED_AT.load(Ordering::Relaxed).saturating_sub(now) <= (FIFO - 1) * timer::ns_to_ticks(BYTE_NS)
    }

    pub(super) fn push() {
        let start = DRAINED_AT.load(Ordering::Relaxed).max(timer::counter());
        DRAINED_AT.store(start + timer::ns_to_ticks(BYTE_NS), Ordering::Relaxed);
    }
}
