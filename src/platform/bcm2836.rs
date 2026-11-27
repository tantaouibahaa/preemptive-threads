//! BCM2836/BCM2837 "ARM local" interrupt controller (base 0x4000_0000).
//!
//! The Raspberry Pi 2/3/Zero 2 W have no GIC. Per-core interrupts (generic
//! timers and the four mailboxes per core) are routed by this block:
//!
//! | register                 | address              |
//! |--------------------------|----------------------|
//! | `COREn_TIMER_IRQCNTL`    | 0x4000_0040 + 4n     |
//! | `COREn_MBOX_IRQCNTL`     | 0x4000_0050 + 4n     |
//! | `COREn_IRQ_SOURCE`       | 0x4000_0060 + 4n     |
//! | `COREn_MBOX0_SET`        | 0x4000_0080 + 16n    |
//! | `COREn_MBOX0_RDCLR`      | 0x4000_00C0 + 16n    |
//!
//! The kernel uses the EL1 non-secure physical timer (`CNTPNSIRQ`) and
//! mailbox 0 for reschedule IPIs. QEMU's `raspi3b` emulates this block.
//!
//! Peripheral ("GPU") interrupts such as the PL011's come from the older
//! BCM2835 interrupt controller at 0x3F00_B200 (`IRQ_PENDING_1/2`,
//! `ENABLE_IRQS_1/2`) and reach exactly one core, chosen by
//! `GPU_INT_ROUTING` (0x4000_000C), as bit 8 of its `IRQ_SOURCE`.

use core::ptr::{read_volatile, write_volatile};

const BASE: usize = 0x4000_0000;
const GPU_INT_ROUTING: usize = BASE + 0x0c;
const IC_BASE: usize = 0x3f00_b200;
const IC_PENDING_1: usize = IC_BASE + 0x04;
const IC_ENABLE_1: usize = IC_BASE + 0x10;
const TIMER_IRQCNTL: usize = BASE + 0x40;
const MBOX_IRQCNTL: usize = BASE + 0x50;
const IRQ_SOURCE: usize = BASE + 0x60;
const MBOX0_SET: usize = BASE + 0x80;
const MBOX0_RDCLR: usize = BASE + 0xc0;

/// `IRQ_SOURCE` / `TIMER_IRQCNTL` bit for the non-secure physical timer.
pub const CNTPNSIRQ: u32 = 1 << 1;
/// `IRQ_SOURCE` bit for mailbox 0.
pub const MBOX0: u32 = 1 << 4;
/// `IRQ_SOURCE` bit for a peripheral interrupt from the BCM2835 controller.
pub const GPU_IRQ: u32 = 1 << 8;
/// BCM2835 interrupt number of the PL011 (UART0).
pub const GPU_IRQ_UART: u32 = 57;

fn rd(addr: usize) -> u32 {
    // SAFETY: ARM-local MMIO, mapped as device memory.
    unsafe { read_volatile(addr as *const u32) }
}

fn wr(addr: usize, v: u32) {
    // SAFETY: ARM-local MMIO, mapped as device memory.
    unsafe { write_volatile(addr as *mut u32, v) }
}

/// Route this core's physical timer and mailbox 0 to its IRQ line.
pub fn init_cpu(cpu: usize) {
    wr(MBOX0_RDCLR + 16 * cpu, u32::MAX);
    wr(TIMER_IRQCNTL + 4 * cpu, CNTPNSIRQ);
    wr(MBOX_IRQCNTL + 4 * cpu, 1);
}

/// Pending interrupt sources for `cpu`.
pub fn pending(cpu: usize) -> u32 {
    rd(IRQ_SOURCE + 4 * cpu)
}

/// Read and clear mailbox 0 of `cpu`.
pub fn take_mailbox0(cpu: usize) -> u32 {
    let v = rd(MBOX0_RDCLR + 16 * cpu);
    wr(MBOX0_RDCLR + 16 * cpu, v);
    v
}

/// Raise mailbox 0 of `cpu`.
pub fn send_mailbox0(cpu: usize, bits: u32) {
    // SAFETY: barrier only.
    unsafe { core::arch::asm!("dsb ish", options(nostack, preserves_flags)) };
    wr(MBOX0_SET + 16 * cpu, bits);
}

/// Enable BCM2835 peripheral interrupt `irq` (0-63) and route all
/// peripheral interrupts to `cpu`.
pub fn enable_gpu_irq(irq: u32, cpu: usize) {
    wr(GPU_INT_ROUTING, (rd(GPU_INT_ROUTING) & !0b1111) | cpu as u32);
    wr(IC_ENABLE_1 + 4 * (irq as usize / 32), 1 << (irq % 32));
}

/// Whether BCM2835 peripheral interrupt `irq` (0-63) is pending.
pub fn gpu_pending(irq: u32) -> bool {
    rd(IC_PENDING_1 + 4 * (irq as usize / 32)) & (1 << (irq % 32)) != 0
}
