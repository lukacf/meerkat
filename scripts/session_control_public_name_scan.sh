#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_FILE="$ROOT_DIR/artifacts/session_control_public_name_scan.txt"

TERMS=(
  "session/runtime_state"
  "session/accept_input"
  "session/retire_runtime"
  "session/reset_runtime"
  "session/input_state"
  "session/inputs"
  "/sessions/{id}/runtime-state"
  "/sessions/{id}/accept-input"
  "/sessions/{id}/retire-runtime"
  "/sessions/{id}/reset-runtime"
  "/sessions/{id}/inputs"
  "/sessions/{session_id}/inputs/{input_id}"
)

SCAN_PATHS=(
  "docs"
  "sdks"
  "artifacts"
  "meerkat-contracts"
  "meerkat-rest"
  "meerkat-rpc"
  "tools/sdk-codegen"
)

matches=()
for term in "${TERMS[@]}"; do
  for scan_path in "${SCAN_PATHS[@]}"; do
    if [ ! -e "$ROOT_DIR/$scan_path" ]; then
      continue
    fi
    while IFS= read -r line; do
      [[ -z "$line" ]] && continue
      if [[ "${line%%:*}" == "$OUTPUT_FILE" ]]; then
        continue
      fi
      matches+=("$line")
    done < <(
      rg -n -F -H "$term" \
        --hidden \
        --no-ignore-vcs \
        --glob '!target/**' \
        --glob '!.git/**' \
        --glob '!node_modules/**' \
        --glob '!.rct/**' \
        --glob '!docs/dogma-*.md' \
        --glob '!docs/wave-*-prep/**' \
        "$ROOT_DIR/$scan_path" || true
    )
  done
done

mkdir -p "$(dirname "$OUTPUT_FILE")"
{
  echo "# Session-Control Public Name Scan"
  echo "Generated: $(date -u +\"%Y-%m-%dT%H:%M:%SZ\")"
  echo
  echo "Blocked legacy public names:"
  for term in "${TERMS[@]}"; do
    echo "- $term"
  done
  echo
  echo "Scanned paths:"
  for scan_path in "${SCAN_PATHS[@]}"; do
    echo "- $scan_path"
  done
  echo
  echo "Matches: ${#matches[@]}"
  echo
  if (( ${#matches[@]} > 0 )); then
    printf "%s\n" "${matches[@]}"
  else
    echo "No blocked matches found"
  fi
} >"$OUTPUT_FILE"

if (( ${#matches[@]} > 0 )); then
  echo "Session-control gate failed: retired public names remain." >&2
  echo "See $OUTPUT_FILE for details." >&2
  printf "  %s\n" "${matches[@]}" >&2
  exit 1
fi

echo "Session-control public-name scan passed. Report: $OUTPUT_FILE"
