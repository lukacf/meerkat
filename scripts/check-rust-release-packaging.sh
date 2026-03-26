#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/meerkat-release-packaging-target}"

mapfile -t RELEASE_CRATES < <("$ROOT/scripts/release-rust-crates.sh")

python3 - "$ROOT" "${RELEASE_CRATES[@]}" <<'PY'
import pathlib
import sys
import tomllib

root = pathlib.Path(sys.argv[1])
expected = set(sys.argv[2:])
workspace = tomllib.loads((root / "Cargo.toml").read_text())

paths = []
for member in workspace["workspace"]["members"]:
    if "*" in member:
        paths.extend(sorted(root.glob(member)))
    else:
        paths.append(root / member)

publishable = set()
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

    for field in ("description", "license", "repository", "homepage", "documentation"):
        value = package.get(field)
        if value is None:
            metadata_errors.append(f"{name}: missing required package metadata field `{field}`")
        elif isinstance(value, str) and not value.strip():
            metadata_errors.append(f"{name}: empty required package metadata field `{field}`")

missing = sorted(publishable - expected)
unexpected = sorted(expected - publishable)
if missing or unexpected or metadata_errors:
    if missing:
        print("Publishable workspace crates missing from release list:", file=sys.stderr)
        for name in missing:
            print(f"  - {name}", file=sys.stderr)
    if unexpected:
        print("Release list contains crates that are not publishable workspace members:", file=sys.stderr)
        for name in unexpected:
            print(f"  - {name}", file=sys.stderr)
    if metadata_errors:
        print("Publishable workspace crates with invalid release metadata:", file=sys.stderr)
        for err in metadata_errors:
            print(f"  - {err}", file=sys.stderr)
    sys.exit(1)
PY

for crate in "${RELEASE_CRATES[@]}"; do
    echo "Packaging $crate"
    tmp_cfg="$(mktemp)"
    "$ROOT/scripts/generate-patch-config.sh" "$ROOT" "$crate" > "$tmp_cfg"
    "$CARGO" package -p "$crate" --locked --allow-dirty --config "$tmp_cfg"
    rm -f "$tmp_cfg"
done

echo "All release crates package successfully"
