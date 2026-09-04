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
# first. It is conservative by design; a lane is removed only when ALL of the
# following hold:
#   - its name is exactly `pre-push-<16 hex>` and it is a real directory
#     directly under one of the given roots;
#   - it is not the caller's lane;
#   - no live process holds its dispatcher lock;
#   - no live process references it (cwd, command line, or environment on
#     Linux via /proc; command line via pgrep on other platforms);
#   - neither its last-used stamp nor any file beneath it changed within the
#     idle window (MEERKAT_PRE_PUSH_LANE_IDLE_SECS, default 6 hours);
#   - it falls outside the newest-N budget (MEERKAT_PRE_PUSH_KEEP_LANES,
#     default 2), ranked by last-used stamp with a directory-mtime fallback.
# Every decision is logged as one `kept:` or `pruned:` line with its reason.
# A candidate is renamed before deletion so concurrent pruners cannot both
# act on the same directory.
#
# Exit status is 0 whenever the retention policy was applied; a failure to
# remove one candidate is reported as a note, never as a push failure.
set -euo pipefail

# A deleted working directory must not change any decision below; every path
# this script handles is absolute.
cd / 2>/dev/null || true

LANE_STAMP_NAME=".meerkat-pre-push-last-used"
DEFAULT_KEEP=2
DEFAULT_IDLE_SECS=21600
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
  --idle-secs <n>           minimum seconds since any activity before a lane
                            may be pruned (default: value of
                            MEERKAT_PRE_PUSH_LANE_IDLE_SECS, then 21600)
  --dry-run                 print decisions without deleting anything
EOF
}

hook_cache_root=""
targets_roots=()
current_lane=""
source_root=""
keep_raw="${MEERKAT_PRE_PUSH_KEEP_LANES:-}"
idle_raw="${MEERKAT_PRE_PUSH_LANE_IDLE_SECS:-}"
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
    --idle-secs)
      [[ $# -ge 2 ]] || { echo "error: --idle-secs requires a count" >&2; exit 2; }
      idle_raw="$2"
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
case "$hook_cache_root" in
  /*) ;;
  *)
    echo "error: --hook-cache-root must be absolute: ${hook_cache_root}" >&2
    exit 2
    ;;
esac
for root in "${targets_roots[@]}"; do
  case "$root" in
    /*) ;;
    *)
      echo "error: --targets-root must be absolute: ${root}" >&2
      exit 2
      ;;
  esac
done

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

if [[ -z "$idle_raw" ]]; then
  idle_secs="$DEFAULT_IDLE_SECS"
elif [[ "$idle_raw" =~ ^[0-9]+$ ]]; then
  idle_secs="$idle_raw"
else
  echo "note: ignoring invalid pre-push lane idle window '${idle_raw}'; using ${DEFAULT_IDLE_SECS}s." >&2
  idle_secs="$DEFAULT_IDLE_SECS"
fi

now_ts="$(date +%s)"
idle_cutoff=$(( now_ts - idle_secs ))

# A reference file whose mtime is the idle cutoff, for `find -newer`. Created
# once here: helper functions run in subshells and could not register it for
# cleanup.
scratch_dir="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-lane-prune.XXXXXX")"
idle_ref="${scratch_dir}/idle-cutoff"
: > "$idle_ref"
# `touch -t` reads local time, so the stamp must be formatted in local time.
idle_stamp="$(date -d "@${idle_cutoff}" +%Y%m%d%H%M.%S 2>/dev/null ||
  date -r "${idle_cutoff}" +%Y%m%d%H%M.%S)"
touch -t "$idle_stamp" "$idle_ref"

# shellcheck disable=SC2317  # invoked through the EXIT trap
cleanup() {
  rm -rf -- "$scratch_dir" 2>/dev/null || true
}
trap cleanup EXIT

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

format_epoch() {
  local epoch="$1"
  date -u -d "@${epoch}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null ||
    date -u -r "${epoch}" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null ||
    printf '%s' "${epoch}"
}

# Live-lock rule: the dispatcher lock is honored while its pid is alive. A lock
# whose pid file has not been written yet is honored for a short grace period,
# mirroring the dispatcher's own stale-lock rule. Prints the owner pid.
lock_owner_if_live() {
  local lane="$1"
  local lock_dir="${hook_cache_root}/dispatcher-${lane}.lock"
  local owner_pid lock_mtime
  [[ -d "$lock_dir" ]] || return 1
  owner_pid="$(cat "${lock_dir}/pid" 2>/dev/null || true)"
  if [[ -n "$owner_pid" ]]; then
    pid_alive "$owner_pid" || return 1
    printf '%s' "$owner_pid"
    return 0
  fi
  lock_mtime="$(mtime_of "$lock_dir" 2>/dev/null || echo 0)"
  (( now_ts - lock_mtime < LOCK_PID_GRACE_SECS )) || return 1
  printf 'unknown'
}

# Recency rule: the explicit stamp or any file beneath the lane changed within
# the idle window. Prints the reason.
recent_activity() {
  local dir="$1"
  local stamp="${dir}/${LANE_STAMP_NAME}"
  local stamp_ts hit
  if [[ -f "$stamp" ]]; then
    stamp_ts="$(mtime_of "$stamp" 2>/dev/null || echo 0)"
    if (( stamp_ts > idle_cutoff )); then
      printf 'last used %s, within the %ss idle window' "$(format_epoch "$stamp_ts")" "$idle_secs"
      return 0
    fi
  fi
  hit="$(find "$dir" -newer "$idle_ref" -print 2>/dev/null | head -n 1 || true)"
  if [[ -n "$hit" ]]; then
    printf 'modified within the %ss idle window: %s' "$idle_secs" "$hit"
    return 0
  fi
  return 1
}

# Live-process rule: any process whose cwd, command line, or environment names
# the lane keeps it. On Linux this is one grep over /proc plus one symlink
# scan per lane; pgrep and lsof are the fallbacks where /proc is unavailable.
# Prints the referencing pid.
live_process_reference() {
  local dir="$1"
  local lane="$2"
  local match
  if [[ -d /proc/self ]]; then
    # Patterns travel by file: pipeline members expand the /proc glob after
    # forking, so a lane string on grep's own command line would match itself.
    printf '%s\n%s\n' "$dir" "$lane" >"${scratch_dir}/patterns"
    match="$(grep -lsaF -f "${scratch_dir}/patterns" /proc/[0-9]*/cmdline /proc/[0-9]*/environ 2>/dev/null |
      grep -Ev "^/proc/($$|${BASHPID})/" | head -n 1 || true)"
    if [[ -z "$match" ]]; then
      match="$(find /proc -mindepth 2 -maxdepth 2 -name cwd \( -lname "$dir" -o -lname "${dir}/*" \) 2>/dev/null |
        grep -Ev "^/proc/($$|${BASHPID})/" | head -n 1 || true)"
    fi
    if [[ -n "$match" ]]; then
      match="${match#/proc/}"
      printf '%s' "${match%%/*}"
      return 0
    fi
    return 1
  fi
  if command -v pgrep >/dev/null 2>&1; then
    match="$(pgrep -f -- "$lane" 2>/dev/null | grep -Fxv -e "$$" -e "$BASHPID" | head -n 1 || true)"
    if [[ -n "$match" ]]; then
      printf '%s' "$match"
      return 0
    fi
  fi
  if command -v lsof >/dev/null 2>&1; then
    match="$(lsof -Fp +d "$dir" 2>/dev/null | sed -n 's/^p//p' | grep -Fxv -e "$$" -e "$BASHPID" | head -n 1 || true)"
    if [[ -n "$match" ]]; then
      printf '%s' "$match"
      return 0
    fi
  fi
  return 1
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
  local reason="$3"
  local staged="${dir}.pruning.$$"

  if [[ "$dry_run" -eq 1 ]]; then
    echo "would prune: ${dir} (${kind}; ${reason})"
    return 0
  fi
  if [[ "$kind" == "hook worktree" && -n "$source_root" ]]; then
    git -C "$source_root" worktree remove --force "$dir" >/dev/null 2>&1 || true
    [[ -e "$dir" || -L "$dir" ]] || { echo "pruned: ${dir} (${kind}; ${reason})"; return 0; }
  fi
  # Claim the directory atomically; a concurrent pruner that loses the rename
  # simply moves on.
  if ! mv "$dir" "$staged" 2>/dev/null; then
    echo "kept: ${dir} (${kind}; claimed by a concurrent pruner)"
    return 0
  fi
  if rm -rf -- "$staged" 2>/dev/null; then
    echo "pruned: ${dir} (${kind}; ${reason})"
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
# given parents together and keep at most `keep` of them. Exempt lanes (the
# caller's, live-locked, recently active, or referenced by a live process)
# count toward the budget first; the newest remaining lanes fill whatever
# budget is left.
prune_root() {
  local kind="$1"
  shift
  local parent entry name lane epoch owner reason
  local -a ranked=()
  local retained=0

  for parent in "$@"; do
    [[ -d "$parent" ]] || continue
    remove_dead_residue "$parent" "$kind"
    for entry in "$parent"/pre-push-*; do
      [[ -d "$entry" && ! -L "$entry" ]] || continue
      name="$(basename "$entry")"
      is_lane_name "$name" || continue
      if [[ -n "$current_lane" && "$name" == "$current_lane" ]]; then
        echo "kept: ${entry} (${kind}; current lane)"
        retained=$((retained + 1))
        continue
      fi
      if owner="$(lock_owner_if_live "$name")"; then
        echo "kept: ${entry} (${kind}; dispatcher lock held by pid ${owner})"
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
    lane="$(basename "$entry")"
    if (( retained < keep )); then
      echo "kept: ${entry} (${kind}; within budget ${keep}, last used $(format_epoch "$epoch"))"
      retained=$((retained + 1))
      continue
    fi
    if reason="$(recent_activity "$entry")"; then
      echo "kept: ${entry} (${kind}; ${reason})"
      retained=$((retained + 1))
      continue
    fi
    if owner="$(live_process_reference "$entry" "$lane")"; then
      echo "kept: ${entry} (${kind}; referenced by live process pid ${owner})"
      retained=$((retained + 1))
      continue
    fi
    # Re-check the lock right before acting: a dispatcher may have claimed this
    # lane while the ranking above was being built.
    if owner="$(lock_owner_if_live "$lane")"; then
      echo "kept: ${entry} (${kind}; dispatcher lock held by pid ${owner})"
      retained=$((retained + 1))
      continue
    fi
    remove_candidate "$entry" "$kind" \
      "beyond budget ${keep}, last used $(format_epoch "$epoch"), idle over ${idle_secs}s, unreferenced"
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
