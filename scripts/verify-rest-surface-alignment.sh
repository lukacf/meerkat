#!/usr/bin/env bash
# Verify the REST OpenAPI catalog and the production axum router stay aligned.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

green() { printf '\033[0;32m%s\033[0m\n' "$*"; }

python3 "$ROOT/scripts/verify_rest_surface_alignment.py" "$ROOT"

green "REST surface alignment check passed"
