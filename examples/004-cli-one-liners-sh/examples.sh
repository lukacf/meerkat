#!/usr/bin/env bash
# 004 — CLI One-Liners (Shell)
#
# Meerkat ships a CLI binary `rkat` for runtime and configuration workflows.
# This script demonstrates a small set of common CLI patterns.
#
# What you'll learn:
# - Running single-turn prompts
# - Managing sessions (create, resume, list)
# - Using realms for isolation
# - Configuring the runtime from the command line
#
# Prerequisites:
#   export ANTHROPIC_API_KEY=sk-...
#   ./scripts/repo-cargo build -p rkat --bin rkat

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$ROOT/../.." && pwd)"

resolve_rkat() {
  if [[ -n "${RKAT:-}" ]]; then
    printf '%s\n' "$RKAT"
    return
  fi

  local candidate
  for candidate in \
    "$WORKSPACE_ROOT/target/debug/rkat" \
    "$WORKSPACE_ROOT/target/release/rkat"
  do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return
    fi
  done

  if [[ -x "$WORKSPACE_ROOT/scripts/repo-cargo" ]]; then
    local target_dir
    target_dir="$("$WORKSPACE_ROOT/scripts/repo-cargo" --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')"
    for candidate in "$target_dir/debug/rkat" "$target_dir/release/rkat"; do
      if [[ -x "$candidate" ]]; then
        printf '%s\n' "$candidate"
        return
      fi
    done
  fi

  printf '%s\n' "rkat"
}

RKAT="$(resolve_rkat)"
WORK="$ROOT/.work"
mkdir -p "$WORK/state" "$WORK/project/.rkat" "$WORK/user"
BASE_ARGS=(
  --state-root "$WORK/state"
  --context-root "$WORK/project"
  --user-config-root "$WORK/user"
)
MODEL="claude-sonnet-4-6"

echo "=== 1. Single-turn prompt ==="
"$RKAT" "${BASE_ARGS[@]}" run --model "$MODEL" "List three benefits of Rust. Be concise."

echo ""
echo "=== 2. Create a session and continue it ==="
"$RKAT" "${BASE_ARGS[@]}" run --model "$MODEL" "Remember: my favorite color is blue."

echo ""
echo "=== 3. Resume the latest session ==="
"$RKAT" "${BASE_ARGS[@]}" run --resume last "What is my favorite color?"

echo ""
echo "=== 4. List sessions ==="
"$RKAT" "${BASE_ARGS[@]}" session list

echo ""
echo "=== 5. Fresh isolated realm ==="
"$RKAT" "${BASE_ARGS[@]}" run --isolated --model "$MODEL" "This session lives in its own isolated realm."

echo ""
echo "=== 6. Configuration from CLI ==="
"$RKAT" "${BASE_ARGS[@]}" config get

echo ""
echo "=== 7. Verbose mode (shows tool calls, events) ==="
"$RKAT" "${BASE_ARGS[@]}" run --model "$MODEL" --verbose "What is 2 + 2?"

echo ""
echo "=== 8. Streaming mode (token-by-token output) ==="
"$RKAT" "${BASE_ARGS[@]}" run --model "$MODEL" --stream "Write a haiku about systems programming."

echo ""
echo "Done! See each example above for the output."
