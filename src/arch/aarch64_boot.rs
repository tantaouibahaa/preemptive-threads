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

use core::arch::asm;

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
pub unsafe extern "C" fn _start() -> ! {
    unsafe {
        // Only CPU 0 should run the kernel
        // Other CPUs should park (wait in low-power state)
        asm!(
            "mrs x0, mpidr_el1",
            "and x0, x0, #0xFF",
            "cbz x0, 2f",       // CPU 0 continues
            "1: wfe",           // Other CPUs wait forever
            "b 1b",
            "2:",
            options(nomem, nostack)
        );

        // Set up stack pointer
        asm!(
            "adrp x0, __stack_top",
            "add x0, x0, :lo12:__stack_top",
            "mov sp, x0",
            options(nomem, nostack)
        );

        // Clear BSS section
        asm!(
            "adrp x0, __bss_start",
            "add x0, x0, :lo12:__bss_start",
            "adrp x1, __bss_end",
            "add x1, x1, :lo12:__bss_end",
            "3:",
            "cmp x0, x1",
            "b.ge 4f",
            "str xzr, [x0], #8",
            "b 3b",
            "4:",
            options(nomem, nostack)
        );

        // Initialize floating point and SIMD
        asm!(
            // Don't trap FP/SIMD access
            "mrs x0, cpacr_el1",
            "orr x0, x0, #(3 << 20)",  // FPEN = 11 (no trapping)
            "msr cpacr_el1, x0",
            "isb",
            options(nomem, nostack)
        );

        // Jump to Rust boot code
        boot_rust();
    }
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
