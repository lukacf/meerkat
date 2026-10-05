#!/usr/bin/env bash
# scripts/require-python picks a Python >= MIN and refuses an older one with a
# clear reason. Fake interpreters stand in for Apple's 3.9 and a good 3.12, so
# no real Python version is assumed.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
helper="${ROOT}/scripts/require-python"
work="$(mktemp -d "${TMPDIR:-/tmp}/require-python-test.XXXXXX")"
trap 'rm -rf "${work}"' EXIT
pass=0; fail=0
ok() { printf '  OK:   %s\n' "$*"; pass=$((pass + 1)); }
bad() { printf '  FAIL: %s\n' "$*"; fail=$((fail + 1)); }

fake() { # fake <dir> <name> <version> <passes-check:0|1>
  mkdir -p "$1"
  cat >"$1/$2" <<SH
#!/bin/sh
case "\$1" in --version) echo "Python $3";; -c) exit $((1 - $4));; *) exit 1;; esac
SH
  chmod +x "$1/$2"
}
fake "${work}/old" python3 3.9.6 0
fake "${work}/good" python3.12 3.12.4 1
tools="${work}/tools"; mkdir -p "${tools}"
for t in bash sed; do ln -s "$(command -v "$t")" "${tools}/$t"; done

run() { env -i PATH="$1" ${2:+PYTHON="$2"} bash "${helper}" 3.11 probe; }

if out="$(run "${work}/old:${tools}" "" 2>&1)"; then bad "an old python3 on PATH was accepted"
elif [[ "${out}" == *"probe needs Python >= 3.11; found Python 3.9.6 (${work}/old/python3)"* ]]; then ok "old PATH python3 refused with the found version"
else bad "unexpected refusal text: ${out}"; fi

if out="$(run "${work}/old:${work}/good:${tools}" "" 2>/dev/null)" && [[ "${out}" == "${work}/good/python3.12" ]]; then
  ok "a newer python3.x is preferred over an older PATH python3"
else bad "did not prefer python3.12 over the old python3 (${out:-none})"; fi

if out="$(run "${work}/good:${tools}" "${work}/old/python3" 2>&1)"; then bad "an explicit old PYTHON was accepted"
elif [[ "${out}" == *"found Python 3.9.6 (${work}/old/python3)"* ]]; then ok "an explicit PYTHON is the only candidate and is refused when old"
else bad "unexpected explicit-PYTHON refusal: ${out}"; fi

if out="$(run "${work}/old:${tools}" "${work}/good/python3.12" 2>/dev/null)" && [[ "${out}" == "${work}/good/python3.12" ]]; then
  ok "an explicit new PYTHON is used"
else bad "explicit new PYTHON not used (${out:-none})"; fi

if env -i PATH="${tools}" bash "${helper}" 3.x probe >/dev/null 2>&1; then bad "a malformed MIN was accepted"
else ok "a malformed MIN is a usage error"; fi

echo "require-python: ${pass} passed, ${fail} failed"
[[ "${fail}" -eq 0 ]]
