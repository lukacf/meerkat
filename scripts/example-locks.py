#!/usr/bin/env python3
"""Validate or refresh locks owned by standalone example workspaces."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys

from check_cargo_lock_consistency import dangling_reference_errors, tomllib


def manifests(root: Path):
    for manifest in sorted((root / "examples").glob("*/Cargo.toml")):
        if "workspace" in tomllib.loads(manifest.read_text()):
            yield manifest


def consistent(lock: Path) -> bool:
    try:
        errors = dangling_reference_errors(tomllib.loads(lock.read_text()))
    except (FileNotFoundError, tomllib.TOMLDecodeError) as error:
        print(f"{lock}: {error}", file=sys.stderr)
        return False
    for error in errors:
        print(f"{lock}: {error}", file=sys.stderr)
    return not errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("check", "refresh"))
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--stage", action="store_true")
    args = parser.parse_args()
    if args.stage and args.mode != "refresh":
        parser.error("--stage requires refresh")
    root = args.root.resolve()
    cargo = os.environ.get("CARGO", str(root / "scripts/repo-cargo"))
    failed = False
    for manifest in manifests(root):
        lock = manifest.with_name("Cargo.lock")
        if args.mode == "check" and not consistent(lock):
            failed = True
            continue
        command = [cargo, "metadata", "--format-version", "1", "--manifest-path", str(manifest)]
        if args.mode == "check":
            command.append("--locked")
        result = subprocess.run(command, cwd=root, stdout=subprocess.DEVNULL, check=False)
        if result.returncode != 0:
            print(f"{manifest.relative_to(root)}: example lock {args.mode} failed", file=sys.stderr)
            print("Repair with scripts/example-locks.py refresh, then review and commit the example locks.",
                  file=sys.stderr)
            failed = True
            continue
        if not consistent(lock):
            failed = True
            continue
        if args.stage:
            subprocess.run(["git", "-C", str(root), "add", "--", str(lock)], check=True)
        print(f"{lock.relative_to(root)}: {args.mode} passed")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
