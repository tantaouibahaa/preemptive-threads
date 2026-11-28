preemptive-threads / pt-os - first boot on a real Pi Zero 2 W
=============================================================

Files (all rebuilt by `make sdkit` from one commit; SHA256SUMS lists them):
  bootcode.bin, start.elf, fixup.dat     Pi firmware (raspberrypi/firmware 1.20260521)
  bcm2710-rpi-zero-2-w.dtb               device tree of the Zero 2 W (same release)
  overlays/disable-bt.dtbo               moves the PL011 UART off Bluetooth onto GPIO14/15
  config.txt                             picks the kernel: one kernel= line
and five kernels, booted in this order:
  uart0.img    serial + ACT LED check, no MMU, no OS            STAGE 0
  test8.img    thread test suite (18 tests)                      STAGE 1
  kernel8.img  threading demo (LED + worker lines)               STAGE 1
  ostest8.img  pt-os test suite (21 tests)                       STAGE 2
  ptos8.img    pt-os, the full OS with its shell                 STAGE 2
None of them uses semihosting: a test kernel prints its verdict and then
halts, it does not try to power off. None uses AES/SHA instructions (the
BCM2837 has none).

Every kernel except uart0.img starts with a few "[boot]" lines:
  [boot] cpu0: entered at EL2, now EL1, MIDR 0x410fd034, CNTFRQ 19200000 Hz, SCTLR_EL1 0x30d01805: mmu on
  [boot] ram 0x0-0x1c000000 (448 MiB; firmware: 0x0+0x1c000000), board rev 0x902120, firmware 0x...
  [boot] cpu1 online      (then cpu2, cpu3)
If a board stops, the last [boot] line says how far it got. Paste them back.

STAGE 0 - wiring and config (uart0.img)
---------------------------------------

1. SD card: format a micro-SD card as FAT32 (on macOS: Disk Utility > Erase >
   "MS-DOS (FAT)", scheme "Master Boot Record"). Copy EVERY file in this folder
   to the root of the card, including the overlays folder (bootcode.bin,
   start.elf, fixup.dat, bcm2710-rpi-zero-2-w.dtb, overlays/disable-bt.dtbo,
   config.txt and the five .img files).

2. Wire the 3.3 V USB-serial adapter (Pi powered OFF):
     adapter GND -> Pi pin 6  (GND)
     adapter RX  -> Pi pin 8  (GPIO14, Pi TX)
     adapter TX  -> Pi pin 10 (GPIO15, Pi RX)
   Do NOT connect the adapter's 5V/3.3V pin. Power the Pi from its own USB port.
   (Stage 2 types into the Pi, so the adapter TX -> pin 10 wire matters there.)

3. Open the serial console on the Mac BEFORE powering the Pi:
     ls /dev/tty.usbserial* /dev/tty.usbmodem*
     screen /dev/tty.usbserial-XXXX 115200
   (quit screen with Ctrl-A then K then y)

4. config.txt already says kernel=uart0.img. Power the Pi. The green ACT LED
   toggles every second, and once a second you should see
     uart0: hello from cpu0 at EL2, MIDR=0x410fd034, #0
     uart0: hello from cpu0 at EL2, MIDR=0x410fd034, #1
   Type a few keys: each one is reported as "uart0: received 0x..".
   Paste a few lines back.
     - LED blinks, no text: RX/TX swapped (most common), GND missing, or
       the baud rate is not 115200.
     - Garbage text: UART clock (init_uart_clock in config.txt).
     - Text, but typed keys are not reported: the adapter TX -> pin 10 wire.
     - No LED at all: the files are not at the card's root, or config.txt.

STAGE 1 - the thread library
----------------------------

1. Edit config.txt on the card: kernel=test8.img (one kernel= line only).
   Power-cycle the Pi with screen open. Within ~2 s you should see the
   [boot] lines, the test kernel banner and 18 "[TEST] ... ok" lines ending in:
     ALL 18 TESTS PASSED on 4 cores
   Copy the whole serial output and paste it back.

2. Demo: change it to kernel=kernel8.img, boot again. You should see
   "4 cores online", worker lines, and the green ACT LED blinking at 2 Hz.
   Paste that output too.

STAGE 2 - pt-os, the full OS
----------------------------

Same card, same wiring, same screen session.

1. Set  kernel=ostest8.img  and power-cycle. After a few seconds it should
   end with:
     ALL 21 OS TESTS PASSED on 4 cores
   then stay silent (the cores halt). Paste the whole output.

2. Set  kernel=ptos8.img  and power-cycle. You should see:
     pt-os (Raspberry Pi (BCM2837), entered at EL2)
     [pt-os] initrd: 18 files
     welcome to pt-os: ...
     pt-os>

3. Type these at the prompt (Enter after each), and paste everything back:
     ls
     ls /bin
     primes 100000 &
     wait
     top -n 1
     mandel
     crash
     echo still alive
   Expected: ls /bin lists the programs; "primes 100000 &" prints
   "[pid N] started on cpuX" and the count 9592; "top -n 1" shows cpu0-cpu3;
   mandel draws an ASCII Mandelbrot set and names the 4 cores its workers
   ran on; crash prints ONE line "[pid N] segfault (write) at 0x0 (pc ...)"
   and the prompt comes back, so "echo still alive" works.
   Bonus: "spin 60000" then Ctrl-C should print "[pid N] interrupted".
   If keys do nothing, the adapter TX -> Pi pin 10 wire is the suspect
   (stage 0 checks it on its own).
