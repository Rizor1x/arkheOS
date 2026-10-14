import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("audit", ROOT / "tools/audit_repo.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class AuditTests(unittest.TestCase):
    def test_source_and_lockfiles_are_allowed(self):
        for path in ["Cargo.lock", "README.md", ".cargo/config.toml", "kernel/src/main.rs",
                     "boot/limine.lock", ".githooks/pre-commit", ".github/workflows/ci.yml",
                     "kernel/src/arch/x86_64/exceptions.S"]:
            self.assertEqual(audit.path_issues(path), [], path)

    def test_artifacts_and_private_paths_are_rejected(self):
        for path in ["target/a.rs", "build/kernel.rs", "disk.tar", "system.iso", ".env",
                     "docs/private.key", ".ssh/config", "docs/../notes.md", "/etc/passwd",
                     "kernel/secrets.toml", "personal.txt", "vendor/code.rs", "docs/new\nfile.md",
                     "AGENTS.md", "SECURITY.md", "CONTRIBUTING.md", "docs/ROADMAP.md",
                     ".local/docs/RESEARCH.md", "kernel/notes.md"]:
            self.assertTrue(audit.path_issues(path), path)

    def test_detects_tokens_without_echoing_content(self):
        examples = [
            ("gh" + "p_" + "a" * 36).encode(),
            ("AK" + "IA" + "A" * 16).encode(),
            ("sk" + "-proj-" + "a" * 40).encode(),
            ("-----BEGIN " + "RSA PRIVATE KEY-----").encode(),
            ("password = '" + "a" * 20 + "'").encode(),
            ("https:" + "//user:pass@example.org").encode(),
        ]
        for data in examples:
            self.assertTrue(audit.content_issues(data))
            self.assertNotIn(data.decode(), " ".join(audit.content_issues(data)))

    def test_detects_personal_paths(self):
        for text in ["/Users" + "/someone/project", "/home" + "/someone/project",
                     "C:" + "\\Users\\someone\\project"]:
            self.assertIn("personal absolute path", audit.content_issues(text.encode()))

    def test_rejects_binary_invalid_encoding_and_large_content(self):
        for data in [b"abc\0def", b"\xff", b"x" * (audit.MAX_BYTES + 1)]:
            self.assertTrue(audit.content_issues(data))

    def test_clean_code_is_allowed(self):
        self.assertEqual(audit.content_issues(b"fn main() {}\n"), [])


if __name__ == "__main__":
    unittest.main()
