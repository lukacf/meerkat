#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PACK_DIR="$ROOT/examples/001-incident-command-center-pack"
ARTIFACT_DIR="${INCIDENT_TOPOLOGY_ARTIFACT_DIR:-$ROOT/output/playwright}"
RUST_LANE_ID="${RUST_LANE_ID:-incident-console}"
CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/incident-topology-smoke.XXXXXX")"
# The server binds port 0 and announces its bound address on stdout
# ("META_FIXTURE_READY {...}"); start_server reads it back. Reserving a free
# port here and binding it later raced every other process for that port.
LISTEN_ADDR=""

cleanup() {
  stop_server
  rm -rf "$STATE_DIR"
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

# Print the server's bound address once it announces it in the log (read
# from byte offset $2, where this start's output begins), then wait for the
# console to answer there.
wait_for_server() {
  python3 - <<'PY' "$1" "$2" "$SERVER_PID" "${INCIDENT_TOPOLOGY_READY_TIMEOUT_SECONDS:-600}"
import json, os, sys, time, urllib.request
log_path = sys.argv[1]
offset = int(sys.argv[2])
server_pid = int(sys.argv[3])
ready_timeout_seconds = float(sys.argv[4])
deadline = time.time() + ready_timeout_seconds
prefix = "META_FIXTURE_READY "
addr = None
while time.time() < deadline:
    try:
        os.kill(server_pid, 0)
    except ProcessLookupError:
        print("offline incident topology server exited before becoming ready", file=sys.stderr)
        sys.exit(2)
    if addr is None:
        with open(log_path, "rb") as log:
            log.seek(offset)
            for line in log.read().decode("utf-8", "replace").splitlines():
                if line.startswith(prefix):
                    addr = json.loads(line[len(prefix):])["addr"]
                    break
    if addr is not None:
        try:
            with urllib.request.urlopen(f"http://{addr}/console/experience", timeout=2) as response:
                if response.status == 200:
                    print(addr)
                    sys.exit(0)
        except Exception:
            pass
    time.sleep(0.25)
print(
    f"timed out after {ready_timeout_seconds:g}s waiting for offline incident topology server",
    file=sys.stderr,
)
sys.exit(1)
PY
}

stop_server() {
  if [[ -z "${SERVER_PID:-}" ]]; then
    return
  fi
  kill -TERM -- -"${SERVER_PID}" >/dev/null 2>&1 || kill "${SERVER_PID}" >/dev/null 2>&1 || true
  wait "${SERVER_PID}" >/dev/null 2>&1 || true
  SERVER_PID=""
}

start_server() {
  local phase="$1"
  local log_offset
  log_offset=0
  if [[ -f "$STATE_DIR/server.log" ]]; then
    log_offset="$(wc -c <"$STATE_DIR/server.log" | tr -d ' ')"
  fi
  echo "[incident-topology] starting deterministic server (${phase})"
  set -m
  (
    cd "$ROOT"
    INCIDENT_COMMAND_CENTER_OFFLINE=1 \
    INCIDENT_COMMAND_CENTER_LISTEN_ADDR="127.0.0.1:0" \
    INCIDENT_COMMAND_CENTER_STATE_DIR="$STATE_DIR/runtime" \
    INCIDENT_COMMAND_CENTER_MEMORY_DIR="$STATE_DIR/memory" \
    RUST_LANE_ID="$RUST_LANE_ID" \
    CARGO_INCREMENTAL="$CARGO_INCREMENTAL" \
    "${INCIDENT_COMMAND_CENTER_CMD[@]}" \
      >>"$STATE_DIR/server.log" 2>&1
  ) &
  SERVER_PID=$!
  set +m
  if ! LISTEN_ADDR="$(wait_for_server "$STATE_DIR/server.log" "$log_offset")"; then
    sed -n '1,240p' "$STATE_DIR/server.log" >&2 || true
    return 1
  fi
  echo "[incident-topology] server (${phase}) listening at http://${LISTEN_ADDR}"
}

echo "[incident-topology] building the stock embedded console"
(cd "$ROOT/console" && npm run build --silent)

echo "[incident-topology] ensuring Playwright dependencies"
(cd "$ROOT/examples" && npm ci --silent --no-fund --no-audit)

# META_EXAMPLE_BIN_DIR lets CI build the meerkat-meta dependency graph once
# in a dedicated job and hand the binaries here, instead of every consumer
# compiling the same graph. Unset - the local default - is unchanged.
#
# Not a cargo warm-up: `cargo run --example` recomputes freshness from
# target/.fingerprint, so a bare prebuilt binary would be rebuilt and ignored.
# The script has to exec the binary directly.
if [[ -n "${META_EXAMPLE_BIN_DIR:-}" ]]; then
  INCIDENT_COMMAND_CENTER_CMD=("${META_EXAMPLE_BIN_DIR}/incident_command_center")
  # Fail rather than silently rebuild. A fallback would still pass, just slowly,
  # so a mis-wired artifact download would look like a successful compile-once
  # run and the measurement would be quietly wrong.
  for required in incident_command_center mcp_fixture; do
    if [[ ! -x "${META_EXAMPLE_BIN_DIR}/${required}" ]]; then
      echo "[incident-topology] META_EXAMPLE_BIN_DIR=${META_EXAMPLE_BIN_DIR} is set but ${required} is missing or not executable" >&2
      exit 1
    fi
  done
  export PATH="${META_EXAMPLE_BIN_DIR}:${PATH}"
  echo "[incident-topology] using prebuilt binaries from ${META_EXAMPLE_BIN_DIR}"
else
  INCIDENT_COMMAND_CENTER_CMD=(./scripts/repo-cargo run -p meerkat-meta --example incident_command_center)
  echo "[incident-topology] building the deterministic Rust example"
  (cd "$ROOT" && RUST_LANE_ID="$RUST_LANE_ID" CARGO_INCREMENTAL="$CARGO_INCREMENTAL" \
    ./scripts/repo-cargo build -p meerkat-meta \
      --example incident_command_center --bin mcp_fixture)
fi

mkdir -p "$ARTIFACT_DIR"
echo "[incident-topology] browser artifacts: $ARTIFACT_DIR"

start_server "prepare"
INCIDENT_TOPOLOGY_ARTIFACT_DIR="$ARTIFACT_DIR" \
  node "$PACK_DIR/topology_browser_smoke.cjs" "http://${LISTEN_ADDR}" --prepare

echo "[incident-topology] restarting against the same durable topology state"
stop_server
start_server "resume"
INCIDENT_TOPOLOGY_ARTIFACT_DIR="$ARTIFACT_DIR" \
  node "$PACK_DIR/topology_browser_smoke.cjs" "http://${LISTEN_ADDR}" --resume
