#!/usr/bin/env python3
"""Validate release crate membership and required package metadata."""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - exercised only on Python < 3.11
    import tomli as tomllib


def load_products(root: pathlib.Path) -> list[dict]:
    path = root / "ci" / "products.json"
    if not path.exists():
        return []
    return json.loads(path.read_text()).get("products", [])


PRODUCT_ROOTS: list[str] = []


def product_publish_orders(root: pathlib.Path) -> dict[str, list[str]]:
    """Each product other than Core (ci/products.json) and its publish order."""
    orders: dict[str, list[str]] = {}
    PRODUCT_ROOTS.clear()
    for product in load_products(root):
        if not product.get("root"):
            continue
        PRODUCT_ROOTS.append(product["root"])
        order = product.get("publish_order")
        orders[product["name"]] = list(order) if isinstance(order, list) else []
    return orders


def product_of(root: pathlib.Path, path: pathlib.Path, orders: dict[str, list[str]]) -> str | None:
    """The product other than Core whose root holds `path`, or None (Core)."""
    relative = path.resolve().relative_to(root.resolve()).as_posix()
    for product in load_products(root):
        product_root = product.get("root")
        if product_root and (relative == product_root or relative.startswith(f"{product_root}/")):
            return product["name"]
    return None


def main() -> int:
    if len(sys.argv) < 2:
        print("Usage: check_rust_release_packaging.py REPO_ROOT [CRATE ...]", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1])
    expected_order = sys.argv[2:]
    expected = set(expected_order)
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    # The other products (ci/products.json) publish their own crates on their
    # own version lines: their publishable members are checked against their
    # own publish order, and their requirements against their own versions.
    product_orders = product_publish_orders(root)

    paths: list[pathlib.Path] = []
    for member in workspace["workspace"]["members"]:
        if "*" in member:
            paths.extend(sorted(root.glob(member)))
        else:
            paths.append(root / member)

    publishable = set()
    product_publishable: dict[str, set[str]] = {}
    workspace_packages = {}
    member_dirs: dict[str, pathlib.Path] = {}
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
        member_dirs[name] = path.resolve()
        if package.get("publish", "default") is False:
            continue
        workspace_packages[name] = data
        product = product_of(root, path, product_orders)
        if product is None:
            publishable.add(name)
        else:
            product_publishable.setdefault(product, set()).add(name)

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
    for product, order in sorted(product_orders.items()):
        listed = set(order)
        members = product_publishable.get(product, set())
        missing += [f"{name} (not in {product}'s publish_order, ci/products.json)" for name in sorted(members - listed)]
        unexpected += [f"{name} ({product}'s publish_order names no publishable {product} member)" for name in sorted(listed - members)]
    core_packages = {name: data for name, data in workspace_packages.items() if name in publishable}
    order_errors = dependency_order_errors(core_packages, expected_order)
    requirement_errors = internal_dependency_requirement_errors(
        root, workspace, workspace_packages, member_dirs
    )
    bazel_errors = bazel_release_binary_version_env_errors(root, workspace_version)
    patch_errors = patch_config_errors(root, expected_order)
    docs_errors = documented_release_order_errors(root, expected_order)
    docs_errors += documented_count_claim_errors(root, expected_order, workspace)
    if (
        missing
        or unexpected
        or metadata_errors
        or order_errors
        or requirement_errors
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
        if requirement_errors:
            print(
                "Release crates publish a non-exact requirement on a workspace crate:",
                file=sys.stderr,
            )
            for err in requirement_errors:
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
    # Core's: an entry whose path lies under another product's root
    # (ci/products.json) tracks that product's version line instead.
    roots = tuple(f"{product_root}/" for product_root in PRODUCT_ROOTS)
    return sum(
        1
        for value in dependencies.values()
        if isinstance(value, dict)
        and "version" in value
        and "path" in value
        and not str(value["path"]).startswith(roots)
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


DEPENDENCY_TABLES = (
    ("dependencies", False),
    ("build-dependencies", False),
    ("dev-dependencies", True),
)


def manifest_dependency_tables(package_manifest: dict):
    """Yield (table label, is_dev, table) for every dependency table, target ones included."""

    for table, is_dev in DEPENDENCY_TABLES:
        yield table, is_dev, package_manifest.get(table) or {}
    for target, target_tables in (package_manifest.get("target") or {}).items():
        for table, is_dev in DEPENDENCY_TABLES:
            yield f"target.{target}.{table}", is_dev, (target_tables or {}).get(table) or {}


def internal_dependency_requirement_errors(
    root: pathlib.Path,
    workspace: dict,
    workspace_packages: dict[str, dict],
    member_dirs: dict[str, pathlib.Path],
) -> list[str]:
    """Every requirement a release crate publishes on a workspace crate is exact.

    meerkat 0.x patch releases may break public API, so a published crate must
    resolve exactly the sibling crates it was released with. A caret
    requirement (`version = "0.9.0"` is `^0.9.0`) let a consumer that pinned
    `=0.9.0` resolve 0.9.1 of some siblings. `cargo package` keeps the version
    of every normal, build and target dependency, and of every dev-dependency
    that has one (it strips a path-only dev-dependency), so each of those that
    names a workspace member must read `=<workspace version>`, the form
    cargo-release keeps exact when it bumps the workspace.

    A dependency is a workspace crate when its package name is a member's or
    its path is a member's directory (meerkat-cli's directory publishes as
    `rkat`). A requirement inherited from `[workspace.dependencies]` is
    reported once, at that entry, with the crates that publish it. Every
    internal entry there is held to the same rule even while no release crate
    uses it, so the next crate to inherit it is exact.
    """

    version = workspace["workspace"]["package"]["version"]
    workspace_deps = workspace.get("workspace", {}).get("dependencies", {})
    member_names = set(member_dirs)
    member_paths = set(member_dirs.values())
    workspace_root = root.resolve()
    # Each member's own version: Core's inherit the workspace's, a product's
    # crates carry their product's (ci/products.json). A requirement on a
    # member is exact on that member's version.
    member_versions: dict[str, str] = {}
    member_by_dir: dict[pathlib.Path, str] = {}
    for name, directory in member_dirs.items():
        package = tomllib.loads((directory / "Cargo.toml").read_text()).get("package", {})
        declared = package.get("version")
        member_versions[name] = declared if isinstance(declared, str) else version
        member_by_dir[directory] = name

    def exact_for(key: str, spec, declared_in: pathlib.Path) -> str:
        if isinstance(spec, dict):
            path = spec.get("path")
            if isinstance(path, str) and (declared_in / path).resolve() in member_by_dir:
                return f"={member_versions[member_by_dir[(declared_in / path).resolve()]]}"
            name = str(spec.get("package", key))
        else:
            name = key
        return f"={member_versions.get(name, version)}"

    def is_internal(key: str, spec, declared_in: pathlib.Path) -> bool:
        if not isinstance(spec, dict):
            return key in member_names
        if str(spec.get("package", key)) in member_names:
            return True
        path = spec.get("path")
        return isinstance(path, str) and (declared_in / path).resolve() in member_paths

    def requirement_of(spec) -> str | None:
        if isinstance(spec, str):
            return spec
        if isinstance(spec, dict):
            value = spec.get("version")
            return value if isinstance(value, str) else None
        return None

    errors: list[str] = []
    publishers: dict[str, list[str]] = {}
    for crate in sorted(workspace_packages):
        crate_dir = member_dirs[crate]
        for table, is_dev, deps in manifest_dependency_tables(workspace_packages[crate]):
            for key, spec in deps.items():
                inherited = isinstance(spec, dict) and spec.get("workspace") is True
                resolved = workspace_deps.get(key) if inherited else spec
                declared_in = workspace_root if inherited else crate_dir
                if resolved is None or not is_internal(key, resolved, declared_in):
                    continue
                requirement = requirement_of(resolved)
                exact = exact_for(key, resolved, declared_in)
                if requirement is None and is_dev:
                    # cargo package strips a path-only dev-dependency.
                    continue
                if inherited:
                    publishers.setdefault(key, []).append(f"{crate} [{table}]")
                    continue
                if requirement is None:
                    errors.append(
                        f"{crate}: [{table}] `{key}` names a workspace crate without a "
                        f"version, so it cannot be published; require `{exact}`"
                    )
                elif requirement != exact:
                    errors.append(
                        f"{crate}: [{table}] `{key}` publishes `{requirement}`; a "
                        f"requirement on a workspace crate must be exact (`{exact}`)"
                    )

    for key in sorted(set(workspace_deps) | set(publishers)):
        spec = workspace_deps.get(key)
        if spec is None or not is_internal(key, spec, workspace_root):
            continue
        requirement = requirement_of(spec)
        exact = exact_for(key, spec, workspace_root)
        users = publishers.get(key, [])
        if requirement is None and not users:
            # A path-only entry (used only as a dev-dependency) publishes
            # nothing.
            continue
        if requirement == exact:
            continue
        published_by = f" (published by {', '.join(users)})" if users else ""
        if requirement is None:
            errors.append(
                f"Cargo.toml [workspace.dependencies] `{key}` has no version{published_by}; "
                f"require `{exact}`"
            )
        else:
            errors.append(
                f"Cargo.toml [workspace.dependencies] `{key}` requires `{requirement}`"
                f"{published_by}; a requirement on a workspace crate must be exact "
                f"(`{exact}`)"
            )

    return errors


if __name__ == "__main__":
    raise SystemExit(main())
