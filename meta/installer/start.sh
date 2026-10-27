#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

prepare_only=false
case "${1:-}" in
  "") ;;
  --prepare-only) prepare_only=true ;;
  *) echo "Usage: bash start.sh [--prepare-only]" >&2; exit 1 ;;
esac
if [ "$#" -gt 1 ]; then
  echo "Usage: bash start.sh [--prepare-only]" >&2
  exit 1
fi
launch_port="${META_FORGE_PORT:-${MOBKIT_LAUNCH_PORT:-8787}}"
if [[ ! "$launch_port" =~ ^[1-9][0-9]{0,4}$ ]] || [ "$launch_port" -gt 65535 ]; then
  echo "META_FORGE_PORT (or MOBKIT_LAUNCH_PORT) must be a port number from 1 to 65535." >&2
  exit 1
fi
if [ -n "${MOBKIT_GATEWAY_BIN:-}" ] && { [ ! -f "$MOBKIT_GATEWAY_BIN" ] || [ ! -x "$MOBKIT_GATEWAY_BIN" ]; }; then
  echo "MOBKIT_GATEWAY_BIN must name an executable file before starting Meta Forge." >&2
  echo "Set it to a valid gateway executable, or run unset MOBKIT_GATEWAY_BIN and retry to prepare the verified runtime." >&2
  exit 1
fi
require_venv_python() {
  local problem=""
  if [ ! -x .venv/bin/python ]; then
    problem="Its Python executable is missing or is not executable."
  elif problem="$(.venv/bin/python -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else "Python 3.11 or newer is required")' 2>&1)"; then
    return 0
  fi
  echo "The $1 .venv cannot run Python 3.11 or newer." >&2
  if [ -n "$problem" ]; then
    printf '%s\n' "$problem" >&2
  fi
  echo "PYTHON_BIN does not upgrade an existing .venv. Saved Forge state and settings have not been changed." >&2
  echo "Move only .venv to an unused backup name, then retry with a supported Python:" >&2
  printf '  cd -- %q\n' "$PWD" >&2
  echo '  test ! -e .venv.previous && test ! -L .venv.previous && mv .venv .venv.previous' >&2
  if [ "$prepare_only" = true ]; then
    echo '  PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh --prepare-only' >&2
  else
    echo '  PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh' >&2
  fi
  return 1
}
show_python_preparation_recovery() {
  printf '%s\n' "$1" >&2
  echo "Use a complete Python 3.11+ installation with venv and pip bootstrapping support (ensurepip)." >&2
  echo "Saved Forge state and settings have not been changed. This attempt stopped before installing dependencies or starting the server." >&2
  printf '  cd -- %q\n' "$PWD" >&2
  if [ -e .venv ] || [ -L .venv ]; then
    echo "After correcting Python, back up only this incomplete .venv, then create a fresh environment by retrying." >&2
    echo "If .venv.previous already exists, choose another unused backup name." >&2
    echo '  test ! -e .venv.previous && test ! -L .venv.previous && mv .venv .venv.previous' >&2
  else
    echo "After correcting Python, retry to create Forge's private environment:" >&2
  fi
  if [ "$prepare_only" = true ]; then
    echo '  PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh --prepare-only' >&2
  else
    echo '  PYTHON_BIN="/absolute/path/to/python3.11" bash start.sh' >&2
  fi
}
prepare_forge_dependency_step() {
  local step="$1"
  shift
  if "$@"; then
    return 0
  fi
  printf 'Forge dependency preparation did not finish: %s.\n' "$step" >&2
  echo "The web server was not started. Dependencies may be partially prepared in .venv." >&2
  echo "Keep .venv, saved Forge state and settings in place. Correct the problem reported above, then retry from the same folder." >&2
  printf '  cd -- %q\n' "$PWD" >&2
  printf '  PYTHON_BIN=%q' "$PYTHON_BIN" >&2
  local setting
  for setting in META_FORGE_PORT MOBKIT_LAUNCH_PORT META_FORGE_HOME MOBKIT_LAUNCH_HOME MOBKIT_GATEWAY_BIN META_FORGE_APPLICATION_HOST_FACTORY META_FORGE_ORGANIZATION; do
    if [ "${!setting+x}" ]; then
      printf ' %s=%q' "$setting" "${!setting}" >&2
    fi
  done
  printf ' bash start.sh' >&2
  if [ "$prepare_only" = true ]; then
    printf ' --prepare-only' >&2
  fi
  printf '\n' >&2
  return 1
}
retained_venv=false
if [ -e .venv ] || [ -L .venv ]; then
  retained_venv=true
fi
if [ -z "${PYTHON_BIN:-}" ] && [ "$retained_venv" = true ]; then
  PYTHON_BIN=.venv/bin/python
fi
PYTHON_BIN="${PYTHON_BIN:-python3}"
if [ "$PYTHON_BIN" != .venv/bin/python ] || [ "$retained_venv" = false ]; then
  if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
    echo "Python 3.11 or newer is required before starting Meta Forge." >&2
    echo "Set PYTHON_BIN to a Python 3.11+ executable." >&2
    exit 1
  fi
  "$PYTHON_BIN" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else "Python 3.11 or newer is required")'
fi
if [ "$retained_venv" = true ]; then
  require_venv_python existing
fi
standalone_sdk_wheel=""
standalone_runtime=""
standalone_sdk_wheels=()
if [ -e standalone-manifest.json ] || [ -L standalone-manifest.json ]; then
  standalone_inputs="$("$PYTHON_BIN" -B -m launchkit.distribution --bootstrap-inputs .)"
  while IFS= read -r item; do
    if [ -z "$standalone_runtime" ]; then
      standalone_runtime="$item"
    elif [ -n "$item" ]; then
      standalone_sdk_wheels+=("$item")
    fi
  done <<< "$standalone_inputs"
  if [ "$standalone_runtime" != legacy ] && [ "$standalone_runtime" != meta-toolkit ]; then
    echo "The standalone runtime selection is invalid. Unpack a fresh copy." >&2
    exit 1
  fi
  if [ "${#standalone_sdk_wheels[@]}" -eq 0 ]; then
    echo "The standalone SDK selection is missing. Unpack a fresh copy." >&2
    exit 1
  fi
  standalone_sdk_wheel="${standalone_sdk_wheels[0]}"
  if [ "$standalone_runtime" = meta-toolkit ] && [ -n "${MOBKIT_GATEWAY_BIN:-}" ]; then
    echo "This Forge candidate selects its bundled Meta runtime. Remove MOBKIT_GATEWAY_BIN and retry." >&2
    exit 1
  fi
elif "$PYTHON_BIN" -c 'import sys,tomllib; from pathlib import Path; project=tomllib.loads(Path("pyproject.toml").read_text()); sys.exit(0 if any(item.startswith("meerkat-meta") for item in project["project"]["dependencies"]) else 1)'; then
  echo "Meta Forge 0.9 startup needs its prepared product runtime." >&2
  echo "Unpack the complete owner-prepared Forge candidate archive into a new folder, then run bash start.sh there." >&2
  echo "A source-only checkout must first be packaged with its exact selected Meta SDK, guide gateway and Connect product." >&2
  exit 1
elif ! command -v npm >/dev/null 2>&1; then
  echo "This source checkout requires Node.js 20+ and npm. The standalone bundle runs without a frontend build." >&2
  exit 1
fi
echo "Preparing the Python environment..."
if [ "$retained_venv" = false ]; then
  if ! "$PYTHON_BIN" -m venv .venv; then
    show_python_preparation_recovery "Python could not create Forge's private .venv."
    exit 1
  fi
  require_venv_python new
fi
echo "Preparing Forge dependencies..."
if ! .venv/bin/python -m pip --version >/dev/null 2>&1; then
  if ! .venv/bin/python -m ensurepip --upgrade || ! .venv/bin/python -m pip --version >/dev/null 2>&1; then
    show_python_preparation_recovery "Pip could not be prepared in Forge's private .venv."
    exit 1
  fi
fi
if [ -n "$standalone_sdk_wheel" ]; then
  prepare_forge_dependency_step "installing pinned dependencies" .venv/bin/python -m pip install --quiet -r requirements-standalone.txt
  if [ "$standalone_runtime" = meta-toolkit ]; then
    prepare_forge_dependency_step "installing the bundled SDKs" .venv/bin/python -m pip install --quiet --force-reinstall --no-deps "${standalone_sdk_wheels[@]}"
  else
    prepare_forge_dependency_step "installing the bundled SDK" .venv/bin/python -m pip install --quiet --force-reinstall --no-deps "$standalone_sdk_wheel"
  fi
  prepare_forge_dependency_step "checking installed dependencies" .venv/bin/python -m pip check
elif [ -f ../sdk/python/pyproject.toml ] && .venv/bin/python -c 'import sys,tomllib; from pathlib import Path; project=tomllib.loads(Path("pyproject.toml").read_text()); sdk=tomllib.loads(Path("../sdk/python/pyproject.toml").read_text()); sys.exit(0 if "meerkat-mobkit=="+sdk["project"]["version"] in project["project"]["dependencies"] else 1)'; then
  prepare_forge_dependency_step "installing the source checkout and SDK" .venv/bin/python -m pip install --quiet -e ../sdk/python -e .
else
  prepare_forge_dependency_step "installing the source checkout" .venv/bin/python -m pip install --quiet -e .
fi
if [ "$standalone_runtime" = meta-toolkit ]; then
  if ! .venv/bin/python -c 'from meerkat_meta import Meta; import sys; sys.exit(0 if callable(getattr(Meta.builder(), "gateway_init_timeout", None)) else 1)' >/dev/null 2>&1; then
    echo "This Forge candidate needs its bundled Meta SDK." >&2
    echo "Unpack a fresh candidate or retry preparation in the same folder." >&2
    exit 1
  fi
elif ! .venv/bin/python -c 'from meerkat_mobkit import MobKit; import sys; sys.exit(0 if callable(getattr(MobKit.builder(), "gateway_init_timeout", None)) else 1)' >/dev/null 2>&1; then
  echo "This installer needs the SDK gateway_init_timeout API." >&2
  echo "Install the SDK version pinned in pyproject.toml, or unpack a fresh standalone bundle." >&2
  exit 1
fi
if [ -z "$standalone_sdk_wheel" ]; then
  echo "Preparing the Forge interface..."
  if [ -f web/package-lock.json ]; then
    npm --prefix web ci --silent
  fi
  npm --prefix web run build --silent
fi

if [ "$standalone_runtime" = meta-toolkit ]; then
  echo "Checking the selected Meta and Connect runtime..."
  prepare_forge_dependency_step "checking the bundled product runtime" .venv/bin/python -B -c 'from launchkit.packaged_runtime import open_selection; open_selection()'
fi
if [ "$prepare_only" = true ]; then
  echo "Meta Forge preparation is complete. The web server and native runtime were not started."
  exit 0
fi
echo ""
echo "Open Meta Forge at http://127.0.0.1:$launch_port"
echo "Keep this terminal open. Press Ctrl+C to stop Meta Forge."
if [ -n "${META_FORGE_APPLICATION_HOST_FACTORY:-}" ]; then
  exec .venv/bin/python -m uvicorn launchkit.application_host_launch:create_app --factory --host 127.0.0.1 --port "$launch_port" --timeout-graceful-shutdown 20
fi
if [ "$standalone_runtime" = meta-toolkit ]; then
  exec .venv/bin/python -m uvicorn launchkit.packaged_runtime:create_app --factory --host 127.0.0.1 --port "$launch_port" --timeout-graceful-shutdown 20
fi
exec .venv/bin/python -m uvicorn launchkit.server:app --host 127.0.0.1 --port "$launch_port" --timeout-graceful-shutdown 20
