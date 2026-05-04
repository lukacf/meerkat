#!/usr/bin/env bash
# 004 — CLI One-Liners (Shell)
#
# Meerkat ships a CLI binary `rkat` that covers every surface without writing
# code. This script demonstrates the most common CLI patterns.
#
# What you'll learn:
# - Running single-turn prompts
# - Managing sessions (create, resume, list, read, archive)
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

echo "=== 1. Single-turn prompt ==="
$RKAT run "List three benefits of Rust. Be concise."

echo ""
echo "=== 2. Create a session and continue it ==="
SESSION_OUTPUT=$($RKAT run "Remember: my favorite color is blue." 2>&1)
echo "$SESSION_OUTPUT"
# Extract session ID from output (rkat prints it to stderr)
SESSION_ID=$(echo "$SESSION_OUTPUT" | sed -n 's/.*Session: \([a-f0-9-]*\).*/\1/p' | head -1)

if [ -n "$SESSION_ID" ]; then
    echo ""
    echo "=== 3. Resume the session ==="
    $RKAT resume "$SESSION_ID" "What is my favorite color?"
fi

echo ""
echo "=== 4. List sessions ==="
$RKAT sessions list

echo ""
echo "=== 5. Isolated realm (ephemeral workspace) ==="
$RKAT --isolated run "This session lives in its own isolated realm."

echo ""
echo "=== 6. Configuration from CLI ==="
$RKAT config get

echo ""
echo "=== 7. Verbose mode (shows tool calls, events) ==="
$RKAT run --verbose "What is 2 + 2?"

echo ""
echo "=== 8. Streaming mode (token-by-token output) ==="
$RKAT run --stream "Write a haiku about systems programming."

echo ""
echo "Done! See each example above for the output."
