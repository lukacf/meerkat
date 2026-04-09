#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_FILE="$ROOT_DIR/artifacts/deprecated_backend_scan.txt"
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
      echo "  --no-fail     Do not exit non-zero when matches are found"
      echo "  --output=PATH Write report to PATH"
      exit 0
      ;;
    *)
      ;;
  esac
done

lower_prefix='red'
lower_suffix='b'
upper_prefix='Red'
lower_token="${lower_prefix}${lower_suffix}"
upper_token="${upper_prefix}${lower_suffix}"
PATTERN="${upper_token}|${lower_token}|sessions_${lower_token}_path|memory\\.${lower_token}|session_index\\.${lower_token}|${lower_token}-store"

matches=$(cd "$ROOT_DIR" && rg -n -H -e "$PATTERN" \
  --hidden \
  --no-ignore-vcs \
  --glob '!CHANGELOG.md' \
  --glob '!.git/**' \
  --glob '!target/**' \
  --glob '!artifacts/**' \
  --glob '!dist/**' \
  --glob '!.rct/**' \
  --glob '!nohup.out' \
  . || true)

mkdir -p "$(dirname "$OUTPUT_FILE")"
{
  echo "# Deprecated Backend Token Scan Report"
  echo "Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  echo
  if [[ -n "$matches" ]]; then
    echo "Matches:"
    printf "%s\n" "$matches"
  else
    echo "No matches found"
  fi
} >"$OUTPUT_FILE"

if [[ -n "$matches" ]]; then
  echo "Deprecated-backend gate failed: found forbidden backend references outside CHANGELOG.md." >&2
  echo "See $OUTPUT_FILE for full report." >&2
  if [[ "$FAIL_ON_MATCHES" == true ]]; then
    exit 1
  fi
fi

echo "Deprecated-backend scan complete. Full report: $OUTPUT_FILE"
