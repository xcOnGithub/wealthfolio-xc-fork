#!/usr/bin/env python3
"""Conservative backend-state fingerprint for image-only rollback decisions."""
import hashlib
from pathlib import Path
import subprocess


def fingerprint(root):
    paths = subprocess.check_output(
        ["git", "ls-files", "-z", "--", "apps/server", "crates", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "Dockerfile", "config"],
        cwd=root,
    ).split(b"\0")
    digest = hashlib.sha256()
    for name in sorted(path for path in paths if path):
        digest.update(name + b"\0")
        digest.update(hashlib.sha256((Path(root) / name.decode()).read_bytes()).digest())
    return digest.hexdigest()


if __name__ == "__main__":
    print(fingerprint(Path(__file__).resolve().parents[2]))
