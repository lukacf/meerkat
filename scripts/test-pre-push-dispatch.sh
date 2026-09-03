#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-dispatch.XXXXXX")"
HARNESS_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-dispatch-harness.XXXXXX")"
CACHE_REPO="${TEST_ROOT}-cache"
ATTRIBUTION_ROOT="${TEST_ROOT}-attribution"
LOCK_ROOT="${TEST_ROOT}-lock"
LOCK_RELEASE="${LOCK_ROOT}/release"
LOCK_FIRST_PID=""
LOCK_SECOND_PID=""
DISPATCH_READINESS_ATTEMPTS=900
DISPATCH_POLL_SECONDS=0.05
DISPATCH_EXIT_ATTEMPTS=100

job_is_running() {
  jobs -pr | grep -Fxq "$1"
}

wait_for_job_exit() {
  local pid="$1"
  for _ in $(seq 1 "$DISPATCH_EXIT_ATTEMPTS"); do
    job_is_running "$pid" || return 0
    sleep "$DISPATCH_POLL_SECONDS"
  done
  return 1
}

cleanup_harness() {
  local residue_status=$? pid
  mkdir -p "$LOCK_ROOT"
  : > "$LOCK_RELEASE"
  for pid in "$LOCK_FIRST_PID" "$LOCK_SECOND_PID"; do
    [[ -n "$pid" ]] || continue
    if ! wait_for_job_exit "$pid"; then
      kill "$pid" 2>/dev/null || true
      if ! wait_for_job_exit "$pid"; then
        kill -KILL "$pid" 2>/dev/null || true
      fi
    fi
    wait "$pid" 2>/dev/null || true
  done
  # Attribution scenarios deliberately create paths the dispatcher cannot
  # remove; make them removable again before tearing the harness down.
  chmod -R u+rwx "$TEST_ROOT" "$HARNESS_ROOT" "$CACHE_REPO" "$ATTRIBUTION_ROOT" \
    "$LOCK_ROOT" 2>/dev/null || true
  rm -rf "$TEST_ROOT" "$HARNESS_ROOT" "$CACHE_REPO" "$ATTRIBUTION_ROOT" \
    "$LOCK_ROOT" 2>/dev/null || true
  exit "$residue_status"
}
trap cleanup_harness EXIT

hash_path() {
  if command -v shasum >/dev/null 2>&1; then
    printf '%s' "$1" | shasum -a 256 | cut -c1-16
  elif command -v sha256sum >/dev/null 2>&1; then
    printf '%s' "$1" | sha256sum | cut -c1-16
  else
    printf '%s' "$1" | cksum | cut -d' ' -f1
  fi
}

default_lane_for_root() {
  local canonical_root
  canonical_root="$(git -C "$1" rev-parse --show-toplevel)"
  printf 'pre-push-%s' "$(hash_path "$canonical_root")"
}

wait_for_observation() {
  local path="$1"
  local minimum_lines="$2"
  local observed

  for _ in $(seq 1 "$DISPATCH_READINESS_ATTEMPTS"); do
    if [[ "$minimum_lines" -eq 0 ]]; then
      [[ -f "$path" ]] && return 0
    elif [[ -f "$path" ]]; then
      observed="$(wc -l < "$path" | tr -d ' ')"
      [[ "$observed" -ge "$minimum_lines" ]] && return 0
    fi
    sleep "$DISPATCH_POLL_SECONDS"
  done
  return 1
}

if ! grep -Fxq 'fail_fast: true' "$REPO_ROOT/.pre-commit-config.yaml"; then
  echo "pre-push configuration must stop after the first rejecting hook" >&2
  exit 1
fi

git -C "$TEST_ROOT" init -q
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "base"
base_sha="$(git -C "$TEST_ROOT" rev-parse HEAD)"
git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "candidate"
head_sha="$(git -C "$TEST_ROOT" rev-parse HEAD)"
git -C "$TEST_ROOT" update-ref refs/remotes/origin/main "$base_sha"
git -C "$TEST_ROOT" symbolic-ref \
  refs/remotes/origin/HEAD refs/remotes/origin/main

FAKE_PRE_COMMIT="${HARNESS_ROOT}/pre-commit"
INVOCATION_LOG="${HARNESS_ROOT}/invocation"
NESTED_INIT_ROOT="${HARNESS_ROOT}/nested-init"
cat > "$FAKE_PRE_COMMIT" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$MEERKAT_DISPATCH_NESTED_INIT_ROOT"
git -C "$MEERKAT_DISPATCH_NESTED_INIT_ROOT" init -q
{
  printf 'args=%s\n' "$*"
  printf 'cwd=%s\n' "$PWD"
  printf 'head=%s\n' "$(git rev-parse HEAD)"
  printf 'git_dir_env=%s\n' "${GIT_DIR:-}"
  printf 'git_work_tree_env=%s\n' "${GIT_WORK_TREE:-}"
  printf 'to=%s\n' "${PRE_COMMIT_TO_REF:-}"
  printf 'from=%s\n' "${PRE_COMMIT_FROM_REF:-}"
  printf 'remote_name=%s\n' "${PRE_COMMIT_REMOTE_NAME:-}"
  printf 'remote_url=%s\n' "${PRE_COMMIT_REMOTE_URL:-}"
  printf 'lane=%s\n' "${RUST_LANE_ID:-}"
  printf 'bazel_output_base=%s\n' "${MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE:-}"
  if [[ -e dirty-source-only ]]; then
    printf 'dirty_source_visible=yes\n'
  fi
} > "$MEERKAT_DISPATCH_INVOCATION_LOG"
EOF
chmod +x "$FAKE_PRE_COMMIT"

run_dispatch() {
  local stdin_payload="$1"
  (
    cd "$TEST_ROOT"
    PATH="${HARNESS_ROOT}:$PATH" \
      GIT_DIR="${TEST_ROOT}/.git" \
      GIT_WORK_TREE="$TEST_ROOT" \
      MEERKAT_DISPATCH_INVOCATION_LOG="$INVOCATION_LOG" \
      MEERKAT_DISPATCH_NESTED_INIT_ROOT="$NESTED_INIT_ROOT" \
      MEERKAT_SKIP_PRE_PUSH_TREE_CACHE=1 \
      MEERKAT_PRE_PUSH_BAZEL_OUTPUT_ROOT="${HARNESS_ROOT}/bazel-output" \
      RUST_LANE_ID="" \
      "$REPO_ROOT/scripts/pre-push-dispatch.sh" origin example.invalid \
      <<<"$stdin_payload"
  )
}

assert_log_line() {
  local expected="$1"
  if ! grep -Fxq "$expected" "$INVOCATION_LOG"; then
    echo "missing dispatcher log line: ${expected}" >&2
    sed -n '1,120p' "$INVOCATION_LOG" >&2
    exit 1
  fi
}

touch "$TEST_ROOT/dirty-source-only"
: > "$INVOCATION_LOG"
run_dispatch "refs/heads/main ${head_sha} refs/heads/main ${base_sha}"
assert_log_line "args=run --config .pre-commit-config.yaml --hook-stage pre-push --from-ref ${base_sha} --to-ref ${head_sha}"
assert_log_line "head=${head_sha}"
assert_log_line "git_dir_env="
assert_log_line "git_work_tree_env="
assert_log_line "to=${head_sha}"
assert_log_line "from=${base_sha}"
assert_log_line "remote_name=origin"
assert_log_line "remote_url=example.invalid"
assert_log_line "lane=$(default_lane_for_root "$TEST_ROOT")"
if grep -Fq "dirty_source_visible=yes" "$INVOCATION_LOG"; then
  echo "dispatcher exposed dirty source-worktree bytes to validation" >&2
  exit 1
fi
validated_cwd="$(sed -n 's/^cwd=//p' "$INVOCATION_LOG")"
validated_bazel_output_base="$(sed -n 's/^bazel_output_base=//p' "$INVOCATION_LOG")"
if [[ -z "${validated_bazel_output_base}" ]]; then
  echo "dispatcher did not export a stable Bazel output base" >&2
  exit 1
fi
if [[ -e "$validated_cwd" ]]; then
  echo "dispatcher leaked its detached validation worktree: ${validated_cwd}" >&2
  exit 1
fi
if [[ "$(git -C "$TEST_ROOT" config --bool core.bare)" != "false" ]]; then
  echo "dispatcher allowed a nested git command to mutate the source repository" >&2
  exit 1
fi

: > "$INVOCATION_LOG"
run_dispatch "refs/heads/new ${head_sha} refs/heads/new ${ZERO_SHA:-0000000000000000000000000000000000000000}"
assert_log_line "args=run --config .pre-commit-config.yaml --hook-stage pre-push --from-ref ${base_sha} --to-ref ${head_sha}"
assert_log_line "from=${base_sha}"
second_validated_cwd="$(sed -n 's/^cwd=//p' "$INVOCATION_LOG")"
second_bazel_output_base="$(sed -n 's/^bazel_output_base=//p' "$INVOCATION_LOG")"
if [[ "${second_validated_cwd}" != "${validated_cwd}" ]]; then
  echo "dispatcher changed detached validation paths across pushed trees" >&2
  printf 'first: %s\nsecond: %s\n' "${validated_cwd}" "${second_validated_cwd}" >&2
  exit 1
fi
if [[ "${second_bazel_output_base}" != "${validated_bazel_output_base}" ]]; then
  echo "dispatcher changed Bazel output bases across pushed trees" >&2
  printf 'first: %s\nsecond: %s\n' \
    "${validated_bazel_output_base}" "${second_bazel_output_base}" >&2
  exit 1
fi

# If the remote default branch is unavailable, branch creation stays
# fail-closed on the empty tree rather than guessing a comparison base.
git -C "$TEST_ROOT" symbolic-ref --delete refs/remotes/origin/HEAD
git -C "$TEST_ROOT" update-ref -d refs/remotes/origin/main
: > "$INVOCATION_LOG"
run_dispatch "refs/heads/fallback ${head_sha} refs/heads/fallback 0000000000000000000000000000000000000000"
assert_log_line "args=run --config .pre-commit-config.yaml --hook-stage pre-push --all-files"
assert_log_line "from=4b825dc642cb6eb9a060e54bf8d69288fbee4904"

tag_object="$(git -C "$TEST_ROOT" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  tag -a dispatch-test -m dispatch-test && git -C "$TEST_ROOT" rev-parse dispatch-test)"
: > "$INVOCATION_LOG"
run_dispatch "refs/tags/dispatch-test ${tag_object} refs/tags/dispatch-test 0000000000000000000000000000000000000000"
assert_log_line "head=${head_sha}"
assert_log_line "to=${head_sha}"

# A successful exact-tree gate is reusable across ref names. This is the tag
# release path: the hook still validates the pushed object and checked-out
# HEAD, but does not recompute an identical tree after its branch push passed.
mkdir -p "$CACHE_REPO"
git -C "$CACHE_REPO" init -q
git -C "$CACHE_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "cache base"
cache_base_sha="$(git -C "$CACHE_REPO" rev-parse HEAD)"
git -C "$CACHE_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "cache candidate"
cache_head_sha="$(git -C "$CACHE_REPO" rev-parse HEAD)"
cache_tag_object="$(git -C "$CACHE_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  tag -a dispatch-cache-test -m dispatch-cache-test && git -C "$CACHE_REPO" rev-parse dispatch-cache-test)"

run_cached_dispatch() {
  local stdin_payload="$1"
  (
    cd "$CACHE_REPO"
    PATH="${HARNESS_ROOT}:$PATH" \
      GIT_DIR="${CACHE_REPO}/.git" \
      GIT_WORK_TREE="$CACHE_REPO" \
      MEERKAT_DISPATCH_INVOCATION_LOG="$INVOCATION_LOG" \
      MEERKAT_DISPATCH_NESTED_INIT_ROOT="$NESTED_INIT_ROOT" \
      RUST_LANE_ID="" \
      "$REPO_ROOT/scripts/pre-push-dispatch.sh" origin example.invalid \
      <<<"$stdin_payload"
  )
}

: > "$INVOCATION_LOG"
run_cached_dispatch "refs/heads/main ${cache_head_sha} refs/heads/main ${cache_base_sha}"
assert_log_line "head=${cache_head_sha}"
: > "$INVOCATION_LOG"
run_cached_dispatch \
  "refs/tags/dispatch-cache-test ${cache_tag_object} refs/tags/dispatch-cache-test 0000000000000000000000000000000000000000"
if [[ -s "$INVOCATION_LOG" ]]; then
  echo "dispatcher recomputed a previously validated exact tree for a tag push" >&2
  exit 1
fi

# A cache-hit process does not own the stable validation worktree. It must not
# remove a path that an in-flight dispatcher may be using for another tree.
cache_common_dir="$(git -C "$CACHE_REPO" rev-parse --path-format=absolute --git-common-dir)"
cache_validation_lane="$(default_lane_for_root "$CACHE_REPO")"
active_validation_tree="${cache_common_dir}/meerkat-hook-cache/worktrees/${cache_validation_lane}"
git -C "$CACHE_REPO" worktree add --detach --quiet "$active_validation_tree" "$cache_head_sha"
run_cached_dispatch \
  "refs/tags/dispatch-cache-test ${cache_tag_object} refs/tags/dispatch-cache-test 0000000000000000000000000000000000000000"
if [[ ! -d "$active_validation_tree" ]]; then
  echo "cached dispatcher removed a stable validation worktree it did not own" >&2
  exit 1
fi
git -C "$CACHE_REPO" worktree remove --force "$active_validation_tree"

# Concurrent pushes share one stable worktree. The later process waits for the
# repository dispatcher lock, then reuses the exact-tree stamp written by the
# first process instead of invoking the broad gate twice.
LOCK_REPO="${LOCK_ROOT}/repo"
LOCK_BIN="${LOCK_ROOT}/bin"
LOCK_ENTERED="${LOCK_ROOT}/entered"
LOCK_INVOCATIONS="${LOCK_ROOT}/invocations"
mkdir -p "$LOCK_REPO" "$LOCK_BIN"
git -C "$LOCK_REPO" init -q
git -C "$LOCK_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "lock base"
lock_base_sha="$(git -C "$LOCK_REPO" rev-parse HEAD)"
git -C "$LOCK_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "lock candidate"
lock_head_sha="$(git -C "$LOCK_REPO" rev-parse HEAD)"
cat > "${LOCK_BIN}/pre-commit" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$$" >> "$MEERKAT_DISPATCH_LOCK_INVOCATIONS"
: > "$MEERKAT_DISPATCH_LOCK_ENTERED"
while [[ -d "${MEERKAT_DISPATCH_LOCK_RELEASE%/*}" && ! -f "$MEERKAT_DISPATCH_LOCK_RELEASE" ]]; do
  sleep 0.05
done
EOF
chmod +x "${LOCK_BIN}/pre-commit"

run_lock_dispatch() {
  local local_ref="$1"
  (
    cd "$LOCK_REPO"
    PATH="${LOCK_BIN}:$PATH" \
      GIT_DIR="${LOCK_REPO}/.git" \
      GIT_WORK_TREE="$LOCK_REPO" \
      MEERKAT_DISPATCH_LOCK_INVOCATIONS="$LOCK_INVOCATIONS" \
      MEERKAT_DISPATCH_LOCK_ENTERED="$LOCK_ENTERED" \
      MEERKAT_DISPATCH_LOCK_RELEASE="$LOCK_RELEASE" \
      RUST_LANE_ID="" \
      "$REPO_ROOT/scripts/pre-push-dispatch.sh" origin example.invalid \
      <<<"${local_ref} ${lock_head_sha} ${local_ref} ${lock_base_sha}"
  )
}

run_lock_dispatch refs/heads/first >"${LOCK_ROOT}/first.log" 2>&1 &
LOCK_FIRST_PID=$!
if ! wait_for_observation "$LOCK_ENTERED" 0; then
  echo "first dispatcher did not enter the validation gate" >&2
  exit 1
fi
run_lock_dispatch refs/heads/second >"${LOCK_ROOT}/second.log" 2>&1 &
LOCK_SECOND_PID=$!
sleep 0.3
if [[ "$(wc -l < "$LOCK_INVOCATIONS" | tr -d ' ')" != "1" ]]; then
  echo "repository dispatcher lock allowed overlapping validation gates" >&2
  exit 1
fi
: > "$LOCK_RELEASE"
wait "$LOCK_FIRST_PID"
LOCK_FIRST_PID=""
wait "$LOCK_SECOND_PID"
LOCK_SECOND_PID=""
if [[ "$(wc -l < "$LOCK_INVOCATIONS" | tr -d ' ')" != "1" ]]; then
  echo "waiting dispatcher ignored exact-tree evidence from the lock owner" >&2
  exit 1
fi

# Different source worktrees derive different validation lanes. Their detached
# worktrees, Cargo targets, Bazel output bases, and dispatcher locks are
# isolated, so they must be able to enter validation concurrently even though
# they share one Git common directory.
printf 'parallel lane proof\n' > "${LOCK_REPO}/parallel.txt"
git -C "$LOCK_REPO" add parallel.txt
git -C "$LOCK_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "parallel candidate"
lock_parallel_sha="$(git -C "$LOCK_REPO" rev-parse HEAD)"
LOCK_PEER_REPO="${LOCK_ROOT}/peer"
git -C "$LOCK_REPO" worktree add --detach --quiet "$LOCK_PEER_REPO" "$lock_parallel_sha"
rm -f "$LOCK_ENTERED" "$LOCK_RELEASE" "$LOCK_INVOCATIONS"

run_parallel_dispatch() {
  local source_repo="$1"
  local local_ref="$2"
  (
    cd "$source_repo"
    PATH="${LOCK_BIN}:$PATH" \
      GIT_DIR="$(git -C "$source_repo" rev-parse --absolute-git-dir)" \
      GIT_WORK_TREE="$source_repo" \
      MEERKAT_DISPATCH_LOCK_INVOCATIONS="$LOCK_INVOCATIONS" \
      MEERKAT_DISPATCH_LOCK_ENTERED="$LOCK_ENTERED" \
      MEERKAT_DISPATCH_LOCK_RELEASE="$LOCK_RELEASE" \
      RUST_LANE_ID="" \
      "$REPO_ROOT/scripts/pre-push-dispatch.sh" origin example.invalid \
      <<<"${local_ref} ${lock_parallel_sha} ${local_ref} ${lock_head_sha}"
  )
}

run_parallel_dispatch "$LOCK_REPO" refs/heads/parallel-first \
  >"${LOCK_ROOT}/parallel-first.log" 2>&1 &
LOCK_FIRST_PID=$!
if ! wait_for_observation "$LOCK_ENTERED" 0; then
  echo "first worktree dispatcher did not enter the validation gate" >&2
  exit 1
fi
run_parallel_dispatch "$LOCK_PEER_REPO" refs/heads/parallel-second \
  >"${LOCK_ROOT}/parallel-second.log" 2>&1 &
LOCK_SECOND_PID=$!
if ! wait_for_observation "$LOCK_INVOCATIONS" 2; then
  echo "independent source worktrees were serialized by one repository dispatcher lock" >&2
  exit 1
fi
: > "$LOCK_RELEASE"
wait "$LOCK_FIRST_PID"
LOCK_FIRST_PID=""
wait "$LOCK_SECOND_PID"
LOCK_SECOND_PID=""
git -C "$LOCK_REPO" worktree remove --force "$LOCK_PEER_REPO"

# The dispatcher-exported output base is useful only if the authoritative
# Bazel lock gate passes it to bb as a startup option.
FAKE_BB="${HARNESS_ROOT}/bb"
BB_ARG_LOG="${HARNESS_ROOT}/bb-args"
cat > "$FAKE_BB" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" > "$MEERKAT_DISPATCH_BB_ARG_LOG"
EOF
chmod +x "$FAKE_BB"
expected_bazel_output_base="${HARNESS_ROOT}/bazel-output/consumption-proof"
BUILDBUDDY_BB="$FAKE_BB" \
  MEERKAT_DISPATCH_BB_ARG_LOG="$BB_ARG_LOG" \
  MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE="$expected_bazel_output_base" \
  "$REPO_ROOT/scripts/pre-push-bazel-locks.sh" --require-bb >/dev/null
expected_bb_args="${HARNESS_ROOT}/expected-bb-args"
cat > "$expected_bb_args" <<EOF
--output_base=${expected_bazel_output_base}
--max_idle_secs=600
mod
deps
--lockfile_mode=error
EOF
if ! cmp -s "$expected_bb_args" "$BB_ARG_LOG"; then
  echo "Bazel lock gate did not consume the dispatcher output-base contract" >&2
  diff -u "$expected_bb_args" "$BB_ARG_LOG" >&2 || true
  exit 1
fi

# The lock gate must also invoke bb successfully when no detached-worktree
# output base is supplied. Bash 3.2 treats expansion of an empty array under
# nounset as an error, so this is a distinct portability contract.
: > "$BB_ARG_LOG"
BUILDBUDDY_BB="$FAKE_BB" \
  MEERKAT_DISPATCH_BB_ARG_LOG="$BB_ARG_LOG" \
  MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE= \
  "$REPO_ROOT/scripts/pre-push-bazel-locks.sh" --require-bb >/dev/null
cat > "$expected_bb_args" <<'EOF'
mod
deps
--lockfile_mode=error
EOF
if ! cmp -s "$expected_bb_args" "$BB_ARG_LOG"; then
  echo "Bazel lock gate did not support an empty startup-argument set" >&2
  diff -u "$expected_bb_args" "$BB_ARG_LOG" >&2 || true
  exit 1
fi

assert_rejected_without_invocation() {
  local label="$1"
  local stdin_payload="$2"
  : > "$INVOCATION_LOG"
  set +e
  run_dispatch "$stdin_payload" >/dev/null 2>&1
  local status=$?
  set -e
  if [[ "$status" -eq 0 ]]; then
    echo "dispatcher ${label} case unexpectedly succeeded" >&2
    exit 1
  fi
  if [[ -s "$INVOCATION_LOG" ]]; then
    echo "dispatcher ${label} case invoked pre-commit before rejecting" >&2
    exit 1
  fi
}

assert_rejected_without_invocation \
  "non-HEAD" \
  "refs/heads/base ${base_sha} refs/heads/base 0000000000000000000000000000000000000000"
assert_rejected_without_invocation \
  "multi-ref" \
  "$(printf 'refs/heads/main %s refs/heads/main %s\nrefs/tags/x %s refs/tags/x %s' \
    "$head_sha" "$base_sha" "$tag_object" 0000000000000000000000000000000000000000)"

# --- failure attribution -----------------------------------------------------
# Git reports only "failed to push some refs". Every nonzero dispatcher exit
# must therefore name its own cause, and a cleanup problem must never turn a
# passing gate into a bare exit 1 with nothing but hook successes on screen.

ATTRIBUTION_REPO="${ATTRIBUTION_ROOT}/repo"
ATTRIBUTION_BIN="${ATTRIBUTION_ROOT}/bin"
ATTRIBUTION_TMP="${ATTRIBUTION_ROOT}/tmp"
mkdir -p "$ATTRIBUTION_BIN" "$ATTRIBUTION_TMP"
git init -q "$ATTRIBUTION_REPO"
git -C "$ATTRIBUTION_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "base"
attribution_base_sha="$(git -C "$ATTRIBUTION_REPO" rev-parse HEAD)"
git -C "$ATTRIBUTION_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "candidate"
attribution_head_sha="$(git -C "$ATTRIBUTION_REPO" rev-parse HEAD)"

install_scripted_pre_commit() {
  local mode="$1"
  printf '#!/usr/bin/env bash\nset -uo pipefail\nmode=%q\n' "$mode" \
    > "${ATTRIBUTION_BIN}/pre-commit"
  cat >> "${ATTRIBUTION_BIN}/pre-commit" <<'FAKEEOF'
printf 'gitleaks.................................................................Passed\n'
printf 'workspace deterministic unit + integration + e2e gate.....................'
case "$mode" in
  hook-failure)
    printf 'Failed\n'
    printf -- '- hook id: cargo-test\n'
    printf -- '- exit code: 101\n'
    exit 1
    ;;
  harness-failure)
    printf 'Passed\n'
    exit 3
    ;;
  undeletable-residue)
    printf 'Passed\n'
    mkdir -p residue/locked
    : > residue/locked/artifact
    chmod 500 residue/locked
    exit 0
    ;;
  *)
    printf 'Passed\n'
    exit 0
    ;;
esac
FAKEEOF
  chmod +x "${ATTRIBUTION_BIN}/pre-commit"
}

attribution_status=0
run_attribution_dispatch() {
  local mode="$1"
  local output_path="$2"
  local attribution_common_dir attribution_validation_tree
  install_scripted_pre_commit "$mode"
  set +e
  (
    cd "$ATTRIBUTION_REPO"
    PATH="${ATTRIBUTION_BIN}:$PATH" \
      TMPDIR="$ATTRIBUTION_TMP" \
      GIT_DIR="${ATTRIBUTION_REPO}/.git" \
      GIT_WORK_TREE="$ATTRIBUTION_REPO" \
      MEERKAT_SKIP_PRE_PUSH_TREE_CACHE=1 \
      RUST_LANE_ID="" \
      "$REPO_ROOT/scripts/pre-push-dispatch.sh" origin example.invalid \
      <<<"refs/heads/main ${attribution_head_sha} refs/heads/main ${attribution_base_sha}" \
      >"$output_path" 2>&1
  )
  attribution_status=$?
  set -e
  attribution_common_dir="$(git -C "$ATTRIBUTION_REPO" rev-parse --path-format=absolute --git-common-dir)"
  attribution_validation_tree="${attribution_common_dir}/meerkat-hook-cache/worktrees/$(default_lane_for_root "$ATTRIBUTION_REPO")"
  chmod -R u+rwx "$attribution_validation_tree" 2>/dev/null || true
  git -C "$ATTRIBUTION_REPO" worktree remove --force "$attribution_validation_tree" \
    >/dev/null 2>&1 || true
  rm -rf -- "$attribution_validation_tree" 2>/dev/null || true
  chmod -R u+rwx "$ATTRIBUTION_TMP" 2>/dev/null || true
  rm -rf "${ATTRIBUTION_TMP:?}"/* 2>/dev/null || true
  git -C "$ATTRIBUTION_REPO" worktree prune 2>/dev/null || true
}

assert_output_contains() {
  local label="$1"
  local output_path="$2"
  local needle="$3"
  if ! grep -Fq "$needle" "$output_path"; then
    echo "dispatcher ${label} output does not contain: ${needle}" >&2
    sed -n '1,120p' "$output_path" >&2
    exit 1
  fi
}

hook_failure_log="${ATTRIBUTION_ROOT}/hook-failure.log"
run_attribution_dispatch hook-failure "$hook_failure_log"
if [[ "$attribution_status" -eq 0 ]]; then
  echo "dispatcher passed a push whose validation hook failed" >&2
  exit 1
fi
assert_output_contains "hook-failure" "$hook_failure_log" "Failing push-stage hook(s):"
assert_output_contains "hook-failure" "$hook_failure_log" \
  "workspace deterministic unit + integration + e2e gate"
assert_output_contains "hook-failure" "$hook_failure_log" \
  "pre-commit run --hook-stage pre-push --all-files cargo-test"

harness_failure_log="${ATTRIBUTION_ROOT}/harness-failure.log"
run_attribution_dispatch harness-failure "$harness_failure_log"
if [[ "$attribution_status" -ne 3 ]]; then
  echo "dispatcher rewrote the validation harness exit status 3 as ${attribution_status}" >&2
  exit 1
fi
assert_output_contains "harness-failure" "$harness_failure_log" \
  "No hook reported Failed, so the failure is in the gate harness"

residue_log="${ATTRIBUTION_ROOT}/residue.log"
run_attribution_dispatch undeletable-residue "$residue_log"
if [[ "$attribution_status" -ne 0 ]]; then
  echo "dispatcher failed a passing push over cleanup residue (exit ${attribution_status})" >&2
  sed -n '1,120p' "$residue_log" >&2
  exit 1
fi
assert_output_contains "residue" "$residue_log" "left behind"

# An internal dispatcher failure must announce itself as such: the operator
# otherwise reads a screen of passing hooks and a push that failed anyway.
attribution_common_dir="$(git -C "$ATTRIBUTION_REPO" rev-parse --path-format=absolute --git-common-dir)"
attribution_stamp_dir="${attribution_common_dir}/meerkat-hook-cache/exact-tree"
mkdir -p "$attribution_stamp_dir"
chmod 500 "$attribution_stamp_dir"
internal_failure_log="${ATTRIBUTION_ROOT}/internal-failure.log"
run_attribution_dispatch pass "$internal_failure_log"
chmod 700 "$attribution_stamp_dir"
if [[ "$attribution_status" -eq 0 ]]; then
  echo "dispatcher recorded success it could not persist" >&2
  exit 1
fi
assert_output_contains "internal-failure" "$internal_failure_log" \
  "Meerkat pre-push dispatcher failed while recording exact-tree validation evidence"
assert_output_contains "internal-failure" "$internal_failure_log" \
  "This is the dispatcher itself, not a validation hook"

echo "pre-push dispatcher ref and attribution contracts hold"
