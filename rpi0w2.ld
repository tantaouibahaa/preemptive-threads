/*
 * Linker script for Raspberry Pi Zero 2 W bare-metal kernel
 *
 * Memory Layout:
 * - 0x00000000 - 0x0007FFFF: GPU reserved / VideoCore
 * - 0x00080000 - ...:        Kernel load address
 *
 * The GPU firmware loads kernel8.img at 0x80000 and jumps to it.
 */

ENTRY(_start)

SECTIONS
{
    /* Kernel loaded at 0x80000 by GPU firmware */
    . = 0x80000;

    /* Boot code must be first - GPU jumps here */
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

    /* BSS - uninitialized data (cleared by boot code) */
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

    /* Discard unwanted sections */
    /DISCARD/ : {
        *(.comment)
        *(.gnu*)
        *(.note*)
        *(.eh_frame*)
        *(.ARM.*)
    }
}

/* Provide symbols for kernel code */
PROVIDE(__kernel_start = 0x80000);
PROVIDE(__kernel_size = __kernel_end - __kernel_start);
