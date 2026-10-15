import importlib.util
import pathlib
import tarfile
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("builder", ROOT / "tools/mkinitramfs.py")
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class BuilderTests(unittest.TestCase):
    def test_reproducible_archive_contains_only_explicit_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            executable = directory / "init"
            executable.write_bytes(b"executable")
            first, second = directory / "one.tar", directory / "two.tar"
            builder.build(first, executable, executable, executable)
            builder.build(second, executable, executable, executable)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with tarfile.open(first) as archive:
                self.assertEqual(archive.getnames(), ["bin/init", "bin/supervisor", "bin/worker", "etc/motd"])
                self.assertEqual(archive.extractfile("bin/init").read(), b"executable")
                for member in archive.getmembers():
                    self.assertEqual(member.mode, 0o555 if member.name.startswith("bin/") else 0o444)
                    self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))
                    self.assertEqual((member.uname, member.gname), ("", ""))

    def test_invalid_image_is_only_in_explicit_test_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            executable = directory / "init"
            executable.write_bytes(b"executable")
            image = directory / "test.tar"
            builder.build(image, executable, executable, executable, invalid_executable=True)
            with tarfile.open(image) as archive:
                self.assertEqual(archive.extractfile("bin/invalid").read(), b"not an ELF")
                self.assertEqual(archive.getmember("bin/invalid").mode, 0o555)
