import contextlib
import importlib.util
import io
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("audit_git", ROOT / "tools/audit_repo.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class GitAuditTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self.temp.name).resolve()
        self.original = audit.ROOT
        audit.ROOT = self.repo
        self.env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull}
        self.git("init", ".")
        (self.repo / ".gitignore").write_text("target/\n*.key\n", encoding="utf-8")

    def tearDown(self):
        audit.ROOT = self.original
        self.temp.cleanup()

    def git(self, *args):
        subprocess.run(["git", "-C", str(self.repo), "-c", "user.name=Test",
                        "-c", "user.email=test@example.invalid", *args], check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=self.env)

    def scan(self, **kwargs):
        stream = io.StringIO()
        with contextlib.redirect_stderr(stream), contextlib.redirect_stdout(stream):
            code = audit.scan(**kwargs)
        return code, stream.getvalue()

    def test_index_secret_cannot_be_hidden_by_clean_working_copy(self):
        token = "gh" + "p_" + "a" * 36
        file = self.repo / "README.md"
        file.write_text(token, encoding="utf-8")
        self.git("add", "README.md")
        file.write_text("clean\n", encoding="utf-8")
        code, output = self.scan(staged=True)
        self.assertEqual(code, 1)
        self.assertIn("GitHub token", output)
        self.assertNotIn(token, output)

    def test_forced_ignored_file_is_blocked(self):
        file = self.repo / "target" / "code.rs"
        file.parent.mkdir()
        file.write_text("fn main() {}\n", encoding="utf-8")
        self.git("add", "--force", "target/code.rs")
        code, output = self.scan(staged=True)
        self.assertEqual(code, 1)
        self.assertIn("already tracked or staged", output)

    def test_deleted_historical_secret_is_still_blocked(self):
        token = "gh" + "p_" + "a" * 36
        file = self.repo / "README.md"
        file.write_text(token, encoding="utf-8")
        self.git("add", "README.md")
        self.git("commit", "-m", "test: add fixture")
        self.git("rm", "README.md")
        self.git("commit", "-m", "test: remove fixture")
        code, output = self.scan(staged=True, history=True)
        self.assertEqual(code, 1)
        self.assertIn("history:README.md", output)
        self.assertNotIn(token, output)

    def test_symlink_is_blocked_even_with_allowed_name(self):
        (self.repo / "README.md").symlink_to("Cargo.toml")
        self.git("add", "README.md")
        code, output = self.scan(staged=True)
        self.assertEqual(code, 1)
        self.assertIn("symbolic link", output)

    def test_parent_repository_is_rejected(self):
        nested = self.repo / "nested"
        nested.mkdir()
        audit.ROOT = nested
        with self.assertRaisesRegex(RuntimeError, "own Git repository"):
            audit.scan()

    def test_clean_index_and_working_tree_pass(self):
        (self.repo / "README.md").write_text("project\n", encoding="utf-8")
        self.git("add", "README.md", ".gitignore")
        self.assertEqual(self.scan(staged=True)[0], 0)
        self.assertEqual(self.scan()[0], 0)


if __name__ == "__main__":
    unittest.main()
