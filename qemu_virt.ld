/*
 * Linker script for QEMU virt machine (aarch64)
 *
 * QEMU virt memory starts at 0x40000000
 */

ENTRY(_start)

SECTIONS
{
    /* QEMU virt loads kernel at 0x40000000 */
    . = 0x40080000;

    .text : {
        *(.text.boot)
        *(.text .text.*)
    }

    .rodata ALIGN(4096) : {
        *(.rodata .rodata.*)
    }

    .data ALIGN(4096) : {
        *(.data .data.*)
    }

    .bss ALIGN(4096) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Stack */
    . = ALIGN(16);
    . = . + 64K;
    __stack_top = .;

    /DISCARD/ : {
        *(.comment)
        *(.gnu*)
        *(.note*)
        *(.eh_frame*)
    }
}
