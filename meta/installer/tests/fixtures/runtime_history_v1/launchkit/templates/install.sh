#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "$0")"
if [[ "$(uname -s)" != "Darwin" && "$(uname -s)" != "Linux" ]]; then
  echo "This package supports macOS and Linux." >&2
  exit 1
fi
PYTHON_BIN="${PYTHON_BIN:-python3}"
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  echo "Install Python 3.10 or newer, then run this installer again." >&2
  exit 1
fi
"$PYTHON_BIN" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else "Python 3.10 or newer is required")'
umask 077
if [[ ! -d .venv ]]; then "$PYTHON_BIN" -m venv .venv; fi
.venv/bin/python -m pip install --disable-pip-version-check -r requirements.txt
.venv/bin/python install_gateway.py
mkdir -p workspace data .mobkit/state
if [[ ! -f .env ]]; then cp .env.example .env; fi
chmod 600 .env
.venv/bin/python app.py --check
if [[ "${1:-}" != "--non-interactive" ]]; then
  .venv/bin/python setup.py
fi
printf '\nInstallation complete. Start your team with:\n  bash start.sh\n\n'
