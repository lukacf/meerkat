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
PYTHONDONTWRITEBYTECODE=1 "$PYTHON_BIN" -c 'from pathlib import Path
from storage_setup import require_project_directory
try:
    require_project_directory(Path.cwd())
except RuntimeError as failure:
    raise SystemExit(str(failure))'
umask 077
show_environment_recovery() {
  printf '%s\n' "$1" >&2
  echo "Use a complete Python 3.10+ installation with venv and pip bootstrapping support (ensurepip)." >&2
  echo "This attempt stopped before installing dependencies. Existing environment files and private settings were preserved." >&2
  printf '  cd -- %q\n' "$PWD" >&2
  if [[ -e .venv || -L .venv ]]; then
    echo "After correcting Python, move only this incomplete .venv to an unused backup name." >&2
    echo "If .venv.previous already exists, choose another unused backup name." >&2
    echo '  test ! -e .venv.previous && test ! -L .venv.previous && mv .venv .venv.previous' >&2
  fi
  printf '  PYTHON_BIN="/absolute/path/to/python3.10" bash install.sh' >&2
  if [[ "$install_mode" == "--non-interactive" ]]; then printf ' --non-interactive' >&2; fi
  printf '\n' >&2
}
install_mode="${1:-}"
show_dependency_recovery() {
  echo "Keep .venv, private settings and application history in place. Correct the problem reported above, then retry from the same folder." >&2
  printf '  cd -- %q\n' "$PWD" >&2
  printf '  PYTHON_BIN=%q bash install.sh' "$PYTHON_BIN" >&2
  if [[ "$install_mode" == "--non-interactive" ]]; then printf ' --non-interactive' >&2; fi
  printf '\n' >&2
}
prepare_application_dependency_step() {
  local step="$1"
  shift
  local result
  if "$@"; then
    return 0
  else
    result=$?
  fi
  printf 'Application dependency preparation did not finish: %s.\n' "$step" >&2
  echo "Installation stopped before the gateway, application checks and setup. Dependencies may be partially prepared in .venv." >&2
  show_dependency_recovery
  return "$result"
}
if [[ ! -e .venv && ! -L .venv ]]; then
  if ! "$PYTHON_BIN" -m venv .venv; then
    show_environment_recovery "Python could not create this application's .venv."
    exit 1
  fi
fi
if [[ ! -x .venv/bin/python ]] || ! .venv/bin/python -c 'import sys; sys.exit(0 if sys.version_info >= (3, 10) else 1)' >/dev/null 2>&1; then
  show_environment_recovery "The application's retained .venv cannot run Python 3.10 or newer."
  exit 1
fi
if ! .venv/bin/python -m pip --version >/dev/null 2>&1; then
  show_environment_recovery "Pip is unavailable in this application's .venv."
  exit 1
fi
prepare_application_dependency_step "installing pinned dependencies" .venv/bin/python -m pip install --disable-pip-version-check -r requirements.txt
.venv/bin/python install_gateway.py
mkdir -p workspace data .mobkit/state
if [[ ! -f .env ]]; then cp .env.example .env; fi
chmod 600 .env
.venv/bin/python app.py --check
if [[ "${1:-}" != "--non-interactive" ]]; then
  .venv/bin/python setup.py
fi
printf '\nInstallation complete. Start your team with:\n  bash start.sh\n\n'
