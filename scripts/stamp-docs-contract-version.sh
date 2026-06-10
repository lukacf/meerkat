#!/usr/bin/env bash
# Stamp the workspace/contract version into version-bearing docs lines.
#
# Public docs must not hand-maintain version strings: every marker line below
# is a read-only projection of `workspace.package.version` (which is
# lock-stepped with `ContractVersion::CURRENT`). This script rewrites those
# projections; `--check` verifies them without writing (wired into
# scripts/verify-version-parity.sh so CI fails closed on drift).
#
# Usage:
#   ./scripts/stamp-docs-contract-version.sh            # stamp from Cargo.toml
#   ./scripts/stamp-docs-contract-version.sh 0.7.0      # stamp explicit version
#   ./scripts/stamp-docs-contract-version.sh --check    # verify, no writes

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

MODE="stamp"
VERSION=""
for arg in "$@"; do
    case "$arg" in
        --check) MODE="check" ;;
        *) VERSION="$arg" ;;
    esac
done

if [ -z "$VERSION" ]; then
    VERSION=$(python3 - "$ROOT/Cargo.toml" <<'PY'
import pathlib
import sys

try:
    import tomllib  # py311+
except ModuleNotFoundError:
    import tomli as tomllib  # py310 fallback

manifest = tomllib.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
print(manifest["workspace"]["package"]["version"])
PY
    )
fi

DOCS_VERSION_MODE="$MODE" DOCS_VERSION="$VERSION" python3 - "$ROOT" <<'PY'
import os
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
mode = os.environ["DOCS_VERSION_MODE"]
version = os.environ["DOCS_VERSION"]

SEMVER = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?")

# A line is a version projection when it matches one of these markers. The
# (marker, path-prefix) pairs keep generic markers (like a bare `"version":`
# JSON key) scoped to the docs trees where they are known to mean the Meerkat
# contract/package version.
MARKERS = [
    (re.compile(r'"contract_version"\s*:\s*"'), "docs/"),
    (re.compile(r'"version"\s*:\s*"'), "docs/api/"),
    (re.compile(r"CONTRACT_VERSION"), "docs/"),
    (re.compile(r"\*\*Contract version:\*\*"), "docs/"),
    (re.compile(r'^\s*meerkat(?:-[a-z-]+)?\s*=\s*(?:"|\{)'), "docs/"),
]

# `ContractVersion` also serializes in struct form
# (`{"major": .., "minor": .., "patch": ..}`); docs showing that wire shape
# are projections of the same fact and are stamped from the core
# (pre-release-stripped) version.
STRUCT_FORM = re.compile(
    r'("contract_version"\s*:\s*\{\s*"major"\s*:\s*)(\d+)'
    r'(\s*,\s*"minor"\s*:\s*)(\d+)(\s*,\s*"patch"\s*:\s*)(\d+)'
)

core_version = version.split("-", 1)[0]
major, minor, patch = core_version.split(".")


def stamp_struct_form(match: "re.Match[str]") -> str:
    return (
        f"{match.group(1)}{major}{match.group(3)}{minor}{match.group(5)}{patch}"
    )


changed = []
stale = []
for path in sorted(root.joinpath("docs").rglob("*.mdx")) + sorted(
    root.joinpath("docs").rglob("*.md")
):
    rel = path.relative_to(root).as_posix()
    text = path.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    rewritten = []
    file_changed = False
    for lineno, line in enumerate(lines, start=1):
        is_marker = any(
            marker.search(line) and rel.startswith(prefix) for marker, prefix in MARKERS
        )
        if is_marker and SEMVER.search(line):
            new_line = SEMVER.sub(version, line)
            if new_line != line:
                file_changed = True
                stale.append(f"{rel}:{lineno}: {line.strip()}")
            rewritten.append(new_line)
        elif rel.startswith("docs/") and STRUCT_FORM.search(line):
            new_line = STRUCT_FORM.sub(stamp_struct_form, line)
            if new_line != line:
                file_changed = True
                stale.append(f"{rel}:{lineno}: {line.strip()}")
            rewritten.append(new_line)
        else:
            rewritten.append(line)
    if file_changed:
        changed.append(rel)
        if mode == "stamp":
            path.write_text("".join(rewritten), encoding="utf-8")

if mode == "check":
    if stale:
        print(f"FAIL: docs version projections are stale (expected {version}):")
        for entry in stale:
            print(f"  {entry}")
        print("Run: ./scripts/stamp-docs-contract-version.sh")
        sys.exit(1)
    print(f"Docs version projections: OK ({version})")
else:
    if changed:
        for rel in changed:
            print(f"  Stamped: {rel}")
    else:
        print(f"  Docs already at {version}")
PY
