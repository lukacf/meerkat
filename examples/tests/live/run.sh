#!/usr/bin/env bash
# Run each example's cheapest path that reaches a provider.
#
#   examples/tests/live/run.sh            run what the environment allows
#   examples/tests/live/run.sh --dry-run  list what would run, and what would be skipped and why
#
# This is not part of `make verify` and of no hosted lane: it needs provider
# keys and spends real calls. Each example gets a fresh home directory and only
# the keys it needs. Logs go to LIVE_LOG_DIR (a new temporary directory by
# default), never into the repository.
#
# Environment:
#   ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY   provider keys; an example whose key is absent is skipped
#   RKAT, RKAT_RPC, RKAT_REST                           host binaries (the Makefile passes the ones `make hosts` builds)
#   RKAT_TARGET                                         cargo target directory holding debug/examples/<name>
#   PYTHON                                              a Python that can import the SDK from sdks/python (default python3)
#   LIVE_ONLY="001 036"                                 run only these examples
#   LIVE_MAX_CALLS                                      refuse to start when the estimate of provider calls is higher (default 600)
#   LIVE_LOG_DIR                                        where the logs go
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
LIVE="$ROOT/examples/tests/live"
DRY=0
[[ "${1:-}" == "--dry-run" ]] && DRY=1
PYTHON="${PYTHON:-python3}"
RKAT_TARGET="${RKAT_TARGET:-}"
RKAT="${RKAT:-$RKAT_TARGET/debug/rkat}"
RKAT_RPC="${RKAT_RPC:-$RKAT_TARGET/debug/rkat-rpc}"
RKAT_REST="${RKAT_REST:-$RKAT_TARGET/debug/rkat-rest}"
MAX_CALLS="${LIVE_MAX_CALLS:-600}"
# The examples run with a fresh HOME; the browsers Playwright installed stay where they are.
BROWSERS="${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}"
ONLY=" ${LIVE_ONLY:-} "

ids=(); needs=(); calls=(); limits=(); requires=(); commands=()

# entry <id> <keys> <calls> <seconds> <required files> <command>
#   keys            letters of the keys the example needs: A (Anthropic), O (OpenAI), G (Gemini)
#   calls           an estimate of the provider calls one run makes
#   seconds         the time limit of the run
#   required files  space-separated paths that must exist, else the example is skipped
#   command         run by `bash -c` in a fresh directory that is also its HOME
entry() {
  ids+=("$1"); needs+=("$2"); calls+=("$3"); limits+=("$4"); requires+=("$5"); commands+=("$6")
}

rust_example() { # <id> <binary name> <calls> <seconds>
  entry "$1" A "$3" "$4" "$RKAT_TARGET/debug/examples/$2" "$RKAT_TARGET/debug/examples/$2"
}
python_example() { # <id> <directory> <keys> <calls> <seconds>
  entry "$1" "$3" "$4" "$5" "$RKAT_RPC" "PYTHONPATH=$ROOT/sdks/python MEERKAT_BIN_PATH=$RKAT_RPC $PYTHON $ROOT/examples/$2/main.py"
}
typescript_example() { # <id> <directory> <calls> <seconds>
  entry "$1" A "$3" "$4" "$RKAT_RPC $ROOT/examples/node_modules/.bin/tsx $ROOT/sdks/typescript/dist/index.js" \
    "cd $ROOT/examples && MEERKAT_BIN_PATH=$RKAT_RPC node_modules/.bin/tsx $2/main.ts"
}

rust_example 001 001-hello-meerkat 1 120
python_example 002 002-hello-meerkat-py A 1 120
typescript_example 003 003-hello-meerkat-ts 1 120
entry 004 A 8 300 "$RKAT" "RKAT=$RKAT $ROOT/examples/004-cli-one-liners-sh/examples.sh"
rust_example 005 005-streaming-events 4 180
rust_example 006 006-custom-tools 4 180
python_example 007 007-multi-turn-sessions-py A 5 180
typescript_example 008 008-structured-output-ts 3 180
rust_example 009 009-budget-and-retry 3 180
entry 010 A 3 300 "$RKAT" "RKAT=$RKAT $ROOT/examples/010-mcp-tool-server-sh/setup.sh"
rust_example 011 011-hooks-guardrails 3 180
rust_example 012 012-skills-loading 4 180
rust_example 013 013-context-compaction 15 420
rust_example 014 014-semantic-memory 4 180
rust_example 015 015-session-persistence 3 180
rust_example 017 017-mob-coding-swarm 8 300
rust_example 018 018-mob-research-team 8 300
rust_example 019 019-mob-pipeline 6 300
python_example 021 021-multi-provider-routing-py AOG 12 300
entry 022 A 4 180 "$RKAT_REST" \
  "$LIVE/with-server.sh http://127.0.0.1:8080/health $RKAT_REST --context-root . -- $PYTHON $ROOT/examples/022-rest-api-client-py/main.py"
typescript_example 023 023-rpc-ide-integration-ts 6 300
rust_example 024 024-host-mode-event-mesh 6 300
rust_example 025 025-full-stack-agent 6 300
python_example 026 026-skills-v21-invoke-py A 2 180
typescript_example 027 027-skills-v21-invoke-ts 2 180
# The browser examples: headless Chromium, the providers reached through the
# proxy of sdks/web, which alone gets the key. One provider (Gemini) and its
# fast model for every agent keeps these three, the largest of the set, cheap.
STRIP="env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u GEMINI_API_KEY"
PROXY="node $ROOT/sdks/web/proxy/cli.mjs"
WASM="$ROOT/sdks/web/wasm/meerkat_web_runtime_bg.wasm"
W031="$ROOT/examples/031-wasm-mini-diplomacy-sh"
entry 031 G 120 480 "$RKAT $WASM $W031/web/node_modules/playwright" \
  "cd $W031 && RKAT_BIN=$RKAT $STRIP ./examples.sh > /dev/null && $LIVE/with-server.sh http://127.0.0.1:4173/ $STRIP python3 -m http.server 4173 --bind 127.0.0.1 --directory web/dist -- $LIVE/with-server.sh http://127.0.0.1:3100/ $PROXY --port 3100 -- $STRIP node $LIVE/web-031.cjs $W031/web/node_modules 'http://127.0.0.1:4173/?proxy=http://127.0.0.1:3100&france=gemini-4-hydrogen&prussia=gemini-4-hydrogen&russia=gemini-4-hydrogen&narrator=gemini-4-hydrogen' 360"
W032="$ROOT/examples/032-wasm-webcm-agent/web"
entry 032 G 40 360 "$W032/public/webcm.wasm $W032/public/meerkat-pkg/meerkat_web_runtime_bg.wasm $W032/node_modules/playwright" \
  "cd $W032 && $LIVE/with-server.sh http://127.0.0.1:4032/ $STRIP npx vite --port 4032 --strictPort --host 127.0.0.1 -- $LIVE/with-server.sh http://127.0.0.1:3102/ $PROXY --port 3102 -- $STRIP node $LIVE/web-032.cjs $W032/node_modules 'http://127.0.0.1:4032/?proxy=http://127.0.0.1:3102&orchestrator=gemini-4-hydrogen&planner=gemini-4-hydrogen&coder=gemini-4-hydrogen&reviewer=gemini-4-hydrogen' 120"
W033="$ROOT/examples/033-the-office-demo-sh/web"
entry 033 G 30 300 "$WASM $W033/node_modules/playwright" \
  "cd $W033 && $STRIP npm run build > /dev/null && $LIVE/with-server.sh http://127.0.0.1:4174/ $STRIP python3 -m http.server 4174 --bind 127.0.0.1 --directory dist -- $LIVE/with-server.sh http://127.0.0.1:3101/ $PROXY --port 3101 -- $STRIP node $LIVE/web-033.cjs $W033/node_modules 'http://127.0.0.1:4174/?proxy=http://127.0.0.1:3101&model=gemini-4-hydrogen' 45"
entry 036 O 3 180 "$RKAT_RPC" \
  "PYTHONPATH=$ROOT/sdks/python MEERKAT_BIN_PATH=$RKAT_RPC $PYTHON $ROOT/examples/036-realtime-audio-py/main.py --text-probe"
entry 037 O 2 240 "$RKAT_RPC $ROOT/examples/037-live-webrtc-web/node_modules/.bin/tsx" \
  "cd $ROOT/examples/037-live-webrtc-web && RKAT_RPC=$RKAT_RPC $LIVE/with-server.sh http://127.0.0.1:4173/ npm start -- npm run smoke:webrtc"

python_example 038 038-mob-control-py A 16 360
typescript_example 039 039-mob-control-ts 16 360

key_name() { case "$1" in A) echo ANTHROPIC_API_KEY ;; O) echo OPENAI_API_KEY ;; G) echo GEMINI_API_KEY ;; esac; }

# Why an entry cannot run, or nothing when it can.
blocker() { # <index>
  local i="$1" letter name path
  for letter in $(echo "${needs[$i]}" | grep -o .); do
    name="$(key_name "$letter")"
    [[ -n "${!name:-}" ]] || { echo "$name is not set"; return; }
  done
  for path in ${requires[$i]}; do
    [[ -e "$path" ]] || { echo "missing ${path#"$ROOT"/}"; return; }
  done
}

selected() { [[ "$ONLY" == "  " || "$ONLY" == *" $1 "* ]]; }

planned=0
for i in "${!ids[@]}"; do
  selected "${ids[$i]}" || continue
  [[ -z "$(blocker "$i")" ]] && planned=$((planned + calls[i]))
done

if [[ $DRY -eq 1 ]]; then
  for i in "${!ids[@]}"; do
    selected "${ids[$i]}" || continue
    why="$(blocker "$i")"
    if [[ -n "$why" ]]; then
      printf 'WOULD SKIP %s  (%s)\n' "${ids[$i]}" "$why"
    else
      printf 'WOULD RUN  %s  keys %-3s  about %3s calls  limit %3ss  %s\n' "${ids[$i]}" "${needs[$i]}" "${calls[$i]}" "${limits[$i]}" "${commands[$i]//$ROOT\//}"
    fi
  done
  echo "about $planned provider calls in all (the cap is $MAX_CALLS)"
  exit 0
fi

if (( planned > MAX_CALLS )); then
  echo "refusing to start: about $planned provider calls are planned and LIVE_MAX_CALLS is $MAX_CALLS" >&2
  exit 2
fi

LOG_DIR="${LIVE_LOG_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/meerkat-examples-live.XXXXXX")}"
mkdir -p "$LOG_DIR"
case "$LOG_DIR/" in "$ROOT"/*) echo "LIVE_LOG_DIR must be outside the repository" >&2; exit 2 ;; esac
echo "logs: $LOG_DIR"

passed=0; failed=0; skipped=0
for i in "${!ids[@]}"; do
  id="${ids[$i]}"
  selected "$id" || continue
  why="$(blocker "$i")"
  if [[ -n "$why" ]]; then
    printf 'SKIP %s  (%s)\n' "$id" "$why"; skipped=$((skipped + 1)); continue
  fi
  home="$LOG_DIR/home-$id"; rm -rf "$home"; mkdir -p "$home"
  keys=()
  for letter in $(echo "${needs[$i]}" | grep -o .); do
    name="$(key_name "$letter")"; keys+=("$name=${!name}")
  done
  start=$(date +%s)
  ( cd "$home" && env -i HOME="$home" PATH="$PATH" TERM=dumb PLAYWRIGHT_BROWSERS_PATH="$BROWSERS" "${keys[@]}" \
      timeout --signal=TERM --kill-after=10 "${limits[$i]}" bash -c "${commands[$i]}" ) > "$LOG_DIR/$id.log" 2>&1 < /dev/null
  rc=$?
  secs=$(( $(date +%s) - start ))
  if [[ $rc -eq 0 ]]; then
    printf 'PASS %s  %3ss\n' "$id" "$secs"; passed=$((passed + 1))
  else
    printf 'FAIL %s  %3ss  exit %s  (see %s.log)\n' "$id" "$secs" "$rc" "$id"; failed=$((failed + 1))
  fi
done
echo "passed $passed, failed $failed, skipped $skipped; about $planned provider calls planned"
[[ $failed -eq 0 ]]
