#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_FILE="$ROOT_DIR/artifacts/d1_auth_identity_scan.txt"
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
  'TokenKey::new\([^)]*profile_id'
  'binding_key = %format!\("\{\}:\{\}", [^)]*profile_id'
  'let binding_key = format!\("\{\}:\{\}", binding\.auth_profile\.id, binding\.backend_profile\.id'
  'binding\.auth_profile\.id\.clone\(\)'
)

SCAN_PATHS=(
  "meerkat-rest/src/auth_endpoints.rs"
  "meerkat-rpc/src/handlers/auth.rs"
  "meerkat-web-runtime/src/external_auth.rs"
  "meerkat-openai/src/runtime/mod.rs"
  "meerkat-anthropic/src/runtime/mod.rs"
  "meerkat-gemini/src/runtime/mod.rs"
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
  echo "# D1 Auth Identity Scan Report"
  echo "Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  echo
  echo "Scan paths:"
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
  echo "D1 auth identity scan failed: profile_id still appears in canonical auth keying/storage paths." >&2
  echo "See $OUTPUT_FILE for full report." >&2
  if [[ "$FAIL_ON_MATCHES" == true ]]; then
    exit 1
  fi
fi

echo "D1 auth identity scan complete. Full report: $OUTPUT_FILE"
