#!/usr/bin/env python3
"""Sets the PE subsystem of an executable: 2 = GUI, 3 = console.

    set-subsystem.py FILE.exe 3
"""
import struct
import sys

path, subsystem = sys.argv[1], int(sys.argv[2])
with open(path, "r+b") as f:
    data = bytearray(f.read())
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        sys.exit("set-subsystem: not a PE file")
    # COFF header is 20 bytes; Subsystem is at offset 68 of the optional header.
    struct.pack_into("<H", data, pe + 4 + 20 + 68, subsystem)
    f.seek(0)
    f.write(data)
