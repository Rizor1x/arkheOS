#!/usr/bin/env python3
import io
import pathlib
import sys
import tarfile


def build(destination, executable):
    files = [("bin/init", pathlib.Path(executable).read_bytes()),
             ("etc/motd", b"arkheOS\n")]
    destination = pathlib.Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(destination, "w", format=tarfile.USTAR_FORMAT) as archive:
        for name, data in sorted(files):
            entry = tarfile.TarInfo(name)
            entry.mode = 0o555 if name.startswith("bin/") else 0o444
            entry.size = len(data)
            entry.mtime = 0
            entry.uid = entry.gid = 0
            archive.addfile(entry, io.BytesIO(data))


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: mkinitramfs.py OUTPUT INIT_ELF")
    build(sys.argv[1], sys.argv[2])
