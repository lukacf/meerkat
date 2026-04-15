#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
TARGET="${TARGET:-x86_64-unknown-linux-gnu}"
EXPECTED_SKILLS="${RKAT_MINI_EXPECT_SKILLS:-1}"
TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/meerkat-mini-skill-size-target}"

if ! rustup target list --installed | grep -qx "$TARGET"; then
  echo "Target $TARGET is not installed" >&2
  exit 1
fi

STRIP_BIN="${STRIP_BIN:-}"
if [[ -z "$STRIP_BIN" ]]; then
  if command -v llvm-strip >/dev/null 2>&1; then
    STRIP_BIN="$(command -v llvm-strip)"
  elif command -v strip >/dev/null 2>&1; then
    STRIP_BIN="$(command -v strip)"
  else
    echo "No strip tool found (llvm-strip or strip required)" >&2
    exit 1
  fi
fi

base_features="anthropic,openai,gemini,jsonl-store,session-store"
with_skill_features="${base_features},skills"

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

build_and_measure() {
  local features="$1"
  local dest="$2"
  CARGO_TARGET_DIR="$TARGET_DIR" "$CARGO" build \
    -p rkat \
    --bin rkat-mini \
    --release \
    --target "$TARGET" \
    --no-default-features \
    --features "$features" >/dev/null

  local src="$TARGET_DIR/$TARGET/release/rkat-mini"
  cp "$src" "$dest"
  "$STRIP_BIN" -o "${dest}.stripped" "$dest"
  if stat -c %s "${dest}.stripped" >/dev/null 2>&1; then
    stat -c %s "${dest}.stripped"
  else
    stat -f %z "${dest}.stripped"
  fi
}

without_skill_size="$(build_and_measure "$base_features" "$work_dir/rkat-mini-without-skill")"
with_skill_size="$(build_and_measure "$with_skill_features" "$work_dir/rkat-mini-with-skill")"

if [[ "$without_skill_size" -eq 0 ]]; then
  echo "Measured zero-size stripped binary for baseline build" >&2
  exit 1
fi

delta=$((with_skill_size - without_skill_size))
percent=$((delta * 100 / without_skill_size))

echo "rkat-mini stripped size without skill: ${without_skill_size} bytes"
echo "rkat-mini stripped size with skill:    ${with_skill_size} bytes"
echo "skill overhead: ${delta} bytes (${percent}%)"

if [[ "$EXPECTED_SKILLS" == "1" ]]; then
  if (( percent > 20 )); then
    echo "Configured shipped profile includes skill, but measured overhead exceeds 20%" >&2
    exit 1
  fi
else
  if (( percent <= 20 )); then
    echo "Configured shipped profile omits skill, but measured overhead is within 20%" >&2
    exit 1
  fi
fi
