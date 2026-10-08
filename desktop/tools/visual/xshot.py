#!/usr/bin/env python3
"""Screenshot an X display's root window to a PNG, through libX11 (ctypes).

    DISPLAY=:77 python3 xshot.py out.png [x y width height]

Reads the root window with XGetImage (ZPixmap) and writes 8-bit RGB with
lib/png.py. Works on Xvfb, where everything drawn is in the root's pixels.
"""

import ctypes
import ctypes.util
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "lib"))
from png import write_png  # noqa: E402


class XImage(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int),
        ("data", ctypes.POINTER(ctypes.c_ubyte)),
        ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int),
        ("bitmap_bit_order", ctypes.c_int),
        ("bitmap_pad", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("bytes_per_line", ctypes.c_int),
        ("bits_per_pixel", ctypes.c_int),
        ("red_mask", ctypes.c_ulong),
        ("green_mask", ctypes.c_ulong),
        ("blue_mask", ctypes.c_ulong),
    ]


class XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int),
        ("y", ctypes.c_int),
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("border_width", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p),
        ("root", ctypes.c_ulong),
        ("class", ctypes.c_int),
        ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int),
        ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong),
        ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int),
        ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int),
        ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long),
        ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int),
        ("screen", ctypes.c_void_p),
    ]


def shoot(path, region=None):
    x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDefaultRootWindow.restype = ctypes.c_ulong
    x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x11.XGetWindowAttributes.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(XWindowAttributes)]
    x11.XGetImage.restype = ctypes.POINTER(XImage)
    x11.XGetImage.argtypes = [
        ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
        ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int,
    ]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]

    dpy = x11.XOpenDisplay(None)
    if not dpy:
        raise SystemExit(f"xshot: can't open display {os.environ.get('DISPLAY')!r}")
    root = x11.XDefaultRootWindow(dpy)
    attrs = XWindowAttributes()
    x11.XGetWindowAttributes(dpy, root, ctypes.byref(attrs))
    x, y, w, h = region or (0, 0, attrs.width, attrs.height)
    ZPixmap = 2
    AllPlanes = ctypes.c_ulong(-1).value
    img_p = x11.XGetImage(dpy, root, x, y, w, h, AllPlanes, ZPixmap)
    if not img_p:
        raise SystemExit("xshot: XGetImage failed")
    img = img_p.contents
    if img.bits_per_pixel != 32:
        raise SystemExit(f"xshot: expected 32 bits per pixel, got {img.bits_per_pixel} (run Xvfb with depth 24)")
    bpl = img.bytes_per_line
    raw = ctypes.string_at(img.data, bpl * h)
    if bpl != w * 4:
        raw = b"".join(raw[r * bpl : r * bpl + w * 4] for r in range(h))
    # Little-endian ZPixmap at depth 24: B, G, R, X per pixel (masks say so).
    if img.red_mask != 0xFF0000 or img.blue_mask != 0xFF:
        raise SystemExit(f"xshot: unexpected masks {img.red_mask:x}/{img.green_mask:x}/{img.blue_mask:x}")
    rgb = bytearray(w * h * 3)
    rgb[0::3] = raw[2::4]
    rgb[1::3] = raw[1::4]
    rgb[2::3] = raw[0::4]
    write_png(path, w, h, bytes(rgb))
    x11.XCloseDisplay(dpy)
    return w, h


if __name__ == "__main__":
    if len(sys.argv) not in (2, 6):
        raise SystemExit(__doc__)
    region = tuple(int(v) for v in sys.argv[2:6]) if len(sys.argv) == 6 else None
    w, h = shoot(sys.argv[1], region)
    print(f"{sys.argv[1]} ({w}x{h})")
