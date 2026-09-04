#!/usr/bin/env bash
# Contract test for bounded pre-push lane retention.
#
# Reproduces the leak shape: several `pre-push-<16 hex>` lanes accumulated
# under one repository's hook cache and Cargo targets root, most of them owned
# by source worktrees that no longer exist. The pruner must keep the newest N
# (by explicit last-used stamp, falling back to mtime) and must never remove
# the caller's lane, a lane whose dispatcher lock is live, a lane with any
# activity inside the idle window, a lane referenced by a live process, or
# anything whose name is not an exact lane id. The dispatcher must run
# retention only after a passed gate.
#
# Offline and file-only: the fake cache layout lives under a temp root.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PRUNER="${REPO_ROOT}/scripts/pre-push-prune-lanes.sh"
DISPATCHER="${REPO_ROOT}/scripts/pre-push-dispatch.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-lane-retention.XXXXXX")"
SOURCE_REPO="${TEST_ROOT}/source"
HOOK_CACHE_ROOT="${TEST_ROOT}/git-common/meerkat-hook-cache"
WORKTREES="${HOOK_CACHE_ROOT}/worktrees"
TARGETS_ROOT="${TEST_ROOT}/rust-workspaces/meerkat-0123456789/targets/v4"
TOOLCHAIN_OLD="${TARGETS_ROOT}/1.93.0-x86_64-unknown-linux-gnu-aaaaaaaaaa"
TOOLCHAIN_NEW="${TARGETS_ROOT}/1.94.1-x86_64-unknown-linux-gnu-bbbbbbbbbb"
OUTSIDE_ROOT="${TEST_ROOT}/other-repo-targets/v4/toolchain"
DISPATCH_REPO="${TEST_ROOT}/dispatch-repo"
DISPATCH_HARNESS="${TEST_ROOT}/dispatch-harness"
DISPATCH_CACHE="${TEST_ROOT}/dispatch-cache"
BACKGROUND_PIDS=()

cleanup() {
  local pending_status=$? pid
  for pid in "${BACKGROUND_PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  git -C "$DISPATCH_REPO" worktree prune >/dev/null 2>&1 || true
  chmod -R u+rwx "$TEST_ROOT" 2>/dev/null || true
  rm -rf -- "$TEST_ROOT"
  exit "$pending_status"
}
trap cleanup EXIT

LAST_LOG=""

fail() {
  echo "pre-push lane retention contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  ${extra}" >&2
  done
  if [[ -n "$LAST_LOG" && -f "$LAST_LOG" ]]; then
    echo "  last pruner log (${LAST_LOG}):" >&2
    sed 's/^/    /' "$LAST_LOG" >&2
  fi
  exit 1
}

assert_present() {
  [[ -d "$1" ]] || fail "$2" "missing: $1"
}

assert_absent() {
  [[ ! -e "$1" && ! -L "$1" ]] || fail "$2" "still present: $1"
}

assert_log_has() {
  grep -Fq -- "$2" "$1" || fail "$3" "$(cat "$1")"
}

assert_log_lacks() {
  if grep -Fq -- "$2" "$1"; then
    fail "$3" "$(cat "$1")"
  fi
}

hash_path() {
  if command -v shasum >/dev/null 2>&1; then
    printf '%s' "$1" | shasum -a 256 | cut -c1-16
  elif command -v sha256sum >/dev/null 2>&1; then
    printf '%s' "$1" | sha256sum | cut -c1-16
  else
    printf '%s' "$1" | cksum | cut -d' ' -f1
  fi
}

# touch_at <minutes-ago> <path>...: portable explicit timestamp (GNU and BSD).
touch_at() {
  local minutes_ago="$1"
  shift
  local epoch stamp
  epoch=$(( $(date +%s) - minutes_ago * 60 ))
  # `touch -t` reads local time, so the stamp must be formatted in local time.
  stamp="$(date -d "@${epoch}" +%Y%m%d%H%M.%S 2>/dev/null ||
    date -r "${epoch}" +%Y%m%d%H%M.%S)"
  touch -t "$stamp" "$@"
}

# age_tree <minutes-ago> <dir>: every entry beneath the directory, including
# the directory itself, gets the same old mtime.
age_tree() {
  local minutes_ago="$1"
  local dir="$2"
  while IFS= read -r -d '' path; do
    touch_at "$minutes_ago" "$path"
  done < <(find "$dir" -depth -print0)
}

# make_target <toolchain-dir> <lane> <stamp-minutes-ago> [content-minutes-ago]:
# a fake Cargo target whose explicit stamp carries the last use and whose
# contents are old enough to be outside the idle window unless stated.
make_target() {
  local toolchain_dir="$1" lane="$2" stamp_minutes_ago="$3"
  local content_minutes_ago="${4:-$3}"
  local dir="${toolchain_dir}/${lane}"
  mkdir -p "${dir}/debug/deps" "${dir}/debug/incremental"
  : > "${dir}/debug/deps/libfake.rlib"
  : > "${dir}/debug/incremental/query-cache.bin"
  age_tree "$content_minutes_ago" "$dir"
  : > "${dir}/.meerkat-pre-push-last-used"
  touch_at "$stamp_minutes_ago" "${dir}/.meerkat-pre-push-last-used"
  touch_at "$content_minutes_ago" "$dir"
}

# make_worktree <lane> <minutes-ago>: hook worktrees carry no stamp (they must
# stay byte-clean), so ordering falls back to directory mtime.
make_worktree() {
  local lane="$1" minutes_ago="$2"
  mkdir -p "${WORKTREES}/${lane}/src"
  : > "${WORKTREES}/${lane}/src/lib.rs"
  age_tree "$minutes_ago" "${WORKTREES}/${lane}"
}

git -C "$TEST_ROOT" init -q "$SOURCE_REPO"
git -C "$SOURCE_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "base"

mkdir -p "$WORKTREES" "$TOOLCHAIN_OLD" "$TOOLCHAIN_NEW" "$OUTSIDE_ROOT"

# Idle window is 6 hours (360 min) by default; "old" below means well past it.
LANE_A="pre-push-aaaaaaaaaaaaaaaa" # oldest orphan
LANE_B="pre-push-bbbbbbbbbbbbbbbb" # second-oldest orphan
LANE_C="pre-push-cccccccccccccccc" # newest peer: used 10 min ago, unlocked
LANE_D="pre-push-dddddddddddddddd" # the caller's own lane, stamped old on purpose
LANE_E="pre-push-eeeeeeeeeeeeeeee" # oldest of all, but its dispatcher lock is live
LANE_F="pre-push-ffffffffffffffff" # old stamp, but a build artifact changed 1 min ago
LANE_G="pre-push-1111111111111111" # old, unlocked, but a live process names it
LANE_H="pre-push-2222222222222222" # old, unlocked, but a live process has cwd inside

# Targets: two toolchain generations under one repo; retention ranks across
# both so a toolchain bump still bounds the total.
make_target "$TOOLCHAIN_OLD" "$LANE_A" 4000
make_target "$TOOLCHAIN_OLD" "$LANE_B" 3000
make_target "$TOOLCHAIN_NEW" "$LANE_C" 10 2000
make_target "$TOOLCHAIN_NEW" "$LANE_D" 2000
make_target "$TOOLCHAIN_OLD" "$LANE_E" 5000
make_target "$TOOLCHAIN_NEW" "$LANE_F" 6000
touch_at 1 "${TOOLCHAIN_NEW}/${LANE_F}/debug/incremental/query-cache.bin"
make_target "$TOOLCHAIN_NEW" "$LANE_G" 7000
make_target "$TOOLCHAIN_NEW" "$LANE_H" 8000
# Directory mtimes are deliberately misleading: the oldest-stamped lane gets the
# freshest directory mtime, so only the stamp can order them correctly.
touch_at 1000 "${TOOLCHAIN_OLD}/${LANE_A}"
touch_at 9000 "${TOOLCHAIN_NEW}/${LANE_C}"

# Names that must never be touched, however old.
UNTOUCHABLES=(meerkat-m1-856ed44470 release-package-alpha pre-push-short
  pre-push-ABCDEF0123456789 pre-push-0123456789abcdef0)
for untouchable in "${UNTOUCHABLES[@]}"; do
  mkdir -p "${TOOLCHAIN_NEW}/${untouchable}" "${WORKTREES}/${untouchable}"
  touch_at 9000 "${TOOLCHAIN_NEW}/${untouchable}" "${WORKTREES}/${untouchable}"
done
# A lane-shaped file (not a directory) and a lane-shaped symlink are not lanes.
: > "${TOOLCHAIN_NEW}/pre-push-3333333333333333"
ln -s "${TOOLCHAIN_NEW}/${LANE_C}" "${TOOLCHAIN_NEW}/pre-push-4444444444444444"
# Lane-shaped directories outside the given roots are somebody else's.
mkdir -p "${OUTSIDE_ROOT}/${LANE_A}"
touch_at 9000 "${OUTSIDE_ROOT}/${LANE_A}"

# Hook worktrees: one real registered git worktree (the common crash residue)
# plus plain directories left by a hard kill.
git -C "$SOURCE_REPO" worktree add --detach --quiet "${WORKTREES}/${LANE_A}" HEAD
age_tree 4000 "${WORKTREES}/${LANE_A}"
make_worktree "$LANE_B" 3000
make_worktree "$LANE_C" 10
make_worktree "$LANE_D" 2000
make_worktree "$LANE_E" 5000

# Background fixtures must not inherit this script's stdio: an inherited pipe
# would keep any consumer of this test's output waiting on them.
# Lane E is mid-validation: its dispatcher lock names a live pid.
sleep 600 >/dev/null 2>&1 &
LOCK_HOLDER_PID=$!
BACKGROUND_PIDS+=("$LOCK_HOLDER_PID")
mkdir -p "${HOOK_CACHE_ROOT}/dispatcher-${LANE_E}.lock"
printf '%s\n' "$LOCK_HOLDER_PID" > "${HOOK_CACHE_ROOT}/dispatcher-${LANE_E}.lock/pid"
# Lane A once had a lock too, but its owner is gone: stale locks do not protect.
mkdir -p "${HOOK_CACHE_ROOT}/dispatcher-${LANE_A}.lock"
printf '%s\n' "999999999" > "${HOOK_CACHE_ROOT}/dispatcher-${LANE_A}.lock/pid"
touch_at 4000 "${HOOK_CACHE_ROOT}/dispatcher-${LANE_A}.lock"

# Lane G is named on a live process's command line (python keeps the extra
# argument in argv); lane H is a live process's working directory (the cwd
# rule is /proc-based and therefore Linux-only).
python3 -c 'import time; time.sleep(600)' "${TOOLCHAIN_NEW}/${LANE_G}" \
  >/dev/null 2>&1 </dev/null &
BACKGROUND_PIDS+=("$!")
(cd "${TOOLCHAIN_NEW}/${LANE_H}" && exec sleep 600 >/dev/null 2>&1) &
BACKGROUND_PIDS+=("$!")
sleep 0.2

# Abandoned rename residue from a pruner that died mid-delete must be swept;
# residue owned by a live pruner must not.
mkdir -p "${TOOLCHAIN_NEW}/pre-push-5555555555555555.pruning.999999999"
mkdir -p "${TOOLCHAIN_NEW}/pre-push-6666666666666666.pruning.${LOCK_HOLDER_PID}"

run_pruner() {
  LAST_LOG="${TEST_ROOT}/last-pruner.log"
  "$PRUNER" \
    --hook-cache-root "$HOOK_CACHE_ROOT" \
    --targets-root "$TARGETS_ROOT" \
    --current-lane "$LANE_D" \
    --source-root "$SOURCE_REPO" "$@" 2>&1 | tee "$LAST_LOG"
  return "${PIPESTATUS[0]}"
}

# 1. A dry run decides but deletes nothing.
dry_log="${TEST_ROOT}/dry.log"
MEERKAT_PRE_PUSH_KEEP_LANES=3 run_pruner --dry-run >"$dry_log" 2>&1 ||
  fail "dry run exited nonzero" "$(cat "$dry_log")"
assert_log_has "$dry_log" "would prune: ${TOOLCHAIN_OLD}/${LANE_A} (Cargo target;" \
  "dry run did not name the oldest target"
for lane in "$LANE_A" "$LANE_B" "$LANE_C" "$LANE_D" "$LANE_E"; do
  assert_present "${WORKTREES}/${lane}" "dry run removed a worktree"
done
assert_present "${TOOLCHAIN_OLD}/${LANE_A}" "dry run removed a target"
assert_present "${TOOLCHAIN_NEW}/pre-push-5555555555555555.pruning.999999999" \
  "dry run removed abandoned residue"

# 2. Budget 3: current lane D and live-locked lane E are exempt and count
#    first; C is the newest peer and fills the remaining slot. F, G, and H are
#    beyond the budget but protected by activity or a live process. A and B go.
run_log="${TEST_ROOT}/run.log"
MEERKAT_PRE_PUSH_KEEP_LANES=3 run_pruner >"$run_log" 2>&1 ||
  fail "prune exited nonzero" "$(cat "$run_log")"

assert_absent "${TOOLCHAIN_OLD}/${LANE_A}" "oldest target survived (stamp ordering ignored)"
assert_absent "${TOOLCHAIN_OLD}/${LANE_B}" "second-oldest target survived"
assert_present "${TOOLCHAIN_NEW}/${LANE_C}" "newest peer target was pruned (dir mtime used over stamp)"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "the caller's own lane target was pruned"
assert_present "${TOOLCHAIN_OLD}/${LANE_E}" "a live-locked lane target was pruned"
[[ -f "${TOOLCHAIN_OLD}/${LANE_E}/debug/deps/libfake.rlib" ]] ||
  fail "live-locked lane contents were disturbed"
assert_present "${TOOLCHAIN_NEW}/${LANE_F}" "a lane with a recently modified artifact was pruned"
assert_present "${TOOLCHAIN_NEW}/${LANE_G}" "a lane named by a live process was pruned"
if [[ -d /proc/self ]]; then
  assert_present "${TOOLCHAIN_NEW}/${LANE_H}" "a lane that is a live process's cwd was pruned"
fi

assert_log_has "$run_log" "kept: ${TOOLCHAIN_NEW}/${LANE_D} (Cargo target; current lane)" \
  "current-lane decision was not logged"
assert_log_has "$run_log" "kept: ${TOOLCHAIN_OLD}/${LANE_E} (Cargo target; dispatcher lock held by pid ${LOCK_HOLDER_PID})" \
  "live-lock decision was not logged"
assert_log_has "$run_log" "kept: ${TOOLCHAIN_NEW}/${LANE_C} (Cargo target; within budget 3" \
  "within-budget decision was not logged"
assert_log_has "$run_log" "kept: ${TOOLCHAIN_NEW}/${LANE_F} (Cargo target; modified within the 21600s idle window" \
  "recent-activity decision was not logged"
assert_log_has "$run_log" "kept: ${TOOLCHAIN_NEW}/${LANE_G} (Cargo target; referenced by live process pid" \
  "live-process decision was not logged"
assert_log_has "$run_log" "pruned: ${TOOLCHAIN_OLD}/${LANE_A} (Cargo target; beyond budget 3" \
  "prune decision was not logged with its reason"

assert_absent "${WORKTREES}/${LANE_A}" "oldest hook worktree (registered) survived"
assert_absent "${WORKTREES}/${LANE_B}" "second-oldest hook worktree survived"
assert_present "${WORKTREES}/${LANE_C}" "newest peer hook worktree was pruned"
assert_present "${WORKTREES}/${LANE_D}" "the caller's own hook worktree was pruned"
assert_present "${WORKTREES}/${LANE_E}" "a live-locked hook worktree was pruned"
if git -C "$SOURCE_REPO" worktree list --porcelain | grep -Fq "${WORKTREES}/${LANE_A}"; then
  fail "pruned hook worktree is still registered with git"
fi

for untouchable in "${UNTOUCHABLES[@]}"; do
  assert_present "${TOOLCHAIN_NEW}/${untouchable}" "non-lane target name was removed"
  assert_present "${WORKTREES}/${untouchable}" "non-lane worktree name was removed"
done
[[ -f "${TOOLCHAIN_NEW}/pre-push-3333333333333333" ]] || fail "lane-shaped file was removed"
[[ -L "${TOOLCHAIN_NEW}/pre-push-4444444444444444" ]] || fail "lane-shaped symlink was removed"
assert_present "${OUTSIDE_ROOT}/${LANE_A}" "a lane outside the given roots was removed"
assert_absent "${TOOLCHAIN_NEW}/pre-push-5555555555555555.pruning.999999999" \
  "abandoned prune residue survived"
assert_present "${TOOLCHAIN_NEW}/pre-push-6666666666666666.pruning.${LOCK_HOLDER_PID}" \
  "a live pruner's staged directory was removed"
for own_residue in "${TOOLCHAIN_OLD}"/*.pruning.* "${TOOLCHAIN_NEW}"/*.pruning.* "${WORKTREES}"/*.pruning.*; do
  [[ -e "$own_residue" ]] || continue
  [[ "$own_residue" == *".pruning.${LOCK_HOLDER_PID}" ]] && continue
  fail "pruner left rename residue behind" "$own_residue"
done

# 3. Re-running is a no-op at the budget.
rerun_log="${TEST_ROOT}/rerun.log"
MEERKAT_PRE_PUSH_KEEP_LANES=3 run_pruner >"$rerun_log" 2>&1 ||
  fail "idempotent rerun exited nonzero" "$(cat "$rerun_log")"
assert_log_lacks "$rerun_log" "pruned:" "a rerun at the budget removed more lanes"

# 4. The default budget (2) is a total bound filled by the two exempt lanes,
#    yet C (used 10 minutes ago, unlocked) survives on recency alone.
MEERKAT_PRE_PUSH_KEEP_LANES="" run_pruner >"${TEST_ROOT}/default.log" 2>&1 ||
  fail "default-budget prune exited nonzero"
assert_present "${TOOLCHAIN_NEW}/${LANE_C}" "a recently used, unlocked lane was pruned"
assert_present "${WORKTREES}/${LANE_C}" "a recently used, unlocked hook worktree was pruned"
assert_log_has "${TEST_ROOT}/default.log" "kept: ${TOOLCHAIN_NEW}/${LANE_C} (Cargo target; last used" \
  "recent-stamp decision was not logged"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "default budget removed the caller's lane"
assert_present "${TOOLCHAIN_OLD}/${LANE_E}" "default budget removed a live-locked lane"
MEERKAT_PRE_PUSH_KEEP_LANES=1 run_pruner >/dev/null 2>&1 || fail "keep=1 prune exited nonzero"
for lane_dir in "${TOOLCHAIN_NEW}/${LANE_C}" "${TOOLCHAIN_NEW}/${LANE_D}" "${TOOLCHAIN_OLD}/${LANE_E}" \
  "${TOOLCHAIN_NEW}/${LANE_F}" "${TOOLCHAIN_NEW}/${LANE_G}" "${WORKTREES}/${LANE_C}" \
  "${WORKTREES}/${LANE_D}" "${WORKTREES}/${LANE_E}"; do
  assert_present "$lane_dir" "keep=1 removed a protected lane"
done

# 5. Shrinking the idle window to zero lets recency lapse, but locks and live
#    process references still hold.
MEERKAT_PRE_PUSH_KEEP_LANES=1 MEERKAT_PRE_PUSH_LANE_IDLE_SECS=0 run_pruner \
  >"${TEST_ROOT}/idle0.log" 2>&1 || fail "idle=0 prune exited nonzero"
assert_absent "${TOOLCHAIN_NEW}/${LANE_C}" "idle=0 kept the unlocked, unreferenced newest peer"
assert_absent "${TOOLCHAIN_NEW}/${LANE_F}" "idle=0 kept the recently modified but unreferenced lane"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "idle=0 removed the caller's lane"
assert_present "${TOOLCHAIN_OLD}/${LANE_E}" "idle=0 removed a live-locked lane"
assert_present "${TOOLCHAIN_NEW}/${LANE_G}" "idle=0 removed a lane named by a live process"

# 6. Invalid settings fall back to the defaults and say so; `all` disables.
#    With E's lock released and no current lane, ordering alone decides.
kill "$LOCK_HOLDER_PID" 2>/dev/null || true
wait "$LOCK_HOLDER_PID" 2>/dev/null || true
make_target "$TOOLCHAIN_NEW" "$LANE_A" 4000
make_target "$TOOLCHAIN_NEW" "$LANE_B" 3000
make_target "$TOOLCHAIN_NEW" "$LANE_C" 10 2000
invalid_log="${TEST_ROOT}/invalid.log"
MEERKAT_PRE_PUSH_KEEP_LANES=banana MEERKAT_PRE_PUSH_LANE_IDLE_SECS=soon "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" --targets-root "$TARGETS_ROOT" \
  >"$invalid_log" 2>&1 || fail "invalid settings made the pruner fail"
assert_log_has "$invalid_log" "ignoring invalid pre-push lane retention 'banana'" \
  "invalid budget was not reported"
assert_log_has "$invalid_log" "ignoring invalid pre-push lane idle window 'soon'" \
  "invalid idle window was not reported"
assert_absent "${TOOLCHAIN_OLD}/${LANE_E}" "released lane E (oldest) survived the default fallback"
assert_absent "${TOOLCHAIN_NEW}/${LANE_A}" "default fallback did not prune the oldest"
assert_absent "${TOOLCHAIN_NEW}/${LANE_B}" "default fallback did not prune the second oldest"
assert_present "${TOOLCHAIN_NEW}/${LANE_C}" "default fallback pruned the newest lane"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "default fallback pruned the second-newest lane"
assert_present "${TOOLCHAIN_NEW}/${LANE_G}" "default fallback pruned a lane named by a live process"

make_target "$TOOLCHAIN_NEW" "$LANE_A" 4000
MEERKAT_PRE_PUSH_KEEP_LANES=all "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" --targets-root "$TARGETS_ROOT" \
  >/dev/null 2>&1 || fail "keep=all made the pruner fail"
assert_present "${TOOLCHAIN_NEW}/${LANE_A}" "keep=all pruned a lane"

# 7. Missing roots are not an error; relative roots are refused.
"$PRUNER" --hook-cache-root "${TEST_ROOT}/nonexistent" \
  --targets-root "${TEST_ROOT}/also-nonexistent" >/dev/null 2>&1 ||
  fail "missing roots made the pruner fail"
if "$PRUNER" --hook-cache-root relative/path >/dev/null 2>&1; then
  fail "a relative hook cache root was accepted"
fi

# 8. The dispatcher prunes only after a PASSED gate. A failed gate leaves every
#    peer lane in place, even lanes that are otherwise prunable.
mkdir -p "${DISPATCH_REPO}/scripts" "$DISPATCH_HARNESS" "$DISPATCH_CACHE"
cp "${REPO_ROOT}/scripts/repo-cargo" "${DISPATCH_REPO}/scripts/repo-cargo"
cp "${REPO_ROOT}/rust-toolchain.toml" "${DISPATCH_REPO}/rust-toolchain.toml"
git -C "$DISPATCH_REPO" init -q
git -C "$DISPATCH_REPO" add scripts/repo-cargo rust-toolchain.toml
git -C "$DISPATCH_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "base"
dispatch_base="$(git -C "$DISPATCH_REPO" rev-parse HEAD)"
git -C "$DISPATCH_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "candidate"
dispatch_head="$(git -C "$DISPATCH_REPO" rev-parse HEAD)"
dispatch_lane="pre-push-$(hash_path "$(git -C "$DISPATCH_REPO" rev-parse --show-toplevel)")"
dispatch_hook_cache="$(git -C "$DISPATCH_REPO" rev-parse --path-format=absolute --git-common-dir)/meerkat-hook-cache"

# The same cache root the dispatcher will resolve for its lane.
dispatch_lane_target="$(cd "$DISPATCH_REPO" && XDG_CACHE_HOME="$DISPATCH_CACHE" \
  RUST_LANE_ID="$dispatch_lane" ./scripts/repo-cargo --print-env |
  sed -n 's/^CARGO_TARGET_DIR=//p')"
[[ -n "$dispatch_lane_target" ]] || fail "could not resolve the dispatcher lane target"
dispatch_toolchain_dir="$(dirname "$dispatch_lane_target")"
PEER_X="pre-push-7777777777777777"
PEER_Y="pre-push-8888888888888888"
PEER_Z="pre-push-9999999999999999"
make_target "$dispatch_toolchain_dir" "$PEER_X" 9000
make_target "$dispatch_toolchain_dir" "$PEER_Y" 8000
make_target "$dispatch_toolchain_dir" "$PEER_Z" 7000

FAKE_PRE_COMMIT="${DISPATCH_HARNESS}/pre-commit"
cat > "$FAKE_PRE_COMMIT" <<'EOF'
#!/usr/bin/env bash
exit "${MEERKAT_FAKE_GATE_STATUS:-0}"
EOF
chmod +x "$FAKE_PRE_COMMIT"

run_dispatch() {
  local gate_status="$1"
  (
    cd "$DISPATCH_REPO"
    PATH="${DISPATCH_HARNESS}:$PATH" \
      XDG_CACHE_HOME="$DISPATCH_CACHE" \
      MEERKAT_FAKE_GATE_STATUS="$gate_status" \
      MEERKAT_SKIP_PRE_PUSH_TREE_CACHE=1 \
      MEERKAT_PRE_PUSH_BAZEL_OUTPUT_ROOT="${DISPATCH_HARNESS}/bazel-output" \
      RUST_LANE_ID="" \
      "$DISPATCHER" origin example.invalid \
      <<<"refs/heads/topic ${dispatch_head} refs/heads/topic ${dispatch_base}"
  )
}

failed_log="${TEST_ROOT}/dispatch-failed.log"
if run_dispatch 1 >"$failed_log" 2>&1; then
  fail "dispatcher passed a failing gate" "$(cat "$failed_log")"
fi
assert_log_has "$failed_log" "Meerkat pre-push gate FAILED" "failing gate was not reported"
assert_log_lacks "$failed_log" "pruned:" "a failed gate pruned lanes"
assert_log_lacks "$failed_log" "pre-push lane retention" "a failed gate ran retention"
for peer in "$PEER_X" "$PEER_Y" "$PEER_Z"; do
  assert_present "${dispatch_toolchain_dir}/${peer}" "a failed gate removed a peer lane"
done
[[ -f "${dispatch_lane_target}/.meerkat-pre-push-last-used" ]] ||
  fail "the dispatcher did not stamp its own lane as used"

passed_log="${TEST_ROOT}/dispatch-passed.log"
run_dispatch 0 >"$passed_log" 2>&1 || fail "dispatcher failed a passing gate" "$(cat "$passed_log")"
assert_log_has "$passed_log" "pre-push lane retention (keep 2, idle 21600s):" \
  "a passed gate did not run retention"
assert_log_has "$passed_log" "kept: ${dispatch_lane_target} (Cargo target; current lane)" \
  "the dispatcher's own lane was not logged as kept"
assert_log_has "$passed_log" "pruned: ${dispatch_toolchain_dir}/${PEER_X} (Cargo target; beyond budget 2" \
  "a passed gate did not prune the oldest peer lane"
assert_absent "${dispatch_toolchain_dir}/${PEER_X}" "a passed gate kept the oldest peer lane"
assert_absent "${dispatch_toolchain_dir}/${PEER_Y}" "a passed gate kept the second-oldest peer lane"
assert_present "${dispatch_toolchain_dir}/${PEER_Z}" "a passed gate pruned the peer lane within budget"
assert_present "$dispatch_lane_target" "a passed gate pruned the dispatcher's own lane"
assert_absent "${dispatch_hook_cache}/dispatcher-${dispatch_lane}.lock" \
  "the dispatcher left its lane lock behind after pruning"

echo "pre-push lane retention contract holds"
