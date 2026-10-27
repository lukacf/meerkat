#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PACK_DIR="$ROOT/examples/001-incident-command-center-pack"
SCENARIO="$PACK_DIR/scenario.yaml"

: "${OPENAI_API_KEY:?Set OPENAI_API_KEY to run the live incident command center pack}"
export RKAT_INCIDENT_MODEL="${RKAT_INCIDENT_MODEL:-gpt-5.5}"
# Meerkat 0.7's machine-authority code allocates huge debug-build stack
# frames. The example binary sizes its own threads, and the workspace
# .cargo/config.toml covers cargo-invoked flows; this export is belt and
# braces for prebuilt-binary runs documented from this script.
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
SERVER_LOG=/tmp/incident-command-center.log

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]]; then
    # The server is started in its own process group (set -m below); kill the
    # whole group so cargo/server descendants die too, not just the subshell.
    kill -TERM -- -"${SERVER_PID}" >/dev/null 2>&1 || kill "${SERVER_PID}" >/dev/null 2>&1 || true
    wait "${SERVER_PID}" >/dev/null 2>&1 || true
    SERVER_PID=""
  fi
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

# The server binds port 0 and announces its bound address in its log
# ("META_FIXTURE_READY {...}"). Print that address once the console answers
# there. Reserving a free port first and binding it later raced every other
# process on the host for that port.
wait_for_server() {
  python3 - <<'PY' "$SERVER_LOG" "$SERVER_PID"
import json, os, sys, time, urllib.request
log_path, server_pid = sys.argv[1], int(sys.argv[2])
prefix = "META_FIXTURE_READY "
addr = None
deadline = time.time() + 120
while time.time() < deadline:
    try:
        os.kill(server_pid, 0)
    except ProcessLookupError:
        print("incident console server exited before becoming ready", file=sys.stderr)
        sys.exit(2)
    if addr is None:
        with open(log_path, encoding="utf-8", errors="replace") as log:
            for line in log:
                if line.startswith(prefix):
                    addr = json.loads(line[len(prefix):])["addr"]
                    break
    if addr is not None:
        try:
            with urllib.request.urlopen(f"http://{addr}/console/experience", timeout=2) as resp:
                if resp.status == 200:
                    print(addr)
                    sys.exit(0)
        except Exception:
            pass
    time.sleep(0.5)
print("timed out waiting for incident console server", file=sys.stderr)
sys.exit(1)
PY
}

# Start the server and set SERVER_ADDR to its bound address.
start_server() {
  echo "[incident-pack] starting incident example server"
  # set -m puts the background job in its own process group so cleanup can
  # kill the entire cargo/server tree via kill -- -PID.
  set -m
  (cd "$ROOT" && INCIDENT_COMMAND_CENTER_LISTEN_ADDR="127.0.0.1:0" cargo run -p meerkat-meta --example incident_command_center > "$SERVER_LOG" 2>&1) &
  SERVER_PID=$!
  set +m
  SERVER_ADDR="$(wait_for_server)"
  echo "[incident-pack] incident example server listening at http://${SERVER_ADDR}"
}

run_leg() {
  local label="$1"
  shift
  start_server
  echo "[incident-pack] running ${label}"
  if ! "$@" "http://${SERVER_ADDR}"; then
    cleanup
    return 1
  fi
  cleanup
}

echo "[incident-pack] building console assets"
(cd "$ROOT/console" && npm run build --silent)

echo "[incident-pack] ensuring example JS deps"
(cd "$ROOT/examples" && npm install --silent --no-fund --no-audit)

echo "[incident-pack] using live model ${RKAT_INCIDENT_MODEL}"

status=0

run_leg "browser smoke" node "$PACK_DIR/browser_smoke.cjs" || status=1
run_leg "TypeScript smoke" bash -lc "cd \"$ROOT/examples\" && npx tsx \"$PACK_DIR/ts_smoke.ts\" \"\$1\"" _ || status=1
run_leg "Python smoke" python3 "$PACK_DIR/python_smoke.py" || status=1

exit "$status"
