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
#   cargo build -p meerkat-cli  # produces ./target/debug/rkat

set -euo pipefail

RKAT="${RKAT:-rkat}"

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
    $RKAT session "$SESSION_ID" "What is my favorite color?"
fi

echo ""
echo "=== 4. List sessions ==="
$RKAT list

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
