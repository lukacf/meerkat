#!/usr/bin/env bash
# ci-drop-superseded-caches.sh <family>...: delete each CI cache family's
# superseded main entries. A family's entries are the rust-cache keys
# "v0-rust-<family>-<os>-..." saved for refs/heads/main; entries of other
# refs are never touched. When a family has an entry created since this
# workflow run started, its entries created before then are dropped, so CI
# keeps one generation per family and its saves do not evict other
# entries. That newer entry is normally this run's save, but a concurrent
# main run's save counts too. A family whose restore hit its exact key
# saved nothing new and keeps its entry. Dropping runs after the saves, so
# it cannot undo an eviction the cache quota already forced. The
# drop-superseded-caches job runs this on pushes to main, after every job
# that saves a rust cache has succeeded. Best effort: any failure is a
# warning and never fails the job.
set -u
if [ "$#" -eq 0 ]; then
  echo "usage: ci-drop-superseded-caches.sh <family>..." >&2
  exit 2
fi
repo="${GITHUB_REPOSITORY:?}"
ref=refs/heads/main
if ! started="$(gh api "repos/${repo}/actions/runs/${GITHUB_RUN_ID:?}" --jq .run_started_at)" || [ -z "${started}" ]; then
  echo "::warning::could not read this run's start time; keeping every cache"
  exit 0
fi
for family in "$@"; do
  prefix="v0-rust-${family}-${RUNNER_OS:-Linux}-"
  if ! listing="$(gh cache list --repo "${repo}" --ref "${ref}" --key "${prefix}" --limit 100 --json id,key,ref,createdAt)"; then
    echo "::warning::could not list the ${prefix}* caches; keeping them"
    continue
  fi
  # The server filters by ref and key prefix; both are checked again here.
  # Creation times lose their fraction to compare with run_started_at.
  if ! ids="$(printf '%s' "${listing}" | jq -r --arg p "${prefix}" --arg r "${ref}" --arg t "${started}" '
      [.[] | select((.key | startswith($p)) and .ref == $r)
           | {id, created: (.createdAt | sub("\\.[0-9]+Z$"; "Z"))}]
      | if any(.[]; .created >= $t) then .[] | select(.created < $t) | .id else empty end')"; then
    echo "::warning::could not read the ${prefix}* cache listing; keeping them"
    continue
  fi
  if [ -z "${ids}" ]; then
    echo "${prefix}*: nothing to drop (no entry from this run, or none older)"
    continue
  fi
  for id in ${ids}; do
    if gh cache delete "${id}" --repo "${repo}"; then
      echo "dropped cache ${id} (${prefix}*, created before ${started})"
    else
      echo "::warning::could not drop cache ${id} (${prefix}*)"
    fi
  done
done
exit 0
