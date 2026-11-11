#!/usr/bin/env bash
# Contract test for scripts/install-ripgrep.sh, the step that puts ripgrep on
# PATH for the tombstone scans, run as written (only its two pinned sha256
# values are swapped for the digests of stand-in archives) against a stub gh,
# curl, sudo and apt-get. No network, and nothing is installed on the host:
# - with rg on PATH it prints the version and fetches nothing;
# - otherwise the pinned release archive for the machine (x86_64 or aarch64
#   Linux, the musl build) comes through the GitHub API with the job's token
#   first, and from the anonymous release URL when there is no token or that
#   fails; the token is unset before the installed binary runs; the binary
#   lands in <dir>/bin, which is added to GITHUB_PATH, and runs once for its
#   version;
# - a download that fails both ways falls back to the Ubuntu package
#   (apt-get update, apt-get install -y ripgrep) and says so;
# - an archive whose sha256 does not match the pin is deleted, never unpacked
#   or run, and the step falls back to the Ubuntu package and says so, with
#   both digests;
# - when the fallback fails too, the step fails;
# - a machine with no pinned build is a clear error: nothing is downloaded
#   and nothing is installed;
# - the two workflow steps call the script with the job's own token, before
#   the scans, and no step installs ripgrep from apt on its own.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${PYTHON:-python3}"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/install-ripgrep.XXXXXX")"
trap '[[ -n "${KEEP_TEST_ROOT:-}" ]] || rm -rf "$TEST_ROOT"' EXIT
INSTALLER="$REPO_ROOT/scripts/install-ripgrep.sh"

version="$(sed -n 's/^RIPGREP_VERSION="\(.*\)"$/\1/p' "$INSTALLER")"
if [[ -z "$version" ]]; then
  echo "scripts/install-ripgrep.sh no longer pins RIPGREP_VERSION on a line of its own" >&2
  exit 1
fi
for pin in RIPGREP_SHA256_X86_64 RIPGREP_SHA256_AARCH64; do
  if ! grep -Eq "^${pin}=\"[0-9a-f]{64}\"\$" "$INSTALLER"; then
    echo "scripts/install-ripgrep.sh no longer pins ${pin} as a 64-digit sha256 on a line of its own" >&2
    exit 1
  fi
done
x86_name="ripgrep-${version}-x86_64-unknown-linux-musl"
arm_name="ripgrep-${version}-aarch64-unknown-linux-musl"
release_base="https://github.com/BurntSushi/ripgrep/releases/download/${version}"

# Stand-in release archives laid out like the real ones. Each binary records
# that it ran, and whether it saw a GH_TOKEN.
make_archive() {
  local label="$1" name="$2" src="$TEST_ROOT/src-$1/$2"
  mkdir -p "$src/doc"
  # shellcheck disable=SC2016  # the stand-in expands these when it runs
  printf '#!/usr/bin/env bash\necho "%s $* token=${GH_TOKEN:+set}" >> "$RG_TEST_RAN"\necho "ripgrep 0.0.0-%s"\n' \
    "$label" "$label" > "$src/rg"
  chmod +x "$src/rg"
  echo "stand-in" > "$src/doc/rg.1"
  tar -czf "$TEST_ROOT/$label.tar.gz" -C "$TEST_ROOT/src-$label" "$name"
}
make_archive release "$x86_name"
make_archive release-arm "$arm_name"
make_archive tampered "$x86_name"
x86_pin="$(sha256sum "$TEST_ROOT/release.tar.gz" | cut -d' ' -f1)"
arm_pin="$(sha256sum "$TEST_ROOT/release-arm.tar.gz" | cut -d' ' -f1)"
tampered_sha="$(sha256sum "$TEST_ROOT/tampered.tar.gz" | cut -d' ' -f1)"

# The script as written, beside the helper it calls, with only the two pins
# swapped for the stand-ins' digests.
mkdir -p "$TEST_ROOT/scripts"
cp "$REPO_ROOT/scripts/ci-release-asset-download" "$TEST_ROOT/scripts/ci-release-asset-download"
sed -e "s/^RIPGREP_SHA256_X86_64=.*/RIPGREP_SHA256_X86_64=\"${x86_pin}\"/" \
  -e "s/^RIPGREP_SHA256_AARCH64=.*/RIPGREP_SHA256_AARCH64=\"${arm_pin}\"/" \
  "$INSTALLER" > "$TEST_ROOT/scripts/install-ripgrep.sh"
changed_lines="$(diff "$INSTALLER" "$TEST_ROOT/scripts/install-ripgrep.sh" | grep -c '^>' || true)"
if [[ "$changed_lines" != 2 ]]; then
  echo "the test copy of scripts/install-ripgrep.sh differs in ${changed_lines} lines, not in its two pins" >&2
  exit 1
fi

# The tools the script needs, and no rg: the host's own rg must not be found.
TOOLS="$TEST_ROOT/tools"
mkdir -p "$TOOLS"
for tool in bash env cat cut dirname mkdir rm sha256sum tar gzip cp chmod grep sed tail; do
  ln -s "$(command -v "$tool")" "$TOOLS/$tool"
done

# Stand-ins. Each records how it was called. gh and curl serve
# STUB_<TOOL>_SERVE to their output path or fail the way an unreachable host
# does; apt-get "installs" a stand-in rg into a directory on PATH or fails
# the way the package mirror did; sleep returns at once.
STUB_BIN="$TEST_ROOT/stub-bin"
mkdir -p "$STUB_BIN"
cat > "$STUB_BIN/uname" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  -s) echo "${STUB_UNAME_S:-Linux}" ;;
  -m) echo "${STUB_UNAME_M:-x86_64}" ;;
esac
EOF
cat > "$STUB_BIN/id" <<'EOF'
#!/usr/bin/env bash
echo 1000
EOF
cat > "$STUB_BIN/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$STUB_BIN/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s token=%s\n' "$*" "${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/gh.log"
out=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  if [[ "${args[i]}" == "--output" ]]; then out="${args[i + 1]}"; fi
done
case "${STUB_GH_MODE:-fail}" in
  serve) cp "$STUB_GH_SERVE" "$out" ;;
  *)
    echo "HTTP 504: Gateway Timeout (https://api.github.com/repos/BurntSushi/ripgrep/releases/tags/${args[2]})" >&2
    exit 1
    ;;
esac
EOF
cat > "$STUB_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
out=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  if [[ "${args[i]}" == "-o" ]]; then out="${args[i + 1]}"; fi
done
printf '%s token=%s\n' "${args[${#args[@]} - 1]}" "${GH_TOKEN:+set}" >> "$STUB_LOG_DIR/curl.log"
case "${STUB_CURL_MODE:-fail}" in
  serve) cp "$STUB_CURL_SERVE" "$out" ;;
  *)
    echo "curl: (6) Could not resolve host: github.com" >&2
    exit 6
    ;;
esac
EOF
cat > "$STUB_BIN/sudo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$STUB_LOG_DIR/sudo.log"
exec "$@"
EOF
cat > "$STUB_BIN/apt-get" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$STUB_LOG_DIR/apt-get.log"
if [[ "${STUB_APT_MODE:-ok}" == fail ]]; then
  echo "E: Failed to fetch http://azure.archive.ubuntu.com/ubuntu/dists/noble/InRelease  Temporary failure resolving 'azure.archive.ubuntu.com'" >&2
  exit 100
fi
if [[ "$1" == install ]]; then
  printf '#!/usr/bin/env bash\necho "apt $* token=${GH_TOKEN:+set}" >> "$RG_TEST_RAN"\necho "ripgrep 0.0.0-apt"\n' > "$STUB_APT_BIN/rg"
  chmod +x "$STUB_APT_BIN/rg"
fi
EOF
chmod +x "$STUB_BIN"/*
PRESENT_BIN="$TEST_ROOT/present-bin"
mkdir -p "$PRESENT_BIN"
# shellcheck disable=SC2016  # the stand-in expands these when it runs
printf '#!/usr/bin/env bash\necho "present $* token=${GH_TOKEN:+set}" >> "$RG_TEST_RAN"\necho "ripgrep 0.0.0-present"\n' > "$PRESENT_BIN/rg"
chmod +x "$PRESENT_BIN/rg"

failures=0
case_dir=""
install_dir=""

# run_case <label> <extra PATH entry or -> <script argument or -> [VAR=value...]
# runs the script the way a workflow step runs it: the step's env plus the
# runner's files. Without a script argument the directory is RUNNER_TEMP's.
run_case() {
  local label="$1" extra_path="$2" argument="$3"
  shift 3
  case_dir="$TEST_ROOT/case-$label"
  mkdir -p "$case_dir/runner-temp" "$case_dir/apt-bin"
  : > "$case_dir/github-path"
  local path="$STUB_BIN:$case_dir/apt-bin:$TOOLS"
  if [[ "$extra_path" != - ]]; then path="$extra_path:$path"; fi
  local arguments=()
  install_dir="$case_dir/runner-temp/ripgrep-install"
  if [[ "$argument" != - ]]; then
    arguments=(--dir "$argument")
    install_dir="$argument"
  fi
  local rc=0
  env -i PATH="$path" RUNNER_TEMP="$case_dir/runner-temp" GITHUB_PATH="$case_dir/github-path" \
    STUB_LOG_DIR="$case_dir" STUB_APT_BIN="$case_dir/apt-bin" RG_TEST_RAN="$case_dir/ran" "$@" \
    bash --noprofile --norc "$TEST_ROOT/scripts/install-ripgrep.sh" "${arguments[@]}" \
    > "$case_dir/stdout" 2> "$case_dir/stderr" || rc=$?
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
    printf '  step exit %s; stdout tail:\n' "$(cat "$case_dir/rc")" >&2
    tail -n 5 "$case_dir/stdout" | sed 's/^/    /' >&2
    printf '  stderr tail:\n' >&2
    tail -n 5 "$case_dir/stderr" | sed 's/^/    /' >&2
  fi
}

exit_is() { [[ "$(cat "$case_dir/rc")" == "$1" ]]; }
exit_is_not() { [[ "$(cat "$case_dir/rc")" != "$1" ]]; }
has_line() { grep -Fxq -- "$2" "$case_dir/$1"; }
last_line_is() { [[ "$(tail -n 1 "$case_dir/$1")" == "$2" ]]; }
is_empty() { [[ ! -s "$case_dir/$1" ]]; }
not_called() { [[ ! -e "$case_dir/$1.log" ]]; }
log_is() { [[ "$(cat "$case_dir/$1.log" 2> /dev/null)" == "$2" ]]; }
ran_is() { [[ "$(cat "$case_dir/ran" 2> /dev/null)" == "$1" ]]; }
never_ran() { [[ ! -e "$case_dir/ran" ]]; }
no_binary() { [[ ! -e "$install_dir/bin/rg" ]]; }
no_archive_left() { [[ -z "$(find "$case_dir" -name 'ripgrep-*.tar.gz' -print -quit)" ]]; }
no_warning() { ! grep -q '^::warning::' "$case_dir/stdout"; }
no_source_line() { ! grep -q '^ripgrep source:' "$case_dir/stdout"; }
asked_gh_for() {
  log_is gh "release download ${version} --repo BurntSushi/ripgrep --pattern $1.tar.gz --output $install_dir/$1.tar.gz --clobber token=set"
}
asked_gh_three_times() { [[ "$(grep -c . "$case_dir/gh.log")" == 3 ]]; }
asked_curl_for() { log_is curl "${release_base}/$1.tar.gz token="; }
apt_ran() { log_is apt-get $'update\ninstall -y ripgrep' && log_is sudo $'apt-get update\napt-get install -y ripgrep'; }
warns() {
  has_line stdout "::warning::ripgrep ${version} was not installed from its GitHub release ($1); falling back to the Ubuntu package (apt-get install ripgrep)"
}
installed_from_release() {
  local label="$1" route="$2" binary="$3"
  check "$label: the step succeeds" exit_is 0
  check "$label: its bin directory reaches PATH for the later steps" has_line github-path "$install_dir/bin"
  check "$label: the verified binary ran once, for its version, without the token" ran_is "$binary --version token="
  check "$label: it says which source it used" last_line_is stdout "ripgrep source: release ($route)"
  check "$label: no warning" no_warning
  check "$label: the Ubuntu package is not touched" not_called apt-get
}
installed_from_apt() {
  local label="$1"
  check "$label: the step succeeds" exit_is 0
  check "$label: apt-get update, then apt-get install -y ripgrep, through sudo" apt_ran
  check "$label: the packaged rg ran once, for its version, without the token" ran_is "apt --version token="
  check "$label: it says which source it used" last_line_is stdout "ripgrep source: apt"
  check "$label: nothing reaches GITHUB_PATH" is_empty github-path
  check "$label: nothing was unpacked" no_binary
  check "$label: no archive is left" no_archive_left
}

run_case present "$PRESENT_BIN" - GH_TOKEN=job-token
check "present: the step succeeds" exit_is 0
check "present: rg ran once, for its version" ran_is "present --version token=set"
check "present: it prints the version" has_line stdout "ripgrep 0.0.0-present"
check "present: it says which source it used" last_line_is stdout "ripgrep source: path"
check "present: the API is not asked" not_called gh
check "present: the release URL is not asked" not_called curl
check "present: the Ubuntu package is not touched" not_called apt-get
check "present: nothing reaches GITHUB_PATH" is_empty github-path
check "present: no install directory is made" no_binary

run_case api - - GH_TOKEN=job-token STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/release.tar.gz"
check "api: the archive comes through the API with the job token" asked_gh_for "$x86_name"
check "api: the anonymous URL is not tried" not_called curl
installed_from_release api api release

run_case api-fails - - GH_TOKEN=job-token STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/release.tar.gz"
check "api-fails: the API was tried three times first" asked_gh_three_times
check "api-fails: then the anonymous release URL, without the token" asked_curl_for "$x86_name"
installed_from_release api-fails url release

run_case no-token - "$TEST_ROOT/case-no-token/custom dir" STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/release.tar.gz"
check "no-token: the API is not asked without a token" not_called gh
check "no-token: the anonymous release URL is" asked_curl_for "$x86_name"
check "no-token: the binary lands in the caller's directory" test -x "$TEST_ROOT/case-no-token/custom dir/bin/rg"
installed_from_release no-token url release

run_case aarch64 - - STUB_UNAME_M=aarch64 STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/release-arm.tar.gz"
check "aarch64: the aarch64 musl archive is the one asked for" asked_curl_for "$arm_name"
installed_from_release aarch64 url release-arm

run_case download-fails - - GH_TOKEN=job-token
check "download-fails: the API was tried first" asked_gh_three_times
check "download-fails: then the anonymous release URL" asked_curl_for "$x86_name"
check "download-fails: the warning names the cause and the fallback" \
  warns "download failed: curl exit 6 for ${release_base}/${x86_name}.tar.gz"
installed_from_apt download-fails

run_case mismatch - - STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/tampered.tar.gz"
check "mismatch: the warning names both digests and the fallback" \
  warns "sha256 mismatch for ${x86_name}.tar.gz (url): pinned ${x86_pin}, downloaded ${tampered_sha}; the archive was deleted and not unpacked"
installed_from_apt mismatch

run_case mismatch-api - - GH_TOKEN=job-token STUB_GH_MODE=serve STUB_GH_SERVE="$TEST_ROOT/tampered.tar.gz"
check "mismatch-api: the warning names both digests and the fallback" \
  warns "sha256 mismatch for ${x86_name}.tar.gz (api): pinned ${x86_pin}, downloaded ${tampered_sha}; the archive was deleted and not unpacked"
check "mismatch-api: the anonymous URL is not tried" not_called curl
installed_from_apt mismatch-api

run_case both-fail - - STUB_APT_MODE=fail
check "both-fail: the step fails when the fallback fails too" exit_is 100
check "both-fail: the fallback was announced" \
  warns "download failed: curl exit 6 for ${release_base}/${x86_name}.tar.gz"
check "both-fail: no source is reported" no_source_line
check "both-fail: no rg ran" never_ran
check "both-fail: nothing reaches GITHUB_PATH" is_empty github-path

run_case unknown-arch - - GH_TOKEN=job-token STUB_UNAME_M=riscv64 STUB_CURL_MODE=serve STUB_CURL_SERVE="$TEST_ROOT/release.tar.gz"
check "unknown-arch: the step fails" exit_is 1
check "unknown-arch: the error names the machine and the pinned builds" has_line stderr \
  "::error::install-ripgrep: no pinned ripgrep ${version} build for Linux/riscv64 (pinned: Linux x86_64 and Linux aarch64); install ripgrep on this machine so that rg is on PATH"
check "unknown-arch: the API is not asked" not_called gh
check "unknown-arch: the release URL is not asked" not_called curl
check "unknown-arch: the Ubuntu package is not touched" not_called apt-get
check "unknown-arch: no rg ran" never_ran
check "unknown-arch: nothing reaches GITHUB_PATH" is_empty github-path

run_case unknown-os - - STUB_UNAME_S=Darwin STUB_UNAME_M=arm64
check "unknown-os: the step fails" exit_is 1
check "unknown-os: the error names the machine" has_line stderr \
  "::error::install-ripgrep: no pinned ripgrep ${version} build for Darwin/arm64 (pinned: Linux x86_64 and Linux aarch64); install ripgrep on this machine so that rg is on PATH"
check "unknown-os: the release URL is not asked" not_called curl

# The two workflow steps, read through the release contract's own workflow
# reader: each calls the script with the job's own token before the scans it
# serves, and no step of either workflow installs ripgrep from apt itself.
case_dir="$TEST_ROOT/structure"
mkdir -p "$case_dir"
echo "-" > "$case_dir/rc"
: > "$case_dir/stderr"
"$PYTHON" - "$REPO_ROOT" > "$case_dir/stdout" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
sys.path.insert(0, str(root / "scripts"))
import check_release_workflow_contract as contract

SCRIPT = "scripts/install-ripgrep.sh"
STEPS = {
    "ci.yml": (
        "Install ripgrep for the tombstone scans",
        [
            "Legacy public surface names stay retired",
            "Retired session-control routes stay retired",
            "Deprecated backend stays unreferenced",
        ],
    ),
    "cargo.yml": ("Install governance scan dependencies", ["Surface/backend gates"]),
}
for workflow, (install, scans) in STEPS.items():
    text = (root / ".github" / "workflows" / workflow).read_text(encoding="utf-8")
    steps = contract.job_steps(contract.job_block(text, "fmt-governance"))
    names = [step.name for step in steps]
    if install not in names:
        print(f"{workflow}: fmt-governance has no `{install}` step")
        continue
    step = steps[names.index(install)]
    if step.run.strip() != SCRIPT:
        print(f"{workflow}: `{install}` runs `{step.run.strip()}`, not `{SCRIPT}`")
    if step.env.get("GH_TOKEN", "") != "${{ github.token }}":
        print(f"{workflow}: `{install}` does not pass the job's own token as GH_TOKEN")
    for scan in scans:
        if scan not in names:
            print(f"{workflow}: fmt-governance has no `{scan}` step")
        elif names.index(scan) < names.index(install):
            print(f"{workflow}: `{scan}` runs before `{install}`")
    for line in text.splitlines():
        if "apt-get" in line and "ripgrep" in line and not line.lstrip().startswith("#"):
            print(f"{workflow}: a step installs ripgrep from apt itself: {line.strip()}")
PY
check "both workflow steps call the script with the job's token, before the scans" is_empty stdout

if ((failures > 0)); then
  printf '%d install-ripgrep check(s) failed\n' "$failures" >&2
  exit 1
fi
printf 'install-ripgrep: all checks passed\n'
