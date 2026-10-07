"""Tiny PNG reader and writer (8-bit RGB / RGBA / grey), standard library only.

write_png(path, width, height, rgb_bytes)  writes 8-bit RGB, filter 0 rows.
read_png(path) -> (width, height, rgb_bytes) reads any non-interlaced 8-bit
                  PNG (grey, grey+alpha, RGB, RGBA, palette), alpha dropped
                  (composited over white, like a browser screenshot would be).
"""

import struct
import zlib


def _chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def write_png(path: str, width: int, height: int, rgb: bytes, level: int = 6) -> None:
    stride = width * 3
    if len(rgb) != stride * height:
        raise ValueError(f"expected {stride * height} bytes of RGB, got {len(rgb)}")
    raw = bytearray((stride + 1) * height)
    mv = memoryview(rgb)
    for y in range(height):
        o = y * (stride + 1)
        raw[o] = 0
        raw[o + 1 : o + 1 + stride] = mv[y * stride : (y + 1) * stride]
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(_chunk(b"IHDR", ihdr))
        f.write(_chunk(b"IDAT", zlib.compress(bytes(raw), level)))
        f.write(_chunk(b"IEND", b""))


def _unfilter(data: bytes, width: int, height: int, bpp: int) -> bytearray:
    stride = width * bpp
    out = bytearray(stride * height)
    prev = bytearray(stride)
    pos = 0
    for y in range(height):
        ftype = data[pos]
        line = bytearray(data[pos + 1 : pos + 1 + stride])
        pos += 1 + stride
        if ftype == 0:
            pass
        elif ftype == 1:  # Sub
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ftype == 2:  # Up
            line = bytearray((a + b) & 0xFF for a, b in zip(line, prev))
        elif ftype == 3:  # Average
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa = abs(p - a)
                pb = abs(p - b)
                pc = abs(p - c)
                if pa <= pb and pa <= pc:
                    pr = a
                elif pb <= pc:
                    pr = b
                else:
                    pr = c
                line[i] = (line[i] + pr) & 0xFF
        else:
            raise ValueError(f"bad PNG filter type {ftype}")
        out[y * stride : (y + 1) * stride] = line
        prev = line
    return out


def read_png(path: str):
    with open(path, "rb") as f:
        blob = f.read()
    if blob[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")
    pos = 8
    idat = []
    palette = None
    width = height = depth = ctype = interlace = None
    while pos < len(blob):
        (length,) = struct.unpack(">I", blob[pos : pos + 4])
        kind = blob[pos + 4 : pos + 8]
        data = blob[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, ctype, _, _, interlace = struct.unpack(">IIBBBBB", data)
        elif kind == b"PLTE":
            palette = data
        elif kind == b"IDAT":
            idat.append(data)
        elif kind == b"IEND":
            break
    if depth != 8 or interlace:
        raise ValueError(f"{path}: only 8-bit, non-interlaced PNGs are supported")
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    px = _unfilter(zlib.decompress(b"".join(idat)), width, height, channels)
    n = width * height
    if ctype == 2:
        return width, height, bytes(px)
    rgb = bytearray(n * 3)
    if ctype == 6:
        # Composite over white (screenshots are opaque, so alpha is normally 255).
        alpha = px[3::4]
        if all(a == 255 for a in alpha[:: max(1, n // 4096)]) and min(alpha) == 255:
            rgb[0::3] = px[0::4]
            rgb[1::3] = px[1::4]
            rgb[2::3] = px[2::4]
        else:
            for i in range(n):
                a = px[4 * i + 3]
                for c in range(3):
                    rgb[3 * i + c] = (px[4 * i + c] * a + 255 * (255 - a)) // 255
    elif ctype == 0:
        rgb[0::3] = px
        rgb[1::3] = px
        rgb[2::3] = px
    elif ctype == 4:
        g = px[0::2]
        rgb[0::3] = g
        rgb[1::3] = g
        rgb[2::3] = g
    elif ctype == 3:
        for i in range(n):
            j = px[i] * 3
            rgb[3 * i : 3 * i + 3] = palette[j : j + 3]
    return width, height, bytes(rgb)
