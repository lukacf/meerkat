#!/usr/bin/env bash
# Runs the Python and the TypeScript program and checks what they print:
# once each without a provider key (the runtime starts, no question is open,
# the rest is skipped), and, when OPENAI_API_KEY is set, twice each on one
# state directory (the planner asks, the host answers after a restart, a
# second question is dismissed; the second run answers the other choice).
#
#   OPENAI_API_KEY=sk-... META_GATEWAY_BIN=<rpc_gateway> ./examples.sh
#
# Without META_GATEWAY_BIN the gateway is built with the repository wrapper,
# which takes a while the first time.
set -euo pipefail

PACK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
META_ROOT="$(cd "$PACK_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$META_ROOT/.." && pwd)"

if [[ -z "${META_GATEWAY_BIN:-}" ]]; then
  echo "[ask-the-user] building the gateway"
  "$REPO_ROOT/scripts/repo-cargo" build --locked -p meerkat-meta --bin rpc_gateway
  META_GATEWAY_BIN="$("$REPO_ROOT/scripts/repo-cargo" --print-env | sed -n "s/^CARGO_TARGET_DIR=//p")/debug/rpc_gateway"
fi
export META_GATEWAY_BIN
[[ -x "$META_GATEWAY_BIN" ]] || { echo "no gateway at $META_GATEWAY_BIN" >&2; exit 1; }

WORK="$(mktemp -d)"
trap "rm -rf \"$WORK\"" EXIT

expect() {
  # expect <label> <log> <pattern>: the log has a line matching the pattern
  grep -qi "$3" "$2" && return 0
  echo "[ask-the-user] $1: no line matches: $3" >&2
  echo "[ask-the-user] $1: the program printed:" >&2
  sed "s/^/    /" "$2" >&2
  exit 1
}

check_without_key() {
  # check_without_key <label> <log>
  expect "$1" "$2" "^\[1\] runtime started, open question sets: 0$"
  expect "$1" "$2" "^\[2\] skipped: "
  [[ "$(wc -l < "$2")" -eq 2 ]] || { echo "[ask-the-user] $1: more than two lines without a key" >&2; exit 1; }
  echo "[ask-the-user] $1, without a provider key: ok"
}

check_with_key() {
  # check_with_key <label> <log> <choice>: the sentences of the planner are
  # its own; the checks name what they must hold, not their wording.
  local label="$1" log="$2" choice="$3"
  expect "$label" "$log" "^\[1\] runtime started, open question sets: 0$"
  expect "$label" "$log" "^\[2\] the planner.s turn ended with: ."
  expect "$label" "$log" "^\[3\] asked by planner: .* Choices: bowling (.*), boat (.*)\. Open for about [0-9]* more minutes\.$"
  expect "$label" "$log" "^\[4\] after a restart the same question is open: yes$"
  expect "$label" "$log" "^\[5\] the host answered $choice for the person: the set is answered$"
  expect "$label" "$log" "^\[5\] the planner went on: .*$choice"
  expect "$label" "$log" "^\[6\] the host dismissed the question lunch: the set is dismissed$"
  expect "$label" "$log" "^\[6\] the planner was told: ."
  expect "$label" "$log" "^\[end\] open question sets: 0$"
  echo "[ask-the-user] $label, with a provider key: ok"
}

key="${OPENAI_API_KEY:-}"
unset OPENAI_API_KEY

run_python() {
  # run_python <state name> <run number> [choice]
  EXAMPLE_STATE_DIR="$WORK/$1" EXAMPLE_CHOICE="${3:-boat}" "$WORK/venv/bin/python" "$PACK_DIR/main.py" > "$WORK/$1-$2.log" 2> "$WORK/$1-$2.err" \
    || { echo "[ask-the-user] Python: run $2 on $1 failed:" >&2; cat "$WORK/$1-$2.log" >&2; tail -n 20 "$WORK/$1-$2.err" >&2; exit 1; }
}

run_typescript() {
  # run_typescript <state name> <run number> [choice]
  (cd "$META_ROOT/examples" && EXAMPLE_STATE_DIR="$WORK/$1" EXAMPLE_CHOICE="${3:-boat}" npx tsx "$PACK_DIR/main.ts" > "$WORK/$1-$2.log" 2> "$WORK/$1-$2.err") \
    || { echo "[ask-the-user] TypeScript: run $2 on $1 failed:" >&2; cat "$WORK/$1-$2.log" >&2; tail -n 20 "$WORK/$1-$2.err" >&2; exit 1; }
}

echo "[ask-the-user] Python"
python3 -m venv "$WORK/venv"
"$WORK/venv/bin/python" -m pip install --quiet "$META_ROOT/sdk/python"
run_python py 1
check_without_key "Python" "$WORK/py-1.log"

echo "[ask-the-user] TypeScript"
(cd "$META_ROOT/sdk/typescript" && npm install --silent --no-fund --no-audit && npm run build --silent)
(cd "$META_ROOT/examples" && npm ci --silent --no-fund --no-audit)
run_typescript ts 1
check_without_key "TypeScript" "$WORK/ts-1.log"

# The two programs are the same steps: without a key their lines are equal.
if ! diff "$WORK/py-1.log" "$WORK/ts-1.log" > "$WORK/languages.diff"; then
  echo "[ask-the-user] Python (<) and TypeScript (>) print different lines:" >&2
  sed "s/^/    /" "$WORK/languages.diff" >&2
  exit 1
fi
echo "[ask-the-user] Python and TypeScript print the same lines"

if [[ -z "$key" ]]; then
  echo "[ask-the-user] OPENAI_API_KEY is not set: the runs in which the agent asks are skipped"
  exit 0
fi

export OPENAI_API_KEY="$key"
run_python py-live 1 boat
check_with_key "Python, first run" "$WORK/py-live-1.log" boat
run_python py-live 2 bowling
check_with_key "Python, second run" "$WORK/py-live-2.log" bowling
run_typescript ts-live 1 boat
check_with_key "TypeScript, first run" "$WORK/ts-live-1.log" boat
run_typescript ts-live 2 bowling
check_with_key "TypeScript, second run" "$WORK/ts-live-2.log" bowling
