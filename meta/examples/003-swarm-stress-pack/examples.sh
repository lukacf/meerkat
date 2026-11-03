#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PACK_DIR="$ROOT/examples/003-swarm-stress-pack"

# Meerkat 0.7's machine-authority code allocates huge debug-build stack
# frames; belt-and-braces for any prebuilt debug binary launched from here.
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"

echo "[swarm-stress-pack] ensuring example JS deps"
(cd "$ROOT/examples" && npm install --silent --no-fund --no-audit)

echo "[swarm-stress-pack] building TypeScript SDK"
(cd "$ROOT/sdk/typescript" && npm install --silent --no-fund --no-audit && npm run build --silent)

echo "[swarm-stress-pack] running structural smoke"
(cd "$ROOT/examples" && npx tsx "$PACK_DIR/ts_smoke.ts")

if [[ "${1:-}" == "--smoke" ]]; then
  exit 0
fi

if [[ " $* " == *" --browser-smoke "* ]]; then
  echo "[swarm-stress-pack] building console assets"
  (cd "$ROOT/console" && npm run build --silent)

  LOG_FILE="$(mktemp -t swarm-stress.XXXXXX.log)"
  cleanup() {
    if [[ -n "${SERVER_PID:-}" ]]; then
      # The runtime is started in its own process group (set -m below); kill
      # the whole group so npx/tsx/node/rpc_gateway descendants die too.
      kill -TERM -- -"$SERVER_PID" >/dev/null 2>&1 || kill "$SERVER_PID" >/dev/null 2>&1 || true
      wait "$SERVER_PID" >/dev/null 2>&1 || true
      SERVER_PID=""
    fi
  }
  trap cleanup EXIT
  trap 'cleanup; exit 130' INT
  trap 'cleanup; exit 143' TERM

  echo "[swarm-stress-pack] starting real Gemini-backed live console for browser smoke"
  # set -m puts the background job in its own process group so cleanup can
  # kill the entire npx/tsx/gateway tree via kill -- -PID.
  set -m
  (cd "$ROOT/examples" && npx tsx "$PACK_DIR/run.ts" >"$LOG_FILE" 2>&1) &
  SERVER_PID=$!
  set +m

  BASE_URL=""
  for _ in $(seq 1 240); do
    if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
      echo "[swarm-stress-pack] runtime exited before printing a console URL" >&2
      cat "$LOG_FILE" >&2
      exit 1
    fi
    LINE="$(grep -m1 "\\[swarm-stress\\] console:" "$LOG_FILE" || true)"
    if [[ -n "$LINE" ]]; then
      BASE_URL="${LINE##*console: }"
      BASE_URL="${BASE_URL%/console}"
      break
    fi
    sleep 0.5
  done

  if [[ -z "$BASE_URL" ]]; then
    echo "[swarm-stress-pack] timed out waiting for console URL" >&2
    cat "$LOG_FILE" >&2
    exit 1
  fi

  ARTIFACT_DIR="$ROOT/output/playwright/example-003"
  echo "[swarm-stress-pack] running browser smoke at $BASE_URL"
  (cd "$ROOT/examples" && META_BROWSER_SMOKE_ARTIFACT_DIR="$ARTIFACT_DIR" node "$PACK_DIR/browser_smoke.cjs" "$BASE_URL")
  exit $?
fi

echo "[swarm-stress-pack] building console assets"
(cd "$ROOT/console" && npm run build --silent)

echo "[swarm-stress-pack] starting real Gemini-backed Swarm Stress pack"
(cd "$ROOT/examples" && npx tsx "$PACK_DIR/run.ts" "$@")
