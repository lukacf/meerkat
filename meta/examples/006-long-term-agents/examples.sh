#!/usr/bin/env bash
# Runs the Python and the TypeScript program, twice each on one state
# directory, and checks what they print. No provider key is needed.
#
#   META_GATEWAY_BIN=<rpc_gateway> ./examples.sh
#
# Without META_GATEWAY_BIN the gateway is built with the repository's own
# wrapper, which takes a while the first time.
set -euo pipefail

PACK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
META_ROOT="$(cd "$PACK_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$META_ROOT/.." && pwd)"

if [[ -z "${META_GATEWAY_BIN:-}" ]]; then
  echo "[long-term-agents] building the gateway"
  "$REPO_ROOT/scripts/repo-cargo" build --locked -p meerkat-meta --bin rpc_gateway
  META_GATEWAY_BIN="$("$REPO_ROOT/scripts/repo-cargo" --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')/debug/rpc_gateway"
fi
export META_GATEWAY_BIN
[[ -x "$META_GATEWAY_BIN" ]] || { echo "no gateway at $META_GATEWAY_BIN" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

expect() {
  # expect <label> <log> <pattern>: the log has a line matching the pattern
  grep -q "$3" "$2" && return 0
  echo "[long-term-agents] $1: no line matches: $3" >&2
  echo "[long-term-agents] $1: the program printed:" >&2
  sed 's/^/    /' "$2" >&2
  exit 1
}

check() {
  # check <label> <first run log> <second run log>
  local label="$1" first="$2" second="$3"
  expect "$label, first run" "$first" '^\[2\] adopt answered done$'
  expect "$label, first run" "$first" '^\[3\] the adoption is committed$'
  expect "$label, first run" "$first" '^\[4\] created editor: state committed, seat seated$'
  expect "$label, first run" "$first" '^\[4\] updated editor.s instructions: applies_at_next_run$'
  expect "$label, first run" "$first" '^\[4\] removed editor: '
  expect "$label, first run" "$first" '^\[5\] refused as expected: name_taken at agents\[0\]\.name: '
  expect "$label, first run" "$first" '^\[end\] agents now: scout '
  # The second run is answered from the record of the first: the same
  # changes, done, and nothing created twice.
  expect "$label, second run" "$second" '^\[2\] adopt answered done$'
  expect "$label, second run" "$second" '^\[4\] created editor: state committed, seat \(None\|null\)$'
  expect "$label, second run" "$second" '^\[4\] updated editor.s instructions: not stated$'
  expect "$label, second run" "$second" '^\[end\] agents now: scout '
  echo "[long-term-agents] $label: ok"
}

echo "[long-term-agents] Python"
python3 -m venv "$WORK/venv"
"$WORK/venv/bin/python" -m pip install --quiet "$META_ROOT/sdk/python"
for run in 1 2; do
  EXAMPLE_STATE_DIR="$WORK/state-py" "$WORK/venv/bin/python" "$PACK_DIR/main.py" > "$WORK/py-$run.log" 2> "$WORK/py-$run.err" \
    || { echo "[long-term-agents] Python: run $run failed:" >&2; tail -n 20 "$WORK/py-$run.err" >&2; exit 1; }
done
check "Python" "$WORK/py-1.log" "$WORK/py-2.log"

echo "[long-term-agents] TypeScript"
(cd "$META_ROOT/sdk/typescript" && npm install --silent --no-fund --no-audit && npm run build --silent)
(cd "$META_ROOT/examples" && npm install --silent --no-fund --no-audit)
for run in 1 2; do
  (cd "$META_ROOT/examples" && EXAMPLE_STATE_DIR="$WORK/state-ts" npx tsx "$PACK_DIR/main.ts" > "$WORK/ts-$run.log" 2> "$WORK/ts-$run.err") \
    || { echo "[long-term-agents] TypeScript: run $run failed:" >&2; tail -n 20 "$WORK/ts-$run.err" >&2; exit 1; }
done
check "TypeScript" "$WORK/ts-1.log" "$WORK/ts-2.log"

# The two programs are the same five steps: apart from how each language
# prints false and null, their lines are equal.
if ! diff <(sed 's/False/false/; s/None/null/' "$WORK/py-1.log") "$WORK/ts-1.log" > "$WORK/languages.diff"; then
  echo "[long-term-agents] Python (<) and TypeScript (>) print different lines:" >&2
  sed 's/^/    /' "$WORK/languages.diff" >&2
  exit 1
fi
echo "[long-term-agents] Python and TypeScript print the same lines"
