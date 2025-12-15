//! GIC-400 (Generic Interrupt Controller v2) driver.
//!
//! This module provides initialization and control of the GIC interrupt controller.
//!
//! # Platform Support
//!
//! The GIC addresses differ between platforms:
//!
//! - **Real Pi / QEMU raspi3b**: BCM2837 GIC @ `0xFF84_1000` (not emulated in QEMU)
//! - **QEMU virt machine**: GICv2 @ `0x0800_0000` (fully emulated)
//!
//! Use the `qemu-virt` feature to target the virt machine for full preemption testing.
//!
//! # Interrupts
//!
//! - Physical Timer (EL1): IRQ 30 (PPI)
//! - Virtual Timer: IRQ 27 (PPI)
//!
//! # Reference
//!
//! ARM Generic Interrupt Controller Architecture Specification v2.0

use core::ptr::{read_volatile, write_volatile};

// GIC base addresses - platform dependent
#[cfg(feature = "qemu-virt")]
const GICD_BASE: usize = 0x0800_0000; // QEMU virt GIC Distributor
#[cfg(feature = "qemu-virt")]
const GICC_BASE: usize = 0x0801_0000; // QEMU virt GIC CPU Interface

#[cfg(not(feature = "qemu-virt"))]
const GICD_BASE: usize = 0xFF84_1000; // BCM2837 GIC Distributor
#[cfg(not(feature = "qemu-virt"))]
const GICC_BASE: usize = 0xFF84_2000; // BCM2837 GIC CPU Interface

// Distributor registers (offsets from GICD_BASE)
const GICD_CTLR: usize = 0x000;       // Distributor Control Register
const GICD_TYPER: usize = 0x004;      // Interrupt Controller Type Register
const GICD_ISENABLER: usize = 0x100;  // Interrupt Set-Enable Registers
const GICD_ICENABLER: usize = 0x180;  // Interrupt Clear-Enable Registers
const GICD_ISPENDR: usize = 0x200;    // Interrupt Set-Pending Registers
const GICD_ICPENDR: usize = 0x280;    // Interrupt Clear-Pending Registers
const GICD_IPRIORITYR: usize = 0x400; // Interrupt Priority Registers
const GICD_ITARGETSR: usize = 0x800;  // Interrupt Processor Targets Registers
const GICD_ICFGR: usize = 0xC00;      // Interrupt Configuration Registers

// CPU Interface registers (offsets from GICC_BASE)
const GICC_CTLR: usize = 0x000;  // CPU Interface Control Register
const GICC_PMR: usize = 0x004;   // Interrupt Priority Mask Register
const GICC_BPR: usize = 0x008;   // Binary Point Register
const GICC_IAR: usize = 0x00C;   // Interrupt Acknowledge Register
const GICC_EOIR: usize = 0x010;  // End of Interrupt Register
const GICC_RPR: usize = 0x014;   // Running Priority Register
const GICC_HPPIR: usize = 0x018; // Highest Priority Pending Interrupt Register

// Interrupt numbers
/// Physical Timer interrupt (EL1 Physical Timer)
pub const TIMER_IRQ: u32 = 30;
/// Virtual Timer interrupt
pub const VTIMER_IRQ: u32 = 27;

// Special interrupt IDs
/// Spurious interrupt ID
pub const SPURIOUS_IRQ: u32 = 1023;

/// GIC-400 Interrupt Controller for Raspberry Pi Zero 2 W.
pub struct Gic400;

impl Gic400 {
    /// Initialize the GIC-400 interrupt controller.
    ///
    /// This sets up both the Distributor and CPU Interface for handling
    /// interrupts on CPU 0.
    ///
    /// # Safety
    ///
    /// Must be called once during system initialization with interrupts
    /// disabled. The GIC memory regions must be mapped and accessible.
    ///
    /// Returns false if GIC is not accessible (e.g., QEMU without full GIC emulation).
    pub unsafe fn init() -> bool {
        // First, check if GIC is accessible by reading GICD_TYPER
        // If this returns 0xFFFFFFFF or causes issues, GIC is not present
        let typer = unsafe { read_volatile((GICD_BASE + GICD_TYPER) as *const u32) };
        if typer == 0xFFFF_FFFF || typer == 0 {
            // GIC not present or not responding - skip initialization
            return false;
        }

        // Disable distributor while configuring
        unsafe {
            write_volatile((GICD_BASE + GICD_CTLR) as *mut u32, 0);
        }

        // Read how many interrupts this GIC supports
        let typer = unsafe { read_volatile((GICD_BASE + GICD_TYPER) as *const u32) };
        let num_irqs = ((typer & 0x1F) + 1) * 32;

        // Disable all interrupts
        for i in (0..num_irqs).step_by(32) {
            unsafe {
                write_volatile(
                    (GICD_BASE + GICD_ICENABLER + (i / 32) as usize * 4) as *mut u32,
                    0xFFFF_FFFF,
                );
            }
        }

        // Clear all pending interrupts
        for i in (0..num_irqs).step_by(32) {
            unsafe {
                write_volatile(
                    (GICD_BASE + GICD_ICPENDR + (i / 32) as usize * 4) as *mut u32,
                    0xFFFF_FFFF,
                );
            }
        }

        // Set all interrupts to lowest priority (0xFF = lowest)
        for i in (0..num_irqs).step_by(4) {
            unsafe {
                write_volatile(
                    (GICD_BASE + GICD_IPRIORITYR + i as usize) as *mut u32,
                    0xFFFF_FFFF,
                );
            }
        }

        // Route all SPIs to CPU 0 (bits 0-7 = CPU targets)
        // PPIs (0-31) are always routed to their own CPU
        for i in (32..num_irqs).step_by(4) {
            unsafe {
                write_volatile(
                    (GICD_BASE + GICD_ITARGETSR + i as usize) as *mut u32,
                    0x0101_0101, // CPU 0 for all 4 interrupts in this word
                );
            }
        }

        // Configure all interrupts as level-triggered
        for i in (0..num_irqs).step_by(16) {
            unsafe {
                write_volatile(
                    (GICD_BASE + GICD_ICFGR + (i / 16) as usize * 4) as *mut u32,
                    0, // Level-triggered
                );
            }
        }

        // Enable distributor
        unsafe {
            write_volatile((GICD_BASE + GICD_CTLR) as *mut u32, 1);
        }

        // Initialize CPU interface
        unsafe {
            Self::init_cpu_interface();
        }

        true
    }

    /// Initialize the CPU interface for the current CPU.
    unsafe fn init_cpu_interface() {
        // Set priority mask to allow all priorities (0xFF = lowest threshold)
        unsafe {
            write_volatile((GICC_BASE + GICC_PMR) as *mut u32, 0xFF);
        }

        // Set binary point (no preemption grouping)
        unsafe {
            write_volatile((GICC_BASE + GICC_BPR) as *mut u32, 0);
        }

        // Enable CPU interface (Enable Group 0 and Group 1 interrupts)
        unsafe {
            write_volatile((GICC_BASE + GICC_CTLR) as *mut u32, 1);
        }
    }

    /// Enable a specific interrupt.
    ///
    /// # Arguments
    ///
    /// * `irq` - Interrupt number to enable (0-1019)
    ///
    /// # Safety
    ///
    /// Must be called after GIC initialization. IRQ number must be valid.
    pub unsafe fn enable_irq(irq: u32) {
        let reg_offset = (irq / 32) as usize * 4;
        let bit = 1u32 << (irq % 32);
        unsafe {
            write_volatile(
                (GICD_BASE + GICD_ISENABLER + reg_offset) as *mut u32,
                bit,
            );
        }
    }

    /// Disable a specific interrupt.
    ///
    /// # Arguments
    ///
    /// * `irq` - Interrupt number to disable (0-1019)
    ///
    /// # Safety
    ///
    /// Must be called after GIC initialization. IRQ number must be valid.
    pub unsafe fn disable_irq(irq: u32) {
        let reg_offset = (irq / 32) as usize * 4;
        let bit = 1u32 << (irq % 32);
        unsafe {
            write_volatile(
                (GICD_BASE + GICD_ICENABLER + reg_offset) as *mut u32,
                bit,
            );
        }
    }

    /// Set the priority of an interrupt.
    ///
    /// # Arguments
    ///
    /// * `irq` - Interrupt number (0-1019)
    /// * `priority` - Priority level (0 = highest, 255 = lowest)
    ///
    /// # Safety
    ///
    /// Must be called after GIC initialization. IRQ number must be valid.
    pub unsafe fn set_priority(irq: u32, priority: u8) {
        let reg_offset = irq as usize;
        let byte_offset = reg_offset & 3;
        let reg_addr = GICD_BASE + GICD_IPRIORITYR + (reg_offset & !3);

        unsafe {
            let mut val = read_volatile(reg_addr as *const u32);
            val &= !(0xFF << (byte_offset * 8));
            val |= (priority as u32) << (byte_offset * 8);
            write_volatile(reg_addr as *mut u32, val);
        }
    }

    /// Enable the physical timer interrupt.
    ///
    /// This enables IRQ 30 (EL1 Physical Timer) with medium priority.
    ///
    /// # Safety
    ///
    /// Must be called after GIC initialization.
    pub unsafe fn enable_timer_interrupt() {
        // Set medium priority for timer
        unsafe {
            Self::set_priority(TIMER_IRQ, 0x80);
        }

        // Enable the interrupt
        unsafe {
            Self::enable_irq(TIMER_IRQ);
        }
    }

    /// Disable the physical timer interrupt.
    ///
    /// # Safety
    ///
    /// Must be called after GIC initialization.
    pub unsafe fn disable_timer_interrupt() {
        unsafe {
            Self::disable_irq(TIMER_IRQ);
        }
    }

    /// Acknowledge an interrupt and get its number.
    ///
    /// This reads GICC_IAR which acknowledges the highest priority pending
    /// interrupt and returns its ID.
    ///
    /// # Returns
    ///
    /// The interrupt number (0-1019) or SPURIOUS_IRQ (1023) if spurious.
    ///
    /// # Safety
    ///
    /// Must be called from interrupt context after GIC initialization.
    #[inline]
    pub unsafe fn acknowledge_interrupt() -> u32 {
        unsafe { read_volatile((GICC_BASE + GICC_IAR) as *const u32) & 0x3FF }
    }

    /// Signal end of interrupt handling.
    ///
    /// This writes to GICC_EOIR to indicate that the interrupt has been
    /// serviced and another interrupt can now be taken.
    ///
    /// # Arguments
    ///
    /// * `irq` - The interrupt number that was acknowledged
    ///
    /// # Safety
    ///
    /// Must be called after `acknowledge_interrupt` with the returned IRQ number.
    #[inline]
    pub unsafe fn end_interrupt(irq: u32) {
        unsafe {
            write_volatile((GICC_BASE + GICC_EOIR) as *mut u32, irq);
        }
    }

    /// Get the currently running interrupt priority.
    pub fn running_priority() -> u32 {
        unsafe { read_volatile((GICC_BASE + GICC_RPR) as *const u32) & 0xFF }
    }

    /// Get the highest pending interrupt.
    pub fn highest_pending() -> u32 {
        unsafe { read_volatile((GICC_BASE + GICC_HPPIR) as *const u32) & 0x3FF }
    }

    /// Check if an interrupt is pending.
    pub fn is_pending(irq: u32) -> bool {
        let reg_offset = (irq / 32) as usize * 4;
        let bit = 1u32 << (irq % 32);
        let val = unsafe { read_volatile((GICD_BASE + GICD_ISPENDR + reg_offset) as *const u32) };
        (val & bit) != 0
    }

    /// Set an interrupt to pending (software trigger).
    ///
    /// # Safety
    ///
    /// Must be called after GIC initialization. IRQ number must be valid.
    pub unsafe fn set_pending(irq: u32) {
        let reg_offset = (irq / 32) as usize * 4;
        let bit = 1u32 << (irq % 32);
        unsafe {
            write_volatile(
                (GICD_BASE + GICD_ISPENDR + reg_offset) as *mut u32,
                bit,
            );
        }
    }

    /// Clear a pending interrupt.
    ///
    /// # Safety
    ///
    /// Must be called after GIC initialization. IRQ number must be valid.
    pub unsafe fn clear_pending(irq: u32) {
        let reg_offset = (irq / 32) as usize * 4;
        let bit = 1u32 << (irq % 32);
        unsafe {
            write_volatile(
                (GICD_BASE + GICD_ICPENDR + reg_offset) as *mut u32,
                bit,
            );
        }
    }
}

/// Initialize the GIC and enable timer interrupts.
///
/// # Safety
///
/// Must be called once during system initialization.
/// Returns true if GIC was initialized, false if GIC is not available.
pub unsafe fn init() -> bool {
    unsafe {
        if Gic400::init() {
            Gic400::enable_timer_interrupt();
            true
        } else {
            false
        }
    }
}
