#!/usr/bin/env python3
"""Every workspace member must be explicitly published or explicitly not.

Published workspace crates are named in hand-written `-p` flags in release.yml. `meta-store-conformance` carried no `publish` key at
all, so it read as publishable to every tool and every reader while never having
been published at any version. Nothing could tell that apart from an accidental
omission, in either direction.

The published set is DERIVED from release.yml rather than restated here. A second
hand-maintained list would drift from the first and the drift would be invisible
until a release, which is the failure this file exists to prevent.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"

# `cargo publish -p NAME`, however the wrapper is spelled.
PUBLISH_INVOCATION = re.compile(r"cargo\s+publish\s+-p\s+([A-Za-z0-9_-]+)")


class CratePublicationError(RuntimeError):
    pass


def workspace_members() -> dict[str, dict]:
    root = tomllib.loads((ROOT / "Cargo.toml").read_text())
    members = root.get("workspace", {}).get("members", [])
    if not members:
        raise CratePublicationError("root Cargo.toml declares no workspace members")
    out = {}
    for member in members:
        manifest_path = ROOT / member / "Cargo.toml"
        if not manifest_path.is_file():
            raise CratePublicationError(f"workspace member {member} has no Cargo.toml")
        manifest = tomllib.loads(manifest_path.read_text())
        package = manifest.get("package")
        if not package:
            raise CratePublicationError(f"{member}/Cargo.toml has no [package]")
        out[package["name"]] = manifest
    return out


def crates_release_publishes() -> set[str]:
    names = set(PUBLISH_INVOCATION.findall(RELEASE_WORKFLOW.read_text()))
    if not names:
        raise CratePublicationError(
            "release.yml contains no `cargo publish -p NAME` invocation. Either "
            "publication moved and this gate can no longer see it, or it was "
            "removed. Both need a human."
        )
    return names


def declares_publish_false(manifest: dict) -> bool:
    return manifest.get("package", {}).get("publish") is False


def versionless_path_dependencies(manifest: dict) -> list[str]:
    """Deps that make `cargo publish` refuse, found before a release does.

    A path dependency without a version is legal in a workspace and fatal at
    publish time. Reaching that error during a release means discovering it
    after every cross-compile, after the GitHub assets, and after PyPI and npm.
    """
    offenders = []
    for table in ("dependencies", "build-dependencies"):
        for name, spec in (manifest.get(table) or {}).items():
            if isinstance(spec, dict) and "path" in spec and "version" not in spec:
                offenders.append(f"{name} ({table})")
    return sorted(offenders)


def main() -> int:
    try:
        members = workspace_members()
        published = crates_release_publishes()
    except CratePublicationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    failures: list[str] = []

    unknown = sorted(published - set(members))
    if unknown:
        failures.append(
            f"release.yml publishes {unknown}, which are not workspace members. "
            "A typo here fails at the publish step, mid-release."
        )

    for name, manifest in sorted(members.items()):
        is_published = name in published
        is_withheld = declares_publish_false(manifest)

        if is_published and is_withheld:
            failures.append(
                f"{name}: release.yml publishes it and its manifest says "
                "publish = false. One of the two is wrong."
            )
        elif not is_published and not is_withheld:
            failures.append(
                f"{name}: not published by release.yml and does not declare "
                "publish = false, so whether it is meant to be public is "
                "unrecorded. Add it to the release, or set publish = false."
            )

        if is_published:
            offenders = versionless_path_dependencies(manifest)
            if offenders:
                failures.append(
                    f"{name} is published but has path dependencies without a "
                    f"version, so cargo publish will refuse: {offenders}"
                )

    if failures:
        print("crate publication declarations are inconsistent:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    withheld = sorted(n for n, m in members.items() if declares_publish_false(m))
    print(
        f"crate publication OK: {len(members)} workspace member(s); "
        f"published {sorted(published)}; withheld {withheld}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
