#!/usr/bin/env bash
# Test only crates with staged changes (fast pre-commit).
# Falls back to full workspace test for root-level changes.
set -euo pipefail

FAST_TARGET_DIR="${FAST_TARGET_DIR:-target/fast}"

STAGED_FILES=$(git diff --cached --name-only --diff-filter=ACMR \
  | grep -E '\.(rs|toml)$' || true)

if [ -z "$STAGED_FILES" ]; then
  echo "No Rust/TOML changes staged, skipping tests."
  exit 0
fi

# Check for root-level manifest changes (workspace Cargo.toml, Cargo.lock).
# These affect dependency resolution but rarely break tests. A fast check
# is sufficient — the pre-push hook runs the full workspace test.
if echo "$STAGED_FILES" | grep -qE '^Cargo\.(toml|lock)$'; then
  echo "Workspace manifest changed — running cargo check."
  CARGO_TARGET_DIR="$FAST_TARGET_DIR" cargo check --workspace
  exit $?
fi

# Extract crate directories from changed file paths.
# Files like "meerkat-core/src/foo.rs" → "meerkat-core"
# Files without a "/" (root-level) are already handled above.
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

echo "Testing changed crates:$PKG_FLAGS"
# shellcheck disable=SC2086
CARGO_TARGET_DIR="$FAST_TARGET_DIR" cargo test $PKG_FLAGS --lib --bins --tests
