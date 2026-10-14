#!/usr/bin/env python3
import pathlib
import struct
import sys


def check(data):
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("expected a little-endian ELF64 kernel")
    kind, machine = struct.unpack_from("<HH", data, 16)
    if (kind, machine) != (2, 62):
        raise ValueError("expected a static x86_64 executable")
    entry, phoff = struct.unpack_from("<QQ", data, 24)
    phsize, phnum = struct.unpack_from("<HH", data, 54)
    if phsize != 56 or phnum == 0 or phoff + phsize * phnum > len(data):
        raise ValueError("invalid program headers")
    executable_entry = False
    for i in range(phnum):
        ptype, flags, offset, address, _, filesz, memsz, _ = struct.unpack_from(
            "<IIQQQQQQ", data, phoff + i * phsize
        )
        if ptype in (2, 3):
            raise ValueError("kernel must not have a dynamic linker")
        if ptype != 1:
            continue
        if filesz > memsz or offset + filesz > len(data):
            raise ValueError("invalid load segment")
        if flags & 3 == 3:
            raise ValueError("writable executable load segment")
        executable_entry |= bool(flags & 1 and address <= entry < address + memsz)
    if not executable_entry:
        raise ValueError("entry point is outside executable memory")


if __name__ == "__main__":
    try:
        check(pathlib.Path(sys.argv[1]).read_bytes())
    except (ValueError, OSError, IndexError, struct.error) as error:
        sys.exit(f"ELF check: {error}")
    print("ELF check passed: static x86_64, executable entry, no W+X segments")
