#!/usr/bin/env python3
"""Validate release crate membership and required package metadata."""

from __future__ import annotations

import pathlib
import sys
import tomllib


def main() -> int:
    if len(sys.argv) < 2:
        print("Usage: check_rust_release_packaging.py REPO_ROOT [CRATE ...]", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1])
    expected_order = sys.argv[2:]
    expected = set(expected_order)
    workspace = tomllib.loads((root / "Cargo.toml").read_text())

    paths: list[pathlib.Path] = []
    for member in workspace["workspace"]["members"]:
        if "*" in member:
            paths.extend(sorted(root.glob(member)))
        else:
            paths.append(root / member)

    publishable = set()
    workspace_packages = {}
    metadata_errors = []
    for path in paths:
        manifest = path / "Cargo.toml"
        if not manifest.exists():
            continue
        data = tomllib.loads(manifest.read_text())
        package = data.get("package", {})
        name = package.get("name")
        if not name:
            continue
        if package.get("publish", "default") is False:
            continue
        publishable.add(name)
        workspace_packages[name] = data

        for field in ("description", "license", "repository", "homepage", "documentation"):
            value = package.get(field)
            if value is None:
                metadata_errors.append(
                    f"{name}: missing required package metadata field `{field}`"
                )
            elif isinstance(value, str) and not value.strip():
                metadata_errors.append(f"{name}: empty required package metadata field `{field}`")

    missing = sorted(publishable - expected)
    unexpected = sorted(expected - publishable)
    order_errors = dependency_order_errors(workspace_packages, expected_order)
    if missing or unexpected or metadata_errors or order_errors:
        if missing:
            print("Publishable workspace crates missing from release list:", file=sys.stderr)
            for name in missing:
                print(f"  - {name}", file=sys.stderr)
        if unexpected:
            print(
                "Release list contains crates that are not publishable workspace members:",
                file=sys.stderr,
            )
            for name in unexpected:
                print(f"  - {name}", file=sys.stderr)
        if metadata_errors:
            print("Publishable workspace crates with invalid release metadata:", file=sys.stderr)
            for err in metadata_errors:
                print(f"  - {err}", file=sys.stderr)
        if order_errors:
            print("Release crate list is not dependency ordered:", file=sys.stderr)
            for err in order_errors:
                print(f"  - {err}", file=sys.stderr)
        return 1

    return 0


def dependency_order_errors(
    workspace_packages: dict[str, dict],
    expected_order: list[str],
) -> list[str]:
    positions = {name: index for index, name in enumerate(expected_order)}
    release_crates = set(positions)
    errors = []

    for crate in expected_order:
        data = workspace_packages.get(crate)
        if not data:
            continue
        for dep in release_dependencies(data):
            if dep == crate or dep not in release_crates:
                continue
            if positions[dep] > positions[crate]:
                errors.append(f"{crate} appears before dependency {dep}")

    return errors


def release_dependencies(package_manifest: dict) -> set[str]:
    deps = set()

    def collect(section: dict | None) -> None:
        if not section:
            return
        for key, value in section.items():
            if isinstance(value, dict):
                deps.add(str(value.get("package", key)))
            else:
                deps.add(str(key))

    collect(package_manifest.get("dependencies"))
    collect(package_manifest.get("build-dependencies"))
    for target in package_manifest.get("target", {}).values():
        collect(target.get("dependencies"))
        collect(target.get("build-dependencies"))

    return deps


if __name__ == "__main__":
    raise SystemExit(main())
