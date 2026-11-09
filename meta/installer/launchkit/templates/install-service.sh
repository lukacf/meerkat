#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
[[ "$(uname -s)" == "Linux" ]] || { echo "This service installer requires Linux and systemd." >&2; exit 1; }
[[ -x .venv/bin/python ]] || { echo "Run bash install.sh first." >&2; exit 1; }
exec .venv/bin/python -I remote_operations.py "${1:-setup}"
