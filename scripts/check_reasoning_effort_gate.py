#!/usr/bin/env python3
"""The SDK gate of the reasoning effort option names the option's release.

The Python and TypeScript SDKs send the provider-neutral `reasoning_effort`
field only to a server whose contract is at least the release that carries
the option. Each SDK names that release once:

    sdks/python/meerkat/client.py      _REASONING_EFFORT_MIN_CONTRACT = (X, Y, Z)
    sdks/typescript/src/client.ts      const REASONING_EFFORT_MIN_CONTRACT: readonly number[] = [X, Y, Z];

This check fails unless both constants name the version of the root
CHANGELOG section that holds the option's Added entry (the entry that opens
with ENTRY_MARKER). While that entry is under `## [Unreleased]` the expected
version is the next patch version after the workspace version in the root
Cargo.toml: a release cut that leaves the entry under Unreleased turns the
check red until the constants move, and so does an entry stamped under a
release the constants do not name.

Only the two constants are checked, not prose.

usage: check_reasoning_effort_gate.py [--root DIR]
exit 0 when both constants name the expected version, 1 otherwise.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ENTRY_MARKER = "- **Reasoning effort, one setting for every provider.**"
PYTHON_GATE = ("_REASONING_EFFORT_MIN_CONTRACT", "sdks/python/meerkat/client.py", re.compile(
    r"^_REASONING_EFFORT_MIN_CONTRACT = \((\d+), (\d+), (\d+)\)$", re.M))
TYPESCRIPT_GATE = ("REASONING_EFFORT_MIN_CONTRACT", "sdks/typescript/src/client.ts", re.compile(
    r"^const REASONING_EFFORT_MIN_CONTRACT: readonly number\[\] = \[(\d+), (\d+), (\d+)\];$", re.M))
HEADING = re.compile(r"^## \[([^\]]+)\]", re.M)
RELEASED = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
WORKSPACE_VERSION = re.compile(
    r"^\[workspace\.package\]\n(?:(?!\[).*\n)*?version = \"(\d+)\.(\d+)\.(\d+)\"", re.M)

Version = tuple[int, int, int]


class GateError(Exception):
    """The check cannot be made, or a constant names another version."""


def dotted(version: Version) -> str:
    return ".".join(str(part) for part in version)


def workspace_version(cargo_toml: str) -> Version:
    match = WORKSPACE_VERSION.search(cargo_toml)
    if match is None:
        raise GateError("Cargo.toml: no [workspace.package] version of the form X.Y.Z")
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)))


def entry_section(changelog: str) -> str:
    """The name of the `## [...]` section that holds the option's entry."""
    count = changelog.count("\n" + ENTRY_MARKER)
    if count != 1:
        raise GateError(
            f"CHANGELOG.md: the option's Added entry ({ENTRY_MARKER!r}) is there {count} times, expected once"
        )
    at = changelog.index("\n" + ENTRY_MARKER)
    section = None
    for heading in HEADING.finditer(changelog):
        if heading.start() > at:
            break
        section = heading.group(1)
    if section is None:
        raise GateError("CHANGELOG.md: the option's Added entry is above every `## [...]` heading")
    return section


def expected_version(changelog: str, cargo_toml: str) -> tuple[Version, str]:
    """The version the gates must name, and why."""
    section = entry_section(changelog)
    if section == "Unreleased":
        major, minor, patch = workspace_version(cargo_toml)
        return (major, minor, patch + 1), (
            f"the option's entry is under [Unreleased] and the workspace version is "
            f"{dotted((major, minor, patch))}, so the next patch release carries it "
            f"(a branch from before a release cut sees this until that release's version bump is merged in)"
        )
    match = RELEASED.match(section)
    if match is None:
        raise GateError(f"CHANGELOG.md: the option's entry is under [{section}], which is not a version")
    version = (int(match.group(1)), int(match.group(2)), int(match.group(3)))
    return version, f"the option's entry is under [{section}]"


def gate_version(text: str, pattern: re.Pattern[str], name: str, where: str) -> Version:
    matches = pattern.findall(text)
    if len(matches) != 1:
        raise GateError(f"{where}: the gate constant `{name}` is there {len(matches)} times, expected once")
    major, minor, patch = matches[0]
    return (int(major), int(minor), int(patch))


def check(root: Path) -> list[str]:
    """Problems found; empty when both gates name the expected version."""
    try:
        expected, why = expected_version(
            (root / "CHANGELOG.md").read_text(encoding="utf-8"),
            (root / "Cargo.toml").read_text(encoding="utf-8"),
        )
    except (GateError, OSError) as error:
        return [str(error)]
    problems = []
    for name, where, pattern in (PYTHON_GATE, TYPESCRIPT_GATE):
        try:
            found = gate_version((root / where).read_text(encoding="utf-8"), pattern, name, where)
        except (GateError, OSError) as error:
            problems.append(str(error))
            continue
        if found != expected:
            # The constant moves with the option's release, by a reviewed
            # change before the cut that carries the option; the check is
            # never waived.
            problems.append(
                f"`{name}` in {where} names {dotted(found)} and must name {dotted(expected)}: {why}. "
                f"Change the constant in {where} to {dotted(expected)} before that release is cut "
                f"(or, if the option's release moved, move its CHANGELOG entry and the constant together)"
            )
    return problems


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    args = parser.parse_args(argv)
    problems = check(args.root)
    for problem in problems:
        print(f"reasoning effort gate: {problem}", file=sys.stderr)
    if not problems:
        print("reasoning effort gate: both SDK constants name the option's release")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
