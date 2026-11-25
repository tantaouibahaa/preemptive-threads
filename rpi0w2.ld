/*
 * Linker script: Raspberry Pi Zero 2 W / QEMU raspi3b. The VideoCore firmware (and QEMU) load kernel8.img at 0x80000.
 *
 * Layout: .text.boot | .vectors | .text | .rodata | .data | .bss |
 *         boot stacks (one 64 KiB stack per core) | heap
 */
ENTRY(_start)

BOOT_STACK_SIZE = 0x10000;
MAX_CPUS = 4;
HEAP_SIZE = 64M;

SECTIONS
{
    . = 0x80000;
    __kernel_start = .;

    .text.boot : { KEEP(*(.text.boot)) }

    .vectors ALIGN(2048) : { KEEP(*(.vectors)) }

    .text ALIGN(4096) : { *(.text .text.*) }

    .rodata ALIGN(4096) : { *(.rodata .rodata.*) }

    .data ALIGN(4096) : { *(.data .data.*) }

    .bss ALIGN(4096) (NOLOAD) : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    .stacks ALIGN(4096) (NOLOAD) : {
        __boot_stacks_bottom = .;
        . += BOOT_STACK_SIZE * MAX_CPUS;
        __boot_stacks_top = .;
    }

    .heap ALIGN(4096) (NOLOAD) : {
        __heap_start = .;
        . += HEAP_SIZE;
        __heap_end = .;
    }

    __kernel_end = .;

    /DISCARD/ : {
        *(.comment)
        *(.gnu*)
        *(.note*)
        *(.eh_frame*)
        *(.ARM.*)
    }
}
