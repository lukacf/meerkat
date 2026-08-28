#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="${ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
RELEASE_PROJECTION_ONLY="${RELEASE_PROJECTION_ONLY:-$SCRIPT_DIR/release-projection-only.mjs}"
VERIFY_VERSION_PARITY="${VERIFY_VERSION_PARITY:-$ROOT/scripts/verify-version-parity.sh}"
AGENT_GATE="${AGENT_GATE:-$ROOT/scripts/agent-gate}"

if [[ -n "${PRE_COMMIT_FROM_REF:-}" && -n "${PRE_COMMIT_TO_REF:-}" ]]; then
  if "$RELEASE_PROJECTION_ONLY" \
    --base "$PRE_COMMIT_FROM_REF" --head "$PRE_COMMIT_TO_REF"; then
    echo "Release projection only; reusing parent source compilation evidence."
    exec "$VERIFY_VERSION_PARITY"
  else
    classifier_status=$?
    if [[ "$classifier_status" -ne 1 ]]; then
      echo "release projection classification failed with status ${classifier_status}" >&2
      exit "$classifier_status"
    fi
  fi
fi

exec "$AGENT_GATE" --committed --clippy-only "$@"
