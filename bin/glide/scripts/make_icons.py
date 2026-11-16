#!/usr/bin/env python3
"""Generates assets/glide-light.ico and glide-dark.ico (dev-only, stdlib only).
A mouse outline with a wheel and two small scroll chevrons, 4x supersampled."""
import struct, zlib, math, os

SIZES = (16, 24, 32, 48)

def inside_round_rect(x, y, x0, y0, x1, y1, r):
    cx = min(max(x, x0 + r), x1 - r); cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r and x0 <= x <= x1 and y0 <= y <= y1

def coverage(u, v):
    """u,v in [0,1]. Returns 1 where the glyph is drawn."""
    # mouse body outline (stroke ~0.085) ...
    outer = inside_round_rect(u, v, 0.28, 0.08, 0.72, 0.92, 0.22)
    inner = inside_round_rect(u, v, 0.28 + 0.085, 0.08 + 0.085, 0.72 - 0.085, 0.92 - 0.085, 0.135)
    body = outer and not inner
    # ... wheel pill ...
    wheel = inside_round_rect(u, v, 0.465, 0.22, 0.535, 0.42, 0.035)
    # ... and a split line under the buttons
    split = 0.455 <= v <= 0.485 and 0.30 <= u <= 0.70
    # chevrons above/below the body hint at vertical scrolling
    def chevron(cx, cy, up):
        dy = (cy - v) if up else (v - cy)
        return abs(u - cx) <= 0.13 - dy * 1.0 and 0 <= dy <= 0.13 and abs(u - cx) >= 0.13 - dy * 1.0 - 0.05
    return body or wheel or split

def render(size, rgb):
    ss = 4; px = []
    for y in range(size):
        row = []
        for x in range(size):
            hit = sum(coverage((x + (i + .5) / ss) / size, (y + (j + .5) / ss) / size)
                      for i in range(ss) for j in range(ss))
            a = round(255 * hit / (ss * ss))
            row.append((rgb[0], rgb[1], rgb[2], a))
        px.append(row)
    return px

def png(px):
    h = len(px); w = len(px[0])
    raw = b"".join(b"\x00" + b"".join(bytes(p) for p in row) for row in px)
    def chunk(t, d): 
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

def ico(images):
    head = struct.pack("<HHH", 0, 1, len(images)); off = 6 + 16 * len(images); dirs = b""; blobs = b""
    for size, data in images:
        dirs += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), off + len(blobs))
        blobs += data
    return head + dirs + blobs

here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
for name, rgb in (("glide-light.ico", (255, 255, 255)), ("glide-dark.ico", (32, 32, 32))):
    data = ico([(s, png(render(s, rgb))) for s in SIZES])
    with open(os.path.join(here, name), "wb") as f: f.write(data)
    print(name, len(data), "bytes")
