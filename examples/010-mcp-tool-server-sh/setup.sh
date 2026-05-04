#!/usr/bin/env bash
# 010 — MCP Tool Server Integration
#
# End-to-end demo:
# - starts from an isolated temp CLI/project config root
# - registers a real stdio MCP server
# - shows the generated project-scoped mcp.toml
# - verifies registration with rkat mcp list/get
# - runs a live agent prompt that must use the MCP tools
#
# Prerequisites:
#   export ANTHROPIC_API_KEY=sk-...
#   ./scripts/repo-cargo build -p rkat --bin rkat

set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$ROOT/../.." && pwd)"
WORK="$ROOT/.work"
STATE_ROOT="$WORK/state"
CONTEXT_ROOT="$WORK/project"
USER_CONFIG_ROOT="$WORK/user"
SERVER_NAME="incident-kit"
SERVER_SCRIPT="$ROOT/demo_mcp_server.py"

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

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  echo "Set ANTHROPIC_API_KEY to run the live agent step."
  exit 1
fi

mkdir -p "$STATE_ROOT" "$CONTEXT_ROOT" "$USER_CONFIG_ROOT"

BASE_ARGS=(
  --state-root "$STATE_ROOT"
  --context-root "$CONTEXT_ROOT"
  --user-config-root "$USER_CONFIG_ROOT"
)

run_in_project() {
  (
    cd "$CONTEXT_ROOT"
    "$RKAT" "${BASE_ARGS[@]}" "$@"
  )
}

echo "=== 010 — MCP Tool Server Integration ==="
echo
echo "Workspace roots:"
echo "  state:   $STATE_ROOT"
echo "  context: $CONTEXT_ROOT"
echo "  user:    $USER_CONFIG_ROOT"
echo

echo "--- 1. Register a real stdio MCP server ---"
run_in_project mcp add "$SERVER_NAME" --scope project -- \
  python3 "$SERVER_SCRIPT"
echo

echo "--- 2. Show registered MCP servers ---"
run_in_project mcp list --scope project
echo

echo "--- 3. Show the generated project MCP config ---"
PROJECT_MCP_FILE="$CONTEXT_ROOT/.rkat/mcp.toml"
cat "$PROJECT_MCP_FILE"
echo

echo "--- 4. Inspect the configured server ---"
run_in_project mcp get "$SERVER_NAME" --scope project
echo

echo "--- 5. Run a live prompt that should use MCP tools ---"
echo "Prompt asks the agent to call incident tools and quote exact fields."
echo
run_in_project run \
  --wait-for-mcp \
  --verbose \
  "You are the on-call incident coordinator.
First call the incident_digest tool for service 'checkout-api'.
Then call the release_readiness tool for service 'checkout-api'.
Return:
1. severity
2. service owner
3. immediate rollback command
4. whether the release should continue
5. one precise next action
Do not answer from prior knowledge; use the MCP tool outputs."
echo

echo "--- 6. Cleanup command ---"
echo "(cd \"$CONTEXT_ROOT\" && \"$RKAT\" ${BASE_ARGS[*]} mcp remove $SERVER_NAME --scope project)"
echo
echo "Done. The example kept all state under:"
echo "  $WORK"
