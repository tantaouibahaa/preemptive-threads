//! QEMU `virt` machine (GICv2, PSCI, PL011 at 0x0900_0000).

use super::{gic, pl011::Pl011, psci, Irq};

/// Human-readable board name.
pub const NAME: &str = "QEMU virt";

/// PL011 UART0.
pub static UART: Pl011 = Pl011::new(0x0900_0000);

/// Translation tables for the `virt` physical map.
pub mod memory_map {
    use crate::arch::mmu::{device_block, normal_block};

    const fn l2() -> [u64; 512] {
        // First GiB: flash, GIC, UART, RTC, virtio... all MMIO.
        let mut t = [0u64; 512];
        let mut i = 0;
        while i < 512 {
            t[i] = device_block((i as u64) << 21);
            i += 1;
        }
        t
    }

    const fn l1() -> [u64; 512] {
        let mut t = [0u64; 512];
        // t[0] is patched at boot to point at the level-2 table.
        t[1] = normal_block(0x4000_0000); // RAM, first GiB
        t[2] = normal_block(0x8000_0000); // RAM, second GiB
        t
    }

    /// Start of RAM.
    pub const RAM_START: usize = 0x4000_0000;
    /// End of the RAM the kernel uses: 512 MiB, what `scripts/qemu-run.sh`
    /// and `make run-virt` pass with `-m 512M`. Run with at least that much.
    pub const RAM_END: usize = 0x6000_0000;

    /// Level-1 entries (1 GiB each).
    pub const L1_ENTRIES: [u64; 512] = l1();
    /// Level-2 entries for the first GiB (2 MiB each).
    pub const L2_ENTRIES: [u64; 512] = l2();
}

/// Bring up the console and pick the PSCI conduit.
pub fn early_init(boot_el: u8) {
    psci::set_conduit_from_boot_el(boot_el);
    // SAFETY: UART0 MMIO. QEMU's PL011 clock is 24 MHz (13 + 1/64 for 115200).
    unsafe { UART.init((13, 1)) };
}

/// Global interrupt controller setup.
pub fn irq_init_global() {
    gic::init_distributor();
}

/// Per-core interrupt controller setup.
pub fn irq_init_cpu(_cpu: usize) {
    gic::init_cpu();
}

/// Handle every pending interrupt of the calling core.
pub fn dispatch_irqs(mut handler: impl FnMut(Irq)) {
    while let Some(iar) = gic::ack() {
        let id = iar & 0x3ff;
        match id {
            gic::TIMER_INTID => handler(Irq::Timer),
            gic::RESCHED_SGI => handler(Irq::Reschedule),
            gic::UART_INTID => handler(Irq::Uart),
            other => handler(Irq::Other(other)),
        }
        // Thread switches happen only after dispatch (in irq_exit), so the
        // interrupt is always completed on the core that acknowledged it.
        gic::eoi(iar);
    }
}

/// Deliver the console UART's interrupt to core 0 (see [`Irq::Uart`]).
pub fn enable_uart_irq() {
    UART.enable_rx_irq();
    gic::enable_spi(gic::UART_INTID, 0);
}

/// Ask `cpu` to run its scheduler.
pub fn send_resched_ipi(cpu: usize) {
    gic::send_sgi(cpu, gic::RESCHED_SGI);
}

/// Power on a secondary core with PSCI `CPU_ON`.
pub fn start_cpu(cpu: usize, entry: usize) -> bool {
    if cpu == 0 || cpu >= super::MAX_CPUS {
        return false;
    }
    psci::cpu_on(cpu as u64, entry, 0) == 0
}
