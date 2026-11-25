//! Raspberry Pi Zero 2 W / Raspberry Pi 3 (BCM2837) and QEMU `raspi3b`.

use super::{bcm2836, pl011::Pl011, Irq};
use crate::arch;

/// Human-readable board name.
pub const NAME: &str = "Raspberry Pi (BCM2837)";

/// PL011 (UART0) on GPIO14 (TXD) / GPIO15 (RXD).
pub static UART: Pl011 = Pl011::new(0x3f20_1000);

const GPIO_BASE: usize = 0x3f20_0000;
const GPFSEL1: usize = GPIO_BASE + 0x04;
const GPPUD: usize = GPIO_BASE + 0x94;
const GPPUDCLK0: usize = GPIO_BASE + 0x98;

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

    /// Level-1 entries (1 GiB each).
    pub const L1_ENTRIES: [u64; 512] = l1();
    /// Level-2 entries for the first GiB (2 MiB each).
    pub const L2_ENTRIES: [u64; 512] = l2();
}

fn delay(cycles: u32) {
    for _ in 0..cycles {
        core::hint::spin_loop();
    }
}

/// Bring up the console: GPIO14/15 to ALT0 (PL011), 115200 8N1 assuming the
/// firmware's default 48 MHz UART clock (IBRD 26, FBRD 3).
pub fn early_init(_boot_el: u8) {
    use core::ptr::{read_volatile, write_volatile};
    // SAFETY: GPIO MMIO, only pins 14 and 15 are touched.
    unsafe {
        let mut sel = read_volatile(GPFSEL1 as *const u32);
        sel &= !((7 << 12) | (7 << 15));
        sel |= (4 << 12) | (4 << 15);
        write_volatile(GPFSEL1 as *mut u32, sel);
        write_volatile(GPPUD as *mut u32, 0);
        delay(150);
        write_volatile(GPPUDCLK0 as *mut u32, (1 << 14) | (1 << 15));
        delay(150);
        write_volatile(GPPUDCLK0 as *mut u32, 0);
        UART.init((26, 3));
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
    let other = src & !(bcm2836::MBOX0 | bcm2836::CNTPNSIRQ);
    if other != 0 {
        handler(Irq::Other(other));
    }
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
