#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
example_dir="$(cd -- "${script_dir}/.." && pwd)"
compose_file="${COMPOSE_FILE:-docker-compose.tux.yml}"
session_name="${TUX_TMUX_SESSION:-mdm-tux-smoke}"

compose=(docker compose -f "${compose_file}")

cd "${example_dir}"

remove_stale_tux_runs() {
  docker ps -a --format '{{.Names}}' \
    | grep -E '^mdm-tux-smoke-tux-run-' \
    | xargs -r docker rm -f >/dev/null 2>&1 || true
}

usage() {
  cat <<'EOF'
Usage: scripts/docker-smoke.sh [smoke|tux|tmux|logs|down|clean]

Commands:
  smoke  Build image, start kennel + two targets, and wait for hive wiring.
  tux    Run the TUX terminal UI inside the compose network.
  tmux   Start the TUX terminal UI in a tmux session and attach to it.
  logs   Follow logs for kennel and targets.
  down   Stop containers, keep named volumes.
  clean  Stop containers and remove named volumes.

Environment:
  OPENAI_API_KEY / ANTHROPIC_API_KEY / GEMINI_API_KEY  Provider keys.
  MDM_MODEL=gpt-5.5                                  Target model.
  MDM_PROVIDER=openai                                Target provider.
  MDM_HIVE_MODEL=gpt-5.5                             Hive model.
  MDM_HIVE_PROVIDER=openai                           Hive provider.
  MDM_TARGET_A_MODEL / MDM_TARGET_A_PROVIDER         target-a override.
  MDM_TARGET_B_MODEL / MDM_TARGET_B_PROVIDER         target-b override.
  DOCKER_CARGO_PROFILE=debug                         Build profile.
  DOCKER_SKIP_BUILD=1                                Reuse existing local image.
  RUST_MIN_STACK=16777216                            Runtime worker stack.
  COMPOSE_FILE=docker-compose.tux.yml                Compose file.
  TUX_TMUX_SESSION=mdm-tux-smoke                     tmux session name.

The default smoke command uses a dummy OPENAI_API_KEY if no provider key is
present. That validates kennel discovery and wiring without live model calls.
EOF
}

wait_for_log() {
  local service="$1"
  local pattern="$2"
  local timeout_sec="${3:-90}"
  local start
  local logs
  start="$(date +%s)"

  while true; do
    logs="$("${compose[@]}" logs --no-color "${service}" 2>&1 || true)"
    if [[ "${logs}" =~ ${pattern} ]]; then
      return 0
    fi
    if (( "$(date +%s)" - start >= timeout_sec )); then
      echo "timed out waiting for ${service} log pattern: ${pattern}" >&2
      echo "--- ${service} logs ---" >&2
      printf '%s\n' "${logs}" >&2
      return 1
    fi
    sleep 2
  done
}

check_hive_rpc_session() {
  local host="${HIVE_RPC_HOST:-127.0.0.1}"
  local port="${HIVE_RPC_PORT:-55010}"

  python3 -c '
import json
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])

with socket.create_connection((host, port), timeout=5) as sock:
    rpc = sock.makefile("rw")
    rpc.write(json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/list",
        "params": {},
    }) + "\n")
    rpc.flush()
    while True:
        line = rpc.readline()
        if not line:
            raise SystemExit("hive RPC closed before session/list response")
        message = json.loads(line)
        if message.get("id") != 1:
            continue
        if "error" in message:
            raise SystemExit(f"hive RPC session/list failed: {message['error']}")
        sessions = message.get("result", {}).get("sessions", [])
        if not sessions:
            raise SystemExit("hive RPC returned no sessions")
        print("Hive RPC session: {}".format(sessions[0]["session_id"]))
        break
' "${host}" "${port}"
}

smoke() {
  remove_stale_tux_runs
  if [[ "${DOCKER_SKIP_BUILD:-0}" == "1" ]]; then
    echo "Skipping Docker image build because DOCKER_SKIP_BUILD=1"
  else
    "${compose[@]}" build kennel
  fi
  "${compose[@]}" up -d --force-recreate kennel target-a target-b
  wait_for_log kennel 'listen[[:space:]]*:'
  wait_for_log target-a 'added hive as trusted peer'
  wait_for_log target-b 'added hive as trusted peer'
  wait_for_log target-a 'peer wired: target-b'
  wait_for_log target-b 'peer wired: target-a'
  check_hive_rpc_session

  cat <<EOF
MDM TUX docker smoke is ready.

Kennel:
  docker compose -f ${compose_file} logs -f kennel

Targets:
  target-a RPC on localhost:54801
  target-b RPC on localhost:54802

Open TUX:
  scripts/docker-smoke.sh tux
  scripts/docker-smoke.sh tmux
EOF
}

run_tux() {
  if [[ ! -t 0 || ! -t 1 ]]; then
    echo "TUX needs an interactive TTY; use scripts/docker-smoke.sh tmux or run this from a terminal." >&2
    exit 1
  fi
  "${compose[@]}" run --rm --no-deps tux
}

run_tmux() {
  if ! command -v tmux >/dev/null 2>&1; then
    echo "tmux is not installed; use scripts/docker-smoke.sh tux instead" >&2
    exit 1
  fi

  if tmux has-session -t "${session_name}" 2>/dev/null; then
    tmux attach -t "${session_name}"
    return
  fi

  local cmd
  printf -v cmd '%q ' "${compose[@]}" run --rm --no-deps tux
  tmux new-session -d -s "${session_name}" -c "${example_dir}" "${cmd}"
  tmux attach -t "${session_name}"
}

case "${1:-smoke}" in
  smoke) smoke ;;
  tux) run_tux ;;
  tmux) run_tmux ;;
  logs) "${compose[@]}" logs -f kennel target-a target-b ;;
  down) remove_stale_tux_runs; "${compose[@]}" down ;;
  clean) remove_stale_tux_runs; "${compose[@]}" down -v ;;
  help|--help|-h) usage ;;
  *)
    usage >&2
    exit 2
    ;;
esac
