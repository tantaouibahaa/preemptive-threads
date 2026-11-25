//! Boot path for the primary and secondary cores.
//!
//! Every core goes through the same steps before running Rust code:
//!
//! 1. Drop from EL3 or EL2 to EL1h (with CNTHCTL_EL2, CNTVOFF_EL2,
//!    VPIDR/VMPIDR_EL2, CPTR_EL2 and SCTLR_EL1 set to sane values).
//! 2. Pick its boot stack: `__boot_stacks_top - cpu * BOOT_STACK_SIZE`.
//! 3. Enable FP/SIMD, the MMU (identity map, caches on) and install VBAR_EL1.
//!
//! The primary core additionally patches the level-1 table, clears `.bss`
//! and calls `boot_primary` with the exception level it was
//! entered at. Secondary cores enter at `__pt_secondary_entry` (released by
//! PSCI `CPU_ON` on QEMU virt or by the spin table on the Raspberry Pi) and
//! call `boot_secondary`.
//!
//! The boot stack of each core later becomes the stack of that core's idle
//! thread.

use core::arch::global_asm;

/// Size of each per-core boot stack (must match the linker scripts).
pub const BOOT_STACK_SIZE: usize = 0x10000;

global_asm!(
    r#"
.section .text.boot, "ax"
.global _start
_start:
    mrs     x19, mpidr_el1
    and     x19, x19, #0xff
    cbz     x19, 1f
    // A secondary core entered the image directly (QEMU starts every core
    // at the ELF entry). Behave like the Pi firmware: wait until the boot
    // core writes an entry address into spin-table slot 0xD8 + 8 * cpu.
    mov     x9, #0xd8
    add     x9, x9, x19, lsl #3
6:  wfe
    ldr     x10, [x9]
    cbz     x10, 6b
    br      x10
0:  wfe
    b       0b
1:
    mrs     x20, CurrentEL
    lsr     x20, x20, #2
    bl      __pt_el_to_el1

    adrp    x9, __boot_stacks_top
    add     x9, x9, :lo12:__boot_stacks_top
    mov     sp, x9

    // BOOT_L1[0] = &BOOT_L2 | table descriptor
    adrp    x9, {l2}
    add     x9, x9, :lo12:{l2}
    orr     x9, x9, #3
    adrp    x10, {l1}
    add     x10, x10, :lo12:{l1}
    str     x9, [x10]
    dsb     sy

    bl      __pt_cpu_setup

    adrp    x9, __bss_start
    add     x9, x9, :lo12:__bss_start
    adrp    x10, __bss_end
    add     x10, x10, :lo12:__bss_end
2:  cmp     x9, x10
    b.hs    3f
    stp     xzr, xzr, [x9], #16
    b       2b
3:
    mov     x0, x20
    bl      {primary}
    b       0b

.global __pt_secondary_entry
__pt_secondary_entry:
    mrs     x19, mpidr_el1
    and     x19, x19, #0xff
    bl      __pt_el_to_el1

    adrp    x9, __boot_stacks_top
    add     x9, x9, :lo12:__boot_stacks_top
    mov     x10, #{stack_size}
    msub    x9, x19, x10, x9
    mov     sp, x9

    bl      __pt_cpu_setup

    mov     x0, x19
    bl      {secondary}
    b       0b

// Drop to EL1h. Clobbers x9 only; returns through x30.
__pt_el_to_el1:
    mrs     x9, CurrentEL
    lsr     x9, x9, #2
    cmp     x9, #2
    b.eq    5f
    b.hi    4f
    ret
4:  // EL3 -> EL2 (non-secure, AArch64 EL2, HVC enabled)
    ldr     x9, =0x5b1
    msr     scr_el3, x9
    msr     cptr_el3, xzr
    mov     x9, #0x3c9
    msr     spsr_el3, x9
    adr     x9, 5f
    msr     elr_el3, x9
    eret
5:  // EL2 -> EL1
    mov     x9, #(1 << 31)          // HCR_EL2.RW: EL1 is AArch64, no traps
    msr     hcr_el2, x9
    mrs     x9, cnthctl_el2
    orr     x9, x9, #3              // EL1PCTEN | EL1PCEN: EL1 may use the physical timer
    msr     cnthctl_el2, x9
    msr     cntvoff_el2, xzr
    mov     x9, #0x33ff             // CPTR_EL2: do not trap FP/SIMD
    msr     cptr_el2, x9
    msr     hstr_el2, xzr
    mrs     x9, midr_el1
    msr     vpidr_el2, x9
    mrs     x9, mpidr_el1
    msr     vmpidr_el2, x9
    ldr     x9, ={sctlr_off}
    msr     sctlr_el1, x9
    mov     x9, #0x3c5              // EL1h, DAIF masked
    msr     spsr_el2, x9
    msr     elr_el2, x30
    eret

// FP/SIMD on, MMU + caches on, vector table installed. Clobbers x9.
__pt_cpu_setup:
    mov     x9, #(3 << 20)
    msr     cpacr_el1, x9
    ldr     x9, ={mair}
    msr     mair_el1, x9
    ldr     x9, ={tcr}
    msr     tcr_el1, x9
    adrp    x9, {l1}
    add     x9, x9, :lo12:{l1}
    msr     ttbr0_el1, x9
    dsb     sy
    tlbi    vmalle1
    ic      iallu
    dsb     sy
    isb
    ldr     x9, ={sctlr_on}
    msr     sctlr_el1, x9
    isb
    adrp    x9, {vectors}
    add     x9, x9, :lo12:{vectors}
    msr     vbar_el1, x9
    isb
    ret
.ltorg
"#,
    l1 = sym super::mmu::BOOT_L1,
    l2 = sym super::mmu::BOOT_L2,
    primary = sym crate::kernel::boot_primary,
    secondary = sym crate::kernel::boot_secondary,
    vectors = sym super::vectors::__pt_vectors,
    stack_size = const BOOT_STACK_SIZE,
    mair = const super::mmu::MAIR,
    tcr = const super::mmu::TCR,
    sctlr_on = const super::mmu::SCTLR_MMU_ON,
    sctlr_off = const super::mmu::SCTLR_MMU_OFF,
);

extern "C" {
    /// Entry point for secondary cores (physical address, identity mapped).
    pub fn __pt_secondary_entry();
}

/// Address secondary cores should be released to.
pub fn secondary_entry_addr() -> usize {
    __pt_secondary_entry as *const () as usize
}

extern "C" {
    static __heap_start: u8;
    static __heap_end: u8;
    static __boot_stacks_top: u8;
}

/// `(start, end)` of the heap region reserved by the linker script.
pub fn heap_region() -> (usize, usize) {
    (core::ptr::addr_of!(__heap_start) as usize, core::ptr::addr_of!(__heap_end) as usize)
}

/// `(bottom, top)` of the boot stack of `cpu`.
pub fn boot_stack(cpu: usize) -> (usize, usize) {
    let top = core::ptr::addr_of!(__boot_stacks_top) as usize - cpu * BOOT_STACK_SIZE;
    (top - BOOT_STACK_SIZE, top)
}
