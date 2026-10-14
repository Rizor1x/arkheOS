import importlib.util
import pathlib
import struct
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("elf", ROOT / "tools/check_elf.py")
elf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(elf)


def executable(flags=5, entry=0x1000, kind=2, segment_type=1):
    data = bytearray(128)
    data[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HH", data, 16, kind, 62)
    struct.pack_into("<QQ", data, 24, entry, 64)
    struct.pack_into("<HH", data, 54, 56, 1)
    struct.pack_into("<IIQQQQQQ", data, 64, segment_type, flags, 120, 0x1000, 0, 8, 8, 4096)
    return data


class ElfTests(unittest.TestCase):
    def test_static_read_execute_segment(self):
        elf.check(executable())

    def test_rejects_writable_code_dynamic_and_bad_entry(self):
        for data in [executable(flags=7), executable(entry=0), executable(kind=3),
                     executable(segment_type=3), executable()[:100], b"not elf"]:
            with self.assertRaises(ValueError):
                elf.check(data)


if __name__ == "__main__":
    unittest.main()
