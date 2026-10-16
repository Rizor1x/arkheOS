#!/usr/bin/env python3
"""Type-check paging roles against the real kernel modules and dependencies."""
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGET = "x86_64-unknown-none"
CASES = {
    "kernel_release": ("E0599", "release"),
    "kernel_load": ("E0599", "load"),
    "user_build": ("E0599", "build_kernel"),
    "private_tables": ("E0616", "tables"),
    "wrong_owner_type": ("E0308", "mismatched types"),
}


def artifacts():
    env = dict(os.environ, ARKHE_TEST_CASE="boot")
    result = subprocess.run(
        ["cargo", "build", "--locked", "--release", "--package", "arkhe-kernel",
         "--target", TARGET, "--message-format=json"],
        cwd=ROOT, env=env, capture_output=True, text=True, check=False,
    )
    if result.returncode:
        raise RuntimeError("kernel dependency build failed\n" + result.stderr)
    libraries = {}
    for line in result.stdout.splitlines():
        item = json.loads(line)
        if item.get("reason") != "compiler-artifact":
            continue
        name = item["target"]["name"]
        if name in {"arkhe_kernel_core", "limine"}:
            paths = [pathlib.Path(p) for p in item["filenames"] if p.endswith(".rlib")]
            if len(paths) != 1:
                raise RuntimeError("ambiguous dependency artifact: " + name)
            libraries[name] = paths[0]
    if libraries.keys() != {"arkhe_kernel_core", "limine"}:
        raise RuntimeError("missing kernel dependency artifacts")
    return libraries


def check():
    output = ROOT / "build" / "check-paging-api"
    output.mkdir(parents=True, exist_ok=True)
    command = [
        "rustc", str(ROOT / "tests" / "paging_api.rs"), "--edition=2024",
        "--target", TARGET, "--crate-type=lib", "--emit=metadata",
        "--error-format=json", "-C", "panic=abort", "--out-dir", str(output),
    ]
    for name, path in artifacts().items():
        command += ["--extern", f"{name}={path}", "-L", f"dependency={path.parent}"]
    for case in [None, *CASES]:
        invocation = command if case is None else command + ["--cfg", f'paging_role="{case}"']
        result = subprocess.run(invocation, cwd=ROOT, capture_output=True, text=True, check=False)
        errors = [item for line in result.stderr.splitlines()
                  if (item := json.loads(line)).get("level") == "error" and item.get("code")]
        if case is None:
            valid = result.returncode == 0 and not errors
        else:
            code, message = CASES[case]
            valid = (result.returncode != 0 and len(errors) == 1
                     and errors[0]["code"]["code"] == code
                     and message in errors[0]["message"])
        if not valid:
            raise RuntimeError(f"unexpected paging API result: {case or 'valid roles'}\n{result.stderr}")
    print(f"Paging API check passed: valid roles and {len(CASES)} rejected misuse cases")


if __name__ == "__main__":
    try:
        check()
    except (OSError, RuntimeError, ValueError, KeyError) as error:
        sys.exit(f"Paging API check: {error}")
