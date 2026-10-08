#!/usr/bin/env python3
"""probe.py IMG row Y [x0 x1] | col X [y0 y1] | px X Y | window
Prints the color runs along a line (start-end#rrggbb), one pixel's color, or (window) where
the first non-black window starts on a screenshot of the whole screen."""
import sys, zlib, struct
def load(path):
    d = open(path, 'rb').read(); assert d[:8] == b'\x89PNG\r\n\x1a\n'
    i = 8; idat = b''; w = h = 0; ct = 2
    while i < len(d):
        n = struct.unpack('>I', d[i:i+4])[0]; t = d[i+4:i+8]; c = d[i+8:i+8+n]; i += 12 + n
        if t == b'IHDR': w, h, bd, ct = struct.unpack('>IIBB', c[:10])
        elif t == b'IDAT': idat += c
    bpp = {2: 3, 6: 4}[ct]; raw = zlib.decompress(idat); rows = []; prev = bytearray(w * bpp); p = 0
    for y in range(h):
        f = raw[p]; p += 1; line = bytearray(raw[p:p + w * bpp]); p += w * bpp
        for x in range(len(line)):
            a = line[x - bpp] if x >= bpp else 0; b = prev[x]; c = prev[x - bpp] if x >= bpp else 0
            if f == 1: line[x] = (line[x] + a) & 255
            elif f == 2: line[x] = (line[x] + b) & 255
            elif f == 3: line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(line); prev = line
    return w, h, bpp, rows
w, h, bpp, rows = load(sys.argv[1])
pix = lambda x, y: '#%02x%02x%02x' % tuple(rows[y][x * bpp:x * bpp + 3])
mode = sys.argv[2]
if mode == 'window':
    dark = lambda x, y: sum(rows[y][x * bpp:x * bpp + 3]) == 0
    x = next(i for i in range(w) if not dark(i, h // 2)); y = next(j for j in range(h) if not dark(w // 2, j))
    print(x, y); sys.exit()
if mode == 'px': print(pix(int(sys.argv[3]), int(sys.argv[4]))); sys.exit()
k = int(sys.argv[3]); a = int(sys.argv[4]) if len(sys.argv) > 4 else 0; b = int(sys.argv[5]) if len(sys.argv) > 5 else (w if mode == 'row' else h)
runs = []
for t in range(a, b):
    c = pix(t, k) if mode == 'row' else pix(k, t)
    if runs and runs[-1][2] == c: runs[-1][1] = t
    else: runs.append([t, t, c])
print(' '.join(f'{s}-{e}{c}' if s != e else f'{s}{c}' for s, e, c in runs))
