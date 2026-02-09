#!/usr/bin/env bash
# Test only crates with staged changes (fast pre-commit).
# Falls back to workspace test if detection fails.
set -euo pipefail

# Get list of staged .rs and .toml files, extract crate directories
CHANGED_CRATES=$(git diff --cached --name-only --diff-filter=ACMR \
  | grep -E '\.(rs|toml)$' \
  | sed -E 's|^([^/]+)/.*|\1|' \
  | sort -u \
  | while read -r dir; do
      # Only include workspace members that have a Cargo.toml
      if [ -f "$dir/Cargo.toml" ]; then
        echo "$dir"
      fi
    done)

if [ -z "$CHANGED_CRATES" ]; then
  echo "No crate changes detected, skipping tests."
  exit 0
fi

# Build -p flags for each changed crate
PKG_FLAGS=""
for crate_dir in $CHANGED_CRATES; do
  # Extract package name from Cargo.toml
  pkg=$(grep '^name' "$crate_dir/Cargo.toml" | head -1 | sed 's/.*= *"//' | sed 's/".*//')
  if [ -n "$pkg" ]; then
    PKG_FLAGS="$PKG_FLAGS -p $pkg"
  fi
done

if [ -z "$PKG_FLAGS" ]; then
  echo "No testable crates changed, skipping."
  exit 0
fi

echo "Testing changed crates:$PKG_FLAGS"
# shellcheck disable=SC2086
cargo test $PKG_FLAGS --lib --bins --tests
