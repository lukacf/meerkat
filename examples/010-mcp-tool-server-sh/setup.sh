#!/usr/bin/env bash
# 010 — MCP Tool Server Integration
#
# End-to-end demo:
# - starts from example-local CLI/project config roots
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
    case "$RKAT" in
      /*) printf '%s\n' "$RKAT" ;;
      */*) printf '%s/%s\n' "$PWD" "$RKAT" ;;
      *) printf '%s\n' "$RKAT" ;;
    esac
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

mkdir -p "$STATE_ROOT" "$CONTEXT_ROOT/.rkat" "$USER_CONFIG_ROOT"

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

print_cleanup() {
  printf '(cd %q && ' "$CONTEXT_ROOT"
  printf '%q ' "$RKAT" "${BASE_ARGS[@]}" mcp remove "$SERVER_NAME" --scope project
  printf ')\n'
}

REGISTERED=false
cleanup() {
  local status=$?
  if [[ "$REGISTERED" == true ]]; then
    echo "--- Cleanup: remove this run's example registration ---"
    print_cleanup
    if ! run_in_project mcp remove "$SERVER_NAME" --scope project; then
      echo "Cleanup failed; run the command above before retrying." >&2
      if [[ "$status" == 0 ]]; then status=1; fi
    fi
  fi
  exit "$status"
}
trap cleanup EXIT

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
REGISTERED=true
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
  --model claude-sonnet-4-6 \
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

echo "Done. The EXIT trap removes this run's MCP registration, even on failure."
echo "Session state remains under:"
echo "  $WORK"
