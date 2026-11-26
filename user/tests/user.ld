/*
 * Linker script for EL0 test programs.
 *
 * User code lives in the user window of every address space
 * (VA 0x40_0000_0000 .. 0x41_0000_0000, see src/arch/mmu.rs). Programs are
 * linked 1 MiB into it, leaving the start unmapped to catch null-ish
 * pointers. Each kind of section gets its own page-aligned PT_LOAD segment
 * so the loader can map them W^X: text R+X, rodata R, data/bss R+W.
 */
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);   /* R X */
    rodata PT_LOAD FLAGS(4);   /* R   */
    data   PT_LOAD FLAGS(6);   /* R W */
}

SECTIONS
{
    . = 0x4000100000;

    .text : { KEEP(*(.text._start)) *(.text .text.*) } :text

    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) } :rodata

    . = ALIGN(4096);
    .data : { *(.data .data.*) *(.got .got.*) } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data

    /DISCARD/ : {
        *(.comment)
        *(.gnu*)
        *(.note*)
        *(.eh_frame*)
        *(.ARM.*)
    }
}
