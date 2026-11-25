//! Board support.
//!
//! Exactly one board is compiled in:
//!
//! | feature     | board                                   | IRQ controller            | SMP bring-up     |
//! |-------------|-----------------------------------------|---------------------------|------------------|
//! | (default)   | Raspberry Pi Zero 2 W / Pi 3 / QEMU `raspi3b` | BCM2836 ARM-local controller | spin table     |
//! | `qemu-virt` | QEMU `virt`                             | GICv2                     | PSCI `CPU_ON`    |
//!
//! Both boards use a PL011 UART for the console.

pub mod pl011;

#[cfg(not(feature = "qemu-virt"))]
pub mod bcm2836;
#[cfg(not(feature = "qemu-virt"))]
mod rpi;
#[cfg(not(feature = "qemu-virt"))]
pub use rpi::*;

#[cfg(feature = "qemu-virt")]
pub mod gic;
#[cfg(feature = "qemu-virt")]
pub mod psci;
#[cfg(feature = "qemu-virt")]
mod virt;
#[cfg(feature = "qemu-virt")]
pub use virt::*;

/// Maximum number of cores supported.
pub const MAX_CPUS: usize = 4;

/// An interrupt source decoded by the board's interrupt controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Irq {
    /// This core's generic timer fired.
    Timer,
    /// Another core asked this one to reschedule.
    Reschedule,
    /// Some other interrupt (id is controller specific).
    Other(u32),
}
