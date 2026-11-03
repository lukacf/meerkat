#!/usr/bin/env bash
# Access Control Pack runner.
#
#   ./examples.sh           build console, start the example server, run the
#                           deterministic HTTP smoke (no API key needed)
#   ./examples.sh --serve   build console, start the server + persona proxies,
#                           and leave them running for browser exploration
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PACK_DIR="$ROOT/examples/005-access-control-pack"
MODE="${1:-smoke}"

# Meerkat 0.7's machine-authority code allocates huge debug-build stack
# frames. The example binary sizes its own threads, and the workspace
# .cargo/config.toml covers cargo-invoked flows; this export is belt and
# braces for prebuilt-binary runs documented from this script.
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"

# Print the bound address the server announces in its log
# ("META_FIXTURE_READY {...}") after binding port 0. Reserving a free port
# first and binding it later raced every other process on the host for it.
bound_addr() {
  python3 - "$1" "$2" <<'PY'
import json, os, sys, time
log_path, server_pid = sys.argv[1], int(sys.argv[2])
prefix = "META_FIXTURE_READY "
deadline = time.time() + 600
while time.time() < deadline:
    try:
        with open(log_path, encoding="utf-8", errors="replace") as log:
            for line in log:
                if line.startswith(prefix):
                    print(json.loads(line[len(prefix):])["addr"])
                    sys.exit(0)
    except FileNotFoundError:
        pass
    try:
        os.kill(server_pid, 0)
    except ProcessLookupError:
        print("access control server exited before announcing its address", file=sys.stderr)
        sys.exit(2)
    time.sleep(0.25)
print("timed out waiting for the access control server's address", file=sys.stderr)
sys.exit(1)
PY
}

wait_for_server() {
  python3 - "$1" <<'PY'
import sys, time, urllib.request
base_url = sys.argv[1]
deadline = time.time() + 120
while time.time() < deadline:
    try:
        with urllib.request.urlopen(base_url + "/console/experience", timeout=2) as resp:
            if resp.status == 200:
                sys.exit(0)
    except Exception:
        pass
    time.sleep(0.5)
print("timed out waiting for access control server", file=sys.stderr)
sys.exit(1)
PY
}

cleanup() {
  # Jobs are started in their own process groups (set -m below); kill the
  # whole group so cargo/server/node descendants die too.
  for pid in "${SERVER_PID:-}" "${PROXY_PID:-}"; do
    [[ -n "$pid" ]] && {
      kill -TERM -- -"$pid" >/dev/null 2>&1 || kill "$pid" >/dev/null 2>&1 || true
      wait "$pid" >/dev/null 2>&1 || true
    }
  done
  SERVER_PID=""
  PROXY_PID=""
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

echo "[access-control-pack] building console assets"
(cd "$ROOT/console" && npm run build --silent)

# Fresh work dir so the seeded access.toml is deterministic each run.
WORK_DIR="$(mktemp -d -t meta-access-control-pack.XXXXXX)"
export ACCESS_CONTROL_WORK_DIR="$WORK_DIR"

if [[ "$MODE" == "--serve" ]]; then
  LISTEN_ADDR="127.0.0.1:7300"
  echo "[access-control-pack] starting example server at http://${LISTEN_ADDR}"
  # set -m puts background jobs in their own process groups so cleanup can
  # kill the entire cargo/server tree via kill -- -PID.
  set -m
  (cd "$ROOT" && ACCESS_CONTROL_LISTEN_ADDR="$LISTEN_ADDR" \
    cargo run -p meerkat-meta --example access_control_console) &
  SERVER_PID=$!
  set +m
  wait_for_server "http://${LISTEN_ADDR}"
  echo "[access-control-pack] starting persona proxies"
  set -m
  ACCESS_CONTROL_TARGET="$LISTEN_ADDR" node "$PACK_DIR/persona-proxy.mjs" &
  PROXY_PID=$!
  set +m
  echo "[access-control-pack] ready — open the persona tabs printed above. Ctrl-C to stop."
  wait "$SERVER_PID"
  exit 0
fi

echo "[access-control-pack] starting example server"
set -m
(cd "$ROOT" && ACCESS_CONTROL_LISTEN_ADDR="127.0.0.1:0" \
  cargo run -p meerkat-meta --example access_control_console > "$WORK_DIR/server.log" 2>&1) &
SERVER_PID=$!
set +m
LISTEN_ADDR="$(bound_addr "$WORK_DIR/server.log" "$SERVER_PID")"
echo "[access-control-pack] example server listening at http://${LISTEN_ADDR}"
wait_for_server "http://${LISTEN_ADDR}"
echo "[access-control-pack] running HTTP smoke"
node "$PACK_DIR/smoke.mjs" "http://${LISTEN_ADDR}"
