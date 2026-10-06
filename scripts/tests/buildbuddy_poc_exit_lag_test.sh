#!/usr/bin/env bash
# Contract test for scripts/buildbuddy-bazel-poc's client exit path (#1744).
#
# A fake `bb` stands in for the BuildBuddy CLI. Like the real client it can
# leave a daemon behind (the Bazel server) that redirects stdin/stdout/stderr
# but keeps every other inherited descriptor. The script must:
#   1. return as soon as the client exits, not when that daemon exits (the
#      tee redirect on a function call leaked a pipe into the daemon and held
#      every build open for the server's 600 s idle timeout);
#   2. fail, without a retry, when the run outlasts Bazel's reported build
#      time by more than BUILDBUDDY_MAX_EXIT_LAG_SECS;
#   3. pass that case when the guard is disabled (0);
#   4. reject a malformed bound.
# The client runs under the script's env allowlist, so the fake reads its
# settings from $HOME/fake-bb.conf (HOME is on the allowlist).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="${root}/scripts/buildbuddy-bazel-poc"
work="$(mktemp -d "${TMPDIR:-/tmp}/buildbuddy-exit-lag.XXXXXX")"
daemon_pids="${work}/daemon.pids"
cleanup() {
  if [[ -f "${daemon_pids}" ]]; then
    while read -r pid; do kill "${pid}" 2>/dev/null || true; done <"${daemon_pids}"
  fi
  rm -rf "${work}"
}
trap cleanup EXIT

cat >"${work}/bb" <<'EOF'
#!/usr/bin/env bash
daemon_secs=0
tail_secs=0
# shellcheck disable=SC1091
source "${HOME}/fake-bb.conf"
echo invoked >>"${HOME}/invocations"
if (( daemon_secs > 0 )); then
  ( exec 0</dev/null 1>/dev/null 2>/dev/null; sleep "${daemon_secs}" ) &
  echo "$!" >>"${HOME}/daemon.pids"
fi
echo "INFO: Elapsed time: 1.000s, Critical Path: 0.10s" >&2
echo "INFO: Build completed successfully, 1 total action" >&2
sleep "${tail_secs}"
exit 0
EOF
chmod +x "${work}/bb"

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# run_case <daemon_secs> <tail_secs> <bound or ""> -> sets case_status,
# case_wall, case_log
run_case() {
  local daemon_secs="$1" tail_secs="$2" bound="$3"
  printf 'daemon_secs=%s\ntail_secs=%s\n' "${daemon_secs}" "${tail_secs}" >"${work}/fake-bb.conf"
  : >"${work}/invocations"
  case_log="${work}/run.log"
  local started
  started="$(date +%s)"
  set +e
  env -i PATH="${PATH}" HOME="${work}" TMPDIR="${work}" \
    BUILDBUDDY_BB="${work}/bb" \
    MEERKAT_BAZEL_BACKEND=gcp-local \
    BUILDBUDDY_BAZEL_COMMAND=security-audit-rbe \
    BUILDBUDDY_OUTPUT_BASE="${work}/output-base" \
    ${bound:+BUILDBUDDY_MAX_EXIT_LAG_SECS="${bound}"} \
    "${script}" >"${case_log}" 2>&1
  case_status=$?
  set -e
  case_wall=$(( $(date +%s) - started ))
}

# 1. A daemon that outlives the client must not hold the run open.
run_case 30 0 ""
[[ "${case_status}" -eq 0 ]] || fail "daemon case exited ${case_status}: $(cat "${case_log}")"
(( case_wall < 15 )) || fail "the run waited ${case_wall} s for the client's daemon (the #1744 stall)"

# 2. A run that outlasts Bazel's build time by more than the bound fails once.
run_case 0 5 2
[[ "${case_status}" -ne 0 ]] || fail "a ${case_wall} s run against a 1 s build passed the 2 s exit-lag bound"
grep -Fq "#1744" "${case_log}" || fail "the exit-lag failure does not name itself: $(cat "${case_log}")"
[[ "$(wc -l <"${work}/invocations")" -eq 1 ]] || fail "the exit-lag failure was retried"

# 3. The same run passes with the guard disabled.
run_case 0 5 0
[[ "${case_status}" -eq 0 ]] || fail "BUILDBUDDY_MAX_EXIT_LAG_SECS=0 did not disable the guard: $(cat "${case_log}")"

# 4. A malformed bound is refused before the client runs.
run_case 0 0 soon
[[ "${case_status}" -eq 2 ]] || fail "a malformed bound exited ${case_status}, not 2"
[[ ! -s "${work}/invocations" ]] || fail "the client ran despite a malformed bound"

if (( failures > 0 )); then
  exit 1
fi
echo "buildbuddy-bazel-poc exit contract holds"
