#!/usr/bin/env bash
set -euo pipefail

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    rustfmt="${RUSTFMT_DARWIN:-}"
    ;;
  Linux-x86_64)
    rustfmt="${RUSTFMT_LINUX:-}"
    ;;
  *)
    rustfmt="${RUSTFMT_DARWIN:-${RUSTFMT_LINUX:-}}"
    ;;
esac

resolve_runfile() {
  local path="$1"
  if [[ -z "${path}" ]]; then
    return 1
  fi
  if [[ -x "${path}" ]]; then
    printf '%s\n' "${path}"
    return 0
  fi
  if [[ -x "${PWD}/${path}" ]]; then
    printf '%s\n' "${PWD}/${path}"
    return 0
  fi
  if [[ -n "${TEST_SRCDIR:-}" && -n "${TEST_WORKSPACE:-}" && -x "${TEST_SRCDIR}/${TEST_WORKSPACE}/${path}" ]]; then
    printf '%s\n' "${TEST_SRCDIR}/${TEST_WORKSPACE}/${path}"
    return 0
  fi
  if [[ -n "${TEST_SRCDIR:-}" && -x "${TEST_SRCDIR}/${path}" ]]; then
    printf '%s\n' "${TEST_SRCDIR}/${path}"
    return 0
  fi
  return 1
}

if resolved="$(resolve_runfile "${rustfmt}")"; then
  exec "${resolved}" "$@"
fi

echo "unable to resolve host rustfmt runfile: ${rustfmt}" >&2
exit 127
