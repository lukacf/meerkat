#!/usr/bin/env bash
set -euo pipefail

pack_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
examples_dir="$(cd "${pack_dir}/.." && pwd)"
targets_file="${MDM_TARGET_BINDINGS_FILE:-${pack_dir}/.state/target-bindings.json}"

if [[ "${1:-}" == "--targets" ]]; then
  [[ $# -ge 2 ]] || { echo "--targets requires a file" >&2; exit 2; }
  targets_file="$2"
  shift 2
elif [[ $# -gt 0 && -f "$1" ]]; then
  targets_file="$1"
  shift
fi

if [[ ! -s "$targets_file" ]]; then
  echo "target binding file not found: $targets_file" >&2
  echo "Start a target first, for example:" >&2
  echo "  ${pack_dir}/scripts/local-target.sh start --id target-a" >&2
  exit 1
fi

cd "$examples_dir"
exec npx tsx 004-mdm-console-pack/run.ts --targets "$targets_file" --wait "$@"
