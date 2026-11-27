//! GICv2 driver (QEMU `virt`: distributor at 0x0800_0000, CPU interface at
//! 0x0801_0000).
//!
//! The distributor is set up once by the boot core; every core then enables
//! its banked SGIs/PPIs and its own CPU interface. The EL1 physical timer is
//! PPI 14 (INTID 30); reschedule IPIs use SGI 1.

use core::ptr::{read_volatile, write_volatile};

const GICD: usize = 0x0800_0000;
const GICC: usize = 0x0801_0000;

const GICD_CTLR: usize = 0x000;
const GICD_TYPER: usize = 0x004;
const GICD_ISENABLER: usize = 0x100;
const GICD_ICENABLER: usize = 0x180;
const GICD_ICPENDR: usize = 0x280;
const GICD_IPRIORITYR: usize = 0x400;
const GICD_ITARGETSR: usize = 0x800;
const GICD_SGIR: usize = 0xf00;

const GICC_CTLR: usize = 0x000;
const GICC_PMR: usize = 0x004;
const GICC_BPR: usize = 0x008;
const GICC_IAR: usize = 0x00c;
const GICC_EOIR: usize = 0x010;

/// INTID of the EL1 non-secure physical timer.
pub const TIMER_INTID: u32 = 30;
/// INTID of the PL011 UART0 on QEMU `virt` (SPI 1).
pub const UART_INTID: u32 = 33;
/// SGI used for reschedule IPIs.
pub const RESCHED_SGI: u32 = 1;
const SPURIOUS: u32 = 1023;

fn rd(addr: usize) -> u32 {
    // SAFETY: GIC MMIO, mapped as device memory.
    unsafe { read_volatile(addr as *const u32) }
}

fn wr(addr: usize, v: u32) {
    // SAFETY: GIC MMIO, mapped as device memory.
    unsafe { write_volatile(addr as *mut u32, v) }
}

/// Initialise the distributor (boot core only).
pub fn init_distributor() {
    wr(GICD + GICD_CTLR, 0);
    let lines = ((rd(GICD + GICD_TYPER) & 0x1f) + 1) * 32;
    for i in (32..lines).step_by(32) {
        wr(GICD + GICD_ICENABLER + (i as usize / 32) * 4, u32::MAX);
        wr(GICD + GICD_ICPENDR + (i as usize / 32) * 4, u32::MAX);
    }
    wr(GICD + GICD_CTLR, 1);
}

/// Enable shared peripheral interrupt `intid`, routed to core `cpu`.
pub fn enable_spi(intid: u32, cpu: usize) {
    let i = intid as usize;
    let shift = (i % 4) * 8;
    let prio = GICD + GICD_IPRIORITYR + (i / 4) * 4;
    wr(prio, (rd(prio) & !(0xff << shift)) | (0x80 << shift));
    let tgt = GICD + GICD_ITARGETSR + (i / 4) * 4;
    wr(tgt, (rd(tgt) & !(0xff << shift)) | ((1u32 << cpu) << shift));
    wr(GICD + GICD_ISENABLER + (i / 32) * 4, 1 << (i % 32));
}

/// Enable the timer PPI and SGIs for the calling core and its CPU interface.
pub fn init_cpu() {
    // Banked registers: SGIs 0-15 and PPIs 16-31 of this core.
    for i in 0..8 {
        wr(GICD + GICD_IPRIORITYR + i * 4, 0x8080_8080);
    }
    wr(GICD + GICD_ISENABLER, (1 << TIMER_INTID) | 0xffff);
    wr(GICC + GICC_PMR, 0xf0);
    wr(GICC + GICC_BPR, 0);
    wr(GICC + GICC_CTLR, 1);
}

/// Acknowledge the highest-priority pending interrupt. Returns the raw IAR
/// value (needed for EOI) or `None` if nothing is pending.
pub fn ack() -> Option<u32> {
    let iar = rd(GICC + GICC_IAR);
    if iar & 0x3ff == SPURIOUS {
        None
    } else {
        Some(iar)
    }
}

/// Signal end of interrupt for a value returned by [`ack`].
pub fn eoi(iar: u32) {
    wr(GICC + GICC_EOIR, iar);
}

/// Send SGI `sgi` to core `cpu`.
pub fn send_sgi(cpu: usize, sgi: u32) {
    // Make prior stores (e.g. run-queue updates) visible before the IPI.
    // SAFETY: barrier only.
    unsafe { core::arch::asm!("dsb ish", options(nostack, preserves_flags)) };
    wr(GICD + GICD_SGIR, (1 << (16 + cpu)) | sgi);
}
