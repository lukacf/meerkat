#!/usr/bin/env python3
"""Validate release crate membership and required package metadata."""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - exercised only on Python < 3.11
    import tomli as tomllib


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

    workspace_version = workspace["workspace"]["package"]["version"]
    missing = sorted(publishable - expected)
    unexpected = sorted(expected - publishable)
    order_errors = dependency_order_errors(workspace_packages, expected_order)
    bazel_errors = bazel_release_binary_version_env_errors(root, workspace_version)
    patch_errors = patch_config_errors(root, expected_order)
    docs_errors = documented_release_order_errors(root, expected_order)
    docs_errors += documented_count_claim_errors(root, expected_order, workspace)
    if (
        missing
        or unexpected
        or metadata_errors
        or order_errors
        or bazel_errors
        or patch_errors
        or docs_errors
    ):
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
        if bazel_errors:
            print("BuildBuddy release binaries have invalid Cargo version metadata:", file=sys.stderr)
            for err in bazel_errors:
                print(f"  - {err}", file=sys.stderr)
        if patch_errors:
            print("Publish patch configuration does not cover the release crates:", file=sys.stderr)
            for err in patch_errors:
                print(f"  - {err}", file=sys.stderr)
        if docs_errors:
            print("Documented enumeration is stale:", file=sys.stderr)
            for err in docs_errors:
                print(f"  - {err}", file=sys.stderr)
        return 1

    return 0


def patch_config_errors(root: pathlib.Path, expected_order: list[str]) -> list[str]:
    """Every release crate must appear in the generated [patch.crates-io] config.

    `cargo publish` verifies each package against the patched workspace, so a
    crate absent here is resolved from crates.io at a version that does not
    exist yet. That failure lands during package verification, several gates
    downstream of the manifest edit that caused it.
    """

    generator = root / "scripts" / "generate-patch-config.sh"
    if not generator.exists():
        return [f"{generator}: patch config generator is missing"]

    try:
        rendered = subprocess.run(
            [str(generator), str(root)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except subprocess.CalledProcessError as error:
        return [f"{generator.name} failed (exit {error.returncode}): {error.stderr.strip()}"]

    patched: dict[str, str] = {}
    errors: list[str] = []
    for line in rendered.splitlines():
        match = re.fullmatch(r'(\S+) = \{ path = "(.+)" \}', line.strip())
        if match:
            patched[match.group(1)] = match.group(2)

    for crate in expected_order:
        crate_path = patched.get(crate)
        if crate_path is None:
            errors.append(f"{crate}: release crate is absent from [patch.crates-io]")
            continue
        manifest = pathlib.Path(crate_path) / "Cargo.toml"
        if not manifest.exists():
            errors.append(f"{crate}: patched path {crate_path} has no Cargo.toml")
            continue
        patched_name = tomllib.loads(manifest.read_text()).get("package", {}).get("name")
        if patched_name != crate:
            errors.append(
                f"{crate}: patched path {crate_path} declares package `{patched_name}`"
            )

    for crate in sorted(set(patched) - set(expected_order)):
        errors.append(f"{crate}: patched but not a release crate")

    return errors


def documented_release_order_errors(root: pathlib.Path, expected_order: list[str]) -> list[str]:
    """CLAUDE.md documents the publish order; it must match the canonical list.

    The order itself carries dependency knowledge that no tool derives, so it
    stays hand-written - but a hand-written list that nothing compares against
    is how a new crate stayed invisible in the documented contract while three
    gates disagreed about how many crates exist.

    Counts are checked separately, by `documented_count_claim_errors`.
    """

    docs_path = root / "CLAUDE.md"
    try:
        lines = docs_path.read_text().splitlines()
    except FileNotFoundError:
        return [f"{docs_path}: file is missing"]

    errors: list[str] = []
    anchor = next(
        (index for index, line in enumerate(lines) if "canonical publish order lives in" in line),
        None,
    )
    if anchor is None:
        errors.append(f"{docs_path.name}: no line documents the canonical publish order")
    else:
        listing_index = next(
            (index for index in range(anchor + 1, len(lines)) if lines[index].strip()),
            None,
        )
        listing = lines[listing_index] if listing_index is not None else ""
        documented = re.findall(r"`([A-Za-z0-9_.-]+)`", listing)
        if documented != expected_order:
            documented_set = set(documented)
            expected_set = set(expected_order)
            for crate in expected_order:
                if crate not in documented_set:
                    errors.append(
                        f"{docs_path.name}:{(listing_index or 0) + 1}: publish order omits `{crate}`"
                    )
            for crate in documented:
                if crate not in expected_set:
                    errors.append(
                        f"{docs_path.name}:{(listing_index or 0) + 1}: publish order lists "
                        f"`{crate}`, which is not a release crate"
                    )
            if not errors:
                errors.append(
                    f"{docs_path.name}:{(listing_index or 0) + 1}: publish order lists the "
                    "right crates in a different order than scripts/release-rust-crates.sh"
                )

    return errors


# Any bare integer that quantifies crates or path deps in CLAUDE.md. A leading
# `~` marks the number as deliberately approximate and exempts it.
COUNT_CLAIM_SHAPE = re.compile(r"(?<!~)\b(\d+)\s+(?:\S+\s+){0,2}?(?:crates|path deps)\b")


def internal_path_dependency_count(workspace: dict) -> int:
    """Internal workspace deps that pin a version alongside their path.

    `version` is the load-bearing part: it is what must track
    `workspace.package.version`, and it is what a published crate resolves
    against once the path is stripped. A path-only dep (`[patch.crates-io]`
    vendoring, for instance) is not part of that contract.
    """

    dependencies = workspace.get("workspace", {}).get("dependencies", {})
    return sum(
        1
        for value in dependencies.values()
        if isinstance(value, dict) and "version" in value and "path" in value
    )


def documented_count_claim_errors(
    root: pathlib.Path,
    expected_order: list[str],
    workspace: dict,
) -> list[str]:
    """Every documented count must be bound to the artifact that derives it.

    Two distinct quantities are documented in CLAUDE.md and they are not the
    same number: the release crate count is owned by
    scripts/release-rust-crates.sh, the internal path dependency count is owned
    by Cargo.toml's [workspace.dependencies]. They were equal once, which is
    precisely how `42 path deps` survived a crate addition looking correct.

    So this is fail-closed on shape, not just on value: a count claim that no
    registered pattern binds to a derived quantity is itself an error. A fourth
    hardcoded pattern would fix one stale line; refusing unbound claims fixes
    the class.
    """

    docs_path = root / "CLAUDE.md"
    try:
        lines = docs_path.read_text().splitlines()
    except FileNotFoundError:
        return [f"{docs_path}: file is missing"]

    bound_claims = (
        # pattern, derived value, what is being counted, deriving authority
        (
            re.compile(r"\((\d+) crates, dependency order\)"),
            len(expected_order),
            "release crates",
            "scripts/release-rust-crates.sh",
        ),
        (
            re.compile(r"Publishes (\d+) Rust crates"),
            len(expected_order),
            "release crates",
            "scripts/release-rust-crates.sh",
        ),
        (
            re.compile(r"all (\d+) publishable Rust crates"),
            len(expected_order),
            "release crates",
            "scripts/release-rust-crates.sh",
        ),
        (
            re.compile(r"\((\d+) path deps\)"),
            internal_path_dependency_count(workspace),
            "internal path deps",
            "Cargo.toml [workspace.dependencies]",
        ),
    )

    errors: list[str] = []
    for line_number, line in enumerate(lines, start=1):
        bound_offsets = set()
        for pattern, derived, noun, authority in bound_claims:
            for match in pattern.finditer(line):
                bound_offsets.add(match.start(1))
                claimed = int(match.group(1))
                if claimed != derived:
                    errors.append(
                        f"{docs_path.name}:{line_number}: claims {claimed} {noun}, "
                        f"{authority} lists {derived}"
                    )
        for match in COUNT_CLAIM_SHAPE.finditer(line):
            if match.start(1) in bound_offsets:
                continue
            errors.append(
                f"{docs_path.name}:{line_number}: count claim "
                f"`{match.group(0).strip()}` is bound to no derived quantity; "
                "register it in check_rust_release_packaging.py or write it as "
                "an approximation with a leading `~`"
            )

    return errors


def bazel_release_binary_version_env_errors(root: pathlib.Path, version: str) -> list[str]:
    """Check the generated Bazel release binary targets embed the crate version."""

    targets = {
        pathlib.Path("crates/meerkat-cli/BUILD.bazel"): ["rkat"],
        pathlib.Path("crates/meerkat-rpc/BUILD.bazel"): ["rkat_rpc_bin"],
        pathlib.Path("crates/meerkat-rest/BUILD.bazel"): ["rkat_rest_bin"],
        pathlib.Path("crates/meerkat-mcp-server/BUILD.bazel"): ["rkat_mcp_bin"],
    }
    expected = f'"CARGO_PKG_VERSION": "{version}"'
    errors = []

    for rel_path, names in targets.items():
        path = root / rel_path
        try:
            text = path.read_text()
        except FileNotFoundError:
            errors.append(f"{rel_path}: generated BUILD file is missing")
            continue

        for name in names:
            block = find_bazel_target_block(text, name)
            if block is None:
                errors.append(f"{rel_path}: target {name} is missing")
            elif expected not in block:
                errors.append(f"{rel_path}: target {name} missing {expected}")

    return errors


def find_bazel_target_block(text: str, name: str) -> str | None:
    marker = f'name = "{name}",'
    marker_index = text.find(marker)
    if marker_index < 0:
        return None

    start = text.rfind("\nrust_", 0, marker_index)
    if start < 0:
        start = 0
    else:
        start += 1
    end = text.find("\n)", marker_index)
    if end < 0:
        end = len(text)
    return text[start:end]


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
