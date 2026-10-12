#!/usr/bin/env python3
"""Generates a simple 256x256 PNG icon for PEAR Configurator (no external deps)."""
import struct
import sys
import zlib

SIZE = 256


def rounded_alpha(px, py, w, h, r):
    if px < 0 or py < 0 or px >= w or py >= h:
        return 0.0
    cx = min(max(px + 0.5, r), w - r)
    cy = min(max(py + 0.5, r), h - r)
    dx = px + 0.5 - cx
    dy = py + 0.5 - cy
    dist = (dx * dx + dy * dy) ** 0.5
    if dist <= r - 1:
        return 1.0
    if dist >= r:
        return 0.0
    return r - dist


def main(path):
    px = bytearray(SIZE * SIZE * 4)
    for y in range(SIZE):
        for x in range(SIZE):
            # Diagonal gradient: #6d8bff -> #9b6dff
            t = (x + y) / (2.0 * SIZE)
            r = int(0x6d + (0x9b - 0x6d) * t)
            g = int(0x8b + (0x6d - 0x8b) * t)
            b = int(0xff + (0xff - 0xff) * t)
            a = rounded_alpha(x, y, SIZE, SIZE, 56)
            i = (y * SIZE + x) * 4
            px[i] = r
            px[i + 1] = g
            px[i + 2] = b
            px[i + 3] = int(255 * a)

    # Inner "device" glyph (white rounded rect + stand)
    for y in range(70, 170):
        for x in range(72, 184):
            a = rounded_alpha(x - 72, y - 70, 112, 78, 14)
            if a <= 0:
                continue
            i = (y * SIZE + x) * 4
            px[i] = 255
            px[i + 1] = 255
            px[i + 2] = 255
            px[i + 3] = int(235 * a)
    for y in range(176, 190):
        for x in range(108, 148):
            a = rounded_alpha(x - 108, y - 176, 40, 14, 6)
            if a <= 0:
                continue
            i = (y * SIZE + x) * 4
            px[i] = 255
            px[i + 1] = 255
            px[i + 2] = 255
            px[i + 3] = int(235 * a)

    raw = bytearray()
    for y in range(SIZE):
        raw.append(0)
        raw += px[y * SIZE * 4:(y + 1) * SIZE * 4]

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")

    with open(path, "wb") as fh:
        fh.write(png)
    print("wrote", path)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "pear-configurator.png")
