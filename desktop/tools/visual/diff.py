#!/usr/bin/env python3
"""diff.py NAME : $FUWA_VISUAL_DIR/out/NAME-web.png vs out/NAME-desktop.png -> out/NAME-side.png (side by side) and
out/NAME-diff.png (web dimmed, pixels that differ in red; desktop-only content in blue tint), plus a % score."""
import sys, os
here = os.path.dirname(os.path.abspath(__file__))
out = os.path.join(os.environ.get('FUWA_VISUAL_DIR', '/tmp/fuwa-visual'), 'out')
sys.path.insert(0, os.path.join(here, 'lib'))
from png import write_png
src = open(os.path.join(here, 'probe.py')).read().split('w, h, bpp, rows = load')[0]
sys.argv = sys.argv[:2]; exec(src)
name = sys.argv[1]
w1, h1, b1, A = load(f'{out}/{name}-web.png')
w2, h2, b2, B = load(f'{out}/{name}-desktop.png')
w, h = min(w1, w2), min(h1, h2)
side = bytearray(); diff = bytearray(); bad = 0
for y in range(h):
    ra, rb = A[y], B[y]
    for x in range(w1): side += ra[x*b1:x*b1+3]
    for x in range(w2): side += rb[x*b2:x*b2+3]
    for x in range(w):
        pa = ra[x*b1:x*b1+3]; pb = rb[x*b2:x*b2+3]
        d = sum(abs(pa[i] - pb[i]) for i in range(3))
        if d > 60:
            bad += 1; diff += bytes((255, 40, 40))
        else:
            diff += bytes(int(v * 0.35 + 165) for v in pa)
write_png(f'{out}/{name}-side.png', w1 + w2, h, bytes(side))
write_png(f'{out}/{name}-diff.png', w, h, bytes(diff))
print(f'{name}: {100 * bad / (w * h):.1f}% of pixels differ; {out}/{name}-side.png {out}/{name}-diff.png')
