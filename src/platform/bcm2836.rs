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
//! BCM2835 interrupt controller at 0x3F00_B200 (BCM2835 ARM Peripherals,
//! section 7.5) and reach exactly one core, chosen by `GPU_INT_ROUTING`
//! (0x4000_000C), as bit 8 of its `IRQ_SOURCE`:
//!
//! | register            | offset | register              | offset |
//! |---------------------|--------|-----------------------|--------|
//! | `IRQ_BASIC_PENDING` | 0x200  | `ENABLE_BASIC_IRQS`   | 0x218  |
//! | `IRQ_PENDING_1`     | 0x204  | `DISABLE_IRQS_1`      | 0x21C  |
//! | `IRQ_PENDING_2`     | 0x208  | `DISABLE_IRQS_2`      | 0x220  |
//! | `ENABLE_IRQS_1`     | 0x210  | `DISABLE_BASIC_IRQS`  | 0x224  |
//! | `ENABLE_IRQS_2`     | 0x214  |                       |        |
//!
//! The firmware may leave sources enabled, so [`init_global`] disables
//! every one (and the ARM-local timer, PMU and AXI interrupts) and the
//! kernel then enables only what it handles.

use core::ptr::{read_volatile, write_volatile};

const BASE: usize = 0x4000_0000;
const GPU_INT_ROUTING: usize = BASE + 0x0c;
const PMU_ROUTING_CLR: usize = BASE + 0x14;
const AXI_IRQ: usize = BASE + 0x2c;
const LOCAL_TIMER_CTL: usize = BASE + 0x34;
const LOCAL_TIMER_CLR: usize = BASE + 0x38;
const IC_BASE: usize = 0x3f00_b200;
const IC_BASIC_PENDING: usize = IC_BASE;
const IC_PENDING_1: usize = IC_BASE + 0x04;
const IC_ENABLE_1: usize = IC_BASE + 0x10;
const IC_DISABLE_1: usize = IC_BASE + 0x1c;
const IC_DISABLE_BASIC: usize = IC_BASE + 0x24;
/// `IRQ_BASIC_PENDING` bits 0-7: the ARM-side sources (ARM timer, ARM
/// mailbox, doorbells, GPU halted, illegal access). Bits 8 and above only
/// mirror `IRQ_PENDING_1/2`.
const IC_BASIC_SOURCES: u32 = 0xff;
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

/// Full system barrier between accesses to different peripherals.
#[inline(always)]
fn dsb() {
    // SAFETY: barrier only.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Disable every interrupt source the firmware may have left enabled: all
/// BCM2835 peripheral and basic interrupts, the ARM-local timer, the PMU
/// and AXI-outstanding interrupts. Peripheral interrupts go to core 0.
/// Called once, on the boot core, before any core unmasks IRQs.
pub fn init_global() {
    dsb();
    wr(IC_DISABLE_1, u32::MAX);
    wr(IC_DISABLE_1 + 4, u32::MAX);
    wr(IC_DISABLE_BASIC, u32::MAX);
    dsb();
    wr(GPU_INT_ROUTING, 0);
    wr(PMU_ROUTING_CLR, 0xff);
    wr(AXI_IRQ, 0);
    wr(LOCAL_TIMER_CTL, 0);
    wr(LOCAL_TIMER_CLR, 1 << 31);
    dsb();
}

/// Route this core's physical timer and mailbox 0 to its IRQ line, and
/// nothing else (the other timers, mailboxes 1-3 and every FIQ stay off).
pub fn init_cpu(cpu: usize) {
    wr(MBOX0_RDCLR + 16 * cpu, u32::MAX);
    wr(TIMER_IRQCNTL + 4 * cpu, CNTPNSIRQ);
    wr(MBOX_IRQCNTL + 4 * cpu, 1);
    dsb();
}

/// Silence ARM-local interrupt sources of `cpu` that the kernel never
/// enables (`IRQ_SOURCE` bits other than [`CNTPNSIRQ`], [`MBOX0`] and
/// [`GPU_IRQ`]), in case one fires anyway: rewrite the core's routing and
/// turn off the shared sources.
pub fn silence_local(cpu: usize, bits: u32) {
    // Bits 0-3: timers, 5-7: mailboxes 1-3.
    if bits & 0b1111 != 0 {
        wr(TIMER_IRQCNTL + 4 * cpu, CNTPNSIRQ);
    }
    if bits & (0b111 << 5) != 0 {
        wr(MBOX_IRQCNTL + 4 * cpu, 1);
    }
    if bits & (1 << 9) != 0 {
        wr(PMU_ROUTING_CLR, 0xff);
    }
    if bits & (1 << 10) != 0 {
        wr(AXI_IRQ, 0);
    }
    if bits & (1 << 11) != 0 {
        wr(LOCAL_TIMER_CTL, 0);
        wr(LOCAL_TIMER_CLR, 1 << 31);
    }
    dsb();
}

/// Pending interrupt sources for `cpu`.
pub fn pending(cpu: usize) -> u32 {
    rd(IRQ_SOURCE + 4 * cpu)
}

/// Read and clear mailbox 0 of `cpu`. The clear has completed (the IRQ
/// line is down) when this returns.
pub fn take_mailbox0(cpu: usize) -> u32 {
    let v = rd(MBOX0_RDCLR + 16 * cpu);
    wr(MBOX0_RDCLR + 16 * cpu, v);
    dsb();
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
    dsb();
    wr(GPU_INT_ROUTING, (rd(GPU_INT_ROUTING) & !0b1111) | cpu as u32);
    dsb();
    wr(IC_ENABLE_1 + 4 * (irq as usize / 32), 1 << (irq % 32));
    dsb();
}

/// Pending BCM2835 interrupts: `(basic sources (bits 0-7), IRQs 0-63)`.
pub fn gpu_pending_all() -> (u32, u64) {
    dsb();
    let basic = rd(IC_BASIC_PENDING) & IC_BASIC_SOURCES;
    let lo = rd(IC_PENDING_1) as u64;
    let hi = rd(IC_PENDING_1 + 4) as u64;
    dsb();
    (basic, lo | (hi << 32))
}

/// Disable BCM2835 basic sources `basic` (bits 0-7) and peripheral IRQs
/// `irqs` (bit n = IRQ n).
pub fn disable_gpu_irqs(basic: u32, irqs: u64) {
    dsb();
    if basic != 0 {
        wr(IC_DISABLE_BASIC, basic);
    }
    if irqs as u32 != 0 {
        wr(IC_DISABLE_1, irqs as u32);
    }
    if (irqs >> 32) as u32 != 0 {
        wr(IC_DISABLE_1 + 4, (irqs >> 32) as u32);
    }
    dsb();
}
