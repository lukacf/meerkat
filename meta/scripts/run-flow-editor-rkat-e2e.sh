#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT="${META_FLOW_EDITOR_E2E_PORT:-4192}"
ADDR="127.0.0.1:${PORT}"
RPC_URL="http://${ADDR}/flow-editor/rpc"
CARGO_CMD="${CARGO:-"$ROOT/scripts/repo-cargo"}"
NPM_SCRIPT="test:rkat-e2e"
SERVER_ARGS=(--listen "$ADDR")
NPM_ENV=(META_FLOW_EDITOR_RPC_URL="$RPC_URL")

if [[ "${1:-}" == "--deploy" ]]; then
  NPM_SCRIPT="test:rkat-deploy-e2e"
  SERVER_ARGS+=(--allow-host-deploy)
  NPM_ENV+=(META_FLOW_EDITOR_EXPECT_HOST_DEPLOY=1)
fi

if ! command -v rkat >/dev/null 2>&1; then
  echo "rkat is required for Flow Editor live mobpack validation" >&2
  exit 127
fi

CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}" "$CARGO_CMD" build -p meerkat-meta --bin meta_flow_editor

# repo-cargo isolates CARGO_TARGET_DIR outside the repo tree; resolve the
# binary from the same environment the build used.
TARGET_DIR="${CARGO_TARGET_DIR:-}"
if [[ -z "$TARGET_DIR" ]]; then
  TARGET_DIR="$("$CARGO_CMD" --print-env 2>/dev/null | sed -n 's/^CARGO_TARGET_DIR=//p' || true)"
fi
TARGET_DIR="${TARGET_DIR:-$ROOT/target}"
FLOW_EDITOR_BIN="${META_FLOW_EDITOR_BIN:-$TARGET_DIR/debug/meta_flow_editor}"

LOG_FILE="$(mktemp -t meta-flow-editor-rkat-e2e.XXXXXX.log)"
"$FLOW_EDITOR_BIN" "${SERVER_ARGS[@]}" >"$LOG_FILE" 2>&1 &
SERVER_PID=$!

cleanup() {
  kill "$SERVER_PID" >/dev/null 2>&1 || true
  wait "$SERVER_PID" >/dev/null 2>&1 || true
  rm -f "$LOG_FILE"
}
trap cleanup EXIT

for _ in $(seq 1 100); do
  if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    echo "meta_flow_editor exited before becoming ready" >&2
    cat "$LOG_FILE" >&2 || true
    exit 1
  fi
  if curl -fsS -X POST "$RPC_URL" \
    -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":"ready","method":"meta/capabilities","params":{}}' \
    >/dev/null 2>&1; then
    env "${NPM_ENV[@]}" npm --prefix "$ROOT/flow-editor" run "$NPM_SCRIPT" --silent
    exit 0
  fi
  sleep 0.2
done

echo "timed out waiting for meta_flow_editor at $RPC_URL" >&2
cat "$LOG_FILE" >&2 || true
exit 1
