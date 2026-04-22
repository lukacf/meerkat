#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_FILE="$ROOT_DIR/artifacts/skill_source_identity_scan.txt"
FAIL_ON_MATCHES=true

for arg in "$@"; do
  case "$arg" in
    --no-fail)
      FAIL_ON_MATCHES=false
      ;;
    --output=*)
      OUTPUT_FILE="${arg#*=}"
      ;;
    --help|-h)
      echo "Usage: $(basename "$0") [--no-fail] [--output=PATH]"
      echo "  --no-fail     Do not exit non-zero when blocked matches are found"
      echo "  --output=PATH Write report to PATH"
      exit 0
      ;;
  esac
done

PATTERNS=(
  '00000000-0000-4000-8000-000000000101'
  '00000000-0000-4000-8000-000000000102'
  '00000000-0000-4000-8000-000000000103'
)

SCAN_PATHS=(
  "meerkat-skills/src/resolve.rs"
  "meerkat-skills/src/source/filesystem.rs"
  "meerkat-rest/src/lib.rs"
  "meerkat-rpc/src/session_runtime.rs"
  "meerkat-rpc/src/handlers/session.rs"
  "meerkat-rpc/src/handlers/config.rs"
  "meerkat-rpc/src/main.rs"
  "meerkat-cli/src/main.rs"
)

BLOCKED=()

for pattern in "${PATTERNS[@]}"; do
  matches=$(cd "$ROOT_DIR" && rg -n -H -e "$pattern" "${SCAN_PATHS[@]}" || true)
  if [[ -n "$matches" ]]; then
    while IFS= read -r match; do
      [[ -z "$match" ]] && continue
      BLOCKED+=("$match")
    done <<<"$matches"
  fi
done

mkdir -p "$(dirname "$OUTPUT_FILE")"
{
  echo "# Skill Source Identity Scan Report"
  echo "Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  echo
  echo "Blocked default source UUID literals:"
  for pattern in "${PATTERNS[@]}"; do
    echo "- $pattern"
  done
  echo
  echo "Scanned paths:"
  for scan_path in "${SCAN_PATHS[@]}"; do
    echo "- $scan_path"
  done
  echo
  echo "Blocked matches: ${#BLOCKED[@]}"
  echo
  if (( ${#BLOCKED[@]} > 0 )); then
    printf "%s\n" "${BLOCKED[@]}"
  else
    echo "No blocked matches found"
  fi
} >"$OUTPUT_FILE"

if (( ${#BLOCKED[@]} > 0 )); then
  echo "Skill source identity scan failed: default source UUID literals remain outside the canonical skills-config seam." >&2
  echo "See $OUTPUT_FILE for full report." >&2
  if [[ "$FAIL_ON_MATCHES" == true ]]; then
    exit 1
  fi
fi

echo "Skill source identity scan complete. Full report: $OUTPUT_FILE"
