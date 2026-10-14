#!/usr/bin/env python3
"""Inspect publishable files without printing their contents."""
import argparse
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
MAX_BYTES = 1024 * 1024
ROOT_FILES = {
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "README.md",
    "LICENSE", ".gitignore", ".editorconfig",
}
SOURCE_DIRS = {"kernel", "crates", "userspace", "xtask", "boot", "tools", "tests"}
SUFFIXES = {".rs", ".S", ".toml", ".py", ".ld", ".conf", ".lock"}
FORBIDDEN_DIRS = {
    "target", "build", "dist", "vendor", "iso_root", "bootloader", ".cache", ".local",
    ".ssh", ".aws", ".codex", ".agents", ".idea", ".vscode", ".venv", "__pycache__",
}
FORBIDDEN_SUFFIXES = {
    ".iso", ".img", ".qcow2", ".bin", ".elf", ".tar", ".zip", ".gz", ".log",
    ".pem", ".key", ".p12", ".pfx", ".keystore", ".dmp", ".core", ".pyc",
}
PATTERNS = [
    ("private key", re.compile(r"-----BEGIN (?:[A-Z ]*PRIVATE KEY|PGP PRIVATE KEY BLOCK)-----")),
    ("GitHub token", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{30,})\b")),
    ("cloud access key", re.compile(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b")),
    ("service token", re.compile(r"\b(?:sk-(?:proj-)?[A-Za-z0-9_-]{20,}|xox[baprs]-[A-Za-z0-9-]{20,})\b")),
    ("credential in URL", re.compile(r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s/:]+:[^\s/@]+@")),
    ("personal absolute path", re.compile(r"(?:/Us" + r"ers/[^/\s]+/|/ho" + r"me/[^/\s]+/|[A-Za-z]:\\Users\\[^\\\s]+\\)")),
    ("literal credential", re.compile(
        r'''(?im)\b(?:api[_-]?key|access[_-]?token|auth[_-]?token|password|client[_-]?secret)\b\s*[:=]\s*["']([^"'\r\n]{12,})["']'''
    )),
]


def git(*args, input_data=None):
    result = subprocess.run(["git", "-C", str(ROOT), *args], input=input_data,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if result.returncode != 0:
        raise RuntimeError("Git operation failed: " + args[0])
    return result.stdout


def path_issues(name):
    if any(ord(char) < 32 or ord(char) == 127 for char in name):
        return ["control character in file name"]
    path = pathlib.PurePosixPath(name)
    parts = path.parts
    if not parts or path.is_absolute() or ".." in parts:
        return ["invalid repository path"]
    if any(part in FORBIDDEN_DIRS for part in parts):
        return ["local or generated directory"]
    if path.suffix.lower() in FORBIDDEN_SUFFIXES:
        return ["artifact or private file"]
    if path.name == ".env" or path.name.startswith((".env.", "id_rsa", "id_ed25519", "credentials", "secrets")):
        return ["private configuration"]
    allowed = (
        name in ROOT_FILES
        or (parts[0] in SOURCE_DIRS and path.suffix in SUFFIXES)
        or name == ".cargo/config.toml"
        or (parts[:2] == (".github", "workflows") and path.suffix in {".yml", ".yaml"})
        or name in {".githooks/pre-commit", ".githooks/pre-push"}
    )
    return [] if allowed else ["file outside publication policy; review tools/audit_repo.py"]


def content_issues(data):
    if len(data) > MAX_BYTES:
        return ["file exceeds 1 MiB publication limit"]
    if b"\0" in data:
        return ["binary content"]
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return ["non-UTF-8 content"]
    return [label for label, pattern in PATTERNS if pattern.search(text)]


def names(data):
    return [name.decode("utf-8") for name in data.split(b"\0") if name]


def object_issues(object_id):
    size = int(git("cat-file", "-s", object_id))
    if size > MAX_BYTES:
        return ["file exceeds 1 MiB publication limit"]
    return content_issues(git("cat-file", "blob", object_id))


def ignored(name):
    result = subprocess.run(
        ["git", "-C", str(ROOT), "check-ignore", "--no-index", "-q", "--", name],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False,
    )
    if result.returncode not in (0, 1):
        raise RuntimeError("cannot check ignore rules")
    return result.returncode == 0


def scan(staged=False, history=False, list_files=False):
    top = pathlib.Path(git("rev-parse", "--show-toplevel").decode().strip()).resolve()
    if top != ROOT or not (ROOT / ".git").exists():
        raise RuntimeError("project must have its own Git repository; refusing parent repository")
    indexed = names(git("ls-files", "--cached", "-z"))
    candidates = indexed if staged else sorted(set(indexed + names(git("ls-files", "--others", "--exclude-standard", "-z"))))
    failures = []
    for name in candidates:
        issues = path_issues(name)
        if name in indexed and ignored(name):
            issues.append("ignored file is already tracked or staged")
        path = ROOT / name
        parts = pathlib.PurePosixPath(name).parts
        if any(ROOT.joinpath(*parts[:i]).is_symlink() for i in range(1, len(parts) + 1)):
            issues.append("symbolic link")
        if name in indexed:
            mode = git("ls-files", "--stage", "--", name).split(b" ", 1)[0]
            if mode in (b"120000", b"160000"):
                issues.append("indexed symbolic link or submodule")
            issues.extend(object_issues(":" + name))
        if not staged and not issues:
            if not path.is_file():
                issues.append("missing working-tree file")
            elif path.stat().st_size > MAX_BYTES:
                issues.append("file exceeds 1 MiB publication limit")
            else:
                issues.extend(content_issues(path.read_bytes()))
        if issues:
            failures.append((name, sorted(set(issues))))
    history_count = 0
    if history:
        for line in git("rev-list", "--objects", "--all").decode().splitlines():
            object_id, _, name = line.partition(" ")
            if git("cat-file", "-t", object_id).strip() != b"blob":
                continue
            history_count += 1
            issues = path_issues(name) + object_issues(object_id)
            if issues:
                failures.append((f"history:{name}", sorted(set(issues))))
    for name, issues in failures:
        print(f"BLOCKED {name}: {', '.join(issues)}", file=sys.stderr)
    if failures:
        return 1
    print(f"Publication audit passed: {len(candidates)} files, {history_count} historical blobs")
    if list_files:
        for name in candidates:
            print(name)
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--staged", action="store_true", help="inspect index contents, not working-tree copies")
    parser.add_argument("--history", action="store_true", help="also inspect all locally reachable committed blobs")
    parser.add_argument("--list", action="store_true", help="list publishable paths without contents")
    args = parser.parse_args()
    try:
        return scan(args.staged, args.history, args.list)
    except (RuntimeError, OSError, UnicodeError) as error:
        print(f"Publication audit failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
