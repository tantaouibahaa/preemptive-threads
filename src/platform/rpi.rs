//! Raspberry Pi Zero 2 W / Raspberry Pi 3 (BCM2837) and QEMU `raspi3b`.

use super::{bcm2836, pl011::Pl011, Irq};
use crate::arch;

/// Human-readable board name.
pub const NAME: &str = "Raspberry Pi (BCM2837)";

/// PL011 (UART0) on GPIO14 (TXD) / GPIO15 (RXD).
pub static UART: Pl011 = Pl011::new(0x3f20_1000);

const GPIO_BASE: usize = 0x3f20_0000;
const GPFSEL1: usize = GPIO_BASE + 0x04;
const GPFSEL3: usize = GPIO_BASE + 0x0c;
const GPPUD: usize = GPIO_BASE + 0x94;
const GPPUDCLK0: usize = GPIO_BASE + 0x98;
const GPPUDCLK1: usize = GPIO_BASE + 0x9c;
const PUD_OFF: u32 = 0;
const PUD_UP: u32 = 2;

/// Translation tables for the BCM2837 physical map.
pub mod memory_map {
    use crate::arch::mmu::{device_block, normal_block};

    const fn l2() -> [u64; 512] {
        let mut t = [0u64; 512];
        let mut i = 0;
        while i < 512 {
            let addr = (i as u64) << 21;
            // 0x3F00_0000..0x4000_0000 is the peripheral window.
            t[i] = if addr >= 0x3f00_0000 { device_block(addr) } else { normal_block(addr) };
            i += 1;
        }
        t
    }

    const fn l1() -> [u64; 512] {
        let mut t = [0u64; 512];
        // t[0] is patched at boot to point at the level-2 table.
        t[1] = device_block(0x4000_0000); // ARM-local peripherals
        t
    }

    /// Start of the RAM the ARM cores may use.
    pub const RAM_START: usize = 0;
    /// End of the RAM the ARM cores may use. The Pi Zero 2 W has 512 MiB
    /// shared with the GPU, whose carve-out sits at the top (its size depends
    /// on `gpu_mem` in `config.txt`); 448 MiB leaves 64 MiB for the GPU,
    /// the firmware default. QEMU `raspi3b` has 1 GiB, of which this uses
    /// the same 448 MiB.
    pub const RAM_END: usize = 0x1c00_0000;

    /// Level-1 entries (1 GiB each).
    pub const L1_ENTRIES: [u64; 512] = l1();
    /// Level-2 entries for the first GiB (2 MiB each).
    pub const L2_ENTRIES: [u64; 512] = l2();
}

/// Full system barrier: the BCM2837's peripherals sit on different AXI
/// ports, so accesses to two of them (or to one and to RAM) are only
/// ordered with a DSB in between.
#[inline(always)]
fn dsb() {
    // SAFETY: barrier only.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Wait at least `ns` nanoseconds on the generic counter (independent of
/// the core clock, unlike a spin loop).
fn delay_ns(ns: u64) {
    let end = arch::timer::counter() + arch::timer::ns_to_ticks(ns) + 1;
    while arch::timer::counter() < end {
        core::hint::spin_loop();
    }
}

/// Set the pull-up/down of the GPIOs in `mask0` (0-31) and `mask1` (32-53)
/// with the BCM2835 GPPUD/GPPUDCLK sequence: each step must be held for at
/// least 150 cycles of the VPU clock (at most 400 MHz, so 375 ns); 2 us is
/// used.
///
/// # Safety
///
/// GPIO MMIO; the pins must belong to the caller.
unsafe fn set_pull(pud: u32, mask0: u32, mask1: u32) {
    use core::ptr::write_volatile;
    // SAFETY: forwarded to the caller.
    unsafe {
        write_volatile(GPPUD as *mut u32, pud);
        dsb();
        delay_ns(2_000);
        write_volatile(GPPUDCLK0 as *mut u32, mask0);
        write_volatile(GPPUDCLK1 as *mut u32, mask1);
        dsb();
        delay_ns(2_000);
        write_volatile(GPPUD as *mut u32, PUD_OFF);
        write_volatile(GPPUDCLK0 as *mut u32, 0);
        write_volatile(GPPUDCLK1 as *mut u32, 0);
        dsb();
    }
}

/// Bring up the console: GPIO14/15 to ALT0 (PL011) with a pull-up on RXD
/// (an unconnected RX then reads idle instead of noise) and none on TXD,
/// GPIO32/33 (where the firmware puts the PL011 for Bluetooth) to plain
/// inputs, then 115200 8N1 assuming the firmware's 48 MHz UART clock
/// (`init_uart_clock=48000000`: IBRD 26, FBRD 3).
pub fn early_init(_boot_el: u8) {
    use core::ptr::{read_volatile, write_volatile};
    // SAFETY: GPIO MMIO, only pins 14, 15, 32 and 33 are touched.
    unsafe {
        dsb();
        let mut sel = read_volatile(GPFSEL3 as *const u32);
        sel &= !((7 << 6) | (7 << 9)); // GPIO32/33: input
        write_volatile(GPFSEL3 as *mut u32, sel);
        let mut sel = read_volatile(GPFSEL1 as *const u32);
        sel &= !((7 << 12) | (7 << 15));
        sel |= (4 << 12) | (4 << 15); // GPIO14/15: ALT0
        write_volatile(GPFSEL1 as *mut u32, sel);
        set_pull(PUD_OFF, 1 << 14, 0);
        set_pull(PUD_UP, 1 << 15, 0);
        UART.init((26, 3));
        dsb();
    }
}

/// Global interrupt controller setup (nothing to do for the local block).
pub fn irq_init_global() {}

/// Per-core interrupt routing.
pub fn irq_init_cpu(cpu: usize) {
    bcm2836::init_cpu(cpu);
}

/// Handle every pending interrupt source of the calling core.
pub fn dispatch_irqs(mut handler: impl FnMut(Irq)) {
    let cpu = arch::cpu_id();
    let src = bcm2836::pending(cpu);
    if src & bcm2836::MBOX0 != 0 {
        bcm2836::take_mailbox0(cpu);
        handler(Irq::Reschedule);
    }
    if src & bcm2836::CNTPNSIRQ != 0 {
        handler(Irq::Timer);
    }
    if src & bcm2836::GPU_IRQ != 0 {
        // Peripheral interrupts go through the BCM2835 controller; only the
        // PL011 is enabled there.
        if bcm2836::gpu_pending(bcm2836::GPU_IRQ_UART) {
            handler(Irq::Uart);
        }
    }
    let other = src & !(bcm2836::MBOX0 | bcm2836::CNTPNSIRQ | bcm2836::GPU_IRQ);
    if other != 0 {
        handler(Irq::Other(other));
    }
}

/// Deliver the console UART's interrupt (BCM2835 IRQ 57) to core 0 (see
/// [`Irq::Uart`]).
pub fn enable_uart_irq() {
    UART.enable_rx_irq();
    bcm2836::enable_gpu_irq(bcm2836::GPU_IRQ_UART, 0);
}

/// Ask `cpu` to run its scheduler.
pub fn send_resched_ipi(cpu: usize) {
    bcm2836::send_mailbox0(cpu, 1);
}

/// Release a secondary core through the firmware spin table: core n polls
/// the 64-bit word at 0xD8 + 8n and jumps there once it is non-zero.
pub fn start_cpu(cpu: usize, entry: usize) -> bool {
    if cpu == 0 || cpu >= super::MAX_CPUS {
        return false;
    }
    let slot = 0xd8 + 8 * cpu;
    // SAFETY: the spin-table words live in the (identity mapped) first page,
    // reserved by the firmware for exactly this purpose.
    unsafe { core::ptr::write_volatile(slot as *mut u64, entry as u64) };
    // The parked core reads the slot with its MMU and caches off.
    arch::dcache_clean_inval(slot);
    arch::sev();
    true
}
