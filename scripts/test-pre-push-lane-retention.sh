#!/usr/bin/env bash
# Contract test for bounded pre-push lane retention.
#
# Reproduces the leak shape: four `pre-push-<16 hex>` lanes accumulated under
# one repository's hook cache and Cargo targets root, most of them owned by
# source worktrees that no longer exist. The pruner must keep the newest N
# (by explicit last-used stamp, falling back to mtime), never touch the caller's
# lane or a lane whose dispatcher lock is live, and never touch anything whose
# name is not an exact lane id.
#
# Offline and file-only: the fake cache layout lives under a temp root.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PRUNER="${REPO_ROOT}/scripts/pre-push-prune-lanes.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-lane-retention.XXXXXX")"
SOURCE_REPO="${TEST_ROOT}/source"
HOOK_CACHE_ROOT="${TEST_ROOT}/git-common/meerkat-hook-cache"
WORKTREES="${HOOK_CACHE_ROOT}/worktrees"
TARGETS_ROOT="${TEST_ROOT}/rust-workspaces/meerkat-0123456789/targets/v4"
TOOLCHAIN_OLD="${TARGETS_ROOT}/1.93.0-x86_64-unknown-linux-gnu-aaaaaaaaaa"
TOOLCHAIN_NEW="${TARGETS_ROOT}/1.94.1-x86_64-unknown-linux-gnu-bbbbbbbbbb"
OUTSIDE_ROOT="${TEST_ROOT}/other-repo-targets/v4/toolchain"
SLEEPER_PID=""

cleanup() {
  local pending_status=$?
  if [[ -n "$SLEEPER_PID" ]]; then
    kill "$SLEEPER_PID" 2>/dev/null || true
    wait "$SLEEPER_PID" 2>/dev/null || true
  fi
  chmod -R u+rwx "$TEST_ROOT" 2>/dev/null || true
  rm -rf -- "$TEST_ROOT"
  exit "$pending_status"
}
trap cleanup EXIT

fail() {
  echo "pre-push lane retention contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  ${extra}" >&2
  done
  exit 1
}

assert_present() {
  [[ -d "$1" ]] || fail "$2" "missing: $1"
}

assert_absent() {
  [[ ! -e "$1" && ! -L "$1" ]] || fail "$2" "still present: $1"
}

# touch_at <path> <minutes-ago>: portable explicit timestamp (GNU and BSD touch).
touch_at() {
  local path="$1"
  local minutes_ago="$2"
  local stamp
  if stamp="$(date -u -d "@$(( $(date +%s) - minutes_ago * 60 ))" +%Y%m%d%H%M.%S 2>/dev/null)"; then
    :
  else
    stamp="$(date -u -r "$(( $(date +%s) - minutes_ago * 60 ))" +%Y%m%d%H%M.%S)"
  fi
  touch -t "$stamp" "$path"
}

LANE_A="pre-push-aaaaaaaaaaaaaaaa" # oldest, orphaned
LANE_B="pre-push-bbbbbbbbbbbbbbbb" # second oldest, orphaned
LANE_C="pre-push-cccccccccccccccc" # newest of the peers
LANE_D="pre-push-dddddddddddddddd" # the caller's own lane, stamped old on purpose
LANE_E="pre-push-eeeeeeeeeeeeeeee" # oldest of all, but its dispatcher lock is live

# make_target <toolchain-dir> <lane> <minutes-ago>: a fake Cargo target whose
# directory mtime is fresh but whose explicit stamp carries the real last use.
make_target() {
  local toolchain_dir="$1" lane="$2" minutes_ago="$3"
  mkdir -p "${toolchain_dir}/${lane}/debug/deps"
  : > "${toolchain_dir}/${lane}/debug/deps/libfake.rlib"
  : > "${toolchain_dir}/${lane}/.meerkat-pre-push-last-used"
  touch_at "${toolchain_dir}/${lane}/.meerkat-pre-push-last-used" "$minutes_ago"
}

# make_worktree <lane> <minutes-ago>: hook worktrees carry no stamp (they must
# stay byte-clean), so ordering falls back to directory mtime.
make_worktree() {
  local lane="$1" minutes_ago="$2"
  mkdir -p "${WORKTREES}/${lane}"
  touch_at "${WORKTREES}/${lane}" "$minutes_ago"
}

git -C "$TEST_ROOT" init -q "$SOURCE_REPO"
git -C "$SOURCE_REPO" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit --allow-empty -qm "base"

mkdir -p "$WORKTREES" "$TOOLCHAIN_OLD" "$TOOLCHAIN_NEW" "$OUTSIDE_ROOT"

# Targets: two toolchain generations under one repo; retention ranks across
# both so a toolchain bump still bounds the total.
make_target "$TOOLCHAIN_OLD" "$LANE_A" 400
make_target "$TOOLCHAIN_OLD" "$LANE_B" 300
make_target "$TOOLCHAIN_NEW" "$LANE_C" 10
make_target "$TOOLCHAIN_NEW" "$LANE_D" 200
make_target "$TOOLCHAIN_OLD" "$LANE_E" 500
# Directory mtimes are deliberately misleading: the oldest-stamped lane gets the
# freshest directory mtime, so only the stamp can order them correctly.
touch "${TOOLCHAIN_OLD}/${LANE_A}"
touch_at "${TOOLCHAIN_NEW}/${LANE_C}" 900

# Names that must never be touched, however old.
for untouchable in meerkat-m1-856ed44470 release-package-alpha pre-push-short \
  pre-push-ABCDEF0123456789 pre-push-0123456789abcdef0; do
  mkdir -p "${TOOLCHAIN_NEW}/${untouchable}"
  touch_at "${TOOLCHAIN_NEW}/${untouchable}" 9000
  mkdir -p "${WORKTREES}/${untouchable}"
  touch_at "${WORKTREES}/${untouchable}" 9000
done
# A lane-shaped file (not a directory) and a lane-shaped symlink are not lanes.
: > "${TOOLCHAIN_NEW}/pre-push-ffffffffffffffff"
ln -s "${TOOLCHAIN_NEW}/${LANE_C}" "${TOOLCHAIN_NEW}/pre-push-1111111111111111"
# Lane-shaped directories outside the given roots are somebody else's.
mkdir -p "${OUTSIDE_ROOT}/${LANE_A}"
touch_at "${OUTSIDE_ROOT}/${LANE_A}" 9000

# Hook worktrees: one real registered git worktree (the common crash residue)
# plus plain directories left by a hard kill.
git -C "$SOURCE_REPO" worktree add --detach --quiet "${WORKTREES}/${LANE_A}" HEAD
touch_at "${WORKTREES}/${LANE_A}" 400
make_worktree "$LANE_B" 300
make_worktree "$LANE_C" 10
make_worktree "$LANE_D" 200
make_worktree "$LANE_E" 500

# Lane E is mid-validation: its dispatcher lock names a live pid.
sleep 600 &
SLEEPER_PID=$!
mkdir -p "${HOOK_CACHE_ROOT}/dispatcher-${LANE_E}.lock"
printf '%s\n' "$SLEEPER_PID" > "${HOOK_CACHE_ROOT}/dispatcher-${LANE_E}.lock/pid"
# Lane A once had a lock too, but its owner is gone: stale locks do not protect.
mkdir -p "${HOOK_CACHE_ROOT}/dispatcher-${LANE_A}.lock"
printf '%s\n' "999999999" > "${HOOK_CACHE_ROOT}/dispatcher-${LANE_A}.lock/pid"
touch_at "${HOOK_CACHE_ROOT}/dispatcher-${LANE_A}.lock" 400

# Abandoned rename residue from a pruner that died mid-delete must be swept;
# residue owned by a live pruner must not.
mkdir -p "${TOOLCHAIN_NEW}/pre-push-2222222222222222.pruning.999999999"
mkdir -p "${TOOLCHAIN_NEW}/pre-push-3333333333333333.pruning.${SLEEPER_PID}"

# 1. A dry run decides but deletes nothing.
dry_log="${TEST_ROOT}/dry.log"
MEERKAT_PRE_PUSH_KEEP_LANES=2 "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" \
  --targets-root "$TARGETS_ROOT" \
  --current-lane "$LANE_D" \
  --source-root "$SOURCE_REPO" \
  --dry-run >"$dry_log" 2>&1 || fail "dry run exited nonzero" "$(cat "$dry_log")"
grep -Fq "would prune Cargo target: ${TOOLCHAIN_OLD}/${LANE_A}" "$dry_log" ||
  fail "dry run did not name the oldest target" "$(cat "$dry_log")"
for lane in "$LANE_A" "$LANE_B" "$LANE_C" "$LANE_D" "$LANE_E"; do
  assert_present "${WORKTREES}/${lane}" "dry run removed a worktree"
done
assert_present "${TOOLCHAIN_OLD}/${LANE_A}" "dry run removed a target"
assert_present "${TOOLCHAIN_NEW}/pre-push-2222222222222222.pruning.999999999" \
  "dry run removed abandoned residue"

# 2. Budget 3: current lane D and live lane E are exempt and count first, C is
#    the newest peer and fills the remaining slot, A and B go.
run_log="${TEST_ROOT}/run.log"
MEERKAT_PRE_PUSH_KEEP_LANES=3 "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" \
  --targets-root "$TARGETS_ROOT" \
  --current-lane "$LANE_D" \
  --source-root "$SOURCE_REPO" >"$run_log" 2>&1 || fail "prune exited nonzero" "$(cat "$run_log")"

assert_absent "${TOOLCHAIN_OLD}/${LANE_A}" "oldest target survived (stamp ordering ignored)"
assert_absent "${TOOLCHAIN_OLD}/${LANE_B}" "second-oldest target survived"
assert_present "${TOOLCHAIN_NEW}/${LANE_C}" "newest peer target was pruned (dir mtime used over stamp)"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "the caller's own lane target was pruned"
assert_present "${TOOLCHAIN_OLD}/${LANE_E}" "an in-use lane target was pruned"
[[ -f "${TOOLCHAIN_OLD}/${LANE_E}/debug/deps/libfake.rlib" ]] || fail "in-use lane contents were disturbed"

assert_absent "${WORKTREES}/${LANE_A}" "oldest hook worktree (registered) survived"
assert_absent "${WORKTREES}/${LANE_B}" "second-oldest hook worktree survived"
assert_present "${WORKTREES}/${LANE_C}" "newest peer hook worktree was pruned"
assert_present "${WORKTREES}/${LANE_D}" "the caller's own hook worktree was pruned"
assert_present "${WORKTREES}/${LANE_E}" "an in-use hook worktree was pruned"
if git -C "$SOURCE_REPO" worktree list --porcelain | grep -Fq "${WORKTREES}/${LANE_A}"; then
  fail "pruned hook worktree is still registered with git"
fi

for untouchable in meerkat-m1-856ed44470 release-package-alpha pre-push-short \
  pre-push-ABCDEF0123456789 pre-push-0123456789abcdef0; do
  assert_present "${TOOLCHAIN_NEW}/${untouchable}" "non-lane target name was removed"
  assert_present "${WORKTREES}/${untouchable}" "non-lane worktree name was removed"
done
[[ -f "${TOOLCHAIN_NEW}/pre-push-ffffffffffffffff" ]] || fail "lane-shaped file was removed"
[[ -L "${TOOLCHAIN_NEW}/pre-push-1111111111111111" ]] || fail "lane-shaped symlink was removed"
assert_present "${OUTSIDE_ROOT}/${LANE_A}" "a lane outside the given roots was removed"
assert_absent "${TOOLCHAIN_NEW}/pre-push-2222222222222222.pruning.999999999" \
  "abandoned prune residue survived"
assert_present "${TOOLCHAIN_NEW}/pre-push-3333333333333333.pruning.${SLEEPER_PID}" \
  "a live pruner's staged directory was removed"
for own_residue in "${TOOLCHAIN_OLD}"/*.pruning.* "${TOOLCHAIN_NEW}"/*.pruning.* "${WORKTREES}"/*.pruning.*; do
  [[ -e "$own_residue" ]] || continue
  [[ "$own_residue" == *".pruning.${SLEEPER_PID}" ]] && continue
  [[ "$own_residue" == *".pruning.999999999" ]] && continue
  fail "pruner left rename residue behind" "$own_residue"
done
grep -Fq "pruned Cargo target: ${TOOLCHAIN_OLD}/${LANE_A}" "$run_log" ||
  fail "prune log does not name the removed target" "$(cat "$run_log")"

# 3. Re-running is a no-op at the budget.
rerun_log="${TEST_ROOT}/rerun.log"
MEERKAT_PRE_PUSH_KEEP_LANES=3 "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" --targets-root "$TARGETS_ROOT" \
  --current-lane "$LANE_D" --source-root "$SOURCE_REPO" >"$rerun_log" 2>&1 ||
  fail "idempotent rerun exited nonzero" "$(cat "$rerun_log")"
if grep -q '^pruned ' "$rerun_log"; then
  fail "a rerun at the budget removed more lanes" "$(cat "$rerun_log")"
fi

# 4. The default budget (2) is a total bound: the two exempt lanes fill it, so
#    the newest peer goes too, while the exempt lanes stay even above budget.
env -u MEERKAT_PRE_PUSH_KEEP_LANES "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" --targets-root "$TARGETS_ROOT" \
  --current-lane "$LANE_D" --source-root "$SOURCE_REPO" >/dev/null 2>&1 ||
  fail "default-budget prune exited nonzero"
assert_absent "${TOOLCHAIN_NEW}/${LANE_C}" "default budget retained a non-exempt peer target"
assert_absent "${WORKTREES}/${LANE_C}" "default budget retained a non-exempt peer worktree"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "default budget removed the caller's lane"
assert_present "${TOOLCHAIN_OLD}/${LANE_E}" "default budget removed an in-use lane"
MEERKAT_PRE_PUSH_KEEP_LANES=1 "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" --targets-root "$TARGETS_ROOT" \
  --current-lane "$LANE_D" --source-root "$SOURCE_REPO" >/dev/null 2>&1 ||
  fail "keep=1 prune exited nonzero"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "keep=1 removed the caller's lane"
assert_present "${TOOLCHAIN_OLD}/${LANE_E}" "keep=1 removed an in-use lane"
assert_present "${WORKTREES}/${LANE_D}" "keep=1 removed the caller's hook worktree"
assert_present "${WORKTREES}/${LANE_E}" "keep=1 removed an in-use hook worktree"

# 5. Invalid budgets fall back to the default (2) and say so; `all` disables.
#    Without a current lane and with E's lock released, ordering alone decides.
kill "$SLEEPER_PID" 2>/dev/null || true
wait "$SLEEPER_PID" 2>/dev/null || true
SLEEPER_PID=""
make_target "$TOOLCHAIN_NEW" "$LANE_A" 400
make_target "$TOOLCHAIN_NEW" "$LANE_B" 300
make_target "$TOOLCHAIN_NEW" "$LANE_C" 10
invalid_log="${TEST_ROOT}/invalid.log"
MEERKAT_PRE_PUSH_KEEP_LANES=banana "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" --targets-root "$TARGETS_ROOT" \
  >"$invalid_log" 2>&1 || fail "invalid budget made the pruner fail"
grep -Fq "ignoring invalid pre-push lane retention 'banana'" "$invalid_log" ||
  fail "invalid budget was not reported" "$(cat "$invalid_log")"
assert_absent "${TOOLCHAIN_OLD}/${LANE_E}" "released lane E (oldest) survived the default fallback"
assert_absent "${TOOLCHAIN_NEW}/${LANE_A}" "default fallback did not prune the oldest"
assert_absent "${TOOLCHAIN_NEW}/${LANE_B}" "default fallback did not prune the second oldest"
assert_present "${TOOLCHAIN_NEW}/${LANE_C}" "default fallback pruned the newest lane"
assert_present "${TOOLCHAIN_NEW}/${LANE_D}" "default fallback pruned the second-newest lane"

make_target "$TOOLCHAIN_NEW" "$LANE_A" 400
MEERKAT_PRE_PUSH_KEEP_LANES=all "$PRUNER" \
  --hook-cache-root "$HOOK_CACHE_ROOT" --targets-root "$TARGETS_ROOT" \
  >/dev/null 2>&1 || fail "keep=all made the pruner fail"
assert_present "${TOOLCHAIN_NEW}/${LANE_A}" "keep=all pruned a lane"

# 6. Missing roots are not an error.
"$PRUNER" --hook-cache-root "${TEST_ROOT}/nonexistent" \
  --targets-root "${TEST_ROOT}/also-nonexistent" >/dev/null 2>&1 ||
  fail "missing roots made the pruner fail"

echo "pre-push lane retention contract holds"
