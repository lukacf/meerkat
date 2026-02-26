#!/usr/bin/env bash
# Pre-commit check: clippy + unit tests on changed crates only.
# Uses the default target dir for warm incremental cache.
set -euo pipefail

STAGED_FILES=$(git diff --cached --name-only --diff-filter=ACMR \
  | grep -E '\.(rs|toml)$' || true)

if [ -z "$STAGED_FILES" ]; then
  echo "No Rust/TOML changes staged, skipping."
  exit 0
fi

# Workspace manifest changes → cargo check --workspace (fast with warm cache)
if echo "$STAGED_FILES" | grep -qE '^Cargo\.(toml|lock)$'; then
  echo "Workspace manifest changed — running cargo check."
  cargo check --workspace
  exit $?
fi

# Extract crate directories from changed file paths
CHANGED_CRATES=$(echo "$STAGED_FILES" \
  | sed -n 's|^\([^/]*\)/.*|\1|p' \
  | sort -u \
  | while read -r dir; do
      if [ -f "$dir/Cargo.toml" ]; then
        echo "$dir"
      fi
    done)

if [ -z "$CHANGED_CRATES" ]; then
  echo "No testable crate changes detected, skipping."
  exit 0
fi

# Build -p flags for each changed crate
PKG_FLAGS=""
for crate_dir in $CHANGED_CRATES; do
  pkg=$(grep '^name' "$crate_dir/Cargo.toml" | head -1 | sed 's/.*= *"//' | sed 's/".*//')
  if [ -n "$pkg" ]; then
    PKG_FLAGS="$PKG_FLAGS -p $pkg"
  fi
done

if [ -z "$PKG_FLAGS" ]; then
  echo "No testable crates changed, skipping."
  exit 0
fi

echo "Checking changed crates:$PKG_FLAGS"

# Clippy first — catches type errors + lint issues in one pass
# shellcheck disable=SC2086
cargo clippy $PKG_FLAGS -- -D warnings

# Unit tests only (--lib) — fast, no integration tests
LIB_FLAGS=""
for crate_dir in $CHANGED_CRATES; do
  if [ -f "$crate_dir/src/lib.rs" ]; then
    pkg=$(grep '^name' "$crate_dir/Cargo.toml" | head -1 | sed 's/.*= *"//' | sed 's/".*//')
    LIB_FLAGS="$LIB_FLAGS -p $pkg"
  fi
done
if [ -n "$LIB_FLAGS" ]; then
  # shellcheck disable=SC2086
  cargo nextest run $LIB_FLAGS --lib
fi
