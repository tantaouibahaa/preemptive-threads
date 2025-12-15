//! Boot code for Raspberry Pi Zero 2 W.
//!
//! This module handles early initialization before the kernel starts:
//! - BSS clearing
//! - Stack setup
//! - Exception vector installation
//! - Architecture initialization
//!
//! # Memory Layout
//!
//! The kernel is loaded at 0x80000 by the Raspberry Pi GPU firmware.
//! The linker script defines:
//! - `.text.boot` - Entry point (must be first)
//! - `.vectors` - Exception vector table (2KB aligned)
//! - `.text` - Code
//! - `.rodata` - Read-only data
//! - `.data` - Initialized data
//! - `.bss` - Uninitialized data (cleared by boot code)
//!
//! Stack and heap are placed after BSS.

use core::arch::{asm, naked_asm};

// Symbols defined by linker script
extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
    static __stack_top: u8;
    static __heap_start: u8;
    static __heap_end: u8;
}

/// Kernel entry point.
///
/// This is the first code executed after the GPU firmware loads the kernel.
/// It runs on CPU 0; other CPUs are parked.
///
/// # Safety
///
/// This function must be the first thing in `.text.boot` section.
/// It sets up the environment and calls `kernel_main`.
#[cfg(target_arch = "aarch64")]
#[link_section = ".text.boot"]
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn _start() -> ! {
    // Boot code in naked assembly - handles EL3/EL2/EL1 entry
    // Works on both real Pi (starts at EL1/EL2) and QEMU (starts at EL3)
    naked_asm!(
            // Park secondary CPUs (only CPU 0 runs the kernel)
            "mrs x0, mpidr_el1",
            "and x0, x0, #0xFF",
            "cbnz x0, .Lpark",

            // Check current exception level and drop to EL1 if needed
            "mrs x0, CurrentEL",
            "lsr x0, x0, #2",           // Extract EL field (bits 3:2)
            "cmp x0, #3",
            "b.eq .Lfrom_el3",
            "cmp x0, #2",
            "b.eq .Lfrom_el2",
            "b .Lat_el1",               // Already at EL1

        ".Lfrom_el3:",
            // At EL3: Configure EL2 and drop to EL1
            // SCR_EL3: RW=1 (EL2 is AArch64), NS=1 (non-secure), HCE=1
            "mov x0, #0b1010001001",    // RW | HCE | NS | RES1 bits
            "msr scr_el3, x0",

            // SPSR_EL3: Return to EL1h with interrupts masked
            "mov x0, #0b00101",         // EL1h
            "orr x0, x0, #(0xF << 6)",  // Mask DAIF
            "msr spsr_el3, x0",

            // Set return address to EL1 entry
            "adr x0, .Lat_el1",
            "msr elr_el3, x0",
            "eret",

        ".Lfrom_el2:",
            // At EL2: Configure and drop to EL1
            // HCR_EL2: RW=1 (EL1 is AArch64)
            "mov x0, #(1 << 31)",       // RW bit
            "msr hcr_el2, x0",

            // SPSR_EL2: Return to EL1h with interrupts masked
            "mov x0, #0b00101",         // EL1h
            "orr x0, x0, #(0xF << 6)",  // Mask DAIF
            "msr spsr_el2, x0",

            // Set return address to EL1 entry
            "adr x0, .Lat_el1",
            "msr elr_el2, x0",
            "eret",

        ".Lat_el1:",
            // Now at EL1 - set up stack
            "adrp x0, __stack_top",
            "add x0, x0, :lo12:__stack_top",
            "mov sp, x0",

            // Clear BSS section
            "adrp x0, __bss_start",
            "add x0, x0, :lo12:__bss_start",
            "adrp x1, __bss_end",
            "add x1, x1, :lo12:__bss_end",
        ".Lclear_bss:",
            "cmp x0, x1",
            "b.ge .Lbss_done",
            "str xzr, [x0], #8",
            "b .Lclear_bss",
        ".Lbss_done:",

            // Enable FP/SIMD (don't trap to EL1)
            "mrs x0, cpacr_el1",
            "orr x0, x0, #(3 << 20)",   // FPEN = 11
            "msr cpacr_el1, x0",
            "isb",

            // Jump to Rust boot code
            "b {boot_rust}",

        ".Lpark:",
            // Secondary CPUs wait forever
            "wfe",
            "b .Lpark",

            boot_rust = sym boot_rust,
    );
}

/// Rust boot code - called after basic ASM setup.
#[cfg(target_arch = "aarch64")]
unsafe fn boot_rust() -> ! {
    unsafe {
        // Install exception vector table
        super::aarch64_vectors::install_vector_table();

        // Initialize GIC interrupt controller
        super::aarch64_gic::init();

        // Initialize architecture-specific features
        super::aarch64::init();

        // Call user's kernel_main
        extern "Rust" {
            fn kernel_main() -> !;
        }

        kernel_main();
    }
}

/// Get the heap start address.
pub fn heap_start() -> usize {
    unsafe { &__heap_start as *const u8 as usize }
}

/// Get the heap end address.
pub fn heap_end() -> usize {
    unsafe { &__heap_end as *const u8 as usize }
}

/// Get the heap size in bytes.
pub fn heap_size() -> usize {
    heap_end() - heap_start()
}

/// Get the stack top address.
pub fn stack_top() -> usize {
    unsafe { &__stack_top as *const u8 as usize }
}

/// Park the current CPU (enter low-power wait state).
///
/// This is useful for parking secondary CPUs that aren't being used.
#[inline]
pub fn park_cpu() -> ! {
    loop {
        unsafe {
            asm!("wfe", options(nomem, nostack));
        }
    }
}

/// Halt the CPU with interrupts disabled.
///
/// This is used for fatal errors.
#[inline]
pub fn halt() -> ! {
    unsafe {
        asm!("msr daifset, #0xf", options(nomem, nostack)); // Disable all interrupts
    }
    loop {
        unsafe {
            asm!("wfe", options(nomem, nostack));
        }
    }
}

// Fallback for non-ARM64 targets (testing)
#[cfg(not(target_arch = "aarch64"))]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    loop {}
}
