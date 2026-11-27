#!/usr/bin/env python3
"""Drive a pt-os shell under QEMU through its serial console.

usage: scripts/shell-session.py <virt|virt-el2|raspi3b> <kernel> [timeout_s] [log]
                                [--demo] [--record=FILE.cast]

QEMU's serial port is connected to this script's pipes (-serial stdio), so
every keystroke goes through the real PL011 receive interrupt, the line
discipline and the shell. Commands are typed one character at a time (with
a small delay, like a person or a terminal paste), each step waits for the
expected output and the next prompt, and the run fails on the first
mismatch or when the overall timeout expires. QEMU is always killed at the
end. The transcript is printed and written to the log.

--demo runs a few more commands after the checks (ls -l, ps, mandel, top,
stress, help). --record=FILE.cast instead types a short demo session at
human speed (still checking every step) and saves the serial output, with
the time each chunk arrived, as an asciicast v2 file; scripts/cast2gif.py
renders it.
"""

import json
import os
import random
import re
import subprocess
import sys
import threading
import time

PROMPT = "pt-os> "


class Session:
    def __init__(self, machine, kernel, deadline):
        args = {
            "virt": ["-M", "virt,gic-version=2", "-cpu", "cortex-a53", "-smp", "4", "-m", "512M"],
            "virt-el2": ["-M", "virt,gic-version=2,virtualization=on", "-cpu", "cortex-a53", "-smp", "4", "-m", "512M"],
            "raspi3b": ["-M", "raspi3b", "-cpu", "cortex-a53"],
        }[machine]
        self.deadline = deadline
        self.buf = ""
        self.lock = threading.Lock()
        self.qemu = subprocess.Popen(
            ["qemu-system-aarch64", *args, "-display", "none", "-monitor", "none", "-serial", "stdio", "-kernel", kernel],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            bufsize=0,
        )
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.mark = 0  # where the current step's output starts
        self.start = time.time()
        self.events = []  # (seconds since start, raw output) for --record
        self.human = None  # random.Random: type at human speed

    def _read(self):
        fd = self.qemu.stdout.fileno()
        while True:
            try:
                data = os.read(fd, 4096)
            except OSError:
                return
            if not data:
                return
            text = data.decode("utf-8", "replace")
            with self.lock:
                self.events.append((time.time() - self.start, text))
                self.buf += text.replace("\r", "")

    def text(self):
        with self.lock:
            return self.buf

    def expect(self, pattern, what, timeout=30.0):
        """Wait until `pattern` (regex) appears after the mark. Returns the match."""
        rx = re.compile(pattern, re.S)
        end = min(time.time() + timeout, self.deadline)
        while time.time() < end:
            m = rx.search(self.text(), self.mark)
            if m:
                return m
            if self.qemu.poll() is not None:
                break
            time.sleep(0.02)
        raise AssertionError(f"{what}: no match for {pattern!r} within {timeout:.0f}s")

    def type(self, s, delay=0.01):
        for ch in s:
            self.qemu.stdin.write(ch.encode())
            self.qemu.stdin.flush()
            if self.human and ch != "\r":
                time.sleep(self.human.uniform(0.05, 0.13))
            else:
                time.sleep(delay)

    def command(self, cmd, pattern=None, what=None, timeout=30.0):
        """Type `cmd` + Enter, wait for `pattern` and the next prompt. Returns the output."""
        self.mark = len(self.text())
        self.type(cmd + "\r")
        m = self.expect(re.escape(PROMPT), f"prompt after {cmd!r}", timeout)
        out = self.text()[self.mark : m.start()]
        if pattern is not None and not re.search(pattern, out, re.S):
            raise AssertionError(f"{what or cmd}: no match for {pattern!r} in output {out!r}")
        self.mark = m.end()
        return out

    def close(self):
        try:
            self.qemu.terminate()
            self.qemu.wait(timeout=5)
        except Exception:
            self.qemu.kill()
            self.qemu.wait()


def run(s, demo):
    s.expect(r"welcome to pt-os", "motd", 60)
    s.expect(re.escape(PROMPT), "first prompt", 60)
    s.mark = len(s.text())
    checks = []

    def check(name, cmd, pattern, timeout=30.0):
        s.command(cmd, pattern, name, timeout)
        checks.append(name)

    check("pipe", "echo hi | wc", r"^\s*echo hi \| wc\n\s+1\s+1\s+3\s*\n")
    check("ls /bin", "ls /bin", r"(?s)mandel.*primes.*sh.*top")
    bg_mark = len(s.text())
    out = s.command("primes 100000 &", r"\[pid (\d+)\] started on cpu[0-3]", "background job")
    pid = re.search(r"\[pid (\d+)\] started on cpu([0-3])", out).group(1)
    checks.append("background job")
    s.command("wait", None, "wait", 60)
    # The job may finish (and print) before `wait` is typed.
    tail = s.text()[bg_mark:]
    for pattern in (r"primes below 100000: 9592 ", rf"\[pid {pid}\] done \(exited 0\)"):
        if not re.search(pattern, tail):
            raise AssertionError(f"wait: no match for {pattern!r} in {tail!r}")
    checks.append("wait")
    check("redirect >", "cat /etc/motd > /tmp/x", r"^\s*cat /etc/motd > /tmp/x\n$")
    check("cat redirected file", "cat /tmp/x", r"welcome to pt-os")
    check(">> append", "echo second line >> /tmp/x", r"^\s*echo second line >> /tmp/x\n$")
    check("< redirect", "wc < /tmp/x", r"\s+\d+\s+\d+\s+\d+\n")
    check("3-stage pipeline", "cat /etc/motd | grep pt-os | wc", r"\s+1\s+\d+\s+\d+\n")
    out = s.command("crash", r"\[pid \d+\] segfault \(write\) at 0x0 \(pc 0x[0-9a-f]+\)\n", "crash")
    if out.count("segfault") != 1:
        raise AssertionError(f"crash: expected one fault line, got {out!r}")
    checks.append("crash: one fault line")
    check("shell survives", "echo still alive", r"still alive")
    check("; and &&", "echo one; grep zzz /etc/motd && echo no ; echo two && echo three",
          r"\none\ntwo\nthree\n$")
    check("mandel", "mandel", r"(?s)@@@@.*\nmandel: 78x32, 150000 iterations, 4 workers .* ms\n$", 60)

    # Ctrl-C a long-running foreground job.
    # (Ctrl-C again if the first one came before the shell started the job.)
    s.mark = len(s.text())
    s.type("spin 60000\r")
    s.expect(r"spin 60000\n", "spin started")
    time.sleep(1.0)
    ctrl_c(s, r"\^C\n.*\[pid \d+\] interrupted\n" + re.escape(PROMPT), "Ctrl-C")
    checks.append("Ctrl-C")
    check("after Ctrl-C", "echo back", r"back")

    # Ctrl-C while `wait` waits for a background job: the job is
    # interrupted and the shell returns to the prompt.
    out = s.command("spin 60000 &", r"\[pid (\d+)\] started", "spin &")
    pid = re.search(r"\[pid (\d+)\] started", out).group(1)
    s.mark = len(s.text())
    s.type("wait\r")
    s.expect(r"wait\n", "wait started")
    time.sleep(0.5)
    ctrl_c(s, rf"\^C\n.*\[pid {pid}\] interrupted\n" + re.escape(PROMPT), "Ctrl-C in wait")
    checks.append("Ctrl-C in wait")
    check("no jobs left", "jobs", r"^\s*jobs\n$")
    if demo:
        s.command("ls -l /tmp")
        s.command("ps")
        s.command("jobs")
        s.command("mandel 64 22 500", r"mandel: 64x22", "mandel", 120)
        s.command("top -n 1", r"cpu0", "top", 30)
        s.command("stress 40", r"before (\d+), after \1 \(\+0\)", "stress", 120)
        s.command("help")
    return checks


def ctrl_c(s, pattern, what):
    """Press Ctrl-C until `pattern` shows up (again if the first one came
    before the shell had started the job), then let any extra prompt from
    an early Ctrl-C come out."""
    for attempt in range(5):
        s.type("\x03")
        try:
            s.expect(pattern, what, 3)
            break
        except AssertionError:
            if attempt == 4:
                raise
    time.sleep(0.5)
    s.mark = len(s.text())


def record(s):
    """A short demo, typed at human speed. Every step is still checked."""
    s.human = random.Random(2)
    s.expect(r"welcome to pt-os", "motd", 60)
    s.expect(re.escape(PROMPT), "first prompt", 60)
    s.mark = len(s.text())
    checks = []

    def step(cmd, pattern=None, timeout=60.0, before=0.7, after=1.4):
        time.sleep(before)
        s.command(cmd, pattern, cmd, timeout)
        checks.append(cmd)
        time.sleep(after)

    step("ls /bin", r"mandel")
    step("primes 50000000 &", r"\[pid \d+\] started on cpu[0-3]", after=0.1)
    step("wait", r"primes below 50000000: 3001134 .*\[pid \d+\] done \(exited 0\)", before=0.2)
    step("cat /etc/motd | grep pt-os | wc", r"\s+1\s+\d+\s+\d+\n")
    step("spin 5000 &", r"started on cpu", after=0.2)
    step("top -n 3 -d 600", r"cpu3 .*\n.*spin", after=1.8)
    step("mandel", r"mandel: 78x32", after=3.0)
    step("crash", r"segfault \(write\) at 0x0")
    step("echo still alive", r"still alive")
    time.sleep(0.7)
    s.mark = len(s.text())
    s.type("spin 60000\r")
    s.expect(r"spin 60000\n", "spin started")
    time.sleep(1.5)
    ctrl_c(s, r"\^C\n.*\[pid \d+\] interrupted\n" + re.escape(PROMPT), "Ctrl-C")
    checks.append("Ctrl-C on spin")
    time.sleep(2.5)
    return checks


def write_cast(s, path, width=80, height=36):
    """Save the serial output as an asciicast v2 recording."""
    with s.lock:
        events = list(s.events)
    with open(path, "w") as f:
        header = {"version": 2, "width": width, "height": height, "timestamp": int(s.start),
                  "title": "pt-os shell on QEMU virt", "env": {"TERM": "xterm-256color"}}
        f.write(json.dumps(header) + "\n")
        for t, text in events:
            if "qemu-system-aarch64: terminating" in text:
                break
            text = text.replace("\r\n", "\n").replace("\n", "\r\n")
            f.write(json.dumps([round(t, 4), "o", text]) + "\n")


def main():
    argv = [a for a in sys.argv[1:] if not a.startswith("--")]
    demo = "--demo" in sys.argv
    cast = next((a.split("=", 1)[1] for a in sys.argv[1:] if a.startswith("--record=")), None)
    machine, kernel = argv[0], argv[1]
    timeout = float(argv[2]) if len(argv) > 2 else 180.0
    log = argv[3] if len(argv) > 3 else None
    s = Session(machine, kernel, time.time() + timeout)
    ok = False
    try:
        checks = record(s) if cast else run(s, demo)
        ok = True
        if cast:
            write_cast(s, cast)
    except AssertionError as e:
        failure = str(e)
    finally:
        s.close()
    transcript = s.text()
    sys.stdout.write(transcript)
    if ok:
        verdict = f"SHELL SESSION PASSED: {len(checks)} checks ({', '.join(checks)})\n"
    else:
        verdict = f"SHELL SESSION FAILED: {failure}\n"
    sys.stdout.write("\n" + verdict)
    if log:
        with open(log, "w") as f:
            f.write(transcript + "\n" + verdict)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
