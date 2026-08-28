#!/usr/bin/env bash
set -euo pipefail

base=""
head=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base)
      [[ $# -ge 2 ]] || { echo "error: --base requires a revision" >&2; exit 2; }
      base="$2"
      shift 2
      ;;
    --head)
      [[ $# -ge 2 ]] || { echo "error: --head requires a revision" >&2; exit 2; }
      head="$2"
      shift 2
      ;;
    -h | --help)
      echo "usage: pre-push-harness-only.sh --base <rev> --head <rev>"
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$base" || -z "$head" ]]; then
  echo "error: --base and --head are required" >&2
  exit 2
fi
if ! git merge-base --is-ancestor "$base" "$head" 2>/dev/null; then
  exit 1
fi

changed=0
while IFS=$'\t' read -r status path remainder; do
  [[ -n "${status:-}" ]] || continue
  changed=1
  if [[ -n "${remainder:-}" || ( "$status" != "M" && "$status" != "A" ) ]]; then
    exit 1
  fi
  case "$path" in
    scripts/pre-push-*.sh | \
    scripts/test-pre-push-*.sh | \
    scripts/release-projection-only.mjs | \
    scripts/test-release-projection-*.sh)
      ;;
    *)
      exit 1
      ;;
  esac
done < <(git diff --name-status --diff-filter=ACDMRT "$base" "$head" --)

[[ "$changed" -eq 1 ]]
