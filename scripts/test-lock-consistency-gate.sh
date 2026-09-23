#!/usr/bin/env bash
# Contract test for the Cargo.lock consistency gate.
#
# Reproduces the 0.8.22 publish failure shape: a branch-only crate whose
# dependency reference names a version the merged lock no longer contains. The
# textual merge produced no conflict marker, so the defect was invisible to
# every cargo mode except --locked, which is what cargo publish uses.
#
# The root merge checks are file-only. Standalone example coverage below uses
# real Cargo against dependency-free local fixtures with networking disabled.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"
CHECKER="${REPO_ROOT}/scripts/check_cargo_lock_consistency.py"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-lock-consistency.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

run_checker() {
  local lock_path="$1"
  local output_path="$2"
  set +e
  "$PYTHON" "$CHECKER" "$lock_path" >"$output_path" 2>&1
  local status=$?
  set -e
  printf '%s' "$status"
}

fail() {
  echo "lock consistency gate contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  ${extra}" >&2
  done
  exit 1
}

# 1. The committed lock must pass.
status="$(run_checker "${REPO_ROOT}/Cargo.lock" "${TEST_ROOT}/committed.log")"
if [[ "$status" -ne 0 ]]; then
  fail "the committed Cargo.lock is reported inconsistent" \
    "$(cat "${TEST_ROOT}/committed.log")"
fi

# 2. The historical shape: a dependency reference to a version no [[package]]
#    block provides. Derived from the committed lock so the fixture cannot
#    drift away from the real file's format.
dangling_lock="${TEST_ROOT}/dangling-Cargo.lock"
"$PYTHON" - "${REPO_ROOT}/Cargo.lock" "$dangling_lock" <<'PYEOF'
import pathlib
import re
import sys

source, destination = (pathlib.Path(argument) for argument in sys.argv[1:3])
text = source.read_text()
match = re.search(r'^ "([A-Za-z0-9_-]+) (\d+\.\d+\.\d+)",$', text, re.MULTILINE)
if match is None:
    raise SystemExit("committed lock has no version-qualified dependency reference to mutate")
name, version = match.group(1), match.group(2)
bumped = f"{version.rsplit('.', 1)[0]}.9999"
if f'version = "{bumped}"' in text:
    raise SystemExit(f"synthetic version {bumped} unexpectedly exists in the lock")
destination.write_text(
    text[: match.start()] + f' "{name} {bumped}",' + text[match.end() :]
)
print(f"{name} {version} -> {name} {bumped}")
PYEOF

status="$(run_checker "$dangling_lock" "${TEST_ROOT}/dangling.log")"
if [[ "$status" -eq 0 ]]; then
  fail "a dangling version reference was accepted"
fi
if ! grep -q "references a version no \[\[package\]\] block provides" "${TEST_ROOT}/dangling.log"; then
  fail "the dangling reference failure does not name the defect" \
    "$(cat "${TEST_ROOT}/dangling.log")"
fi

# 3. A reference to a package the lock does not contain at all.
missing_lock="${TEST_ROOT}/missing-Cargo.lock"
cat > "$missing_lock" <<'EOF'
version = 4

[[package]]
name = "example"
version = "0.1.0"
dependencies = [
 "absent-crate",
]
EOF
status="$(run_checker "$missing_lock" "${TEST_ROOT}/missing.log")"
if [[ "$status" -eq 0 ]]; then
  fail "a reference to an absent package was accepted"
fi
if ! grep -q "absent-crate" "${TEST_ROOT}/missing.log"; then
  fail "the absent-package failure does not name the package" \
    "$(cat "${TEST_ROOT}/missing.log")"
fi

"$PYTHON" "$REPO_ROOT/scripts/test_example_locks.py"

echo "lock consistency gate contract holds"
