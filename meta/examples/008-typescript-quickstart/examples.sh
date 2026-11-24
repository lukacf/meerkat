#!/usr/bin/env bash
# Runs the program twice on one state directory and checks what it prints:
# without a provider key (the start and the agent), and once more with
# OPENAI_API_KEY when it is set (the question, the answer, and what the agent
# remembers in the second run).
#
#   META_GATEWAY_BIN=<rpc_gateway> ./examples.sh
#
# It uses the SDK of this repository. Without META_GATEWAY_BIN the gateway is
# built with the repository wrapper, which takes a while the first time.
set -euo pipefail

PACK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
META_ROOT="$(cd "$PACK_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$META_ROOT/.." && pwd)"

if [[ -z "${META_GATEWAY_BIN:-}" ]]; then
  echo "[typescript-quickstart] building the gateway"
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
  echo "[typescript-quickstart] $1: no line matches: $3" >&2
  echo "[typescript-quickstart] $1: the program printed:" >&2
  sed "s/^/    /" "$2" >&2
  exit 1
}

run() {
  # run <label> <state dir name> <run number>: one run of the program
  local label="$1" state="$WORK/$2" log="$WORK/$2-$3"
  (cd "$META_ROOT/examples" && EXAMPLE_STATE_DIR="$state" npx tsx "$PACK_DIR/main.ts" > "$log.log" 2> "$log.err") \
    || { echo "[typescript-quickstart] $label, run $3: the program failed:" >&2; tail -n 20 "$log.err" >&2; exit 1; }
}

same_agent() {
  # same_agent <label> <state dir name>: both runs print the same agent line, session included
  local first second
  first="$(grep "^\[1\] assistant: " "$WORK/$2-1.log")"
  second="$(grep "^\[1\] assistant: " "$WORK/$2-2.log")"
  [[ -n "$first" && "$first" == "$second" ]] && return 0
  echo "[typescript-quickstart] $1: the second run found another agent:" >&2
  printf "    %s\n    %s\n" "$first" "$second" >&2
  exit 1
}

(cd "$META_ROOT/sdk/typescript" && npm install --silent --no-fund --no-audit && npm run build --silent)
(cd "$META_ROOT/examples" && npm ci --silent --no-fund --no-audit)

echo "[typescript-quickstart] without a provider key"
key="${OPENAI_API_KEY:-}"
unset OPENAI_API_KEY
for n in 1 2; do
  run "without a key" nokey "$n"
  expect "without a key, run $n" "$WORK/nokey-$n.log" "^\[1\] runtime running: true$"
  expect "without a key, run $n" "$WORK/nokey-$n.log" "^\[1\] assistant: active, generation 0, session [0-9a-f-]\{36\}$"
  expect "without a key, run $n" "$WORK/nokey-$n.log" "^\[2\] skipped: "
  expect "without a key, run $n" "$WORK/nokey-$n.log" "^\[3\] skipped: "
done
same_agent "without a key" nokey
echo "[typescript-quickstart] without a provider key: ok"

if [[ -z "$key" ]]; then
  echo "[typescript-quickstart] OPENAI_API_KEY is not set: the run with a provider is skipped"
  exit 0
fi

echo "[typescript-quickstart] with a provider key"
export OPENAI_API_KEY="$key"
for n in 1 2; do
  run "with a key" live "$n"
  expect "with a key, run $n" "$WORK/live-$n.log" "^\[3\] answer: [Nn]oted\.\? (output text, attributed to this send: true)$"
done
expect "with a key, run 1" "$WORK/live-1.log" "^\[2\] the word from an earlier run: [Nn]one\.\?$"
# The second run is the same agent with the conversation of the first.
expect "with a key, run 2" "$WORK/live-2.log" "^\[2\] the word from an earlier run: [Hh]eron\.\?$"
same_agent "with a key" live
echo "[typescript-quickstart] with a provider key: ok"
