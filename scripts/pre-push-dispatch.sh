#!/usr/bin/env bash
# Git's raw pre-push boundary. Unlike pre-commit's generic pre-push adapter,
# this sees every ref update on stdin. Validate one exact pushed object in an
# immutable detached worktree so concurrent edits cannot change tested bytes.
#
# Every nonzero exit from here must name its own cause. Git reports only
# "failed to push some refs", so an unattributed exit leaves the operator with
# no way to tell a failing hook from a failing dispatcher.
set -euo pipefail

dispatch_step="parsing hook arguments"

report_internal_failure() {
  local status="$1"
  local line="$2"
  printf '\n' >&2
  echo "Meerkat pre-push dispatcher failed while ${dispatch_step}" >&2
  echo "  exit status : ${status}" >&2
  echo "  location    : $(basename "${BASH_SOURCE[0]}"):${line}" >&2
  echo "This is the dispatcher itself, not a validation hook: no hook result" >&2
  echo "printed above is the cause of this push failure." >&2
}
trap 'report_internal_failure "$?" "$LINENO"' ERR

if [[ "$#" -ne 2 ]]; then
  echo "usage: pre-push-dispatch.sh <remote-name> <remote-url>" >&2
  exit 2
fi

REMOTE_NAME="$1"
REMOTE_URL="$2"
SOURCE_ROOT="$(git rev-parse --show-toplevel)"
ZERO_SHA="0000000000000000000000000000000000000000"
CACHE_VERSION="v1"
LOCK_WAIT_SECS="${MEERKAT_PRE_PUSH_DISPATCH_LOCK_WAIT_SECS:-3600}"

hash_path() {
  if command -v shasum >/dev/null 2>&1; then
    printf '%s' "$1" | shasum -a 256 | cut -c1-16
  elif command -v sha256sum >/dev/null 2>&1; then
    printf '%s' "$1" | sha256sum | cut -c1-16
  else
    printf '%s' "$1" | cksum | cut -d' ' -f1
  fi
}

sanitize_cache_key() {
  local raw="${1:-}"
  local key
  key="$(printf '%s' "${raw}" | LC_ALL=C tr -c 'A-Za-z0-9._-' '-')"
  printf '%s' "${key:-default}"
}

# Git exports repository-local variables to hooks. They must not cross into the
# detached validation worktree: nested git commands would otherwise continue
# targeting the source repository regardless of their cwd or `-C` argument.
while IFS= read -r git_local_env; do
  [[ -n "$git_local_env" ]] && unset "$git_local_env"
done < <(git -C "$SOURCE_ROOT" rev-parse --local-env-vars)

dispatch_step="reading pushed refs from stdin"
ref_count=0
local_ref=""
local_sha=""
remote_ref=""
remote_sha=""
while read -r next_local_ref next_local_sha next_remote_ref next_remote_sha; do
  [[ -n "${next_local_ref:-}" ]] || continue
  ref_count=$((ref_count + 1))
  local_ref="$next_local_ref"
  local_sha="$next_local_sha"
  remote_ref="$next_remote_ref"
  remote_sha="$next_remote_sha"
done

if [[ "$ref_count" -ne 1 ]]; then
  echo "Meerkat's pre-push gate requires exactly one ref update; received ${ref_count}." >&2
  echo "Push refs one at a time so every pushed object is validated exactly." >&2
  exit 1
fi

# A single deletion contains no local object to compile. It is safe to pass
# without running source validation; mixed deletion/update pushes are rejected
# by the single-ref rule above.
if [[ "$local_sha" == "$ZERO_SHA" ]]; then
  exit 0
fi

if ! pushed_commit="$(git -C "$SOURCE_ROOT" rev-parse --verify "${local_sha}^{commit}" 2>/dev/null)"; then
  echo "Pushed ref ${local_ref} does not resolve to a commit: ${local_sha}" >&2
  exit 1
fi
checked_out_commit="$(git -C "$SOURCE_ROOT" rev-parse --verify HEAD)"
if [[ "$pushed_commit" != "$checked_out_commit" ]]; then
  echo "Pre-push validation for ${local_ref} -> ${remote_ref} requires the pushed commit (${pushed_commit}) to equal checked-out HEAD (${checked_out_commit})." >&2
  echo "Push the checked-out branch or tag alone from its own checkout." >&2
  exit 1
fi

dispatch_step="resolving the exact-tree evidence cache"
pushed_tree="$(git -C "$SOURCE_ROOT" rev-parse "${pushed_commit}^{tree}")"
git_common_dir="$(git -C "$SOURCE_ROOT" rev-parse --path-format=absolute --git-common-dir)"
hook_cache_root="${git_common_dir}/meerkat-hook-cache"
hook_cache_dir="${hook_cache_root}/exact-tree"
# This accelerates identical tracked trees only. CI remains authoritative for
# toolchain, environment, credential, and other inputs outside the Git tree.
hook_stamp="${hook_cache_dir}/${CACHE_VERSION}-${pushed_tree}.ok"
# Each source worktree gets a stable validation lane unless the caller names
# one explicitly. The detached worktree, Cargo target, Bazel output base, and
# dispatcher lock are all lane-owned, so unrelated worktrees can validate in
# parallel without sharing mutable build state. Concurrent pushes from the
# same source worktree still serialize on the same lane.
default_validation_lane="pre-push-$(hash_path "${SOURCE_ROOT}")"
validation_lane="$(sanitize_cache_key "${RUST_LANE_ID:-${default_validation_lane}}")"
dispatcher_lock_dir="${hook_cache_root}/dispatcher-${validation_lane}.lock"
dispatcher_lock_pid="${dispatcher_lock_dir}/pid"
validation_tree="${hook_cache_root}/worktrees/${validation_lane}"
validation_run_root=""
validation_tree_owned=0
dispatcher_lock_held=0
export RUST_LANE_ID="${validation_lane}"

release_dispatcher_lock() {
  if [[ "${dispatcher_lock_held}" -eq 1 ]]; then
    rm -rf -- "${dispatcher_lock_dir}"
    dispatcher_lock_held=0
  fi
}

cleanup() {
  local pending_status=$?
  if [[ "${validation_tree_owned}" -eq 1 && -n "${validation_tree}" && -d "${validation_tree}" ]]; then
    if ! git -C "$SOURCE_ROOT" worktree remove --force "$validation_tree" >/dev/null 2>&1; then
      echo "note: validation worktree left behind: ${validation_tree}" >&2
      echo "      prune it with: git -C ${SOURCE_ROOT} worktree prune" >&2
    fi
  fi
  if [[ -n "${validation_run_root}" && -d "${validation_run_root}" ]]; then
    if ! rm -rf -- "${validation_run_root}" 2>/dev/null; then
      echo "note: validation scratch directory left behind: ${validation_run_root}" >&2
    fi
  fi
  release_dispatcher_lock
  # Residue must never decide the push. A failing command inside an EXIT trap
  # under `set -e` otherwise rewrites a fully passing gate into a bare exit 1
  # with nothing but hook successes on screen, which is exactly the
  # unattributable push failure this dispatcher used to produce.
  exit "$pending_status"
}
trap cleanup EXIT

acquire_dispatcher_lock() {
  local start_ts now_ts owner_pid last_notice_ts stale_lock_dir lock_mtime
  start_ts="$(date +%s)"
  last_notice_ts="${start_ts}"
  while ! mkdir "${dispatcher_lock_dir}" 2>/dev/null; do
    owner_pid=""
    if [[ -f "${dispatcher_lock_pid}" ]]; then
      owner_pid="$(cat "${dispatcher_lock_pid}" 2>/dev/null || true)"
    fi
    if [[ ! "${owner_pid}" =~ ^[0-9]+$ ]]; then
      owner_pid=""
    fi
    lock_mtime=""
    if stat -f %m "${dispatcher_lock_dir}" >/dev/null 2>&1; then
      lock_mtime="$(stat -f %m "${dispatcher_lock_dir}")"
    elif stat -c %Y "${dispatcher_lock_dir}" >/dev/null 2>&1; then
      lock_mtime="$(stat -c %Y "${dispatcher_lock_dir}")"
    fi
    now_ts="$(date +%s)"
    if { [[ -n "${owner_pid}" ]] && ! kill -0 "${owner_pid}" 2>/dev/null; } ||
      { [[ -z "${owner_pid}" ]] && [[ "${lock_mtime:-0}" =~ ^[0-9]+$ ]] &&
        (( now_ts - lock_mtime >= 5 )); }; then
      stale_lock_dir="${dispatcher_lock_dir}.stale.$$"
      if mv "${dispatcher_lock_dir}" "${stale_lock_dir}" 2>/dev/null; then
        rm -rf -- "${stale_lock_dir}"
      fi
      continue
    fi
    if (( now_ts - start_ts >= LOCK_WAIT_SECS )); then
      echo "Timed out waiting ${LOCK_WAIT_SECS}s for the repository pre-push dispatcher lock." >&2
      return 1
    fi
    if (( now_ts - last_notice_ts >= 30 )); then
      echo "Waiting for repository pre-push validation already owned by pid ${owner_pid:-unknown}..." >&2
      last_notice_ts="${now_ts}"
    fi
    sleep 1
  done
  dispatcher_lock_held=1
  printf '%s\n' "$$" >"${dispatcher_lock_pid}"
}

mkdir -p "$hook_cache_dir" "$(dirname "${validation_tree}")"

if [[ "${MEERKAT_SKIP_PRE_PUSH_TREE_CACHE:-0}" != "1" && -f "$hook_stamp" ]]; then
  echo "complete pre-push gate already validated for tree ${pushed_tree}; reusing exact-tree evidence."
  exit 0
fi

dispatch_step="waiting for the repository pre-push validation lane"
acquire_dispatcher_lock

# Another push may have validated this exact tree while this process waited.
if [[ "${MEERKAT_SKIP_PRE_PUSH_TREE_CACHE:-0}" != "1" && -f "$hook_stamp" ]]; then
  echo "complete pre-push gate already validated for tree ${pushed_tree}; reusing exact-tree evidence."
  exit 0
fi

dispatch_step="creating the stable detached validation worktree"
git -C "$SOURCE_ROOT" worktree remove --force "$validation_tree" >/dev/null 2>&1 || true
if [[ -e "${validation_tree}" || -L "${validation_tree}" ]]; then
  case "${validation_tree}" in
    "${hook_cache_root}"/worktrees/*)
      rm -rf -- "${validation_tree}"
      ;;
    *)
      echo "Refusing to remove unexpected validation path: ${validation_tree}" >&2
      exit 1
      ;;
  esac
fi
validation_run_root="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-exact.XXXXXX")"

validation_tree_owned=1
git -C "$SOURCE_ROOT" worktree add --detach --quiet "$validation_tree" "$pushed_commit"

export PRE_COMMIT_REMOTE_NAME="$REMOTE_NAME"
export PRE_COMMIT_REMOTE_URL="$REMOTE_URL"
export PRE_COMMIT_TO_REF="$pushed_commit"
empty_tree="$(git -C "$SOURCE_ROOT" hash-object -t tree /dev/null)"
if [[ "$remote_sha" == "$ZERO_SHA" ]]; then
  PRE_COMMIT_FROM_REF="$empty_tree"
  # A newly created branch still has a proven diff base. Use the fetched
  # remote default branch when available so pre-commit does not reinterpret
  # branch creation as "every repository file changed" and launch unrelated
  # machine/TLC lanes. Tags remain fail-closed on the empty tree unless their
  # exact tree already has reusable evidence from the branch push.
  if [[ "$local_ref" == refs/heads/* ]]; then
    remote_default_ref="$(
      git -C "$SOURCE_ROOT" symbolic-ref --quiet \
        "refs/remotes/${REMOTE_NAME}/HEAD" 2>/dev/null || true
    )"
    if [[ -z "$remote_default_ref" ]] &&
      git -C "$SOURCE_ROOT" rev-parse --verify \
        "refs/remotes/${REMOTE_NAME}/main^{commit}" >/dev/null 2>&1; then
      remote_default_ref="refs/remotes/${REMOTE_NAME}/main"
    fi
    if [[ -n "$remote_default_ref" ]] &&
      git -C "$SOURCE_ROOT" rev-parse --verify \
        "${remote_default_ref}^{commit}" >/dev/null 2>&1; then
      PRE_COMMIT_FROM_REF="$(
        git -C "$SOURCE_ROOT" merge-base "$pushed_commit" "$remote_default_ref"
      )"
    fi
  fi
else
  PRE_COMMIT_FROM_REF="$remote_sha"
fi
export PRE_COMMIT_FROM_REF
# Keep Bazel's disk state stable across the same detached validation lane. The
# workspace path above is stable too, so Bazel can retain both the output base
# and its server instead of starting a new pair for every pushed tree.
if [[ -z "${MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE:-}" ]]; then
  bazel_output_root="${MEERKAT_PRE_PUSH_BAZEL_OUTPUT_ROOT:-${XDG_CACHE_HOME:-${HOME}/.cache}/meerkat/pre-push-bazel}"
  common_dir_hash="$(hash_path "${git_common_dir}")"
  export MEERKAT_PRE_PUSH_BAZEL_OUTPUT_BASE="${bazel_output_root}/repo-${common_dir_hash}-${validation_lane}"
fi

cd "$validation_tree"

# The hook transcript is captured so a failure can name the hook that caused
# it. Python buffers block-wise when its stdout is a pipe; unbuffering keeps
# the operator's terminal live through the long deterministic lanes.
gate_log="${validation_run_root}/push-stage-hooks.log"
dispatch_step="running push-stage validation hooks"
# Hook failure is a reported outcome, not a dispatcher fault: the ERR trap fires
# even with errexit disabled, so it is lifted for exactly this pipeline. Capture
# the whole PIPESTATUS array at once; any later command resets it.
trap - ERR
set +e
if [[ "$PRE_COMMIT_FROM_REF" == "$empty_tree" ]]; then
  PYTHONUNBUFFERED=1 pre-commit run --config .pre-commit-config.yaml \
    --hook-stage pre-push --all-files 2>&1 | tee "$gate_log"
else
  PYTHONUNBUFFERED=1 pre-commit run --config .pre-commit-config.yaml \
    --hook-stage pre-push --from-ref "$PRE_COMMIT_FROM_REF" \
    --to-ref "$pushed_commit" \
    2>&1 | tee "$gate_log"
fi
gate_pipeline_status=("${PIPESTATUS[@]}")
set -e
trap 'report_internal_failure "$?" "$LINENO"' ERR
gate_status="${gate_pipeline_status[0]}"
capture_status="${gate_pipeline_status[1]:-0}"

if [[ "$capture_status" -ne 0 ]]; then
  echo "note: could not capture the hook transcript (tee exit ${capture_status});" >&2
  echo "      failure attribution below may be incomplete." >&2
fi

if [[ "$gate_status" -ne 0 ]]; then
  gate_plain="$(sed -E $'s/\x1b\\[[0-9;]*[A-Za-z]//g' "$gate_log" 2>/dev/null || true)"
  failed_names="$(sed -n -E 's/^(.+[^.])[.]{2,}Failed$/\1/p' <<<"$gate_plain")"
  failed_ids="$(sed -n -E 's/^- hook id: (.+)$/\1/p' <<<"$gate_plain")"
  printf '\n' >&2
  echo "Meerkat pre-push gate FAILED (pre-commit exit ${gate_status})." >&2
  if [[ -n "$failed_names" ]]; then
    echo "Failing push-stage hook(s):" >&2
    while IFS= read -r failed_name; do
      if [[ -n "$failed_name" ]]; then
        echo "  - ${failed_name}" >&2
      fi
    done <<<"$failed_names"
    if [[ -n "$failed_ids" ]]; then
      echo "Rerun the failing hook alone with:" >&2
      while IFS= read -r failed_id; do
        if [[ -n "$failed_id" ]]; then
          echo "  pre-commit run --hook-stage pre-push --all-files ${failed_id}" >&2
        fi
      done <<<"$failed_ids"
    fi
  else
    echo "No hook reported Failed, so the failure is in the gate harness, not" >&2
    echo "in a validation hook. Last 40 transcript lines:" >&2
    tail -40 "$gate_log" >&2 || true
  fi
  exit "$gate_status"
fi

dispatch_step="recording exact-tree validation evidence"
stamp_tmp="${hook_stamp}.tmp.$$"
printf 'tree=%s\ncommit=%s\n' "$pushed_tree" "$pushed_commit" > "$stamp_tmp"
mv "$stamp_tmp" "$hook_stamp"
