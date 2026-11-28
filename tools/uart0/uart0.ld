/* uart0.img: loaded at 0x80000 by the Pi firmware (kernel_address) and QEMU. */
ENTRY(_start)

SECTIONS
{
    . = 0x80000;
    .text : { KEEP(*(.text.boot)) *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    /DISCARD/ : { *(.comment) *(.gnu*) *(.note*) *(.eh_frame*) *(.ARM.*) }
}
