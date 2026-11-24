#!/usr/bin/env python3
"""Packs PNG files into a Windows .ico (PNG-compressed entries, Vista+).

    make-ico.py OUT.ico IN-16.png IN-32.png ... IN-256.png
"""
import struct
import sys


def png_size(data):
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        sys.exit("make-ico: not a PNG file")
    return struct.unpack(">II", data[16:24])


def main():
    out, pngs = sys.argv[1], [open(p, "rb").read() for p in sys.argv[2:]]
    header = struct.pack("<HHH", 0, 1, len(pngs))
    entries, offset = b"", 6 + 16 * len(pngs)
    for data in pngs:
        w, h = png_size(data)
        entries += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    with open(out, "wb") as f:
        f.write(header + entries + b"".join(pngs))


if __name__ == "__main__":
    main()
