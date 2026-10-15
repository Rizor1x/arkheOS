#!/usr/bin/env python3
import io
import pathlib
import sys
import tarfile


def build(destination, executable, supervisor, worker, invalid_executable=False):
    files = [("bin/init", pathlib.Path(executable).read_bytes()),
             ("bin/supervisor", pathlib.Path(supervisor).read_bytes()),
             ("bin/worker", pathlib.Path(worker).read_bytes()),
             ("etc/motd", b"arkheOS\n")]
    if invalid_executable:
        files.append(("bin/invalid", b"not an ELF"))
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
    if len(sys.argv) not in (5, 6) or (len(sys.argv) == 6 and sys.argv[5] != "--invalid-executable"):
        sys.exit("usage: mkinitramfs.py OUTPUT INIT_ELF SUPERVISOR_ELF WORKER_ELF [--invalid-executable]")
    build(*sys.argv[1:5], invalid_executable=len(sys.argv) == 6)
