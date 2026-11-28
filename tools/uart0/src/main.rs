//! `uart0.img`: the first thing to boot on a new Raspberry Pi Zero 2 W.
//!
//! A few hundred bytes of assembly with no MMU, no caches setup, no stack
//! beyond a few frames and no Rust runtime. It checks the SD card, the
//! firmware configuration and the serial wiring before anything else:
//!
//! - GPIO14/15 to ALT0 (PL011 TXD/RXD), a pull-up on RXD, none on TXD;
//!   GPIO32/33 (the Bluetooth UART pins) to inputs;
//! - PL011 at 115200 8N1 from the 48 MHz clock (`init_uart_clock=48000000`);
//! - once a second, forever:
//!   `uart0: hello from cpu0 at EL<n>, MIDR=0x........, #<count>`
//!   and the green ACT LED (GPIO29, active high) toggles;
//! - every byte received is reported as `uart0: received 0x..`, which
//!   checks the adapter's TX -> Pi RX wire.
//!
//! The ACT LED blinking without serial output points at the wiring or the
//! baud rate; no blinking at all points at the card or `config.txt`. The
//! image also runs on QEMU `raspi3b` (`make run-uart0`).

#![no_std]
#![no_main]

use core::arch::global_asm;

global_asm!(
    r#"
.equ GPIO,      0x3f200000
.equ GPFSEL1,   0x04
.equ GPFSEL2,   0x08
.equ GPFSEL3,   0x0c
.equ GPSET0,    0x1c
.equ GPCLR0,    0x28
.equ GPPUD,     0x94
.equ GPPUDCLK0, 0x98
.equ UART,      0x3f201000
.equ DR,        0x00
.equ FR,        0x18
.equ IBRD,      0x24
.equ FBRD,      0x28
.equ LCRH,      0x2c
.equ CR,        0x30
.equ IMSC,      0x38
.equ ICR,       0x44

.section .text.boot, "ax"
.global _start
_start:
    mrs     x0, mpidr_el1
    and     x0, x0, #0xff
    cbz     x0, 2f
1:  wfe                         // only core 0 runs this
    b       1b
2:  ldr     x0, =0x80000        // stack below the image
    mov     sp, x0
    mov     x19, #0             // x19: message count
    mov     x20, #0             // x20: LED state
    mrs     x21, cntfrq_el0     // x21: counter ticks per second
    cbnz    x21, 3f
    ldr     x21, =19200000      // the Pi's crystal, if nobody set CNTFRQ
3:  ldr     x22, =GPIO
    ldr     x23, =UART

    // Pins. GPIO14/15: ALT0 (0b100).
    dsb     sy
    ldr     w0, [x22, #GPFSEL1]
    and     w0, w0, #~(0x3f << 12)
    ldr     w1, =(0x24 << 12)
    orr     w0, w0, w1
    str     w0, [x22, #GPFSEL1]
    // GPIO29 (ACT LED): output (0b001).
    ldr     w0, [x22, #GPFSEL2]
    and     w0, w0, #~(7 << 27)
    orr     w0, w0, #(1 << 27)
    str     w0, [x22, #GPFSEL2]
    // GPIO32/33: input.
    ldr     w0, [x22, #GPFSEL3]
    and     w0, w0, #~(0x3f << 6)
    str     w0, [x22, #GPFSEL3]
    // Pulls: none on GPIO14, up on GPIO15.
    mov     w0, #0
    mov     w1, #(1 << 14)
    bl      set_pull
    mov     w0, #2
    mov     w1, #(1 << 15)
    bl      set_pull

    // PL011: off, wait until idle, 115200 8N1 with FIFOs, no interrupts.
    str     wzr, [x23, #CR]
4:  ldr     w0, [x23, #FR]
    tbnz    w0, #3, 4b
    mov     w0, #0x7ff
    str     w0, [x23, #ICR]
    str     wzr, [x23, #IMSC]
    mov     w0, #26
    str     w0, [x23, #IBRD]
    mov     w0, #3
    str     w0, [x23, #FBRD]
    mov     w0, #0x70
    str     w0, [x23, #LCRH]
    mov     w0, #0x301
    str     w0, [x23, #CR]
    dsb     sy

main_loop:
    adrp    x0, msg_hello
    add     x0, x0, :lo12:msg_hello
    bl      puts
    mrs     x0, CurrentEL
    ubfx    x0, x0, #2, #2
    add     w0, w0, #48
    bl      putc
    adrp    x0, msg_midr
    add     x0, x0, :lo12:msg_midr
    bl      puts
    mrs     x0, midr_el1
    mov     x1, #8
    bl      puthex
    adrp    x0, msg_count
    add     x0, x0, :lo12:msg_count
    bl      puts
    mov     x0, x19
    bl      putdec
    mov     w0, #10
    bl      putc
    add     x19, x19, #1

    // Toggle the ACT LED.
    eor     x20, x20, #1
    mov     w0, #(1 << 29)
    dsb     sy
    cbz     x20, 5f
    str     w0, [x22, #GPSET0]
    b       6f
5:  str     w0, [x22, #GPCLR0]
6:  dsb     sy

    // Wait a second, reporting received bytes.
    isb
    mrs     x24, cntpct_el0
    add     x24, x24, x21
7:  ldr     w0, [x23, #FR]
    tbnz    w0, #4, 8f          // RXFE: nothing received
    ldr     w25, [x23, #DR]
    adrp    x0, msg_rx
    add     x0, x0, :lo12:msg_rx
    bl      puts
    and     x0, x25, #0xff
    mov     x1, #2
    bl      puthex
    mov     w0, #10
    bl      putc
8:  isb
    mrs     x0, cntpct_el0
    cmp     x0, x24
    b.lo    7b
    b       main_loop

// set_pull(w0 = GPPUD value, w1 = GPPUDCLK0 mask): the BCM2835 sequence,
// each step held for 10 us on the counter (at least 150 VPU cycles).
set_pull:
    str     x30, [sp, #-16]!
    str     w0, [x22, #GPPUD]
    dsb     sy
    bl      wait_10us
    str     w1, [x22, #GPPUDCLK0]
    dsb     sy
    bl      wait_10us
    str     wzr, [x22, #GPPUD]
    str     wzr, [x22, #GPPUDCLK0]
    dsb     sy
    ldr     x30, [sp], #16
    ret

// Clobbers x2-x4.
wait_10us:
    ldr     x2, =100000
    udiv    x2, x21, x2         // ticks per 10 us
    add     x2, x2, #1
    isb
    mrs     x3, cntpct_el0
    add     x3, x3, x2
9:  isb
    mrs     x4, cntpct_el0
    cmp     x4, x3
    b.lo    9b
    ret

// putc(w0): blocking. Clobbers w9.
putc:
    ldr     w9, [x23, #FR]
    tbnz    w9, #5, putc        // TXFF
    str     w0, [x23, #DR]
    ret

// puts(x0 = NUL-terminated string), \n -> \r\n. Clobbers x0, x10, w9.
puts:
    str     x30, [sp, #-16]!
    mov     x10, x0
10: ldrb    w0, [x10], #1
    cbz     w0, 12f
    cmp     w0, #10
    b.ne    11f
    mov     w0, #13
    bl      putc
    mov     w0, #10
11: bl      putc
    b       10b
12: ldr     x30, [sp], #16
    ret

// puthex(x0 = value, x1 = digits): "0x" and x1 hex digits. Clobbers x0-x2, x10-x11, w9.
puthex:
    str     x30, [sp, #-16]!
    mov     x10, x0
    lsl     x11, x1, #2         // bit position past the top digit
    mov     w0, #48
    bl      putc
    mov     w0, #120
    bl      putc
13: sub     x11, x11, #4
    lsr     x0, x10, x11
    and     x0, x0, #0xf
    cmp     x0, #10
    add     x1, x0, #48
    add     x2, x0, #87
    csel    x0, x1, x2, lo
    bl      putc
    cbnz    x11, 13b
    ldr     x30, [sp], #16
    ret

// putdec(x0): unsigned decimal. Clobbers x0-x2, x10-x11, w9.
putdec:
    stp     x30, xzr, [sp, #-48]!
    add     x11, sp, #48        // digits are stored backwards below here
    mov     x10, x0
    mov     x2, #10
14: udiv    x1, x10, x2
    msub    x0, x1, x2, x10
    add     w0, w0, #48
    strb    w0, [x11, #-1]!
    mov     x10, x1
    cbnz    x10, 14b
15: ldrb    w0, [x11], #1
    bl      putc
    add     x0, sp, #48
    cmp     x11, x0
    b.lo    15b
    ldp     x30, xzr, [sp], #48
    ret
.ltorg

.section .rodata.uart0, "a"
msg_hello: .asciz "uart0: hello from cpu0 at EL"
msg_midr:  .asciz ", MIDR="
msg_count: .asciz ", #"
msg_rx:    .asciz "uart0: received "
"#
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
