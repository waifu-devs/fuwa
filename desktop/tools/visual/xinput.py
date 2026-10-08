#!/usr/bin/env python3
"""Fake mouse and keyboard input on an X display through XTest (ctypes).

    DISPLAY=:77 python3 xinput.py click X Y          left click (also: rclick, mclick, dblclick)
    DISPLAY=:77 python3 xinput.py move X Y           move the pointer (hover)
    DISPLAY=:77 python3 xinput.py key ctrl+comma     a key or combo (Escape, ctrl+shift+n, Return, F5, ...)
    DISPLAY=:77 python3 xinput.py type "hello there" type text (ASCII; \\n is Return)
    DISPLAY=:77 python3 xinput.py scroll X Y N       wheel N notches at X,Y (positive scrolls down)
    DISPLAY=:77 python3 xinput.py drag X1 Y1 X2 Y2   press at 1, move to 2 in steps, release
    DISPLAY=:77 python3 xinput.py wait MS
    DISPLAY=:77 python3 xinput.py run FILE           run an actions file (see README.md)

Actions files are JSON lists of steps, the same format web-shot.mjs takes:
    [{"click": [72, 120]}, {"wait": 400}, {"key": "ctrl+comma"}, {"type": "hi"}]
Steps with "only": "web" are skipped here. A plain text file with one
command per line (the CLI forms above, '#' comments) works too.
"""

import ctypes
import ctypes.util
import json
import os
import shlex
import sys
import time

x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
xtst = ctypes.CDLL(ctypes.util.find_library("Xtst") or "libXtst.so.6")

x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XKeycodeToKeysym.restype = ctypes.c_ulong
x11.XKeycodeToKeysym.argtypes = [ctypes.c_void_p, ctypes.c_ubyte, ctypes.c_int]
x11.XFlush.argtypes = [ctypes.c_void_p]
x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

# Friendly names (shared with web-shot.mjs) to X keysym names.
NAMES = {
    "ctrl": "Control_L", "control": "Control_L", "shift": "Shift_L", "alt": "Alt_L", "option": "Alt_L",
    "super": "Super_L", "meta": "Super_L", "cmd": "Super_L", "win": "Super_L",
    "esc": "Escape", "escape": "Escape", "enter": "Return", "return": "Return", "tab": "Tab",
    "space": "space", "backspace": "BackSpace", "delete": "Delete", "del": "Delete", "insert": "Insert",
    "up": "Up", "down": "Down", "left": "Left", "right": "Right", "arrowup": "Up", "arrowdown": "Down",
    "arrowleft": "Left", "arrowright": "Right", "home": "Home", "end": "End", "pageup": "Prior",
    "pagedown": "Next", "comma": "comma", "period": "period", "slash": "slash", "backslash": "backslash",
    "semicolon": "semicolon", "quote": "apostrophe", "minus": "minus", "equal": "equal", "plus": "plus",
    "bracketleft": "bracketleft", "bracketright": "bracketright", "backquote": "grave", "grave": "grave",
    "menu": "Menu", "contextmenu": "Menu",
}
SHIFTED = {"plus"}  # names whose key needs Shift on a US layout


class Input:
    def __init__(self):
        self.dpy = x11.XOpenDisplay(None)
        if not self.dpy:
            raise SystemExit(f"xinput: can't open display {os.environ.get('DISPLAY')!r}")

    def flush(self):
        x11.XSync(self.dpy, 0)

    def move(self, x, y):
        x += int(os.environ.get('XOFF', 0)); y += int(os.environ.get('YOFF', 0))
        xtst.XTestFakeMotionEvent(self.dpy, -1, int(x), int(y), 0)
        self.flush()

    def button(self, button, press):
        xtst.XTestFakeButtonEvent(self.dpy, button, 1 if press else 0, 0)
        self.flush()

    def click(self, x, y, button=1, count=1):
        self.move(x, y)
        time.sleep(0.05)
        for n in range(count):
            self.button(button, True)
            time.sleep(0.03)
            self.button(button, False)
            if n + 1 < count:
                time.sleep(0.06)

    def scroll(self, x, y, notches):
        self.move(x, y)
        time.sleep(0.05)
        button = 5 if notches > 0 else 4
        for _ in range(abs(int(notches))):
            self.button(button, True)
            self.button(button, False)
            time.sleep(0.03)

    def drag(self, x1, y1, x2, y2, steps=12):
        self.move(x1, y1)
        time.sleep(0.05)
        self.button(1, True)
        for i in range(1, steps + 1):
            time.sleep(0.03)
            self.move(x1 + (x2 - x1) * i / steps, y1 + (y2 - y1) * i / steps)
        time.sleep(0.05)
        self.button(1, False)

    def keycode(self, keysym):
        code = x11.XKeysymToKeycode(self.dpy, keysym)
        if not code:
            raise SystemExit(f"xinput: no key for keysym 0x{keysym:x} in this keymap")
        return code

    def keysym_of(self, name):
        if name.lower() in NAMES:
            name = NAMES[name.lower()]
        elif len(name) == 1:
            return ord(name.lower()) if name.isalpha() else ord(name)
        elif name.lower().startswith("f") and name[1:].isdigit():
            name = name.upper()
        sym = x11.XStringToKeysym(name.encode())
        if not sym:
            raise SystemExit(f"xinput: unknown key {name!r}")
        return sym

    def key(self, combo):
        parts = [p for p in combo.replace(" ", "").split("+") if p] if combo != "+" else ["plus"]
        codes = []
        for p in parts:
            if p.lower() in SHIFTED:
                codes.append(self.keycode(self.keysym_of("shift")))
            codes.append(self.keycode(self.keysym_of(p)))
        for c in codes:
            xtst.XTestFakeKeyEvent(self.dpy, c, 1, 0)
            self.flush()
            time.sleep(0.02)
        for c in reversed(codes):
            xtst.XTestFakeKeyEvent(self.dpy, c, 0, 0)
            self.flush()
            time.sleep(0.02)

    def type(self, text):
        shift = self.keycode(self.keysym_of("shift"))
        for ch in text:
            sym = 0xFF0D if ch == "\n" else (0xFF09 if ch == "\t" else ord(ch))
            if sym > 0xFF and sym < 0xFF00:
                raise SystemExit(f"xinput: can't type {ch!r} (only ASCII and Latin-1)")
            code = self.keycode(sym)
            need_shift = x11.XKeycodeToKeysym(self.dpy, code, 0) != sym and x11.XKeycodeToKeysym(self.dpy, code, 1) == sym
            if need_shift:
                xtst.XTestFakeKeyEvent(self.dpy, shift, 1, 0)
            xtst.XTestFakeKeyEvent(self.dpy, code, 1, 0)
            xtst.XTestFakeKeyEvent(self.dpy, code, 0, 0)
            if need_shift:
                xtst.XTestFakeKeyEvent(self.dpy, shift, 0, 0)
            self.flush()
            time.sleep(0.015)


def point(value):
    if isinstance(value, (list, tuple)) and len(value) >= 2:
        return int(value[0]), int(value[1])
    raise SystemExit(f"xinput: the desktop needs [x, y] coordinates, not {value!r}")


def run_step(inp, step):
    """One JSON step: {"click": [x, y]}, {"key": "..."}, ..."""
    if step.get("only") not in (None, "desktop", "desk"):
        return
    for action in ("click", "rclick", "mclick", "dblclick", "move", "hover", "key", "type", "wait", "scroll", "drag", "sleep"):
        if action in step:
            value = step[action]
            break
    else:
        # Web-only steps (goto, waitFor, ...) are skipped here.
        if not any(k in step for k in ("goto", "waitFor", "eval")):
            print(f"xinput: skipping unknown step {step!r}", file=sys.stderr)
        return
    if action == "click":
        inp.click(*point(value))
    elif action == "rclick":
        inp.click(*point(value), button=3)
    elif action == "mclick":
        inp.click(*point(value), button=2)
    elif action == "dblclick":
        inp.click(*point(value), count=2)
    elif action in ("move", "hover"):
        inp.move(*point(value))
    elif action == "key":
        inp.key(value)
    elif action == "type":
        inp.type(value)
    elif action in ("wait", "sleep"):
        time.sleep(float(value) / 1000)
    elif action == "scroll":
        x, y = point(value)
        inp.scroll(x, y, int(value[2]) if len(value) > 2 else 3)
    elif action == "drag":
        inp.drag(*[int(v) for v in value[:4]])
    # Let the app take each step in before the next one.
    time.sleep(float(step.get("after", 150)) / 1000)


def run_line(inp, argv):
    cmd, args = argv[0], argv[1:]
    if cmd in ("click", "rclick", "mclick", "dblclick", "move", "hover"):
        run_step(inp, {cmd: [int(args[0]), int(args[1])], "after": 0})
    elif cmd == "key":
        for combo in args:
            inp.key(combo)
    elif cmd == "type":
        inp.type(" ".join(args).encode().decode("unicode_escape"))
    elif cmd == "scroll":
        inp.scroll(int(args[0]), int(args[1]), int(args[2]) if len(args) > 2 else 3)
    elif cmd == "drag":
        inp.drag(*[int(a) for a in args[:4]])
    elif cmd in ("wait", "sleep"):
        time.sleep(float(args[0]) / 1000)
    elif cmd == "run":
        run_file(inp, args[0])
    else:
        raise SystemExit(__doc__)


def run_file(inp, path):
    with open(path) as f:
        text = f.read()
    if text.lstrip().startswith("["):
        for step in json.loads(text):
            run_step(inp, step)
        return
    for line in text.splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            run_line(inp, shlex.split(line))
            time.sleep(0.15)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    run_line(Input(), sys.argv[1:])
