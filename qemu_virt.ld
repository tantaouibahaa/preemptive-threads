/*
 * Linker script for QEMU virt machine (aarch64)
 *
 * QEMU virt memory starts at 0x40000000
 * -kernel flag loads ELF at its specified addresses
 */

ENTRY(_start)

SECTIONS
{
    /* QEMU virt loads kernel at 0x40080000 */
    . = 0x40080000;

    /* Boot code must be first */
    .text.boot : {
        KEEP(*(.text.boot))
    }

    /* Exception vector table - must be 2048-byte aligned */
    .vectors ALIGN(2048) : {
        KEEP(*(.vectors))
    }

    /* Code section */
    .text ALIGN(4096) : {
        *(.text .text.*)
    }

    /* Read-only data */
    .rodata ALIGN(4096) : {
        *(.rodata .rodata.*)
    }

    /* Initialized data */
    .data ALIGN(4096) : {
        *(.data .data.*)
    }

    /* BSS - uninitialized data */
    .bss ALIGN(4096) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Stack - grows downward from __stack_top */
    . = ALIGN(16);
    __stack_bottom = .;
    . = . + 1M;  /* 1MB kernel stack */
    __stack_top = .;

    /* Heap for dynamic allocation */
    . = ALIGN(16);
    __heap_start = .;
    . = . + 16M;  /* 16MB heap */
    __heap_end = .;

    /* End of kernel image */
    __kernel_end = .;

    /DISCARD/ : {
        *(.comment)
        *(.gnu*)
        *(.note*)
        *(.eh_frame*)
        *(.ARM.*)
    }
}

PROVIDE(__kernel_start = 0x40080000);
PROVIDE(__kernel_size = __kernel_end - __kernel_start);
