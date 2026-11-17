#!/usr/bin/env bash
# Runs the Python and the TypeScript program, twice each on one state
# directory, and checks what they print: without a provider key, and once
# more with OPENAI_API_KEY when it is set (the delivery to the agents, and
# the agents announcing and subscribing themselves).
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
  echo "[mob-announcements] building the gateway"
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
  echo "[mob-announcements] $1: no line matches: $3" >&2
  echo "[mob-announcements] $1: the program printed:" >&2
  sed "s/^/    /" "$2" >&2
  exit 1
}

common() {
  # common <label> <log>: the lines every run prints, with or without a key
  expect "$1" "$2" "^\[1\] runtime started, mob announcements enabled: yes$"
  expect "$1" "$2" "^\[2\] put answered: active, revision 1$"
  expect "$1" "$2" "^\[3\] deploy-41 on deploys: captured for 1$"
  expect "$1" "$2" "^\[3\] lunch-1 on lunch: captured for 0$"
  expect "$1" "$2" "^\[3\] deploy-41 again, same content: replayed true, sequence 1$"
  expect "$1" "$2" "^\[3\] deploy-41 with other content: refused, announcement_conflict$"
  expect "$1" "$2" "^\[4\] ops is owed 1 batch: deploy-41$"
  expect "$1" "$2" "^\[6\] ops-deploys: cancelled, revision 2$"
  expect "$1" "$2" "^\[6\] deploy-42 on deploys: captured for 0$"
}

check_without_key() {
  # check_without_key <label> <first run log> <second run log>
  local label="$1" first="$2" second="$3"
  common "$label, first run" "$first"
  expect "$label, first run" "$first" "^\[2\] ops-deploys now: active, revision 1, topics deploys$"
  expect "$label, first run" "$first" "^\[4\] no provider key, so the agent cannot run: the batch is sealed$"
  expect "$label, first run" "$first" "^\[5\] skipped: "
  expect "$label, first run" "$first" "^\[end\] batches for ops: 1, cancelled$"
  # The second run is answered from the record of the first: the same
  # answers, the subscription as the first run left it, and still one batch.
  common "$label, second run" "$second"
  expect "$label, second run" "$second" "^\[2\] ops-deploys now: cancelled, revision 2, topics deploys$"
  expect "$label, second run" "$second" "^\[end\] batches for ops: 1, cancelled$"
  expect "$label, second run" "$second" "^\[end\] subscriptions: ops on deploys (cancelled)$"
  echo "[mob-announcements] $label, without a provider key: ok"
}

check_with_key() {
  # check_with_key <label> <first run log> <second run log>
  local label="$1" first="$2" second="$3" log
  for log in "$first" "$second"; do
    common "$label, with a key" "$log"
    expect "$label, with a key" "$log" "^\[4\] the batch is delivered, its turn handled$"
    expect "$label, with a key" "$log" "^\[4\] ops knows the version: 4\.2\.\?$"
    expect "$label, with a key" "$log" "^\[5\] support was delivered 2 batches, announced by: the host, the member ops$"
    expect "$label, with a key" "$log" "^\[5\] support knows the card processor is back: [Yy]es\.\?$"
    expect "$label, with a key" "$log" "^\[end\] batches for ops: 1, delivered$"
    expect "$label, with a key" "$log" "^\[end\] subscriptions: .*support on incidents (active)"
  done
  expect "$label, with a key, first run" "$first" "^\[5\] support subscribed itself: active, topics incidents$"
  expect "$label, with a key, first run" "$first" "^\[5\] incident-7 by the host: captured for 1$"
  # Nothing is delivered twice: the second run finds the subscription of
  # the first and the same two batches.
  expect "$label, with a key, second run" "$second" "^\[5\] support subscribed itself in an earlier run: active, topics incidents$"
  echo "[mob-announcements] $label, with a provider key: ok"
}

key="${OPENAI_API_KEY:-}"
unset OPENAI_API_KEY

run_python() {
  # run_python <state name> <run number>
  EXAMPLE_STATE_DIR="$WORK/$1" "$WORK/venv/bin/python" "$PACK_DIR/main.py" > "$WORK/$1-$2.log" 2> "$WORK/$1-$2.err" \
    || { echo "[mob-announcements] Python: run $2 on $1 failed:" >&2; tail -n 20 "$WORK/$1-$2.err" >&2; exit 1; }
}

run_typescript() {
  # run_typescript <state name> <run number>
  (cd "$META_ROOT/examples" && EXAMPLE_STATE_DIR="$WORK/$1" npx tsx "$PACK_DIR/main.ts" > "$WORK/$1-$2.log" 2> "$WORK/$1-$2.err") \
    || { echo "[mob-announcements] TypeScript: run $2 on $1 failed:" >&2; tail -n 20 "$WORK/$1-$2.err" >&2; exit 1; }
}

echo "[mob-announcements] Python"
python3 -m venv "$WORK/venv"
"$WORK/venv/bin/python" -m pip install --quiet "$META_ROOT/sdk/python"
for run in 1 2; do run_python py "$run"; done
check_without_key "Python" "$WORK/py-1.log" "$WORK/py-2.log"

echo "[mob-announcements] TypeScript"
(cd "$META_ROOT/sdk/typescript" && npm install --silent --no-fund --no-audit && npm run build --silent)
(cd "$META_ROOT/examples" && npm ci --silent --no-fund --no-audit)
for run in 1 2; do run_typescript ts "$run"; done
check_without_key "TypeScript" "$WORK/ts-1.log" "$WORK/ts-2.log"

# The two programs are the same six steps: their lines are equal.
if ! diff "$WORK/py-1.log" "$WORK/ts-1.log" > "$WORK/languages.diff"; then
  echo "[mob-announcements] Python (<) and TypeScript (>) print different lines:" >&2
  sed "s/^/    /" "$WORK/languages.diff" >&2
  exit 1
fi
echo "[mob-announcements] Python and TypeScript print the same lines"

if [[ -z "$key" ]]; then
  echo "[mob-announcements] OPENAI_API_KEY is not set: the runs with a provider are skipped"
  exit 0
fi

export OPENAI_API_KEY="$key"
for run in 1 2; do run_python py-live "$run"; done
check_with_key "Python" "$WORK/py-live-1.log" "$WORK/py-live-2.log"
for run in 1 2; do run_typescript ts-live "$run"; done
check_with_key "TypeScript" "$WORK/ts-live-1.log" "$WORK/ts-live-2.log"
