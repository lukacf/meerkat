#!/usr/bin/env bash
# Self-test for scripts/release-find-prebuild with a stubbed `gh` (no network).
#
# The lookup decides whether a tag run rebuilds every release artifact or
# publishes a pre-tag prebuild's: it must select only a prebuild that serves
# the release commit (release.yml, workflow_dispatch, main, this repository,
# a successful "Release prebuild gate", downloadable critical artifacts), as
# soon as that gate passed even while the run's trailing lanes still build,
# prefer one of exactly the release commit (waiting for one still under way)
# and then the newest, never take an earlier commit's whose trailing lanes
# failed, wait for a gate that has not concluded unless a job it needs
# already failed, and fail loudly when the API fails instead of reporting
# "no prebuild".

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOOKUP="${REPO_ROOT}/scripts/release-find-prebuild"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-find-prebuild.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

SHA="0123456789abcdef0123456789abcdef01234567"
OTHER_SHA="89abcdef0123456789abcdef0123456789abcdef"
REPO="example/meerkat"
FIXTURES="${TEST_ROOT}/fixtures"
mkdir -p "${TEST_ROOT}/bin"

# gh api [--paginate] <url> --jq <filter>: answers from
# ${FIXTURES}/<url with non-alphanumerics as _>[.<call number>].json, the
# highest-numbered variant not above this call's number, through real jq. A
# missing fixture is an API failure.
cat >"${TEST_ROOT}/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == "api" ]] || { echo "stub gh: unsupported $*" >&2; exit 2; }
shift
url="" filter="."
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --paginate) shift ;;
    --jq) filter="$2"; shift 2 ;;
    *) url="$1"; shift ;;
  esac
done
key="$(printf '%s' "${url}" | tr -c 'A-Za-z0-9' '_')"
count_file="${FIXTURES}/.calls-${key}"
calls=$(( $(cat "${count_file}" 2>/dev/null || echo 0) + 1 ))
echo "${calls}" >"${count_file}"
fixture=""
for ((n = calls; n >= 1; n--)); do
  if [[ -f "${FIXTURES}/${key}.${n}.json" ]]; then
    fixture="${FIXTURES}/${key}.${n}.json"
    break
  fi
done
[[ -n "${fixture}" || ! -f "${FIXTURES}/${key}.json" ]] || fixture="${FIXTURES}/${key}.json"
[[ -n "${fixture}" ]] || { echo "stub gh: HTTP 502 for ${url}" >&2; exit 1; }
jq -r "${filter}" "${fixture}"
STUB
chmod +x "${TEST_ROOT}/bin/gh"

fail() {
  echo "release-find-prebuild contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  ${extra}" >&2
  done
  exit 1
}

key_for() {
  printf '%s' "$1" | tr -c 'A-Za-z0-9' '_'
}

runs_url="repos/${REPO}/actions/workflows/release.yml/runs?event=workflow_dispatch&branch=main&head_sha=${SHA}&per_page=100"
# Every recent dispatch on main: where runs of earlier commits come from.
others_url="repos/${REPO}/actions/workflows/release.yml/runs?event=workflow_dispatch&branch=main&per_page=100"

# listing <url> <run json>...
listing() {
  local url="$1"
  shift
  local joined
  joined="$(IFS=,; printf '%s' "$*")"
  printf '{"workflow_runs": [%s]}\n' "${joined}" >"${FIXTURES}/$(key_for "${url}").json"
}

reset() {
  rm -rf "${FIXTURES}"
  mkdir -p "${FIXTURES}"
  listing "${others_url}"
}

# run_json <id> <status> [path] [event] [branch] [sha] [repo] [started] [actor]
# (attempt 2, triggered by the actor, default the repository owner "example")
run_json() {
  local started="${8:-$(date -u -d '-1 minutes' +%Y-%m-%dT%H:%M:%SZ)}"
  local actor="${9:-example}"
  printf '{"id": %s, "status": "%s", "path": "%s", "event": "%s", "head_branch": "%s", "head_sha": "%s", "repository": {"full_name": "%s", "owner": {"login": "example"}}, "run_started_at": "%s", "created_at": "%s", "run_attempt": 2, "actor": {"login": "%s"}, "triggering_actor": {"login": "%s"}}' \
    "$1" "$2" "${3:-.github/workflows/release.yml}" "${4:-workflow_dispatch}" "${5:-main}" \
    "${6:-${SHA}}" "${7:-${REPO}}" "${started}" "${started}" "${actor}" "${actor}"
}

# runs <variant suffix or ""> <run json>...
runs() {
  local suffix="$1"
  shift
  local joined
  joined="$(IFS=,; printf '%s' "$*")"
  printf '{"workflow_runs": [%s]}\n' "${joined}" >"${FIXTURES}/$(key_for "${runs_url}")${suffix}.json"
}

# jobs <run id> <prebuild gate conclusion, "pending" or "absent"> [variant suffix]
#      [another job's name and conclusion, as JSON members]
jobs() {
  local gate=""
  case "$2" in
    absent) ;;
    pending) gate=', {"name": "Release prebuild gate", "conclusion": null}' ;;
    *) gate=", {\"name\": \"Release prebuild gate\", \"conclusion\": \"$2\"}" ;;
  esac
  printf '{"jobs": [{"name": "Release binary build gate", "conclusion": null}%s%s]}\n' "${gate}" "${4:+, {$4\}}" \
    >"${FIXTURES}/$(key_for "repos/${REPO}/actions/runs/$1/jobs?per_page=100")${3:-}.json"
}

# artifacts <run id> <name:expired>...
artifacts() {
  local run_id="$1"
  shift
  local entries=() entry
  for entry in "$@"; do
    entries+=("{\"name\": \"${entry%%:*}\", \"expired\": ${entry##*:}}")
  done
  local joined
  joined="$(IFS=,; printf '%s' "${entries[*]}")"
  printf '{"artifacts": [%s]}\n' "${joined}" \
    >"${FIXTURES}/$(key_for "repos/${REPO}/actions/runs/${run_id}/artifacts?per_page=100").json"
}

complete_artifacts() {
  artifacts "$1" \
    "meerkat-x86_64-unknown-linux-gnu-buildbuddy:false" \
    "meerkat-aarch64-unknown-linux-gnu-buildbuddy:false" \
    "meerkat-aarch64-apple-darwin-buildbuddy:false" \
    "meerkat-x86_64-apple-darwin-buildbuddy:false" \
    "meerkat-x86_64-pc-windows-msvc-windows-latest:false" \
    "rkat-web-package:false" \
    "buildbuddy-release-validate-1-1:true"
}

# lookup <label> <expected stdout> <expected status>
lookup() {
  local label="$1" expected="$2" expected_status="$3" output status
  set +e
  output="$(PATH="${TEST_ROOT}/bin:${PATH}" FIXTURES="${FIXTURES}" GITHUB_REPOSITORY="${REPO}" \
    MEERKAT_PREBUILD_POLL_SECONDS=0 \
    MEERKAT_PREBUILD_WAIT_SECONDS="$([[ "${WAIT_SECONDS:-}" == default ]] || echo "${WAIT_SECONDS:-600}")" \
    "${LOOKUP}" "${SHA}" 2>"${TEST_ROOT}/${label}.err")"
  status=$?
  set -e
  if [[ "${status}" -ne "${expected_status}" || "${output}" != "${expected}" ]]; then
    fail "${label}: expected status ${expected_status} and output '${expected}', got ${status} and '${output}'" \
      "$(cat "${TEST_ROOT}/${label}.err")"
  fi
  echo "ok: ${label}"
}

export FIXTURES

# 1. No release.yml dispatch on main at the commit: no prebuild.
reset
runs ""
lookup no-runs "" 0

# 2. A complete prebuild of the commit is selected.
reset
runs "" "$(run_json 101 completed)"
jobs 101 success
complete_artifacts 101
lookup complete-prebuild "101 2" 0

# 3. A dispatch without the prebuild gate (an asset or package recovery run
#    on main at the same commit) is not a prebuild.
reset
runs "" "$(run_json 102 completed)"
jobs 102 absent
complete_artifacts 102
lookup recovery-run "" 0

# 4. A prebuild whose gate failed is not reused.
reset
runs "" "$(run_json 103 completed)"
jobs 103 failure
complete_artifacts 103
lookup failed-gate "" 0

# 5. An expired archive, a missing critical target or a missing Web SDK
#    package is not reusable; the trailing Intel macOS and Windows archives
#    are not required.
reset
runs "" "$(run_json 104 completed)"
jobs 104 success
critical_only=(
  "meerkat-x86_64-unknown-linux-gnu-buildbuddy:false"
  "meerkat-aarch64-unknown-linux-gnu-buildbuddy:false"
  "meerkat-aarch64-apple-darwin-buildbuddy:false"
)
artifacts 104 "${critical_only[@]}" "meerkat-x86_64-apple-darwin-macos-15-rkat:true" "rkat-web-package:false"
lookup expired-archive "" 0
artifacts 104 "meerkat-x86_64-unknown-linux-gnu-buildbuddy:false" \
  "meerkat-aarch64-apple-darwin-buildbuddy:false" "rkat-web-package:false"
lookup missing-critical "" 0
artifacts 104 "${critical_only[@]}"
lookup missing-web-sdk "" 0
artifacts 104 "${critical_only[@]}" "rkat-web-package:false"
lookup trailing-not-required "104 2" 0

# 6. Runs the API returns that are not exactly this commit's release.yml
#    dispatch on main in this repository are ignored, whatever their jobs.
reset
runs "" \
  "$(run_json 201 completed .github/workflows/ci.yml)" \
  "$(run_json 202 completed .github/workflows/release.yml push)" \
  "$(run_json 203 completed .github/workflows/release.yml workflow_dispatch release/v0.0.0)" \
  "$(run_json 204 completed .github/workflows/release.yml workflow_dispatch main "${OTHER_SHA}")" \
  "$(run_json 205 completed .github/workflows/release.yml workflow_dispatch main "${SHA}" fork/meerkat)"
for run_id in 201 202 203 204 205; do
  jobs "${run_id}" success
  complete_artifacts "${run_id}"
done
lookup foreign-runs "" 0

# 6b. A prebuild started or triggered by anyone but the repository owner is
#     not trusted.
reset
runs "" "$(run_json 251 completed .github/workflows/release.yml workflow_dispatch main "${SHA}" "${REPO}" "" collaborator)"
jobs 251 success
complete_artifacts 251
lookup foreign-actor "" 0
MEERKAT_PREBUILD_ACTORS="example collaborator" lookup allowlisted-actor "251 2" 0

# 7. The newest complete prebuild wins.
reset
runs "" "$(run_json 301 completed)" "$(run_json 305 completed)" "$(run_json 303 completed)"
for run_id in 301 303 305; do
  jobs "${run_id}" success
  complete_artifacts "${run_id}"
done
lookup newest-prebuild "305 2" 0

# 8. A prebuild whose gate has not concluded is waited for, and selected as
#    soon as the gate passes, while its trailing lanes still run.
reset
runs "" "$(run_json 401 in_progress)"
jobs 401 absent ".1"
jobs 401 pending ".2"
jobs 401 success ".3"
complete_artifacts 401
lookup waits-for-gate "401 2" 0
calls="$(cat "${FIXTURES}/.calls-$(key_for "${runs_url}")")"
[[ "${calls}" -eq 3 ]] || fail "waits-for-gate: expected three listings while waiting, got ${calls}"
reset
runs "" "$(run_json 402 in_progress)"
jobs 402 success
artifacts 402 "${critical_only[@]}" "rkat-web-package:false"
lookup critical-ready-while-trailing-run "402 2" 0

# 8b. A gate that has not concluded is not waited for once a job it needs
#     failed, timed out or was cancelled; a trailing job's failure does not
#     stop the wait.
for ended in '"name": "Build Web SDK package", "conclusion": "failure"' \
  '"name": "Build and package surface binaries with GitHub-hosted runners (rkat, aarch64-apple-darwin)", "conclusion": "cancelled"' \
  '"name": "Release critical binary build gate", "conclusion": "timed_out"'; do
  reset
  runs "" "$(run_json 451 in_progress)"
  jobs 451 pending "" "${ended}"
  WAIT_SECONDS=120 lookup critical-job-ended "" 0
  [[ "$(cat "${FIXTURES}/.calls-$(key_for "${runs_url}")")" -eq 1 ]] ||
    fail "critical-job-ended: waited for a gate that cannot pass (${ended})"
done
reset
runs "" "$(run_json 452 in_progress)"
trailing='"name": "Cross-compile and package Windows surface binaries on Linux", "conclusion": "failure"'
jobs 452 pending ".1" "${trailing}"
jobs 452 success ".2" "${trailing}"
complete_artifacts 452
lookup trailing-job-failed "452 2" 0

# 9. A run whose gate has not concluded past the wait budget is not waited
#    for forever.
reset
runs "" "$(run_json 501 in_progress .github/workflows/release.yml workflow_dispatch main "${SHA}" "${REPO}" \
  "$(date -u -d '-3 hours' +%Y-%m-%dT%H:%M:%SZ)")"
jobs 501 pending
WAIT_SECONDS=600 lookup stale-running "" 0

# 9b. The default budget outlasts the slowest measured prebuild gate on the
#     tag's backend (v0.9.2's BuildBuddy gate concluded after 94.7 minutes,
#     past v0.9.3's former 100-minute default): a run 110 minutes old is still
#     waited for, and one 145 minutes old is not.
reset
runs "" "$(run_json 502 in_progress .github/workflows/release.yml workflow_dispatch main "${SHA}" "${REPO}" \
  "$(date -u -d '-110 minutes' +%Y-%m-%dT%H:%M:%SZ)")"
jobs 502 pending ".1"
jobs 502 success ".2"
complete_artifacts 502
WAIT_SECONDS=default lookup default-budget-waits "502 2" 0
reset
runs "" "$(run_json 503 in_progress .github/workflows/release.yml workflow_dispatch main "${SHA}" "${REPO}" \
  "$(date -u -d '-145 minutes' +%Y-%m-%dT%H:%M:%SZ)")"
jobs 503 pending
WAIT_SECONDS=default lookup default-budget-ends "" 0

# 10. An API failure fails the lookup instead of reading as "no prebuild".
reset
lookup api-failure "" 1
runs "" "$(run_json 601 completed)"
lookup jobs-api-failure "" 1
jobs 601 success
lookup artifacts-api-failure "" 1
complete_artifacts 601
rm "${FIXTURES}/$(key_for "${others_url}").json"
lookup other-runs-api-failure "" 1

# 11. A malformed commit is refused.
set +e
PATH="${TEST_ROOT}/bin:${PATH}" GITHUB_REPOSITORY="${REPO}" "${LOOKUP}" main >/dev/null 2>&1
status=$?
set -e
[[ "${status}" -eq 2 ]] || fail "malformed commit: expected status 2, got ${status}"
echo "ok: malformed-commit"

# 12. The lookup's PREBUILD_GATE_JOB and every CRITICAL_JOBS pattern name a
#     job of release.yml: a renamed gate would leave every run's gate absent,
#     so no prebuild would ever be accepted, and a renamed critical job would
#     silently stop the lookup from giving up on a prebuild whose gate can no
#     longer pass.
job_names="$(sed -n 's/^    name: //p' "${REPO_ROOT}/.github/workflows/release.yml" | sed 's/^"\(.*\)"$/\1/')"
gate_job="$(sed -n 's/^PREBUILD_GATE_JOB="\(.*\)"$/\1/p' "${LOOKUP}")"
[[ -n "${gate_job}" ]] || fail "critical-job-names: no PREBUILD_GATE_JOB in ${LOOKUP}"
grep -Fxq -- "${gate_job}" <<<"${job_names}" || fail "critical-job-names: PREBUILD_GATE_JOB '${gate_job}' names no job of release.yml"
patterns="$(sed -n '/^CRITICAL_JOBS=(/,/^)/s/^  "\(.*\)"$/\1/p' "${LOOKUP}")"
[[ -n "${patterns}" ]] || fail "critical-job-names: no CRITICAL_JOBS patterns in ${LOOKUP}"
# Every entry is read: one written with other quoting or indentation would
# otherwise skip the check below. Bash itself counts the array.
# shellcheck disable=SC2016 # expanded by the inner shell
critical_count="$(bash -c 'eval "$(sed -n "/^CRITICAL_JOBS=(/,/^)/p" "$1")" && echo "${#CRITICAL_JOBS[@]}"' _ "${LOOKUP}")"
pattern_count="$(grep -c '' <<<"${patterns}")"
[[ "${pattern_count}" == "${critical_count}" ]] || fail "critical-job-names: read ${pattern_count} of the ${critical_count:-?} CRITICAL_JOBS entries"
while IFS= read -r pattern; do
  matched=false
  while IFS= read -r job_name; do
    # shellcheck disable=SC2053 # the pattern is a glob
    if [[ "${job_name}" == ${pattern} ]]; then
      matched=true
      break
    fi
  done <<<"${job_names}"
  [[ "${matched}" == true ]] || fail "critical-job-names: CRITICAL_JOBS pattern '${pattern}' names no job of release.yml"
done <<<"${patterns}"
echo "ok: critical-job-names"

# 12b. Every TRAILING_JOBS pattern names a job of release.yml too: a renamed
#      trailing job would let the lookup take an earlier commit's prebuild
#      whose trailing lanes already failed.
trailing_patterns="$(sed -n '/^TRAILING_JOBS=(/,/^)/s/^  "\(.*\)"$/\1/p' "${LOOKUP}")"
[[ -n "${trailing_patterns}" ]] || fail "trailing-job-names: no TRAILING_JOBS patterns in ${LOOKUP}"
trailing_lines="$(sed -n '/^TRAILING_JOBS=(/,/^)/p' "${LOOKUP}" | grep -vc -e '^TRAILING_JOBS=(' -e '^)$')"
[[ "$(grep -c '' <<<"${trailing_patterns}")" == "${trailing_lines}" ]] ||
  fail "trailing-job-names: read $(grep -c '' <<<"${trailing_patterns}") of the ${trailing_lines} TRAILING_JOBS lines"
while IFS= read -r pattern; do
  matched=false
  while IFS= read -r job_name; do
    # shellcheck disable=SC2053 # the pattern is a glob
    if [[ "${job_name}" == ${pattern} ]]; then
      matched=true
      break
    fi
  done <<<"${job_names}"
  [[ "${matched}" == true ]] || fail "trailing-job-names: TRAILING_JOBS pattern '${pattern}' names no job of release.yml"
done <<<"${trailing_patterns}"
echo "ok: trailing-job-names"

# 13. A prebuild of another commit serves the release only through
#     scripts/release-prebuild-equivalent: that commit is an ancestor of the
#     release commit, and every path changed since is under another product's
#     root (not a Cargo.toml there), a product's changelog or a CI-only file.
#     A fixture history (ci/products.json is the real one) provides the
#     commits; a run of exactly the release commit still comes first.
HISTORY="${TEST_ROOT}/history"
git_history() {
  git -C "${HISTORY}" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false "$@"
}
mkdir -p "${HISTORY}/ci" "${HISTORY}/crates/core/src" "${HISTORY}/connect/crates/app/src" "${HISTORY}/scripts"
git_history init -q -b main
cp "${REPO_ROOT}/ci/products.json" "${HISTORY}/ci/products.json"
echo 'pub fn core() {}' >"${HISTORY}/crates/core/src/lib.rs"
echo '# Changelog' >"${HISTORY}/CHANGELOG.md"
printf '[package]\nname = "app"\n' >"${HISTORY}/connect/crates/app/Cargo.toml"
echo 'pub fn app() {}' >"${HISTORY}/connect/crates/app/src/lib.rs"
echo '// planner' >"${HISTORY}/scripts/ci-cargo-lanes.mjs"
git_history add -A
git_history commit -q -m base
BASE_SHA="$(git_history rev-parse HEAD)"
echo '- a fix' >>"${HISTORY}/CHANGELOG.md"
echo '- a fix' >"${HISTORY}/connect/CHANGELOG.md"
echo 'pub fn app_fix() {}' >>"${HISTORY}/connect/crates/app/src/lib.rs"
mkdir -p "${HISTORY}/meta"
echo notes >"${HISTORY}/meta/NOTES.md"
echo '// planner, fixed' >>"${HISTORY}/scripts/ci-cargo-lanes.mjs"
git_history add -A
git_history commit -q -m fix
FIX_SHA="$(git_history rev-parse HEAD)"
echo 'pub fn core_fix() {}' >>"${HISTORY}/crates/core/src/lib.rs"
git_history commit -q -am core
CORE_SHA="$(git_history rev-parse HEAD)"
git_history switch -q -c manifest "${FIX_SHA}"
echo 'version = "0.0.1"' >>"${HISTORY}/connect/crates/app/Cargo.toml"
git_history commit -q -am manifest
MANIFEST_SHA="$(git_history rev-parse HEAD)"
git_history switch -q -c products "${FIX_SHA}"
echo '{}' >"${HISTORY}/ci/products.json"
git_history commit -q -am products
PRODUCTS_SHA="$(git_history rev-parse HEAD)"
git_history switch -q -c sibling "${BASE_SHA}"
echo other >"${HISTORY}/connect/OTHER.md"
git_history add -A
git_history commit -q -m sibling
SIBLING_SHA="$(git_history rev-parse HEAD)"
# A Core file moved under a product's root, and a Core file deleted: both
# change Core's sources, so both are inputs (git diff --no-renames lists the
# old name of a rename, and a deletion).
git_history switch -q -c rename "${FIX_SHA}"
git_history mv crates/core/src/lib.rs connect/crates/app/src/moved.rs
git_history commit -q -m rename
RENAME_SHA="$(git_history rev-parse HEAD)"
git_history switch -q -c deletion "${FIX_SHA}"
git_history rm -q crates/core/src/lib.rs
git_history commit -q -m deletion
DELETION_SHA="$(git_history rev-parse HEAD)"
git_history switch -q main

# equivalent <label> <prebuild> <release> <expected status> [line the output must hold]
equivalent() {
  local label="$1" status
  set +e
  (cd "${HISTORY}" && "${REPO_ROOT}/scripts/release-prebuild-equivalent" "$2" "$3") >"${TEST_ROOT}/${label}.out" 2>&1
  status=$?
  set -e
  [[ "${status}" -eq "$4" ]] || fail "${label}: expected status $4, got ${status}" "$(cat "${TEST_ROOT}/${label}.out")"
  if [[ -n "${5:-}" ]] && ! grep -Fxq -- "$5" "${TEST_ROOT}/${label}.out"; then
    fail "${label}: no line '$5'" "$(cat "${TEST_ROOT}/${label}.out")"
  fi
  echo "ok: ${label}"
}
equivalent same-commit "${FIX_SHA}" "${FIX_SHA}" 0
for line in "  CHANGELOG.md: core's changelog" "  connect/CHANGELOG.md: connect's changelog" \
  "  connect/crates/app/src/lib.rs: under connect's root connect/" "  meta/NOTES.md: under meta's root meta/" \
  "  scripts/ci-cargo-lanes.mjs: read only by CI (the CI planner)"; do
  equivalent non-inputs-only "${BASE_SHA}" "${FIX_SHA}" 0 "${line}"
done
equivalent core-source "${BASE_SHA}" "${CORE_SHA}" 1 "  crates/core/src/lib.rs: an input"
equivalent product-manifest "${FIX_SHA}" "${MANIFEST_SHA}" 1 "  connect/crates/app/Cargo.toml: an input"
equivalent unreadable-products "${FIX_SHA}" "${PRODUCTS_SHA}" 1
grep -Fq "ci/products.json is unreadable" "${TEST_ROOT}/unreadable-products.out" ||
  fail "unreadable-products: the refusal is not named" "$(cat "${TEST_ROOT}/unreadable-products.out")"
equivalent sibling "${SIBLING_SHA}" "${FIX_SHA}" 1
equivalent descendant "${CORE_SHA}" "${FIX_SHA}" 1
grep -Fq "is not an ancestor of ${FIX_SHA}" "${TEST_ROOT}/descendant.out" ||
  fail "descendant: the refusal is not named" "$(cat "${TEST_ROOT}/descendant.out")"
equivalent unknown-commit "${SHA}" "${FIX_SHA}" 1
equivalent malformed-commit main "${FIX_SHA}" 2
equivalent rename-out-of-core "${FIX_SHA}" "${RENAME_SHA}" 1 "  crates/core/src/lib.rs: an input"
equivalent deletion "${FIX_SHA}" "${DELETION_SHA}" 1 "  crates/core/src/lib.rs: an input"

# A helper whose CI_ONLY_PATHS lists a file outside scripts/ci-* (here the
# helper itself, which the release commit could then edit to serve itself)
# refuses every prebuild of another commit.
mutated_helper="${TEST_ROOT}/release-prebuild-equivalent"
sed 's|^    "scripts/ci-cargo-lanes.mjs": "the CI planner",$|&\n    "scripts/release-prebuild-equivalent": "itself",|' \
  "${REPO_ROOT}/scripts/release-prebuild-equivalent" >"${mutated_helper}"
grep -Fq '"scripts/release-prebuild-equivalent": "itself"' "${mutated_helper}" || fail "ci-only-shape: the fixture did not apply"
chmod +x "${mutated_helper}"
set +e
(cd "${HISTORY}" && "${mutated_helper}" "${BASE_SHA}" "${FIX_SHA}") >"${TEST_ROOT}/ci-only-shape.out" 2>&1
status=$?
set -e
[[ "${status}" -eq 1 ]] || fail "ci-only-shape: expected status 1, got ${status}" "$(cat "${TEST_ROOT}/ci-only-shape.out")"
grep -Fq "CI_ONLY_PATHS lists scripts/release-prebuild-equivalent, outside scripts/ci-*" "${TEST_ROOT}/ci-only-shape.out" ||
  fail "ci-only-shape: the refusal is not named" "$(cat "${TEST_ROOT}/ci-only-shape.out")"
echo "ok: ci-only-shape"

# lookup_for <label> <release commit> <expected stdout> <expected status> [directory]
lookup_for() {
  local label="$1" release="$2" expected="$3" expected_status="$4" output status
  set +e
  output="$(cd "${5:-${HISTORY}}" && PATH="${TEST_ROOT}/bin:${PATH}" FIXTURES="${FIXTURES}" \
    GITHUB_REPOSITORY="${REPO}" MEERKAT_PREBUILD_POLL_SECONDS=0 MEERKAT_PREBUILD_WAIT_SECONDS=600 \
    "${LOOKUP}" "${release}" 2>"${TEST_ROOT}/${label}.err")"
  status=$?
  set -e
  if [[ "${status}" -ne "${expected_status}" || "${output}" != "${expected}" ]]; then
    fail "${label}: expected status ${expected_status} and output '${expected}', got ${status} and '${output}'" \
      "$(cat "${TEST_ROOT}/${label}.err")"
  fi
  echo "ok: ${label}"
}
exact_url() {
  printf 'repos/%s/actions/workflows/release.yml/runs?event=workflow_dispatch&branch=main&head_sha=%s&per_page=100' "${REPO}" "$1"
}
usable() {
  local run_id
  for run_id in "$@"; do
    jobs "${run_id}" success
    complete_artifacts "${run_id}"
  done
}

reset
listing "$(exact_url "${FIX_SHA}")"
listing "${others_url}" "$(run_json 801 completed "" "" "" "${BASE_SHA}")"
usable 801
lookup_for ancestor-serves "${FIX_SHA}" "801 2" 0
grep -Fq "prebuilt the critical release artifacts of ${BASE_SHA}, which serve ${FIX_SHA}" "${TEST_ROOT}/ancestor-serves.err" ||
  fail "ancestor-serves: the log does not name both commits" "$(cat "${TEST_ROOT}/ancestor-serves.err")"
lookup_for no-history "${FIX_SHA}" "" 0 "${TEST_ROOT}"

reset
listing "$(exact_url "${FIX_SHA}")" "$(run_json 802 completed "" "" "" "${FIX_SHA}")"
listing "${others_url}" "$(run_json 803 completed "" "" "" "${BASE_SHA}")" "$(run_json 802 completed "" "" "" "${FIX_SHA}")"
usable 802 803
lookup_for exact-commit-first "${FIX_SHA}" "802 2" 0

# An exact-commit prebuild still under way is preferred: the lookup waits for
# it rather than take an earlier commit's ready one, and takes the earlier
# one only once the exact one can no longer pass.
reset
listing "$(exact_url "${FIX_SHA}")" "$(run_json 804 in_progress "" "" "" "${FIX_SHA}")"
listing "${others_url}" "$(run_json 805 completed "" "" "" "${BASE_SHA}")" "$(run_json 804 in_progress "" "" "" "${FIX_SHA}")"
jobs 804 pending ".1"
jobs 804 success ".2"
complete_artifacts 804
usable 805
lookup_for waits-for-exact-while-ancestor-ready "${FIX_SHA}" "804 2" 0
reset
listing "$(exact_url "${FIX_SHA}")" "$(run_json 806 in_progress "" "" "" "${FIX_SHA}")"
listing "${others_url}" "$(run_json 807 completed "" "" "" "${BASE_SHA}")" "$(run_json 806 in_progress "" "" "" "${FIX_SHA}")"
jobs 806 pending "" '"name": "Build Web SDK package", "conclusion": "failure"'
usable 807
lookup_for ancestor-once-exact-cannot-pass "${FIX_SHA}" "807 2" 0

# An earlier commit's prebuild whose trailing lanes (or its full binary build
# gate) already failed is never taken: its tag run would publish the critical
# archives and then stop without the trailing ones. An older one whose
# trailing lanes did not fail still serves.
for ended in '"name": "Cross-compile and package Windows surface binaries on Linux", "conclusion": "failure"' \
  '"name": "Verify Windows binaries on a Windows runner", "conclusion": "cancelled"' \
  '"name": "Cross-compile and package Intel macOS surface binaries on Apple silicon (rkat-rpc)", "conclusion": "timed_out"' \
  '"name": "Release binary build gate", "conclusion": "failure"'; do
  reset
  listing "$(exact_url "${FIX_SHA}")"
  listing "${others_url}" "$(run_json 823 completed "" "" "" "${BASE_SHA}")"
  jobs 823 success "" "${ended}"
  complete_artifacts 823
  lookup_for ancestor-trailing-failed "${FIX_SHA}" "" 0
  grep -Fq "run 823: " "${TEST_ROOT}/ancestor-trailing-failed.err" ||
    fail "ancestor-trailing-failed: the refusal is not named (${ended})" "$(cat "${TEST_ROOT}/ancestor-trailing-failed.err")"
  listing "${others_url}" "$(run_json 823 completed "" "" "" "${BASE_SHA}")" "$(run_json 822 completed "" "" "" "${BASE_SHA}")"
  usable 822
  lookup_for older-ancestor-serves "${FIX_SHA}" "822 2" 0
done

for case in "core-change ${CORE_SHA} ${BASE_SHA} ${FIX_SHA}" "manifest-change ${MANIFEST_SHA} ${FIX_SHA}" \
  "sibling ${FIX_SHA} ${SIBLING_SHA}" "descendant ${FIX_SHA} ${CORE_SHA}"; do
  read -r label release prebuilt <<<"${case}"
  reset
  listing "$(exact_url "${release}")"
  others=()
  run_id=810
  for commit in ${prebuilt}; do
    run_id=$((run_id + 1))
    others+=("$(run_json "${run_id}" completed "" "" "" "${commit}")")
    usable "${run_id}"
  done
  listing "${others_url}" "${others[@]}"
  lookup_for "refused-${label}" "${release}" "" 0
done

echo "release-find-prebuild contract holds"
