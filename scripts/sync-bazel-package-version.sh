#!/usr/bin/env bash
# Sync the hardcoded CARGO_PKG_VERSION literals in
# meerkat-machine-codegen/BUILD.bazel to the Rust workspace version.
#
# Bazel does not read CARGO_PKG_VERSION from Cargo.toml the way `cargo` does, so
# each rust_* target's rustc_env carries the version literal. This script
# rewrites every occurrence to match `workspace.package.version`, keeping the
# Bazel build's version metadata in lockstep with the canonical source of truth.
# It is invoked by `make regen-schemas` and gated by
# `scripts/verify-version-parity.sh`.
#
# Usage:
#   ./scripts/sync-bazel-package-version.sh          # reads version from Cargo.toml
#   ./scripts/sync-bazel-package-version.sh 0.8.0    # explicit version

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
BAZEL_BUILD="$ROOT/meerkat-machine-codegen/BUILD.bazel"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
    VERSION=$("$CARGO" metadata --manifest-path "$ROOT/Cargo.toml" \
        --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "meerkat") | .version')
fi

if [ -z "$VERSION" ]; then
    echo "error: could not determine workspace version" >&2
    exit 1
fi

if [ ! -f "$BAZEL_BUILD" ]; then
    echo "error: $BAZEL_BUILD not found" >&2
    exit 1
fi

echo "Syncing Bazel CARGO_PKG_VERSION to $VERSION"

# Rewrite every `"CARGO_PKG_VERSION": "<anything>"` literal to the workspace
# version. The leading-space-tolerant pattern matches the rustc_env entries
# regardless of indentation.
if sed --version >/dev/null 2>&1; then
    sed -i -E "s/(\"CARGO_PKG_VERSION\": *\")[^\"]*(\")/\1${VERSION}\2/" "$BAZEL_BUILD"
else
    sed -i '' -E "s/(\"CARGO_PKG_VERSION\": *\")[^\"]*(\")/\1${VERSION}\2/" "$BAZEL_BUILD"
fi

echo "  Updated: ${BAZEL_BUILD#"$ROOT"/} (CARGO_PKG_VERSION)"
