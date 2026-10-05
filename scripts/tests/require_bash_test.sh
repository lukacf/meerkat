#!/usr/bin/env bash
# scripts/lib/require-bash.sh stops a script on Bash < 4.4 with a clear reason.
# BASH_VERSINFO is read-only, so the guard runs with its version variables
# renamed to fakes; the guard text itself is otherwise the real file.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
guard="${ROOT}/scripts/lib/require-bash.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/require-bash-test.XXXXXX")"
trap 'rm -rf "${work}"' EXIT
sed -e 's/BASH_VERSINFO/FAKE_VERSINFO/g' -e 's/BASH_VERSION/FAKE_VERSION/g' "${guard}" >"${work}/guard.sh"
pass=0; fail=0
ok() { printf '  OK:   %s\n' "$*"; pass=$((pass + 1)); }
bad() { printf '  FAIL: %s\n' "$*"; fail=$((fail + 1)); }

probe() { # probe <major> <minor> <version>
  bash -c 'FAKE_VERSINFO=("$1" "$2"); FAKE_VERSION="$3"; . "$4"; echo GUARD_PASSED' \
    probe-script "$1" "$2" "$3" "${work}/guard.sh" 2>&1
}

out="$(probe 3 2 3.2.57)" || true
[[ "${out}" == *"needs Bash >= 4.4; found 3.2.57"* && "${out}" != *GUARD_PASSED* ]] \
  && ok "Bash 3.2 is refused with the found version" || bad "Bash 3.2 not refused: ${out}"
out="$(probe 4 3 4.3.48)" || true
[[ "${out}" == *"needs Bash >= 4.4; found 4.3.48"* ]] && ok "Bash 4.3 is refused" || bad "Bash 4.3 not refused: ${out}"
out="$(probe 4 4 4.4.23)" || true
[[ "${out}" == *GUARD_PASSED* ]] && ok "Bash 4.4 passes" || bad "Bash 4.4 refused: ${out}"
out="$(probe 5 2 5.2.21)" || true
[[ "${out}" == *GUARD_PASSED* ]] && ok "Bash 5.2 passes" || bad "Bash 5.2 refused: ${out}"
# The guard must not use Bash 4+ syntax itself (it has to run on 3.2).
if grep -nE '\[\[|mapfile|readarray|declare -A|BASHPID|\$\{[A-Za-z_]+(,,|\^\^)\}' "${guard}" | grep -v '^[0-9]*:#'; then
  bad "the guard uses Bash 4+ syntax"
else
  ok "the guard uses only Bash 3.2 syntax"
fi
echo "require-bash: ${pass} passed, ${fail} failed"
[[ "${fail}" -eq 0 ]]
