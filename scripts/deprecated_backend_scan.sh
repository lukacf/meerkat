#!/usr/bin/env bash
# GOVERNANCE CLASSIFICATION: deliberately text-level tombstone gate.
# The deprecated backend name must not reappear anywhere — code, comments,
# docs, or strings — so raw text (not AST structure) is the contract being
# enforced. Structural source-shape governance lives in the xtask syn-AST
# gates (rmat-audit, effect-authority, machine-check-drift).
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
  --glob '!.rct-*/**' \
  --glob '!.claude/worktrees/**' \
  --glob '!.claude/skills/**' \
  --glob '!examples/*/target/**' \
  --glob '!target_*/**' \
  --glob '!target-*/**' \
  --glob '!**/node_modules/**' \
  --glob '!scripts/deprecated_backend_scan.sh' \
  --glob '!xtask/tests/buildbuddy_static_lanes.rs' \
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

# Filter allowlisted patterns: rejection tests and historical comments
# that reference the deprecated backend name in a safe context.
if [[ -n "$matches" ]]; then
  filtered=$(printf "%s\n" "$matches" | grep -v \
    -e 'rejects_unsupported_redb_backend' \
    -e '"backend": "redb"' \
    -e 'redb backend must be rejected' \
    -e 'error.contains.*unsupported.*redb' \
    -e 'avoids opening redb at' \
    -e 'only checks the redb store' \
    || true)
  if [[ -n "$filtered" ]]; then
    echo "Deprecated-backend gate failed: found forbidden backend references outside CHANGELOG.md." >&2
    echo "See $OUTPUT_FILE for full report." >&2
    if [[ "$FAIL_ON_MATCHES" == true ]]; then
      exit 1
    fi
  fi
fi

echo "Deprecated-backend scan complete. Full report: $OUTPUT_FILE"
