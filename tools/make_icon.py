"""Regenerates assets/icon.ico (same drawing as src/icon.rs). Run: python tools/make_icon.py"""
import math, struct, zlib, os

def rgba(size):
    px = bytearray(size * size * 4)
    s = float(size); c = s / 2; corner = s * 0.22
    for y in range(size):
        for x in range(size):
            fx, fy = x + 0.5, y + 0.5
            qx = abs(fx - c) - (c - corner); qy = abs(fy - c) - (c - corner)
            outside = math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - corner
            bg = min(max(0.5 - outside, 0), 1)
            d = math.hypot(fx - c, fy - c) / s
            ring = min(max(1 - (abs(d - 0.33) - 0.045) * s, 0), 1)
            dot = min(max(1 - (d - 0.19) * s, 0), 1)
            r = g = b = 0.0
            r = r * (1 - ring) + ring; g = g * (1 - ring) + ring; b = b * (1 - ring) + ring
            r = r * (1 - dot) + dot; g = g * (1 - dot) + 0.23 * dot; b = b * (1 - dot) + 0.27 * dot
            i = (y * size + x) * 4
            px[i:i + 4] = bytes([int(r * 255), int(g * 255), int(b * 255), int(bg * 255)])
    return bytes(px)

def png(size):
    raw = b"".join(b"\0" + rgba(size)[y * size * 4:(y + 1) * size * 4] for y in range(size))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

sizes = [16, 20, 24, 32, 48, 64, 256]
images = [png(s) for s in sizes]
out = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
for s, img in zip(sizes, images):
    out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(img), offset)
    offset += len(img)
out += b"".join(images)
path = os.path.join(os.path.dirname(__file__), "..", "assets", "icon.ico")
open(path, "wb").write(out)
print("wrote", path, len(out), "bytes")
