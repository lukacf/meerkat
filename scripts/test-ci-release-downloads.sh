#!/usr/bin/env bash
# Contract test for the CI steps that install a pinned tool from a GitHub
# release (scripts/ci-release-asset-download and its callers), run as written
# against stub tools (gh, curl, wget, sudo, tar and the rest), with the steps
# read through the release contract's own workflow and action readers:
# - scripts/ci-release-asset-download fetches the asset with
#   `gh release download` and the job's token, tries three times, and on any
#   failure (no token, no gh, three failed tries) exits non-zero with no
#   partial file left and the reason on stderr, never the token;
# - each caller (setup-tlc-ci's jar, setup-mold-ci's archive, every wasm-pack
#   install, setup-cargo-tool-ci's nextest and cargo-deny) passes the job's
#   own token, downloads its anonymous release URL as before only when that
#   fails or there is no token, and unsets the token before anything else
#   runs;
# - setup-tlc-ci checks the jar's sha256 whichever way it came, and a
#   mismatch fails the step without trying the other way;
# - setup-mold-ci unpacks mold into /usr/local and links the system's ld to
#   it as rui314/setup-mold did, with the same wget command when it falls back;
# - setup-cargo-tool-ci (run with the real tar, sha256sum and install on
#   stand-in archives laid out like the releases) checks the archive's sha256
#   before it unpacks anything, refuses a mismatch from either route without
#   trying the other, and installs where taiki-e/install-action did:
#   $CARGO_HOME/bin when the cargo on PATH is there, else
#   $HOME/.install-action/bin, added to PATH;
# - no run step of the CI workflows downloads a release URL without the API
#   attempt first, none uses rui314/setup-mold or taiki-e/install-action, and
#   every setup-cargo-tool-ci step names a tool it installs.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${PYTHON:-python3}"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/ci-release-downloads.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT
HELPER="$REPO_ROOT/scripts/ci-release-asset-download"

"$PYTHON" - "$REPO_ROOT" "$TEST_ROOT" <<'PY'
import re
import sys
from pathlib import Path

root, out = Path(sys.argv[1]), Path(sys.argv[2])
sys.path.insert(0, str(root / "scripts"))
import check_release_workflow_contract as contract

WORKFLOWS = [
    "ci.yml",
    "meta-ci.yml",
    "cargo.yml",
    "nightly.yml",
    "mob-dense-topology.yml",
    "published-crates-standalone.yml",
]
ACTIONS = [
    "./.github/actions/setup-tlc-ci",
    "./.github/actions/setup-mold-ci",
    "./.github/actions/setup-cargo-tool-ci",
    "./.github/actions/run-nextest-archive",
]
CARGO_TOOLS = {"nextest", "cargo-deny"}


def step_named(action, name):
    for step in action.steps:
        if step.name == name:
            return step
    sys.exit(f"{action.name} has no `{name}` step (steps: {[s.name for s in action.steps]})")


tlc_action = contract._local_action(ACTIONS[0], root)
tlc = step_named(tlc_action, "Install TLC")
(out / "tlc.sh").write_text(tlc.raw_run, encoding="utf-8")
(out / "tlc-url").write_text(tlc_action.defaults.get("url", "") + "\n", encoding="utf-8")
(out / "tlc-token").write_text(tlc.env.get("GH_TOKEN", "") + "\n", encoding="utf-8")

mold_action = contract._local_action(ACTIONS[1], root)
mold = step_named(mold_action, "Install mold")
(out / "mold.sh").write_text(mold.raw_run, encoding="utf-8")
(out / "mold-version").write_text(mold.env.get("MOLD_VERSION", "") + "\n", encoding="utf-8")
(out / "mold-token").write_text(mold.env.get("GH_TOKEN", "") + "\n", encoding="utf-8")

tool_action = contract._local_action(ACTIONS[2], root)
tool = step_named(tool_action, "Install the tool")
(out / "cargo-tool.sh").write_text(tool.raw_run, encoding="utf-8")
for key in ("NEXTEST_VERSION", "NEXTEST_SHA256", "CARGO_DENY_VERSION", "CARGO_DENY_SHA256", "GH_TOKEN", "TOOL"):
    (out / f"cargo-tool-{key}").write_text(tool.env.get(key, "") + "\n", encoding="utf-8")


def jobs_of(text):
    names, in_jobs = [], False
    for line in text.splitlines():
        if re.match(r"^\S", line):
            in_jobs = line.startswith("jobs:")
            continue
        match = contract.JOB_HEADER.match(line) if in_jobs else None
        if match:
            names.append(match.group(1))
    return names


problems = []
wasm = 0
tool_uses = 0
steps_by_place = []
for workflow in WORKFLOWS:
    text = (root / ".github" / "workflows" / workflow).read_text(encoding="utf-8")
    for job in jobs_of(text):
        try:
            steps = contract.job_steps(contract.job_block(text, job))
        except contract.ContractError:
            continue  # a job that calls a reusable workflow has no steps
        steps_by_place.extend((f"{workflow} {job}", step) for step in steps)
for uses in ACTIONS:
    action = contract._local_action(uses, root)
    steps_by_place.extend((uses[2:], step) for step in action.steps)

for place, step in steps_by_place:
    uses = step.fields.get("uses", "")
    if "rui314/setup-mold" in uses:
        problems.append(f"{place}: `{step.name}` installs mold through rui314/setup-mold")
    if "taiki-e/install-action" in uses:
        problems.append(f"{place}: `{step.name}` installs through taiki-e/install-action")
    if uses == "./.github/actions/setup-cargo-tool-ci":
        tool_uses += 1
        named = step.nested("with").get("tool", "")
        if named not in CARGO_TOOLS:
            problems.append(f"{place}: `{step.name}` asks setup-cargo-tool-ci for `{named}`")
    # `run` holds a one-line value too; raw_run only a block scalar.
    if "/releases/download/" in step.run and "ci-release-asset-download" not in step.run:
        problems.append(f"{place}: `{step.name}` downloads a release URL with no API attempt first")
    if "wasm-pack/releases/download" in step.run:
        wasm += 1
        (out / f"wasm-{wasm}.sh").write_text(step.raw_run, encoding="utf-8")
        (out / f"wasm-{wasm}.place").write_text(place + "\n", encoding="utf-8")
        (out / f"wasm-{wasm}.token").write_text(step.env.get("GH_TOKEN", "") + "\n", encoding="utf-8")
(out / "wasm-count").write_text(f"{wasm}\n", encoding="utf-8")
(out / "cargo-tool-uses").write_text(f"{tool_uses}\n", encoding="utf-8")
(out / "guard-problems").write_text("".join(f"{p}\n" for p in problems), encoding="utf-8")
PY

# Stand-in tools. Each records its arguments and whether it saw a GH_TOKEN
# in $STUB_LOG_DIR/<tool>.log. gh and curl serve STUB_<TOOL>_SERVE to their
# --output path ("serve"), succeed without writing ("ok", for steps whose
# output path is outside the test directory), or fail the way the release
# hosts did ("fail"; gh's "fail-once" fails its first call only).
STUB_BIN="$TEST_ROOT/stub-bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s token=%s\n' "$*" "${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/gh.log"
out=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  if [[ "${args[i]}" == "--output" ]]; then out="${args[i + 1]}"; fi
done
mode="$STUB_GH_MODE"
if [[ "$mode" == fail-once ]]; then
  if [[ "$(wc -l < "$STUB_LOG_DIR/gh.log")" -eq 1 ]]; then mode=fail; else mode=serve; fi
fi
case "$mode" in
  serve) cp "$STUB_GH_SERVE" "$out" ;;
  ok) ;;
  fail)
    if [[ -n "${STUB_GH_PARTIAL:-}" ]]; then printf 'partial' > "$out"; fi
    echo "HTTP 502: Bad Gateway (https://api.github.com/repos/${args[3]:-}/releases/tags/${args[2]:-})" >&2
    exit 1
    ;;
esac
EOF
cat > "$STUB_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s token=%s\n' "$*" "${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/curl.log"
out=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  if [[ "${args[i]}" == "--output" || "${args[i]}" == "-o" ]]; then out="${args[i + 1]}"; fi
done
case "$STUB_CURL_MODE" in
  serve) cp "$STUB_CURL_SERVE" "$out" ;;
  ok) ;;
  fail)
    echo "curl: (22) The requested URL returned error: 504" >&2
    exit 22
    ;;
esac
EOF
cat > "$STUB_BIN/wget" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s token=%s\n' "$*" "${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/wget.log"
case "$STUB_WGET_MODE" in
  serve) cat "$STUB_WGET_SERVE" ;;
  fail)
    echo "ERROR 504: Gateway Time-out." >&2
    exit 8
    ;;
esac
EOF
cat > "$STUB_BIN/sudo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s token=%s\n' "$*" "${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/sudo.log"
exec "$@"
EOF
# tar records the sha256 of what it unpacked: the file after -xzf, or stdin.
cat > "$STUB_BIN/tar" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
args=("$@")
input=""
for ((i = 0; i < ${#args[@]}; i++)); do
  if [[ "${args[i]}" == "-xzf" ]]; then input="${args[i + 1]}"; fi
done
if [[ "$input" == "-" ]]; then
  digest="$(sha256sum | cut -d' ' -f1)"
elif [[ -f "$input" ]]; then
  digest="$(sha256sum < "$input" | cut -d' ' -f1)"
else
  digest=none
fi
printf '%s token=%s input=%s\n' "$*" "${GH_TOKEN:+set}" "$digest" >> "$STUB_LOG_DIR/tar.log"
EOF
for tool in ln install java wasm-pack; do
  cat > "$STUB_BIN/$tool" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s token=%s\n' "$*" "${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/$(basename "$0").log"
case "$(basename "$0")" in
  java) echo "TLC2 Version 2.19 of Day Month 20?? (stand-in)" ;;
  wasm-pack) echo "wasm-pack 0.0.0-stand-in" ;;
esac
EOF
done
cat > "$STUB_BIN/whoami" <<'EOF'
#!/usr/bin/env bash
echo "${STUB_WHOAMI:-runner}"
EOF
cat > "$STUB_BIN/realpath" <<'EOF'
#!/usr/bin/env bash
if [[ "$*" != /usr/bin/ld ]]; then
  echo "unexpected realpath $*" >&2
  exit 99
fi
echo "$STUB_LD"
EOF
cat > "$STUB_BIN/uname" <<'EOF'
#!/usr/bin/env bash
echo x86_64
EOF
cat > "$STUB_BIN/sleep" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_LOG_DIR/sleep.log"
EOF
chmod +x "$STUB_BIN"/*
# The wasm-pack steps write under /tmp; their cases stub rm too, so no
# step or helper run here touches /tmp.
STUB_RM="$TEST_ROOT/stub-rm"
mkdir -p "$STUB_RM"
cat > "$STUB_RM/rm" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$STUB_LOG_DIR/rm.log"
EOF
chmod +x "$STUB_RM/rm"
NO_TOOLS="$TEST_ROOT/no-tools"
mkdir -p "$NO_TOOLS"

TOKEN_VALUE="job-token-value-never-printed"
# shellcheck disable=SC2016 # the expression as a workflow writes it
JOB_TOKEN='${{ github.token }}'
failures=0
case_dir=""

# Runs a script the way a step runs: from the checkout's root, in a clean
# environment plus the given assignments, the stubs first on PATH.
run_case() {
  local label="$1" path="$2" script="$3"
  shift 3
  case_dir="$TEST_ROOT/case-$label"
  mkdir -p "$case_dir/logs" "$case_dir/home" "$case_dir/runner-temp"
  : > "$case_dir/github-path"
  local rc=0
  (
    cd "$REPO_ROOT"
    env -i PATH="$path" STUB_LOG_DIR="$case_dir/logs" HOME="$case_dir/home" \
      RUNNER_TEMP="$case_dir/runner-temp" GITHUB_PATH="$case_dir/github-path" "$@" \
      "$BASH" --noprofile --norc -eo pipefail "$script"
  ) > "$case_dir/stdout" 2> "$case_dir/stderr" || rc=$?
  echo "$rc" > "$case_dir/rc"
}

check() {
  local label="$1"
  shift
  if "$@"; then
    printf 'PASS %s\n' "$label"
  else
    printf 'FAIL %s\n' "$label" >&2
    failures=$((failures + 1))
    printf '  exit %s; stdout tail:\n' "$(cat "$case_dir/rc" 2> /dev/null)" >&2
    tail -n 5 "$case_dir/stdout" 2> /dev/null | sed 's/^/    /' >&2
    printf '  stderr tail:\n' >&2
    tail -n 5 "$case_dir/stderr" 2> /dev/null | sed 's/^/    /' >&2
  fi
}

exit_is() { [[ "$(cat "$case_dir/rc")" == "$1" ]]; }
exit_is_not() { [[ "$(cat "$case_dir/rc")" != "$1" ]]; }
log_is() { [[ "$(cat "$case_dir/logs/$1.log" 2> /dev/null)" == "$2" ]]; }
not_called() { [[ ! -e "$case_dir/logs/$1.log" ]]; }
calls_are() {
  local count=0
  if [[ -e "$case_dir/logs/$1.log" ]]; then count="$(wc -l < "$case_dir/logs/$1.log")"; fi
  (( count == $2 ))
}
says() { grep -Fq -- "$2" "$case_dir/$1"; }
token_never_printed() { ! grep -Fq -- "$TOKEN_VALUE" "$case_dir/stdout" "$case_dir/stderr"; }
no_tool_saw_token() {
  local log
  for log in "$case_dir"/logs/*.log; do
    [[ "$(basename "$log")" == gh.log ]] && continue
    if grep -q 'token=set' "$log"; then return 1; fi
  done
}
absent() { [[ ! -e "$1" ]]; }
same_file() { cmp -s "$1" "$2"; }

STUB_PATH="$STUB_BIN:$PATH"

# --- scripts/ci-release-asset-download -----------------------------------
printf 'asset bytes\n' > "$TEST_ROOT/asset"
api_args="release download v1.2.3 --repo owner/tool --pattern tool.tar.gz --output $TEST_ROOT/case-helper-api/out.tar.gz --clobber token=set"

# A one-line script that runs the helper with the given arguments, through
# this bash (so a case can leave bash off PATH); prints its path.
helper_script() {
  local script="$TEST_ROOT/helper-$1.sh" arg
  shift
  {
    # shellcheck disable=SC2016 # $BASH expands when the script runs
    printf 'exec "$BASH" %q' "$HELPER"
    for arg in "$@"; do printf ' %q' "$arg"; done
    printf '\n'
  } > "$script"
  printf '%s' "$script"
}

run_case helper-api "$STUB_PATH" "$(helper_script api owner/tool v1.2.3 tool.tar.gz "$TEST_ROOT/case-helper-api/out.tar.gz")" \
  GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/asset"
check "helper api: exit 0" exit_is 0
check "helper api: gh release download with the token, the exact asset" log_is gh "$api_args"
check "helper api: the asset is in place" same_file "$TEST_ROOT/asset" "$case_dir/out.tar.gz"
check "helper api: says it came through the API" says stdout "fetched through the API with the job token"
check "helper api: no retry" not_called sleep
check "helper api: the token is never printed" token_never_printed

run_case helper-retry "$STUB_PATH" "$(helper_script retry owner/tool v1.2.3 tool.tar.gz "$TEST_ROOT/case-helper-retry/out.tar.gz")" \
  GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=fail-once STUB_GH_SERVE="$TEST_ROOT/asset"
check "helper retry: exit 0 after one failed try" exit_is 0
check "helper retry: two gh calls" calls_are gh 2
check "helper retry: one 2 s wait" log_is sleep "2"
check "helper retry: the asset is in place" same_file "$TEST_ROOT/asset" "$case_dir/out.tar.gz"

run_case helper-down "$STUB_PATH" "$(helper_script down owner/tool v1.2.3 tool.tar.gz "$TEST_ROOT/case-helper-down/out.tar.gz")" \
  GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=fail STUB_GH_PARTIAL=1
check "helper down: exit 1" exit_is 1
check "helper down: three gh calls" calls_are gh 3
check "helper down: waits 2 s, then 4 s" log_is sleep $'2\n4'
check "helper down: no partial file is left" absent "$case_dir/out.tar.gz"
check "helper down: says gh failed 3 times" says stderr "failed 3 times"
check "helper down: the token is never printed" token_never_printed

run_case helper-no-token "$STUB_PATH" "$(helper_script no-token owner/tool v1.2.3 tool.tar.gz "$TEST_ROOT/case-helper-no-token/out.tar.gz")" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/asset"
check "helper no-token: exit 1" exit_is 1
check "helper no-token: gh is not called" not_called gh
check "helper no-token: says there is no token" says stderr "no job token"
check "helper no-token: no file" absent "$case_dir/out.tar.gz"

run_case helper-no-gh "$NO_TOOLS" "$(helper_script no-gh owner/tool v1.2.3 tool.tar.gz "$TEST_ROOT/case-helper-no-gh/out.tar.gz")" \
  GH_TOKEN="$TOKEN_VALUE"
check "helper no-gh: exit 1" exit_is 1
check "helper no-gh: says gh is missing" says stderr "no gh on PATH"

for bad in "arity owner/tool v1 a.tar.gz" "glob owner/tool v1 a*.tar.gz out" "slash owner/tool v1 dir/a.tar.gz out" \
  "repo owner v1 a.tar.gz out" "query owner/tool v1 a?.gz out"; do
  label="${bad%% *}"
  read -r -a usage_args <<< "${bad#* }"
  run_case "helper-usage-$label" "$STUB_PATH" "$(helper_script "usage-$label" "${usage_args[@]}")" \
    GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/asset"
  check "helper usage ($label): exit 2" exit_is 2
  check "helper usage ($label): gh is not called" not_called gh
done

# --- setup-tlc-ci: Install TLC ---------------------------------------------
tlc_url="$(cat "$TEST_ROOT/tlc-url")"
printf 'the pinned jar\n' > "$TEST_ROOT/jar"
printf 'another jar\n' > "$TEST_ROOT/other-jar"
jar_sha="$(sha256sum "$TEST_ROOT/jar" | cut -d' ' -f1)"
tlc_case() {
  local label="$1"
  shift
  run_case "tlc-$label" "$STUB_PATH" "$TEST_ROOT/tlc.sh" \
    GITHUB_ACTION_PATH="$REPO_ROOT/.github/actions/setup-tlc-ci" TLC_JAR_SHA256="$jar_sha" "$@"
}
tlc_gh_args() {
  [[ "$tlc_url" =~ ^https://github\.com/([^/]+/[^/]+)/releases/download/([^/]+)/([^/]+)$ ]] || return 1
  log_is gh "release download ${BASH_REMATCH[2]} --repo ${BASH_REMATCH[1]} --pattern ${BASH_REMATCH[3]} --output $case_dir/home/.local/lib/tla2tools.jar --clobber token=set"
}
tlc_installed() {
  check "$1: exit 0" exit_is 0
  check "$1: the pinned jar is in place" same_file "$TEST_ROOT/jar" "$case_dir/home/.local/lib/tla2tools.jar"
  check "$1: the tlc wrapper reaches PATH" log_is_path
  check "$1: nothing after the download saw the token" no_tool_saw_token
  check "$1: the token is never printed" token_never_printed
}
log_is_path() { [[ "$(cat "$case_dir/github-path")" == "$case_dir/home/.local/bin" && -x "$case_dir/home/.local/bin/tlc" ]]; }
tlc_refused() {
  check "$1: the step fails" exit_is_not 0
  check "$1: on the sha256 pin" says stdout "does not match the pinned"
  check "$1: no tlc wrapper" absent "$case_dir/home/.local/bin/tlc"
  check "$1: nothing reaches PATH" log_is_empty_path
}
log_is_empty_path() { [[ ! -s "$case_dir/github-path" ]]; }
curl_got_tlc_url() { log_is curl "--fail --location --retry 5 --retry-delay 2 $tlc_url --output $case_dir/home/.local/lib/tla2tools.jar token="; }

tlc_case api TLC_JAR_URL="$tlc_url" GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/jar" STUB_CURL_MODE=fail
check "tlc api: the mirror comes through gh with the job token" tlc_gh_args
check "tlc api: the anonymous URL is not tried" not_called curl
tlc_installed "tlc api"

tlc_case api-down TLC_JAR_URL="$tlc_url" GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=fail STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/jar"
check "tlc api-down: gh is tried three times" calls_are gh 3
check "tlc api-down: then the anonymous URL, as before, without the token" curl_got_tlc_url
tlc_installed "tlc api-down"

tlc_case no-token TLC_JAR_URL="$tlc_url" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/jar" STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/jar"
check "tlc no-token: gh is not called" not_called gh
check "tlc no-token: the anonymous URL is" curl_got_tlc_url
tlc_installed "tlc no-token"

tlc_case api-tampered TLC_JAR_URL="$tlc_url" GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/other-jar" STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/jar"
check "tlc api-tampered: the anonymous URL is not tried after a mismatch" not_called curl
tlc_refused "tlc api-tampered"

tlc_case curl-tampered TLC_JAR_URL="$tlc_url" STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/other-jar"
tlc_refused "tlc curl-tampered"

tlc_case both-down TLC_JAR_URL="$tlc_url" GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=fail STUB_CURL_MODE=fail
check "tlc both-down: the step fails" exit_is_not 0
check "tlc both-down: no tlc wrapper" absent "$case_dir/home/.local/bin/tlc"

tlc_case other-url TLC_JAR_URL="https://example.com/tla2tools.jar" GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/jar" STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/jar"
check "tlc other-url: a URL that is no release asset skips gh" not_called gh
check "tlc other-url: and goes to curl" log_is curl "--fail --location --retry 5 --retry-delay 2 https://example.com/tla2tools.jar --output $case_dir/home/.local/lib/tla2tools.jar token="
tlc_installed "tlc other-url"

# --- setup-mold-ci: Install mold ------------------------------------------
mold_version="$(cat "$TEST_ROOT/mold-version")"
mold_asset="mold-${mold_version}-x86_64-linux.tar.gz"
printf 'mold release archive\n' > "$TEST_ROOT/mold.tar.gz"
mold_sha="$(sha256sum "$TEST_ROOT/mold.tar.gz" | cut -d' ' -f1)"
mold_case() {
  local label="$1"
  shift
  run_case "mold-$label" "$STUB_PATH" "$TEST_ROOT/mold.sh" \
    GITHUB_ACTION_PATH="$REPO_ROOT/.github/actions/setup-mold-ci" MOLD_VERSION="$mold_version" "$@"
}
mold_gh_args() {
  log_is gh "release download v${mold_version} --repo rui314/mold --pattern ${mold_asset} --output $case_dir/runner-temp/mold-install/${mold_asset} --clobber token=set"
}
mold_wget_args() {
  log_is wget "-O- --timeout=10 --waitretry=3 --retry-connrefused --progress=dot:mega https://github.com/rui314/mold/releases/download/v${mold_version}/${mold_asset} token="
}
untar_file() {
  log_is tar "-C /usr/local --strip-components=1 --no-overwrite-dir -xzf $case_dir/runner-temp/mold-install/${mold_asset} token= input=${mold_sha}"
}
untar_stdin() { log_is tar "-C /usr/local --strip-components=1 --no-overwrite-dir -xzf - token= input=${mold_sha}"; }
links_ld() { log_is ln "-sf /usr/local/bin/mold /usr/bin/x86_64-linux-gnu-ld token="; }
mold_installed() {
  check "$1: exit 0" exit_is 0
  check "$1: links the system's ld to mold" links_ld
  check "$1: nothing after the download saw the token" no_tool_saw_token
  check "$1: the token is never printed" token_never_printed
}

mold_case api GH_TOKEN="$TOKEN_VALUE" STUB_LD=/usr/bin/x86_64-linux-gnu-ld STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/mold.tar.gz" STUB_WGET_MODE=fail
check "mold api: the release comes through gh with the job token" mold_gh_args
check "mold api: the anonymous URL is not tried" not_called wget
check "mold api: the archive is unpacked into /usr/local" untar_file
mold_installed "mold api"

mold_case api-down GH_TOKEN="$TOKEN_VALUE" STUB_LD=/usr/bin/x86_64-linux-gnu-ld STUB_GH_MODE=fail STUB_WGET_MODE=serve STUB_WGET_SERVE="$TEST_ROOT/mold.tar.gz"
check "mold api-down: gh is tried three times" calls_are gh 3
check "mold api-down: then rui314/setup-mold's wget, without the token" mold_wget_args
check "mold api-down: its stream is unpacked into /usr/local" untar_stdin
mold_installed "mold api-down"

mold_case no-token STUB_LD=/usr/bin/x86_64-linux-gnu-ld STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/mold.tar.gz" STUB_WGET_MODE=serve STUB_WGET_SERVE="$TEST_ROOT/mold.tar.gz"
check "mold no-token: gh is not called" not_called gh
check "mold no-token: the anonymous URL is" mold_wget_args
check "mold no-token: its stream is unpacked into /usr/local" untar_stdin
mold_installed "mold no-token"

mold_case both-down GH_TOKEN="$TOKEN_VALUE" STUB_LD=/usr/bin/x86_64-linux-gnu-ld STUB_GH_MODE=fail STUB_WGET_MODE=fail
check "mold both-down: the step fails" exit_is_not 0
check "mold both-down: ld is left alone" not_called ln

mold_case linked STUB_LD=/usr/local/bin/mold STUB_WGET_MODE=serve STUB_WGET_SERVE="$TEST_ROOT/mold.tar.gz"
check "mold linked: exit 0" exit_is 0
check "mold linked: an ld that already is mold is left alone" not_called ln

mold_case root STUB_WHOAMI=root STUB_LD=/usr/bin/x86_64-linux-gnu-ld GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/mold.tar.gz"
check "mold root: exit 0" exit_is 0
check "mold root: root unpacks without sudo" not_called sudo
check "mold root: and links ld" links_ld

mold_case runner STUB_LD=/usr/bin/x86_64-linux-gnu-ld GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/mold.tar.gz"
check "mold runner: another user unpacks and links through sudo" calls_are sudo 2

# --- every wasm-pack install ----------------------------------------------
wasm_count="$(cat "$TEST_ROOT/wasm-count")"
case_dir="$TEST_ROOT/structure"
mkdir -p "$case_dir"
echo "-" > "$case_dir/rc"
: > "$case_dir/stdout"
: > "$case_dir/stderr"
has_wasm_steps() { (( wasm_count > 0 )); }
check "the workflows install wasm-pack" has_wasm_steps
wasm_archive="wasm-pack-v0.13.1-x86_64-unknown-linux-musl.tar.gz"
for ((n = 1; n <= wasm_count; n++)); do
  place="$(cat "$TEST_ROOT/wasm-$n.place")"
  token_is_the_jobs() { [[ "$(cat "$TEST_ROOT/wasm-$n.token")" == "$JOB_TOKEN" ]]; }
  check "wasm-pack ($place): GH_TOKEN is the job's own token" token_is_the_jobs
  run_case "wasm-$n-api" "$STUB_RM:$STUB_PATH" "$TEST_ROOT/wasm-$n.sh" \
    GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=ok STUB_CURL_MODE=fail
  check "wasm-pack ($place) api: exit 0" exit_is 0
  check "wasm-pack ($place) api: gh with the job token" log_is gh \
    "release download v0.13.1 --repo wasm-bindgen/wasm-pack --pattern $wasm_archive --output /tmp/$wasm_archive --clobber token=set"
  check "wasm-pack ($place) api: the anonymous URL is not tried" not_called curl
  check "wasm-pack ($place) api: nothing after the download saw the token" no_tool_saw_token
  run_case "wasm-$n-down" "$STUB_RM:$STUB_PATH" "$TEST_ROOT/wasm-$n.sh" \
    GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=fail STUB_CURL_MODE=ok
  check "wasm-pack ($place) api-down: exit 0" exit_is 0
  check "wasm-pack ($place) api-down: the anonymous URL as before, without the token" log_is curl \
    "--fail --location --retry 5 --retry-delay 2 https://github.com/wasm-bindgen/wasm-pack/releases/download/v0.13.1/$wasm_archive --output /tmp/$wasm_archive token="
  check "wasm-pack ($place) api-down: nothing after the download saw the token" no_tool_saw_token
  run_case "wasm-$n-no-token" "$STUB_RM:$STUB_PATH" "$TEST_ROOT/wasm-$n.sh" STUB_GH_MODE=ok STUB_CURL_MODE=ok
  check "wasm-pack ($place) no-token: gh is not called" not_called gh
  check "wasm-pack ($place) no-token: the anonymous URL is" calls_are curl 1
done

# --- setup-cargo-tool-ci: Install the tool ----------------------------------
# The real tar, sha256sum and install, on stand-in archives laid out like the
# releases (cargo-nextest at the root, cargo-deny under its versioned
# directory) whose binaries record how they ran; stubs for gh, curl and cargo.
nextest_version="$(cat "$TEST_ROOT/cargo-tool-NEXTEST_VERSION")"
deny_version="$(cat "$TEST_ROOT/cargo-tool-CARGO_DENY_VERSION")"
nextest_asset="cargo-nextest-${nextest_version}-x86_64-unknown-linux-gnu.tar.gz"
deny_asset="cargo-deny-${deny_version}-x86_64-unknown-linux-musl.tar.gz"
deny_member="cargo-deny-${deny_version}-x86_64-unknown-linux-musl/cargo-deny"
TOOL_STUBS="$TEST_ROOT/tool-stubs"
mkdir -p "$TOOL_STUBS"
cp "$STUB_BIN/gh" "$STUB_BIN/curl" "$STUB_BIN/sleep" "$TOOL_STUBS/"
make_tool_archive() {
  local label="$1" member="$2"
  local src="$TEST_ROOT/tool-src-$label"
  mkdir -p "$src/$(dirname "$member")"
  cat > "$src/$member" <<'EOF'
#!/usr/bin/env bash
echo "$0 $* token=${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/ran.log"
EOF
  printf 'echo "stand-in %s"\n' "$label" >> "$src/$member"
  chmod +x "$src/$member"
  if [[ "$member" == */* ]]; then printf 'readme\n' > "$src/${member%/*}/README.md"; fi
  tar -czf "$TEST_ROOT/$label.tar.gz" -C "$src" "${member%%/*}"
}
make_tool_archive nextest cargo-nextest
make_tool_archive nextest-tampered cargo-nextest
make_tool_archive deny "$deny_member"
make_tool_archive deny-tampered "$deny_member"
nextest_sha="$(sha256sum "$TEST_ROOT/nextest.tar.gz" | cut -d' ' -f1)"
deny_sha="$(sha256sum "$TEST_ROOT/deny.tar.gz" | cut -d' ' -f1)"

# <label> <tool> <toolchain|cargo-home: where the cargo on PATH lives> [VAR=value...]
tool_case() {
  local label="$1" tool="$2" cargo_at="$3" dir cargo_dir
  shift 3
  dir="$TEST_ROOT/case-tool-$label"
  if [[ "$cargo_at" == cargo-home ]]; then cargo_dir="$dir/cargo-home/bin"; else cargo_dir="$dir/toolchain/bin"; fi
  mkdir -p "$cargo_dir"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$cargo_dir/cargo"
  chmod +x "$cargo_dir/cargo"
  run_case "tool-$label" "$TOOL_STUBS:$cargo_dir:$PATH" "$TEST_ROOT/cargo-tool.sh" \
    GITHUB_ACTION_PATH="$REPO_ROOT/.github/actions/setup-cargo-tool-ci" RUNNER_OS=Linux RUNNER_ARCH=X64 \
    CARGO_HOME="$dir/cargo-home" TOOL="$tool" \
    NEXTEST_VERSION="$nextest_version" NEXTEST_SHA256="$nextest_sha" \
    CARGO_DENY_VERSION="$deny_version" CARGO_DENY_SHA256="$deny_sha" "$@"
}
work_of() { printf '%s/runner-temp/setup-cargo-tool-ci/%s' "$case_dir" "$1"; }
# The directory install-action used when the cargo on PATH is not in $CARGO_HOME/bin.
action_bin() { printf '%s/home/.install-action/bin' "$case_dir"; }
nextest_gh_args() {
  log_is gh "release download cargo-nextest-${nextest_version} --repo nextest-rs/nextest --pattern ${nextest_asset} --output $(work_of nextest)/${nextest_asset} --clobber token=set"
}
nextest_curl_args() {
  log_is curl "--proto =https --tlsv1.2 -fsSL --retry 10 -o $(work_of nextest)/${nextest_asset} https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${nextest_version}/${nextest_asset} token="
}
deny_gh_args() {
  log_is gh "release download ${deny_version} --repo EmbarkStudios/cargo-deny --pattern ${deny_asset} --output $(work_of cargo-deny)/${deny_asset} --clobber token=set"
}
deny_curl_args() {
  log_is curl "--proto =https --tlsv1.2 -fsSL --retry 10 -o $(work_of cargo-deny)/${deny_asset} https://github.com/EmbarkStudios/cargo-deny/releases/download/${deny_version}/${deny_asset} token="
}
installed_in() { [[ -x "$1/$2" ]] && cmp -s "$1/$2" "$TEST_ROOT/tool-src-$3/$4"; }
path_is() { [[ "$(cat "$case_dir/github-path")" == "$1" ]]; }
ran_version() { log_is ran "$1/$2 $3 --version token="; }
nothing_unpacked() { [[ ! -e "$(work_of "$1")/$2" ]]; }
no_tool_binary() { [[ ! -e "$(action_bin)/$1" && ! -e "$case_dir/cargo-home/bin/$1" ]]; }
tool_installed() {
  local label="$1" dir="$2" bin="$3" src="$4" member="$5"
  check "$label: exit 0" exit_is 0
  check "$label: the pinned archive's binary is installed in $(basename "$(dirname "$dir")")/$(basename "$dir")" installed_in "$dir" "$bin" "$src" "$member"
  check "$label: it ran once, for its version, without the token" ran_version "$dir" "$bin" "${bin#cargo-}"
  check "$label: nothing after the download saw the token" no_tool_saw_token
  check "$label: the token is never printed" token_never_printed
}
tool_refused() {
  local label="$1" tool="$2" member="$3" bin="$4"
  check "$label: the step fails" exit_is_not 0
  check "$label: on the sha256 check" says stdout ": FAILED"
  check "$label: nothing was unpacked" nothing_unpacked "$tool" "$member"
  check "$label: nothing was installed" no_tool_binary "$bin"
  check "$label: nothing reaches PATH" log_is_empty_path
  check "$label: no tool binary ran" not_called ran
}

tool_case nextest-api nextest toolchain GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/nextest.tar.gz" STUB_CURL_MODE=fail
check "nextest api: the release comes through gh with the job token" nextest_gh_args
check "nextest api: the anonymous URL is not tried" not_called curl
tool_installed "nextest api" "$(action_bin)" cargo-nextest nextest cargo-nextest
check "nextest api: install-action's directory reaches PATH" path_is "$(action_bin)"

tool_case nextest-api-down nextest toolchain GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=fail STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/nextest.tar.gz"
check "nextest api-down: gh is tried three times" calls_are gh 3
check "nextest api-down: then install-action's anonymous curl, without the token" nextest_curl_args
tool_installed "nextest api-down" "$(action_bin)" cargo-nextest nextest cargo-nextest

tool_case nextest-no-token nextest toolchain \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/nextest.tar.gz" STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/nextest.tar.gz"
check "nextest no-token: gh is not called" not_called gh
check "nextest no-token: the anonymous URL is" nextest_curl_args
tool_installed "nextest no-token" "$(action_bin)" cargo-nextest nextest cargo-nextest

tool_case nextest-api-tampered nextest toolchain GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/nextest-tampered.tar.gz" \
  STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/nextest.tar.gz"
check "nextest api-tampered: the anonymous URL is not tried after a mismatch" not_called curl
tool_refused "nextest api-tampered" nextest cargo-nextest cargo-nextest

tool_case nextest-curl-tampered nextest toolchain \
  STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/nextest-tampered.tar.gz"
tool_refused "nextest curl-tampered" nextest cargo-nextest cargo-nextest

tool_case nextest-both-down nextest toolchain GH_TOKEN="$TOKEN_VALUE" STUB_GH_MODE=fail STUB_CURL_MODE=fail
check "nextest both-down: the step fails" exit_is_not 0
check "nextest both-down: nothing was installed" no_tool_binary cargo-nextest
check "nextest both-down: nothing reaches PATH" log_is_empty_path

tool_case nextest-cargo-home nextest cargo-home GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/nextest.tar.gz"
tool_installed "nextest cargo-home" "$case_dir/cargo-home/bin" cargo-nextest nextest cargo-nextest
check "nextest cargo-home: \$CARGO_HOME/bin is left to PATH as it is" log_is_empty_path
check "nextest cargo-home: install-action's own directory is not used" absent "$(action_bin)/cargo-nextest"

tool_case deny-api cargo-deny toolchain GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/deny.tar.gz" STUB_CURL_MODE=fail
check "cargo-deny api: the release comes through gh with the job token" deny_gh_args
check "cargo-deny api: the anonymous URL is not tried" not_called curl
tool_installed "cargo-deny api" "$(action_bin)" cargo-deny deny "$deny_member"
check "cargo-deny api: install-action's directory reaches PATH" path_is "$(action_bin)"

tool_case deny-api-down cargo-deny toolchain GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=fail STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/deny.tar.gz"
check "cargo-deny api-down: then install-action's anonymous curl, without the token" deny_curl_args
tool_installed "cargo-deny api-down" "$(action_bin)" cargo-deny deny "$deny_member"

tool_case deny-api-tampered cargo-deny toolchain GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/deny-tampered.tar.gz" \
  STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/deny.tar.gz"
check "cargo-deny api-tampered: the anonymous URL is not tried after a mismatch" not_called curl
tool_refused "cargo-deny api-tampered" cargo-deny "$deny_member" cargo-deny

tool_case unknown-tool cargo-audit toolchain GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/nextest.tar.gz"
check "unknown tool: the step fails" exit_is_not 0
check "unknown tool: says which tools it installs" says stderr "installs nextest or cargo-deny, not 'cargo-audit'"
check "unknown tool: gh is not called" not_called gh

tool_case other-runner nextest toolchain RUNNER_OS=macOS RUNNER_ARCH=ARM64 GH_TOKEN="$TOKEN_VALUE" \
  STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/nextest.tar.gz"
check "other runner: the step fails" exit_is_not 0
check "other runner: says it pins Linux x86_64 builds only" says stderr "pins Linux x86_64 builds only, not macOS/ARM64"
check "other runner: gh is not called" not_called gh

# --- the steps' own wiring ----------------------------------------------------
case_dir="$TEST_ROOT/structure"
tlc_token_is_the_jobs() { [[ "$(cat "$TEST_ROOT/tlc-token")" == "$JOB_TOKEN" ]]; }
check "setup-tlc-ci's GH_TOKEN is the job's own token" tlc_token_is_the_jobs
mold_token_is_the_jobs() { [[ "$(cat "$TEST_ROOT/mold-token")" == "$JOB_TOKEN" ]]; }
check "setup-mold-ci's GH_TOKEN is the job's own token" mold_token_is_the_jobs
tool_env_is() { [[ "$(cat "$TEST_ROOT/cargo-tool-$1")" =~ $2 ]]; }
# shellcheck disable=SC2016 # the expressions as the action writes them
check "setup-cargo-tool-ci's GH_TOKEN is the job's own token" tool_env_is GH_TOKEN '^\$\{\{ github\.token \}\}$'
# shellcheck disable=SC2016
check "setup-cargo-tool-ci's TOOL is its tool input" tool_env_is TOOL '^\$\{\{ inputs\.tool \}\}$'
check "setup-cargo-tool-ci pins an exact nextest release" tool_env_is NEXTEST_VERSION '^[0-9]+\.[0-9]+\.[0-9]+$'
check "setup-cargo-tool-ci pins nextest's sha256" tool_env_is NEXTEST_SHA256 '^[0-9a-f]{64}$'
check "setup-cargo-tool-ci pins an exact cargo-deny release" tool_env_is CARGO_DENY_VERSION '^[0-9]+\.[0-9]+\.[0-9]+$'
check "setup-cargo-tool-ci pins cargo-deny's sha256" tool_env_is CARGO_DENY_SHA256 '^[0-9a-f]{64}$'
installs_through_tool_action() { (( $(cat "$TEST_ROOT/cargo-tool-uses") > 0 )); }
check "the workflows install their cargo tools through setup-cargo-tool-ci" installs_through_tool_action
no_guard_problems() {
  if [[ -s "$TEST_ROOT/guard-problems" ]]; then
    sed 's/^/    /' "$TEST_ROOT/guard-problems" >&2
    return 1
  fi
}
check "every CI release download tries the API first; none uses rui314/setup-mold or taiki-e/install-action" no_guard_problems

if (( failures > 0 )); then
  printf '%d CI release download check(s) failed\n' "$failures" >&2
  exit 1
fi
printf 'CI release downloads: all checks passed\n'
