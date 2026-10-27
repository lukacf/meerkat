#!/usr/bin/env python3
"""Every published crate must ship the license texts its manifest declares.

The workspace declares `license = "MIT OR Apache-2.0"`, but cargo packages only
files under the crate directory, so the root LICENSE-MIT and LICENSE-APACHE
never reached the published `meerkat-meta` archive. The crate carries
symlinks to the root files, which `cargo package` follows.

This gate asks cargo for each published crate's package file list, so it also
catches an `include`/`exclude` that drops a file. The published set is derived
from release.yml by scripts/verify-crate-publication.py, not restated here.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Files every published crate package must contain at its root. A notices
# file that must travel with the crate belongs here too.
REQUIRED_FILES = (
    "LICENSE-MIT",
    "LICENSE-APACHE",
)


def published_crates() -> set[str]:
    spec = importlib.util.spec_from_file_location(
        "verify_crate_publication", ROOT / "scripts/verify-crate-publication.py"
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load scripts/verify-crate-publication.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.crates_release_publishes()


def package_listing(cargo: str, crate: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [cargo, "package", "--list", "-p", crate, "--allow-dirty"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


def main() -> int:
    cargo = os.environ.get("CARGO", str(ROOT / "scripts/repo-cargo"))
    try:
        crates = sorted(published_crates())
    except Exception as error:  # the publication gate reports its own detail
        print(f"error: {error}", file=sys.stderr)
        return 1

    failures: list[str] = []
    for crate in crates:
        listing = package_listing(cargo, crate)
        if listing.returncode != 0:
            detail = listing.stderr.strip().splitlines()[-1:] or ["no output"]
            failures.append(f"{crate}: cargo package --list failed: {detail[0]}")
            continue
        packaged = set(listing.stdout.splitlines())
        missing = [name for name in REQUIRED_FILES if name not in packaged]
        if missing:
            failures.append(
                f"{crate}: package lacks {', '.join(missing)}. Add symlinks to "
                f"the root files in the crate directory (ln -s ../../<file>)."
            )

    if failures:
        print("published crates are missing package files:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(f"crate package files OK: {crates} ship {', '.join(REQUIRED_FILES)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
