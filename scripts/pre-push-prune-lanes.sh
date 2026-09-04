#!/usr/bin/env bash
# Bounded retention for pre-push validation lanes.
#
# Every source worktree that pushes gets a stable `pre-push-<16 hex>` lane
# (pre-push-dispatch.sh). The lane owns a detached hook worktree under
# `<git-common-dir>/meerkat-hook-cache/worktrees/` and a Cargo target directory
# under `<rust-workspaces cache>/<repo-key>/targets/<schema>/<toolchain>/`.
# Nothing else removes them: when the source worktree disappears, its lane's
# tens of gigabytes of target output stay behind forever.
#
# This script keeps at most N lanes per root and deletes the rest, oldest
# first. It is deliberately narrow:
#   - only direct children of the given roots are considered;
#   - only names matching the exact `pre-push-<16 hex>` lane shape (plus this
#     script's own `.pruning.<pid>` rename residue) are ever removed;
#   - the current lane and every lane whose dispatcher lock is held by a live
#     process are never removed, even beyond the retention budget;
#   - a candidate is renamed before deletion, so concurrent pruners cannot
#     both act on the same directory.
# Ranking uses the explicit `.meerkat-pre-push-last-used` stamp the dispatcher
# touches on every validation; directories without a stamp (legacy lanes and
# hook worktrees, which must stay byte-clean) fall back to their own mtime.
#
# Exit status is 0 whenever the retention policy was applied; a failure to
# remove one candidate is reported as a note, never as a push failure.
set -euo pipefail

LANE_STAMP_NAME=".meerkat-pre-push-last-used"
DEFAULT_KEEP=2
LOCK_PID_GRACE_SECS=5

usage() {
  cat <<'EOF'
usage: pre-push-prune-lanes.sh --hook-cache-root <dir> [options]

  --hook-cache-root <dir>   meerkat-hook-cache root (holds worktrees/ and the
                            dispatcher-<lane>.lock directories)
  --targets-root <dir>      Cargo targets schema root (.../targets/v4); every
                            <dir>/<toolchain>/pre-push-<hex> is a candidate.
                            May be repeated.
  --current-lane <lane>     lane owned by the caller; never pruned
  --source-root <dir>       Git checkout used for `git worktree remove/prune`
  --keep <n|all>            lanes to retain per root (default: value of
                            MEERKAT_PRE_PUSH_KEEP_LANES, then 2)
  --dry-run                 print decisions without deleting anything
EOF
}

hook_cache_root=""
targets_roots=()
current_lane=""
source_root=""
keep_raw="${MEERKAT_PRE_PUSH_KEEP_LANES:-}"
dry_run=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --hook-cache-root)
      [[ $# -ge 2 ]] || { echo "error: --hook-cache-root requires a directory" >&2; exit 2; }
      hook_cache_root="$2"
      shift 2
      ;;
    --targets-root)
      [[ $# -ge 2 ]] || { echo "error: --targets-root requires a directory" >&2; exit 2; }
      targets_roots+=("$2")
      shift 2
      ;;
    --current-lane)
      [[ $# -ge 2 ]] || { echo "error: --current-lane requires a lane id" >&2; exit 2; }
      current_lane="$2"
      shift 2
      ;;
    --source-root)
      [[ $# -ge 2 ]] || { echo "error: --source-root requires a directory" >&2; exit 2; }
      source_root="$2"
      shift 2
      ;;
    --keep)
      [[ $# -ge 2 ]] || { echo "error: --keep requires a count" >&2; exit 2; }
      keep_raw="$2"
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$hook_cache_root" ]]; then
  echo "error: --hook-cache-root is required" >&2
  exit 2
fi

case "$keep_raw" in
  "")
    keep="$DEFAULT_KEEP"
    ;;
  all | unlimited)
    echo "pre-push lane retention disabled (MEERKAT_PRE_PUSH_KEEP_LANES=${keep_raw})."
    exit 0
    ;;
  *)
    if [[ "$keep_raw" =~ ^[0-9]+$ ]] && (( keep_raw >= 1 )); then
      keep="$keep_raw"
    else
      echo "note: ignoring invalid pre-push lane retention '${keep_raw}'; keeping ${DEFAULT_KEEP}." >&2
      keep="$DEFAULT_KEEP"
    fi
    ;;
esac

is_lane_name() {
  [[ "$1" =~ ^pre-push-[0-9a-f]{16}$ ]]
}

is_pruning_residue() {
  [[ "$1" =~ ^pre-push-[0-9a-f]{16}\.pruning\.[0-9]+$ ]]
}

pid_alive() {
  [[ "$1" =~ ^[0-9]+$ ]] && kill -0 "$1" 2>/dev/null
}

mtime_of() {
  local path="$1"
  if stat -f %m "$path" >/dev/null 2>&1; then
    stat -f %m "$path"
  else
    stat -c %Y "$path"
  fi
}

# A lane is in use while its dispatcher lock is owned by a live process. A lock
# whose pid file has not been written yet is honored for a short grace period,
# mirroring the dispatcher's own stale-lock rule.
lane_in_use() {
  local lane="$1"
  local lock_dir="${hook_cache_root}/dispatcher-${lane}.lock"
  local owner_pid lock_mtime now_ts
  [[ -d "$lock_dir" ]] || return 1
  owner_pid="$(cat "${lock_dir}/pid" 2>/dev/null || true)"
  if [[ -n "$owner_pid" ]]; then
    pid_alive "$owner_pid"
    return
  fi
  lock_mtime="$(mtime_of "$lock_dir" 2>/dev/null || echo 0)"
  now_ts="$(date +%s)"
  (( now_ts - lock_mtime < LOCK_PID_GRACE_SECS ))
}

last_used_epoch() {
  local dir="$1"
  if [[ -f "${dir}/${LANE_STAMP_NAME}" ]]; then
    mtime_of "${dir}/${LANE_STAMP_NAME}"
  else
    mtime_of "$dir"
  fi
}

remove_candidate() {
  local dir="$1"
  local kind="$2"
  local staged="${dir}.pruning.$$"

  if [[ "$dry_run" -eq 1 ]]; then
    echo "would prune ${kind}: ${dir}"
    return 0
  fi
  if [[ "$kind" == "hook worktree" && -n "$source_root" ]]; then
    git -C "$source_root" worktree remove --force "$dir" >/dev/null 2>&1 || true
    [[ -e "$dir" || -L "$dir" ]] || { echo "pruned ${kind}: ${dir}"; return 0; }
  fi
  # Claim the directory atomically; a concurrent pruner that loses the rename
  # simply moves on.
  if ! mv "$dir" "$staged" 2>/dev/null; then
    return 0
  fi
  if rm -rf -- "$staged" 2>/dev/null; then
    echo "pruned ${kind}: ${dir}"
  else
    echo "note: could not fully remove ${kind} ${dir} (residue at ${staged})" >&2
  fi
}

remove_dead_residue() {
  local parent="$1"
  local kind="$2"
  local entry name pid
  for entry in "$parent"/pre-push-*.pruning.*; do
    [[ -e "$entry" || -L "$entry" ]] || continue
    name="$(basename "$entry")"
    is_pruning_residue "$name" || continue
    pid="${name##*.}"
    pid_alive "$pid" && continue
    if [[ "$dry_run" -eq 1 ]]; then
      echo "would remove abandoned ${kind} prune residue: ${entry}"
    elif rm -rf -- "$entry" 2>/dev/null; then
      echo "removed abandoned ${kind} prune residue: ${entry}"
    else
      echo "note: could not remove abandoned ${kind} prune residue: ${entry}" >&2
    fi
  done
}

# prune_root <kind> <parent>...: rank every lane directory directly under the
# given parents together and keep at most `keep` of them. The caller's lane and
# every lane with a live dispatcher lock are exempt and count toward the budget
# first; the newest remaining lanes fill whatever budget is left.
prune_root() {
  local kind="$1"
  shift
  local parent entry name lane epoch
  local -a ranked=()
  local retained=0

  for parent in "$@"; do
    [[ -d "$parent" ]] || continue
    remove_dead_residue "$parent" "$kind"
    for entry in "$parent"/pre-push-*; do
      [[ -d "$entry" && ! -L "$entry" ]] || continue
      name="$(basename "$entry")"
      is_lane_name "$name" || continue
      if [[ -n "$current_lane" && "$name" == "$current_lane" ]] || lane_in_use "$name"; then
        retained=$((retained + 1))
        continue
      fi
      epoch="$(last_used_epoch "$entry" 2>/dev/null || echo 0)"
      ranked+=("${epoch}"$'\t'"${entry}")
    done
  done
  [[ "${#ranked[@]}" -gt 0 ]] || return 0

  while IFS=$'\t' read -r epoch entry; do
    [[ -n "$entry" ]] || continue
    if (( retained < keep )); then
      retained=$((retained + 1))
      continue
    fi
    # Re-check right before acting: a dispatcher may have claimed this lane
    # while the ranking above was being built.
    lane="$(basename "$entry")"
    if lane_in_use "$lane"; then
      continue
    fi
    remove_candidate "$entry" "$kind"
  done < <(printf '%s\n' "${ranked[@]}" | LC_ALL=C sort -t $'\t' -k1,1nr -k2,2)
}

prune_root "hook worktree" "${hook_cache_root}/worktrees"

if [[ "${#targets_roots[@]}" -gt 0 ]]; then
  toolchain_dirs=()
  for root in "${targets_roots[@]}"; do
    [[ -d "$root" ]] || continue
    for toolchain_dir in "$root"/*/; do
      toolchain_dir="${toolchain_dir%/}"
      [[ -d "$toolchain_dir" && ! -L "$toolchain_dir" ]] || continue
      toolchain_dirs+=("$toolchain_dir")
    done
  done
  if [[ "${#toolchain_dirs[@]}" -gt 0 ]]; then
    prune_root "Cargo target" "${toolchain_dirs[@]}"
  fi
fi

if [[ "$dry_run" -eq 0 && -n "$source_root" ]]; then
  git -C "$source_root" worktree prune >/dev/null 2>&1 || true
fi
