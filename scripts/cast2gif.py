#!/usr/bin/env python3
"""Render an asciicast v2 recording (scripts/shell-session.py --record) to
an animated GIF.

usage: scripts/cast2gif.py <in.cast> <out.gif> [--font=PATH] [--size=PX]
                           [--max-idle=SECONDS]

A small terminal emulator replays the recorded output with its recorded
timing: printable characters, CR, LF, backspace and the few escape
sequences pt-os programs use (cursor home, clear screen, erase line).
Idle gaps longer than --max-idle are shortened. Needs Pillow.
"""

import json
import os
import re
import sys

from PIL import Image, ImageDraw, ImageFont

BG = (24, 26, 33)
FG = (214, 219, 226)
BAR = (44, 47, 58)
TITLE = (150, 156, 168)
DOTS = [(237, 106, 94), (245, 191, 79), (98, 197, 84)]
LEVELS = 16  # grey levels between BG and FG for antialiased text

CSI = re.compile(r"\x1b\[([0-9;?]*)([A-Za-z])")


class Screen:
    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.lines = [[" "] * cols for _ in range(rows)]
        self.r = self.c = 0
        self.pending = ""

    def newline(self):
        self.r += 1
        if self.r == self.rows:
            self.lines.pop(0)
            self.lines.append([" "] * self.cols)
            self.r = self.rows - 1

    def feed(self, text):
        # An escape sequence can be split across two recorded chunks.
        text, self.pending = self.pending + text, ""
        i = 0
        while i < len(text):
            ch = text[i]
            if ch == "\x1b":
                m = CSI.match(text, i)
                if m:
                    self.csi(m.group(1), m.group(2))
                    i = m.end()
                    continue
                if re.fullmatch(r"\x1b(\[[0-9;?]*)?", text[i:]):
                    self.pending = text[i:]
                    return
            elif ch == "\r":
                self.c = 0
            elif ch == "\n":
                self.newline()
            elif ch == "\b":
                self.c = max(0, self.c - 1)
            elif ch == "\t":
                self.c = min(self.cols - 1, (self.c // 8 + 1) * 8)
            elif ch >= " ":
                if self.c >= self.cols:
                    self.c = 0
                    self.newline()
                self.lines[self.r][self.c] = ch
                self.c += 1
            i += 1

    def csi(self, params, cmd):
        nums = [int(p) if p.isdigit() else 0 for p in params.split(";")] if params else []
        if cmd == "H":
            self.r = max(0, (nums[0] if nums else 1) - 1)
            self.c = max(0, (nums[1] if len(nums) > 1 else 1) - 1)
        elif cmd == "J" and (nums[:1] == [2] or nums[:1] == [3]):
            self.lines = [[" "] * self.cols for _ in range(self.rows)]
        elif cmd == "J":
            self.lines[self.r][self.c :] = [" "] * (self.cols - self.c)
            for r in range(self.r + 1, self.rows):
                self.lines[r] = [" "] * self.cols
        elif cmd == "K":
            self.lines[self.r][self.c :] = [" "] * (self.cols - self.c)

    def snapshot(self):
        return ["".join(l) for l in self.lines], (self.r, min(self.c, self.cols - 1))


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    opts = dict(a[2:].split("=", 1) for a in sys.argv[1:] if a.startswith("--") and "=" in a)
    src, dst = args
    fonts = ["/System/Library/Fonts/Menlo.ttc", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"]
    font_path = opts.get("font") or next((f for f in fonts if os.path.exists(f)), fonts[0])
    size = int(opts.get("size", "14"))
    max_idle = float(opts.get("max-idle", "2.5"))

    with open(src) as f:
        header = json.loads(f.readline())
        events = [json.loads(l) for l in f if l.strip()]
    cols, rows = header["width"], header["height"]
    title = header.get("title", "")

    font = ImageFont.truetype(font_path, size)
    cw = font.getlength("M")
    ch = int(size * 1.25)
    pad, bar = 12, 28
    width = int(cols * cw) + 2 * pad
    height = rows * ch + 2 * pad + bar

    palette = []
    for k in range(LEVELS):
        t = k / (LEVELS - 1)
        palette += [round(BG[j] + (FG[j] - BG[j]) * t) for j in range(3)]
    extra = [BAR, TITLE] + DOTS
    for colour in extra:
        palette += list(colour)
    palette += [0] * (768 - len(palette))
    idx = {c: LEVELS + i for i, c in enumerate(extra)}

    chrome = Image.new("P", (width, height), 0)
    chrome.putpalette(palette)
    d = ImageDraw.Draw(chrome)
    d.rectangle([0, 0, width, bar], fill=idx[BAR])
    for i, colour in enumerate(DOTS):
        x = 16 + i * 20
        d.ellipse([x - 6, bar // 2 - 6, x + 6, bar // 2 + 6], fill=idx[colour])
    tw = d.textlength(title, font=font)
    d.text(((width - tw) / 2, (bar - size) / 2 - 1), title, font=font, fill=idx[TITLE])

    def render(lines, cursor):
        text = Image.new("L", (width, height - bar), 0)
        td = ImageDraw.Draw(text)
        for r, line in enumerate(lines):
            if line.strip():
                td.text((pad, pad + r * ch), line.rstrip(), font=font, fill=255)
        cr, cc = cursor
        x0 = pad + int(cc * cw)
        y0 = pad + cr * ch
        td.rectangle([x0, y0 + 1, x0 + int(cw) - 1, y0 + ch - 2], fill=200)
        text = text.point(lambda v: v * (LEVELS - 1) // 255)
        frame = chrome.copy()
        frame.paste(text, (0, bar))
        return frame

    # Group output into frames: everything within 40 ms of a frame's first
    # event shows up together. A frame lasts until the next one starts.
    screen = Screen(cols, rows)
    frames, times = [], []
    t_shift = 0.0
    last_t = None
    group_start = None
    for t, kind, data in events:
        if kind != "o":
            continue
        if last_t is not None and t - last_t > max_idle:
            t_shift += t - last_t - max_idle
        last_t = t
        t -= t_shift
        if group_start is not None and t - group_start < 0.04:
            screen.feed(data)
            frames[-1] = screen.snapshot()
            continue
        screen.feed(data)
        group_start = t
        frames.append(screen.snapshot())
        times.append(t)

    images, durations = [], []
    prev = None
    for i, snap in enumerate(frames):
        end = times[i + 1] if i + 1 < len(times) else times[i] + 4.0
        ms = max(20, int(round((end - times[i]) * 1000)))
        if snap == prev:
            durations[-1] += ms
            continue
        prev = snap
        images.append(render(*snap))
        durations.append(ms)
    images[0].save(dst, save_all=True, append_images=images[1:], duration=durations, loop=0, optimize=False,
                   disposal=1)
    total = sum(durations) / 1000
    print(f"{dst}: {len(images)} frames, {total:.1f} s, {width}x{height}")


if __name__ == "__main__":
    main()
