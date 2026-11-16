#!/usr/bin/env bash
# Runs the program and checks what it prints: once without a provider key
# (the start, the restart, the crash and the two mistakes), and once more
# with OPENAI_API_KEY when it is set (the same, plus the work, the respawn
# and what the agent remembers).
#
#   META_GATEWAY_BIN=<rpc_gateway> ./examples.sh
#
# Without META_GATEWAY_BIN the gateway is built with the repository wrapper,
# which takes a while the first time.
set -euo pipefail

PACK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
META_ROOT="$(cd "$PACK_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$META_ROOT/.." && pwd)"

if [[ -z "${META_GATEWAY_BIN:-}" ]]; then
  echo "[supervised-app] building the gateway"
  "$REPO_ROOT/scripts/repo-cargo" build --locked -p meerkat-meta --bin rpc_gateway
  META_GATEWAY_BIN="$("$REPO_ROOT/scripts/repo-cargo" --print-env | sed -n "s/^CARGO_TARGET_DIR=//p")/debug/rpc_gateway"
fi
export META_GATEWAY_BIN
[[ -x "$META_GATEWAY_BIN" ]] || { echo "no gateway at $META_GATEWAY_BIN" >&2; exit 1; }

WORK="$(mktemp -d)"
trap "rm -rf \"$WORK\"" EXIT

expect() {
  # expect <label> <log> <pattern>: the log has a line matching the pattern
  grep -q "$3" "$2" && return 0
  echo "[supervised-app] $1: no line matches: $3" >&2
  echo "[supervised-app] $1: the program printed:" >&2
  sed "s/^/    /" "$2" >&2
  exit 1
}

run() {
  # run <label> <state dir name>: one run of the program on a fresh state directory
  local label="$1" state="$WORK/$2/meta"
  EXAMPLE_STATE_DIR="$state" "$WORK/venv/bin/python" "$PACK_DIR/main.py" > "$WORK/$2.log" 2> "$WORK/$2.err" \
    || { echo "[supervised-app] $label: the program failed:" >&2; tail -n 20 "$WORK/$2.err" >&2; exit 1; }
}

check_lifecycle() {
  # The lines every run prints, with or without a key.
  local label="$1" log="$2"
  expect "$label" "$log" "^\[1\] desk:intake: active, generation 0$"
  expect "$label" "$log" "^\[4\] same agents after the restart: yes$"
  expect "$label" "$log" "^\[5\] the gateway is gone and the runtime says so: True$"
  expect "$label" "$log" "^\[5\] same agents after the crash: yes$"
  expect "$label" "$log" "^\[6\] a second runtime on the same state was refused, state in use: yes$"
  expect "$label" "$log" "^\[6\] a start that ran out of time: outcome unknown, last phase seen: True$"
  expect "$label" "$log" "^\[end\] same agents at the end: yes$"
}

python3 -m venv "$WORK/venv"
"$WORK/venv/bin/python" -m pip install --quiet "$META_ROOT/sdk/python"

echo "[supervised-app] without a provider key"
key="${OPENAI_API_KEY:-}"
unset OPENAI_API_KEY
run "without a key" nokey
check_lifecycle "without a key" "$WORK/nokey.log"
expect "without a key" "$WORK/nokey.log" "^\[2\] skipped: "
expect "without a key" "$WORK/nokey.log" "^\[3\] skipped: "
expect "without a key" "$WORK/nokey.log" "^\[end\] desk:intake: active, generation 0$"
echo "[supervised-app] without a provider key: ok"

if [[ -z "$key" ]]; then
  echo "[supervised-app] OPENAI_API_KEY is not set: the run with a provider is skipped"
  exit 0
fi

echo "[supervised-app] with a provider key"
export OPENAI_API_KEY="$key"
run "with a key" live
check_lifecycle "with a key" "$WORK/live.log"
expect "with a key" "$WORK/live.log" "^\[2\] send_and_wait answered: noted$"
expect "with a key" "$WORK/live.log" "^\[2\] the tracked turn ended completed, with text: yes$"
expect "with a key" "$WORK/live.log" "^\[3\] respawned desk:intake: generation 0 to 1, same session: yes$"
expect "with a key" "$WORK/live.log" "^\[3\] after the respawn, intake still knows the word: yes$"
expect "with a key" "$WORK/live.log" "^\[4\] after the restart, intake still knows the word: yes$"
expect "with a key" "$WORK/live.log" "^\[5\] after the crash, intake still knows the word: yes$"
expect "with a key" "$WORK/live.log" "^\[end\] desk:intake: active, generation 1$"
echo "[supervised-app] with a provider key: ok"
