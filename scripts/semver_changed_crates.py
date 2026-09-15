#!/usr/bin/env python3
"""Classify publishable crates against the published baseline tag.

`cargo-semver-checks` reports findings on a crate's own public items, built
from that crate's source and the dependency specs it declares. A crate whose
source directory is byte-identical to the baseline release and whose declared
dependency specs (including the workspace-inherited ones) are unchanged cannot
produce a finding, so measuring it again only spends runner time. This module
decides, per publishable crate, whether the release must rebuild and compare it
(`changed`), may record it as identical to the baseline (`unchanged`), or is
publishing it for the first time (`first_publish`).

The classification is deliberately conservative: any difference in the crate
directory, in the workspace dependency table entries the crate references, or
in `[workspace.package]` fields other than `version` marks the crate changed.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field

DEPENDENCY_TABLES = ("dependencies", "build-dependencies")

# Paths inside a crate directory that never reach the library's public API:
# generated Bazel metadata (rewritten with the version on every release), and
# test, bench, example and documentation trees. Everything else, including
# Cargo.toml, build.rs and src/, counts.
NON_API_PATHSPECS = (
    "BUILD.bazel",
    "*.bazel",
    "tests",
    "benches",
    "examples",
    "docs",
    "README.md",
    "CHANGELOG.md",
)


@dataclass
class Classification:
    baseline_tag: str
    changed: list[str] = field(default_factory=list)
    unchanged: list[str] = field(default_factory=list)
    first_publish: list[str] = field(default_factory=list)
    reasons: dict[str, str] = field(default_factory=dict)

    def as_json(self) -> str:
        return json.dumps(
            {
                "baseline_tag": self.baseline_tag,
                "changed": self.changed,
                "unchanged": self.unchanged,
                "first_publish": self.first_publish,
                "reasons": self.reasons,
            },
            indent=2,
            sort_keys=True,
        )


def _git(repo_root: pathlib.Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", "-C", str(repo_root), *args],
        check=False,
        capture_output=True,
        text=True,
    )


def git_show(repo_root: pathlib.Path, rev: str, path: str) -> str | None:
    result = _git(repo_root, "show", f"{rev}:{path}")
    if result.returncode != 0:
        return None
    return result.stdout


def tree_changed(repo_root: pathlib.Path, baseline: str, head: str, rel_dir: str) -> bool:
    """True when any API-relevant tracked file under `rel_dir` differs between the revisions."""
    pathspecs = [rel_dir] + [f":(exclude){rel_dir}/{path}" for path in NON_API_PATHSPECS]
    result = _git(repo_root, "diff", "--quiet", baseline, head, "--", *pathspecs)
    if result.returncode == 0:
        return False
    if result.returncode == 1:
        return True
    raise RuntimeError(f"git diff failed for {rel_dir}: {result.stderr.strip()}")


def tree_exists(repo_root: pathlib.Path, rev: str, rel_dir: str) -> bool:
    return _git(repo_root, "cat-file", "-e", f"{rev}:{rel_dir}/Cargo.toml").returncode == 0


def workspace_member_dirs(repo_root: pathlib.Path, rev: str) -> dict[str, str]:
    """Map package name to its repo-relative directory for the workspace at `rev`."""
    root_text = git_show(repo_root, rev, "Cargo.toml")
    if root_text is None:
        raise RuntimeError(f"no root Cargo.toml at {rev}")
    root = tomllib.loads(root_text)
    members: list[str] = []
    for member in root["workspace"]["members"]:
        if "*" in member:
            listing = _git(repo_root, "ls-tree", "-d", "--name-only", rev, member.rstrip("*"))
            members.extend(line for line in listing.stdout.splitlines() if line)
        else:
            members.append(member)
    dirs: dict[str, str] = {}
    for member in members:
        manifest = git_show(repo_root, rev, f"{member}/Cargo.toml")
        if manifest is None:
            continue
        name = tomllib.loads(manifest).get("package", {}).get("name")
        if name:
            dirs[name] = member
    return dirs


def workspace_dependency_table(repo_root: pathlib.Path, rev: str) -> dict[str, object]:
    root_text = git_show(repo_root, rev, "Cargo.toml")
    if root_text is None:
        return {}
    return tomllib.loads(root_text).get("workspace", {}).get("dependencies", {})


def workspace_package_fields(repo_root: pathlib.Path, rev: str) -> dict[str, object]:
    root_text = git_show(repo_root, rev, "Cargo.toml")
    if root_text is None:
        return {}
    fields = dict(tomllib.loads(root_text).get("workspace", {}).get("package", {}))
    fields.pop("version", None)
    return fields


def normalize_spec(spec: object) -> object:
    """Drop the `version` of workspace-member (path) dependencies.

    A release bumps every member's version in lockstep, so that field changes
    on every release by construction; the member's own API is measured on the
    member itself. Every other field (features, default-features, registry
    versions, optional flags) still counts as a spec change.
    """
    if isinstance(spec, dict) and "path" in spec:
        return {key: value for key, value in spec.items() if key != "version"}
    return spec


def declared_workspace_dependencies(manifest_text: str) -> set[str]:
    """Names of dependencies a crate manifest inherits from `[workspace.dependencies]`."""
    data = tomllib.loads(manifest_text)
    names: set[str] = set()
    tables = [data.get(table, {}) for table in DEPENDENCY_TABLES]
    for target in data.get("target", {}).values():
        tables.extend(target.get(table, {}) for table in DEPENDENCY_TABLES)
    for table in tables:
        for name, spec in table.items():
            if isinstance(spec, dict) and spec.get("workspace") is True:
                names.add(str(spec.get("package", name)))
    return names


def classify(
    repo_root: pathlib.Path,
    baseline_tag: str,
    head: str,
    release_crates: list[str],
) -> Classification:
    result = Classification(baseline_tag=baseline_tag)
    head_dirs = workspace_member_dirs(repo_root, head)
    baseline_deps = workspace_dependency_table(repo_root, baseline_tag)
    head_deps = workspace_dependency_table(repo_root, head)
    package_fields_changed = workspace_package_fields(repo_root, baseline_tag) != (
        workspace_package_fields(repo_root, head)
    )

    for name in release_crates:
        rel_dir = head_dirs.get(name)
        if rel_dir is None:
            result.changed.append(name)
            result.reasons[name] = "not a workspace member at HEAD; measured to fail closed"
            continue
        if not tree_exists(repo_root, baseline_tag, rel_dir):
            result.first_publish.append(name)
            result.reasons[name] = f"no crate directory at {baseline_tag}"
            continue
        if package_fields_changed:
            result.changed.append(name)
            result.reasons[name] = "[workspace.package] fields other than version changed"
            continue
        if tree_changed(repo_root, baseline_tag, head, rel_dir):
            result.changed.append(name)
            result.reasons[name] = f"source differs from {baseline_tag}"
            continue
        manifest_text = git_show(repo_root, head, f"{rel_dir}/Cargo.toml") or ""
        drifted = sorted(
            dep
            for dep in declared_workspace_dependencies(manifest_text)
            if normalize_spec(baseline_deps.get(dep)) != normalize_spec(head_deps.get(dep))
        )
        if drifted:
            result.changed.append(name)
            result.reasons[name] = "workspace dependency spec changed: " + ", ".join(drifted)
            continue
        result.unchanged.append(name)
        result.reasons[name] = f"identical source and dependency specs vs {baseline_tag}"
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", required=True, type=pathlib.Path)
    parser.add_argument("--baseline-tag", required=True)
    parser.add_argument("--head", default="HEAD")
    parser.add_argument(
        "--release-crate",
        action="append",
        default=[],
        help="publishable crate to classify; repeatable, or supply names on stdin",
    )
    args = parser.parse_args()
    crates = list(args.release_crate)
    if not crates and not sys.stdin.isatty():
        crates = [line.strip() for line in sys.stdin if line.strip()]
    if not crates:
        print("error: no release crates supplied", file=sys.stderr)
        return 2
    try:
        classification = classify(args.repo_root, args.baseline_tag, args.head, crates)
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(classification.as_json())
    return 0


if __name__ == "__main__":
    sys.exit(main())
