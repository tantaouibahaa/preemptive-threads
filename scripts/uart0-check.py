#!/usr/bin/env python3
"""Boot uart0.img on QEMU raspi3b and check what it prints.

usage: scripts/uart0-check.py <uart0.img> [log]

Expects at least three `uart0: hello from cpu0 at EL2, MIDR=0x...` lines
about a second apart, and the two bytes typed into the serial port after
the first one to be reported back as `uart0: received 0x48` / `0x69`.
"""
import os
import re
import select
import subprocess
import sys
import time

img = sys.argv[1]
log = sys.argv[2] if len(sys.argv) > 2 else None

qemu = subprocess.Popen(
    ["qemu-system-aarch64", "-M", "raspi3b", "-cpu", "cortex-a53", "-display", "none",
     "-monitor", "none", "-serial", "stdio", "-kernel", img],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
out = b""
sent = False
start = time.monotonic()
try:
    while time.monotonic() - start < 4.5:
        ready, _, _ = select.select([qemu.stdout], [], [], 0.1)
        if ready:
            chunk = os.read(qemu.stdout.fileno(), 4096)
            if not chunk:
                break
            out += chunk
        if not sent and b"#0" in out:
            qemu.stdin.write(b"Hi")
            qemu.stdin.flush()
            sent = True
finally:
    qemu.kill()
    qemu.wait()

text = out.decode(errors="replace").replace("\r\n", "\n")
sys.stdout.write(text)
if log:
    with open(log, "w") as f:
        f.write(text)

hello = re.findall(r"^uart0: hello from cpu0 at EL2, MIDR=0x410fd034, #(\d+)$", text, re.M)
problems = []
if len(hello) < 3 or hello[:3] != ["0", "1", "2"]:
    problems.append(f"expected hello lines #0, #1, #2, got {hello}")
for b in ("0x48", "0x69"):
    if f"uart0: received {b}\n" not in text:
        problems.append(f"no 'received {b}' line")
if problems:
    print("UART0 CHECK FAILED: " + "; ".join(problems))
    sys.exit(1)
print(f"UART0 CHECK PASSED: {len(hello)} hello lines, RX echo of 'Hi'")
