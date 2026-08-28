#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-status.XXXXXX")"
HARNESS_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-harness.XXXXXX")"
trap 'rm -rf "$TEST_ROOT" "$HARNESS_ROOT"' EXIT

if ! grep -Fq "deterministic-locks/\${stamp_key}.lock" "$REPO_ROOT/scripts/pre-push-unit.sh"; then
  echo "pre-push deterministic gate must lock only an identical source fingerprint" >&2
  exit 1
fi
if grep -Fq 'HOOK_CACHE_ROOT}/deterministic.lock' "$REPO_ROOT/scripts/pre-push-unit.sh"; then
  echo "pre-push deterministic gate must not serialize unrelated source fingerprints" >&2
  exit 1
fi

git -C "$TEST_ROOT" init -q
printf 'lock revision 0\n' > "$TEST_ROOT/Cargo.lock"
printf 'module lock revision 0\n' > "$TEST_ROOT/MODULE.bazel.lock"
git -C "$TEST_ROOT" add Cargo.lock MODULE.bazel.lock
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "test fixture"
test_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"

FAKE_CARGO="$HARNESS_ROOT/fake-cargo"
FAKE_GIT="$HARNESS_ROOT/fake-git"
LANE_LOG="$HARNESS_ROOT/lanes"
cat > "$FAKE_CARGO" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

args=" $* "
if [[ "${1:-}" == "metadata" ]]; then
  printf '{}\n'
  exit 0
elif [[ "$args" == *" nextest list "* && "$args" == *" --test cold_restart_mob_resume "* ]]; then
  lane="headcanonical-build"
elif [[ "$args" == *" nextest list "* && "$args" == *" --workspace "* ]]; then
  [[ "$args" == *" -E kind(lib) or kind(test) "* ]] || exit 98
  lane="workspace-build"
elif [[ "$args" == *" --binaries-metadata "* && "$args" == *" -E kind(lib) "* ]]; then
  lane="unit"
elif [[ "$args" == *" --binaries-metadata "* && "$args" == *" -E kind(test) "* ]]; then
  lane="integration"
elif [[ "$args" == *" --binaries-metadata "* && "$args" == *" -E binary(cold_restart_mob_resume) "* ]]; then
  lane="headcanonical-process-death"
elif [[ "$args" == *" --binaries-metadata "* && "$args" == *" -E binary(e2e_fast_lane) "* ]]; then
  lane="e2e-fast"
else
  exit 99
fi

if [[ "$args" == *" --binaries-metadata "* && "$args" != *" --no-tests=fail "* ]]; then
  exit 97
fi
printf '%s\n' "$lane" >> "$MEERKAT_PRE_PUSH_TEST_LANE_LOG"
timeout_lane="${MEERKAT_PRE_PUSH_TEST_TIMEOUT_LANE:-}"
timeout_attempts="${MEERKAT_PRE_PUSH_TEST_TIMEOUT_ATTEMPTS:-0}"
if [[ "$lane" == "$timeout_lane" && "$timeout_attempts" -gt 0 ]]; then
  attempt_file="${MEERKAT_PRE_PUSH_TEST_ATTEMPT_ROOT}/${lane}"
  touch "$attempt_file"
  attempt="$(( $(wc -l < "$attempt_file") + 1 ))"
  printf '%s\n' "$attempt" >> "$attempt_file"
  if [[ "$attempt" -le "$timeout_attempts" ]]; then
    exit 124
  fi
fi
if [[ "$lane" == "$MEERKAT_PRE_PUSH_TEST_FAIL_LANE" ]]; then
  exit "$MEERKAT_PRE_PUSH_TEST_FAIL_STATUS"
fi
if [[ "$lane" == "workspace-build" || "$lane" == "headcanonical-build" ]]; then
  printf '{}\n'
fi
EOF
chmod +x "$FAKE_CARGO"

cat > "$FAKE_GIT" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == "status" ]]; then
  exit 73
fi
exec git "$@"
EOF
chmod +x "$FAKE_GIT"

assert_failure_case() {
  local fail_lane="$1"
  local fail_status="$2"
  local expected_log="$3"
  : > "$LANE_LOG"
  rm -rf "$TEST_ROOT/.git/meerkat-hook-cache"

  set +e
  (
    cd "$TEST_ROOT"
    ROOT="$REPO_ROOT" \
      CARGO="$FAKE_CARGO" \
      MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE=1 \
      MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_UNIT_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_INTEGRATION_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_BUILD_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_TEST_LANE_LOG="$LANE_LOG" \
      MEERKAT_PRE_PUSH_TEST_FAIL_LANE="$fail_lane" \
      MEERKAT_PRE_PUSH_TEST_FAIL_STATUS="$fail_status" \
      PRE_COMMIT_TO_REF="$test_head" \
      "$REPO_ROOT/scripts/pre-push-unit.sh"
  )
  local status=$?
  set -e

  if [[ "$status" -ne "$fail_status" ]]; then
    echo "pre-push ${fail_lane} failure returned ${status}; expected ${fail_status}" >&2
    exit 1
  fi
  if [[ "$(paste -sd ' ' "$LANE_LOG")" != "$expected_log" ]]; then
    echo "pre-push ${fail_lane} lane order was '$(paste -sd ' ' "$LANE_LOG")'; expected '${expected_log}'" >&2
    exit 1
  fi
  if find "$TEST_ROOT/.git/meerkat-hook-cache" -name '*.ok' -print -quit 2>/dev/null | grep -q .; then
    echo "pre-push ${fail_lane} failure created a success stamp" >&2
    exit 1
  fi
}

assert_failure_case unit 37 "workspace-build unit"
assert_failure_case integration 38 "workspace-build unit integration"
assert_failure_case headcanonical-process-death 39 \
  "workspace-build unit integration headcanonical-build headcanonical-process-death"
assert_failure_case e2e-fast 40 \
  "workspace-build unit integration headcanonical-build headcanonical-process-death e2e-fast"

assert_timeout_case() {
  local timeout_lane="$1"
  local timeout_attempts="$2"
  local expected_status="$3"
  local expected_log="$4"
  local expect_stamp="$5"
  local attempt_root="$HARNESS_ROOT/attempts"
  : > "$LANE_LOG"
  rm -rf "$attempt_root" "$TEST_ROOT/.git/meerkat-hook-cache"
  mkdir -p "$attempt_root"

  set +e
  (
    cd "$TEST_ROOT"
    # The fake cargo command injects status 124 deterministically for the
    # selected lane. Give unrelated process startup enough room that host
    # saturation cannot manufacture an additional timeout and retry.
    ROOT="$REPO_ROOT" \
      CARGO="$FAKE_CARGO" \
      MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE=1 \
      MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_UNIT_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_INTEGRATION_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_BUILD_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_TEST_LANE_LOG="$LANE_LOG" \
      MEERKAT_PRE_PUSH_TEST_FAIL_LANE="" \
      MEERKAT_PRE_PUSH_TEST_FAIL_STATUS=99 \
      MEERKAT_PRE_PUSH_TEST_TIMEOUT_LANE="$timeout_lane" \
      MEERKAT_PRE_PUSH_TEST_TIMEOUT_ATTEMPTS="$timeout_attempts" \
      MEERKAT_PRE_PUSH_TEST_ATTEMPT_ROOT="$attempt_root" \
      PRE_COMMIT_TO_REF="$test_head" \
      "$REPO_ROOT/scripts/pre-push-unit.sh"
  )
  local status=$?
  set -e

  if [[ "$status" -ne "$expected_status" ]]; then
    echo "pre-push timeout case returned ${status}; expected ${expected_status}" >&2
    exit 1
  fi
  if [[ "$(paste -sd ' ' "$LANE_LOG")" != "$expected_log" ]]; then
    echo "pre-push timeout lane order was '$(paste -sd ' ' "$LANE_LOG")'; expected '${expected_log}'" >&2
    exit 1
  fi

  local stamp_found=0
  if find "$TEST_ROOT/.git/meerkat-hook-cache" -name '*.ok' -print -quit 2>/dev/null | grep -q .; then
    stamp_found=1
  fi
  if [[ "$stamp_found" -ne "$expect_stamp" ]]; then
    echo "pre-push timeout stamp state was ${stamp_found}; expected ${expect_stamp}" >&2
    exit 1
  fi
}

assert_timeout_case unit 1 0 \
  "workspace-build unit unit integration headcanonical-build headcanonical-process-death e2e-fast" 1
assert_timeout_case unit 2 124 "workspace-build unit unit" 0
assert_timeout_case workspace-build 1 0 \
  "workspace-build workspace-build unit integration headcanonical-build headcanonical-process-death e2e-fast" 1
assert_timeout_case workspace-build 2 124 "workspace-build workspace-build" 0
assert_timeout_case headcanonical-process-death 1 0 \
  "workspace-build unit integration headcanonical-build headcanonical-process-death headcanonical-process-death e2e-fast" 1
assert_timeout_case headcanonical-process-death 2 124 \
  "workspace-build unit integration headcanonical-build headcanonical-process-death headcanonical-process-death" 0
assert_timeout_case headcanonical-build 1 0 \
  "workspace-build unit integration headcanonical-build headcanonical-build headcanonical-process-death e2e-fast" 1
assert_timeout_case headcanonical-build 2 124 \
  "workspace-build unit integration headcanonical-build headcanonical-build" 0

assert_preflight_rejection() {
  local label="$1"
  shift
  : > "$LANE_LOG"
  rm -rf "$TEST_ROOT/.git/meerkat-hook-cache"

  set +e
  (
    cd "$TEST_ROOT"
    ROOT="$REPO_ROOT" \
      CARGO="$FAKE_CARGO" \
      MEERKAT_SKIP_PRE_PUSH_UNIT_CACHE=1 \
      MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_UNIT_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_INTEGRATION_NEXTEST_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_BUILD_TIMEOUT_SECS=10 \
      MEERKAT_PRE_PUSH_TEST_LANE_LOG="$LANE_LOG" \
      MEERKAT_PRE_PUSH_TEST_FAIL_LANE="" \
      MEERKAT_PRE_PUSH_TEST_FAIL_STATUS=99 \
      "$@" \
      "$REPO_ROOT/scripts/pre-push-unit.sh"
  ) >/dev/null 2>&1
  local status=$?
  set -e

  if [[ "$status" -eq 0 ]]; then
    echo "pre-push ${label} preflight unexpectedly succeeded" >&2
    exit 1
  fi
  if [[ -s "$LANE_LOG" ]]; then
    echo "pre-push ${label} preflight ran test lanes before rejecting" >&2
    exit 1
  fi
}

touch "$TEST_ROOT/untracked-test-input"
assert_preflight_rejection dirty-worktree env PRE_COMMIT_TO_REF="$(git -C "$TEST_ROOT" rev-parse HEAD)"
rm "$TEST_ROOT/untracked-test-input"
assert_preflight_rejection mismatched-push-ref env PRE_COMMIT_TO_REF=deadbeef
assert_preflight_rejection cleanliness-probe-failure env \
  PRE_COMMIT_TO_REF="$test_head" GIT_BIN="$FAKE_GIT"

: > "$LANE_LOG"
rm -rf "$TEST_ROOT/.git/meerkat-hook-cache"
(
  cd "$TEST_ROOT"
  ROOT="$REPO_ROOT" \
    CARGO="$FAKE_CARGO" \
    MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_UNIT_NEXTEST_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_INTEGRATION_NEXTEST_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_BUILD_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_TEST_LANE_LOG="$LANE_LOG" \
    MEERKAT_PRE_PUSH_TEST_FAIL_LANE="" \
    MEERKAT_PRE_PUSH_TEST_FAIL_STATUS=99 \
    PRE_COMMIT_TO_REF="$test_head" \
    "$REPO_ROOT/scripts/pre-push-unit.sh"
)
if [[ "$(paste -sd ' ' "$LANE_LOG")" != \
  "workspace-build unit integration headcanonical-build headcanonical-process-death e2e-fast" ]]; then
  echo "successful pre-push lane order was '$(paste -sd ' ' "$LANE_LOG")'" >&2
  exit 1
fi
if ! find "$TEST_ROOT/.git/meerkat-hook-cache" -name '*.ok' -print -quit | grep -q .; then
  echo "successful pre-push run did not create a cache stamp" >&2
  exit 1
fi

initial_stamp_count="$(find "$TEST_ROOT/.git/meerkat-hook-cache" -name '*.ok' | wc -l | tr -d ' ')"
printf 'lock revision 1\n' > "$TEST_ROOT/Cargo.lock"
printf 'module lock revision 1\n' > "$TEST_ROOT/MODULE.bazel.lock"
git -C "$TEST_ROOT" add Cargo.lock MODULE.bazel.lock
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "lock-only repair"
test_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
: > "$LANE_LOG"
(
  cd "$TEST_ROOT"
  ROOT="$REPO_ROOT" \
    CARGO="$FAKE_CARGO" \
    MEERKAT_PRE_PUSH_TEST_LANE_LOG="$LANE_LOG" \
    MEERKAT_PRE_PUSH_TEST_FAIL_LANE="" \
    MEERKAT_PRE_PUSH_TEST_FAIL_STATUS=99 \
    PRE_COMMIT_TO_REF="$test_head" \
    "$REPO_ROOT/scripts/pre-push-unit.sh"
)
if [[ -s "$LANE_LOG" ]]; then
  echo "root lockfile-only repair reran source-test lanes: $(paste -sd ' ' "$LANE_LOG")" >&2
  exit 1
fi
lock_stamp_count="$(find "$TEST_ROOT/.git/meerkat-hook-cache" -name '*.ok' | wc -l | tr -d ' ')"
if [[ "$lock_stamp_count" != "$initial_stamp_count" ]]; then
  echo "root lockfile-only repair created a different source-test fingerprint" >&2
  exit 1
fi

printf 'pub fn changed_source() {}\n' > "$TEST_ROOT/source.rs"
git -C "$TEST_ROOT" add source.rs
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "source repair"
test_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
: > "$LANE_LOG"
(
  cd "$TEST_ROOT"
  ROOT="$REPO_ROOT" \
    CARGO="$FAKE_CARGO" \
    MEERKAT_PRE_PUSH_NEXTEST_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_UNIT_NEXTEST_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_INTEGRATION_NEXTEST_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_BUILD_TIMEOUT_SECS=10 \
    MEERKAT_PRE_PUSH_TEST_LANE_LOG="$LANE_LOG" \
    MEERKAT_PRE_PUSH_TEST_FAIL_LANE="" \
    MEERKAT_PRE_PUSH_TEST_FAIL_STATUS=99 \
    PRE_COMMIT_TO_REF="$test_head" \
    "$REPO_ROOT/scripts/pre-push-unit.sh"
)
expected_source_lanes="workspace-build unit integration headcanonical-build headcanonical-process-death e2e-fast"
if [[ "$(paste -sd ' ' "$LANE_LOG")" != "$expected_source_lanes" ]]; then
  echo "source change lane order was '$(paste -sd ' ' "$LANE_LOG")'; expected '${expected_source_lanes}'" >&2
  exit 1
fi
source_stamp_count="$(find "$TEST_ROOT/.git/meerkat-hook-cache" -name '*.ok' | wc -l | tr -d ' ')"
if [[ "$source_stamp_count" -ne $((initial_stamp_count + 1)) ]]; then
  echo "source change did not create a distinct source-test evidence stamp" >&2
  exit 1
fi

echo "pre-push deterministic gate status and source-fingerprint contracts hold"
