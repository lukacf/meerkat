#!/usr/bin/env bash
# Pre-push clippy gate: lint only changed crates instead of the full workspace.
# Falls back to workspace clippy when root Cargo.toml/Cargo.lock changes.
set -euo pipefail

# Determine the remote tracking ref to diff against.
UPSTREAM=$(git rev-parse --abbrev-ref '@{upstream}' 2>/dev/null || true)
if [ -z "$UPSTREAM" ]; then
  UPSTREAM="origin/$(git rev-parse --abbrev-ref HEAD)"
fi
MERGE_BASE=$(git merge-base "$UPSTREAM" HEAD 2>/dev/null || echo "")
if [ -z "$MERGE_BASE" ]; then
  echo "No merge base with $UPSTREAM; running full workspace clippy."
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  exit $?
fi

CHANGED_FILES=$(git diff --name-only "$MERGE_BASE"..HEAD \
  | grep -E '\.(rs|toml)$' || true)

if [ -z "$CHANGED_FILES" ]; then
  echo "No Rust/TOML changes to push, skipping clippy."
  exit 0
fi

# Root workspace manifest changes → full workspace clippy
if echo "$CHANGED_FILES" | grep -qE '^Cargo\.(toml|lock)$'; then
  echo "Workspace manifest changed — running full workspace clippy."
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  exit $?
fi

# Extract crate directories from changed file paths
CHANGED_CRATES=$(echo "$CHANGED_FILES" \
  | sed -n 's|^\([^/]*\)/.*|\1|p' \
  | sort -u \
  | while read -r dir; do
      if [ -f "$dir/Cargo.toml" ]; then
        echo "$dir"
      fi
    done)

if [ -z "$CHANGED_CRATES" ]; then
  echo "No testable crate changes detected, skipping clippy."
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
  echo "No testable crates changed, skipping clippy."
  exit 0
fi

echo "Clippy on changed crates:$PKG_FLAGS"
# shellcheck disable=SC2086
cargo clippy $PKG_FLAGS --all-targets --all-features -- -D warnings
