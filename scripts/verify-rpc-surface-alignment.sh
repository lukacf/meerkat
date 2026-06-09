#!/usr/bin/env bash
# Verify router, RPC catalog, and docs method inventory remain aligned
# (name-set parity), then verify per-method signature parity (typed
# param/result refs) across the docs table and the TS/Python SDK wrappers.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

green() { printf '\033[0;32m%s\033[0m\n' "$*"; }

python3 "$ROOT/scripts/verify_rpc_surface_alignment.py" "$ROOT"
python3 "$ROOT/scripts/verify_rpc_signature_parity.py" "$ROOT"

green "RPC surface alignment check passed"
