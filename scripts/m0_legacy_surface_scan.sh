#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALLOWLIST_FILE="$ROOT_DIR/scripts/m0_legacy_surface_allowlist.txt"
OUTPUT_FILE="$ROOT_DIR/artifacts/m0_legacy_surface_inventory.txt"
FAIL_ON_BLOCKED=true

for arg in "$@"; do
  case "$arg" in
    --no-fail)
      FAIL_ON_BLOCKED=false
      ;;
    --output=*)
      OUTPUT_FILE="${arg#*=}"
      ;;
    --help|-h)
      echo "Usage: $(basename \"$0\") [--no-fail] [--output=PATH]"
      echo "  --no-fail     Do not exit non-zero when blocked matches are found"
      echo "  --output=PATH Write inventory report to PATH"
      exit 0
      ;;
    *)
      ;;
  esac
done

TERMS=(
  "event/push"
  "send_message"
  "send_request"
  "send_response"
  "list_peers"
  "inject_with_subscription"
  "EventInjector"
  "SubscribableInjector"
  "interaction_subscriber"
  "event_injector"
)

PATTERN=$(printf '%s|' "${TERMS[@]}")
PATTERN="${PATTERN%|}"

SCAN_PATHS=(
  "docs"
  ".claude/skills/meerkat-platform"
  "sdks"
  "CHANGELOG.md"
  "meerkat/src"
  "meerkat-cli"
  "meerkat-core"
  "meerkat-comms"
  "meerkat-rest"
  "meerkat-rpc"
  "meerkat-session"
  "meerkat-tools"
  "meerkat-contracts/src/version.rs"
)

is_allowed_file() {
  local file="$1"
  while IFS= read -r allow_rule; do
    [[ -z "${allow_rule}" ]] && continue
    [[ "${allow_rule}" == \#* ]] && continue
    if [[ "$file" =~ $allow_rule ]]; then
      return 0
    fi
  done <"$ALLOWLIST_FILE"
  return 1
}

ALLOWED=()
BLOCKED=()

for scan_path in "${SCAN_PATHS[@]}"; do
  if [ ! -e "$ROOT_DIR/$scan_path" ]; then
    continue
  fi

  matches=$(rg -n -H -e "$PATTERN" \
    --hidden \
    --no-ignore-vcs \
    --glob '!target/**' \
    --glob '!.git/**' \
    --glob '!.rct/**' \
    --glob '!dist/**' \
    --glob '!nohup.out' \
    "$scan_path" || true)

  if [ -z "$matches" ]; then
    continue
  fi

  while IFS= read -r match; do
    [[ -z "$match" ]] && continue
    file="${match%%:*}"
    # Normalize to repo-root-relative paths.
    file="./$file"
    if is_allowed_file "$file"; then
      ALLOWED+=("$match")
    else
      BLOCKED+=("$match")
    fi
  done <<<"$matches"
done

mkdir -p "$(dirname "$OUTPUT_FILE")"
{
  echo "# Legacy Surface Scan Report"
  echo "Generated: $(date -u +\"%Y-%m-%dT%H:%M:%SZ\")"
  echo
  echo "Terms:"
  for term in "${TERMS[@]}"; do
    echo "- $term"
  done
  echo
  echo "Scanned paths:"
  for scan_path in "${SCAN_PATHS[@]}"; do
    echo "- $scan_path"
  done
  echo
  echo "Allowlist: $ALLOWLIST_FILE"
  echo
  echo "Blocked matches: ${#BLOCKED[@]}"
  echo
  if (( ${#BLOCKED[@]} > 0 )); then
    printf "%s\n" "${BLOCKED[@]}"
  else
    echo "No blocked matches found"
  fi
  echo
  echo "Allowed matches: ${#ALLOWED[@]}"
  echo
  if (( ${#ALLOWED[@]} > 0 )); then
    printf "%s\n" "${ALLOWED[@]}"
  else
    echo "No allowed matches found"
  fi
} >"$OUTPUT_FILE"

if (( ${#BLOCKED[@]} > 0 )); then
  echo "M0 gate failed: legacy public-surface tokens found outside allowlist." >&2
  echo "See $OUTPUT_FILE for full report." >&2
  if [[ "$FAIL_ON_BLOCKED" == true ]]; then
    echo "Blocked entries:" >&2
    printf "  %s\n" "${BLOCKED[@]}" >&2
    exit 1
  fi
fi

echo "Legacy surface scan complete. Full report: $OUTPUT_FILE"
if (( ${#BLOCKED[@]} > 0 )); then
  echo "Blocked: ${#BLOCKED[@]}"
fi
