#!/usr/bin/env python3
"""Verify the released public Live dependency and explicit private-feature boundary."""

import json
from pathlib import Path
import re
import sys
import tomllib


VERSION = "0.5.0"
CHECKSUM = "c04e6cdf6919f09de4be2af34f4d0be489dc78c32f0a6a0083c2299e7844c0ac"
PRIVATE_FEATURE = "oai-rt-rs/experimental-gpt-live"


def package_name(key, entry, workspace):
    if not isinstance(entry, dict):
        return key
    if entry.get("workspace") is True:
        return package_name(key, workspace.get(key, {}), {})
    return entry.get("package", key)


def check(root: Path) -> list[str]:
    manifest = tomllib.loads((root / "Cargo.toml").read_text())
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    provider = tomllib.loads((root / "meerkat-openai/Cargo.toml").read_text())
    bazel = json.loads((root / "MODULE.bazel.lock").read_text())
    module = (root / "MODULE.bazel").read_text()
    errors = []
    workspace = manifest.get("workspace", {}).get("dependencies", {})
    aliases = {key for key, entry in workspace.items()
               if package_name(key, entry, {}) == "oai-rt-rs"}
    if "oai-rt-rs" not in aliases:
        errors.append("canonical workspace oai-rt-rs dependency is missing")
    for key in aliases:
        dependency = workspace[key]
        if (
            not isinstance(dependency, dict)
            or dependency.get("version") != f"={VERSION}"
            or any(field in dependency for field in ("path", "git", "registry", "registry-index"))
            or "experimental-gpt-live" in dependency.get("features", [])
        ):
            errors.append("workspace oai-rt-rs must exact-pin public registry 0.5.0 without private features")
    packages = [entry for entry in lock.get("package", []) if entry["name"] == "oai-rt-rs"]
    if len(packages) != 1 or (
        packages[0].get("version") != VERSION
        or packages[0].get("checksum") != CHECKSUM
        or packages[0].get("source") != "registry+https://github.com/rust-lang/crates.io-index"
    ):
        errors.append("Cargo.lock must contain only the approved registry version and checksum")
    for entries in manifest.get("patch", {}).values():
        if any(name == "oai-rt-rs" or value.get("package") == "oai-rt-rs"
               for name, value in entries.items() if isinstance(value, dict)):
            errors.append("oai-rt-rs must not be patched")
    if any(key.split(":")[0] == "oai-rt-rs" for key in manifest.get("replace", {})):
        errors.append("oai-rt-rs must not be replaced")
    if (root / "third-party/oai-rt-rs/Cargo.toml").exists():
        errors.append("obsolete vendored oai-rt-rs is still present")
    if re.search(r"""\bcrate\s*=\s*["']oai-rt-rs["']""", module):
        errors.append("Bazel must not override oai-rt-rs")

    specs = []
    for extension in bazel.get("moduleExtensions", {}).values():
        for platform in extension.values():
            for name, spec in platform.get("generatedRepoSpecs", {}).items():
                if name.startswith("crates__oai-rt-rs-"):
                    specs.append((name, spec.get("attributes", {})))
    if not specs or any(
        name != f"crates__oai-rt-rs-{VERSION}"
        or attributes.get("sha256") != CHECKSUM
        or attributes.get("urls") != [f"https://static.crates.io/crates/oai-rt-rs/{VERSION}/download"]
        for name, attributes in specs
    ):
        errors.append("Bazel lock must fetch only the approved registry archive and checksum")

    provider_aliases = set()
    for target in [provider, *provider.get("target", {}).values()]:
        for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
            for key, direct in target.get(kind, {}).items():
                if package_name(key, direct, workspace) != "oai-rt-rs":
                    continue
                provider_aliases.add(key)
                if (
                    not isinstance(direct, dict)
                    or direct.get("workspace") is not True
                    or "experimental-gpt-live" in direct.get("features", [])
                    or any(field in direct for field in ("path", "git", "registry", "registry-index"))
                ):
                    errors.append("provider dependency must inherit the registry pin without private features")
    features = provider.get("features", {})
    if PRIVATE_FEATURE not in features.get("experimental-gpt-live", []):
        errors.append("the private dependency feature must remain explicit in experimental-gpt-live")
    for public in ("default", "live", "realtime"):
        pending, visited = [public], set()
        while pending:
            feature = pending.pop()
            if feature in visited:
                continue
            visited.add(feature)
            if any(feature in (f"{key}/experimental-gpt-live", f"{key}?/experimental-gpt-live")
                   for key in provider_aliases):
                errors.append(f"public feature {public} enables private Live")
            pending.extend(features.get(feature, []))
    return errors


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent
    try:
        errors = check(root)
    except (OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        print(f"public Live dependency inputs are unreadable or invalid: {type(error).__name__}", file=sys.stderr)
        return 1
    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    if errors:
        return 1
    print("Public Live uses registry oai-rt-rs 0.5.0 with exact checksum and explicit private-feature isolation")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
