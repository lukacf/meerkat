#!/usr/bin/env bash
# MobMachine arm-coverage gate.
#
# usage: arm_coverage_gate.sh [--list-uncovered]
#
# Every arm of the generated model's Next relation (specs/machines/mob_machine/
# model.tla, one disjunct per expanded transition) must be either:
# - covered: named in a `*covered_actions` array of a hand-written audit runner
#   next to this script (`*_audit.sh`). Each such runner requires TLC coverage
#   to show every arm of its arrays firing, so a covered arm is checked, not
#   just mentioned; or
# - not covered: matched by a line of arm_coverage_not_covered.txt, which names
#   the arm (or an arm family as `Prefix*`) and the reason it is not driven.
#
# The gate fails on an arm in neither set, on a not-covered line that names no
# uncovered arm (stale or redundant), on an exact not-covered line that names
# a covered arm (contradiction), on a duplicate line and on a line without a
# reason. The generated ci profile has no identities and proves nothing about
# member arms, so it never counts as coverage.
#
# With --list-uncovered it prints the arms in neither set, one per line, and
# exits 0 (an aid for writing the list). The check reads files only; it runs
# no TLC.
set -euo pipefail

mode="check"
if [[ "${1:-}" == "--list-uncovered" ]]; then
  mode="list-uncovered"
  shift
fi
if [[ $# -ne 0 ]]; then
  echo "usage: arm_coverage_gate.sh [--list-uncovered]" >&2
  exit 2
fi

spec_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${spec_dir}/model.tla" \
  SPEC_DIR="${spec_dir}" \
  NOT_COVERED="${spec_dir}/arm_coverage_not_covered.txt" \
  MODE="${mode}" \
  python3 - <<'PY'
import os
import re
import sys
from pathlib import Path

model_path = Path(os.environ["MODEL"])
spec_dir = Path(os.environ["SPEC_DIR"])
not_covered_path = Path(os.environ["NOT_COVERED"])
mode = os.environ["MODE"]

# Arms of the generated Next relation: each disjunct line ends with the arm
# operator, called with its bound parameters or bare.
lines = model_path.read_text().split("\n")
try:
    start = lines.index("Next ==")
except ValueError:
    sys.exit(f"error: no `Next ==` definition in {model_path}")
arm_pattern = re.compile(r"([A-Za-z][A-Za-z0-9_]*)(\([^()]*\))?\s*$")
arms = []
for line in lines[start + 1:]:
    if not line.startswith("    \\/ "):
        break
    match = arm_pattern.search(line)
    if match is None:
        sys.exit(f"error: cannot read the arm of Next disjunct: {line[:160]}")
    name = match.group(1)
    if name != "TerminalStutter":
        arms.append(name)
if not arms:
    sys.exit(f"error: the Next relation of {model_path} names no arm")
arm_set = set(arms)

# Covered arms: every name in a `*covered_actions=( ... )` array of an audit
# runner. Names that are not model arms (audit wrappers) count for nothing.
array_start = re.compile(r"^\s*((?:[A-Za-z0-9_]*_)?covered_actions)=\(\s*$")
covered = {}
for runner in sorted(spec_dir.glob("*_audit.sh")):
    in_array = None
    for number, raw in enumerate(runner.read_text().split("\n"), 1):
        if in_array is None:
            found = array_start.match(raw)
            if found:
                in_array = found.group(1)
            continue
        entry = raw.split("#", 1)[0].strip()
        if entry == ")":
            in_array = None
            continue
        if not entry:
            continue
        for name in entry.split():
            if name in arm_set:
                covered.setdefault(name, f"{runner.name}:{in_array}")
    if in_array is not None:
        sys.exit(f"error: {runner.name}: array {in_array} is not closed")

# The not-covered list.
errors = []
entries = []
seen = set()
if not not_covered_path.exists():
    sys.exit(f"error: missing {not_covered_path}")
for number, raw in enumerate(not_covered_path.read_text().split("\n"), 1):
    text = raw.split("#", 1)[0].strip()
    if not text:
        continue
    parts = text.split(None, 1)
    pattern = parts[0]
    reason = parts[1].strip() if len(parts) > 1 else ""
    where = f"{not_covered_path.name}:{number}"
    if not reason:
        errors.append(f"{where}: `{pattern}` has no reason")
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*\*?", pattern):
        errors.append(f"{where}: `{pattern}` is not an arm name or `Prefix*`")
        continue
    if pattern in seen:
        errors.append(f"{where}: `{pattern}` is listed twice")
    seen.add(pattern)
    entries.append((where, pattern))

def matches(pattern, arm):
    if pattern.endswith("*"):
        return arm.startswith(pattern[:-1])
    return arm == pattern

uncovered = [arm for arm in arms if arm not in covered]
listed = set()
for where, pattern in entries:
    justified = [arm for arm in uncovered if matches(pattern, arm)]
    if not pattern.endswith("*") and pattern in covered:
        errors.append(
            f"{where}: `{pattern}` is covered by {covered[pattern]}; drop it from the not-covered list"
        )
    elif not justified:
        errors.append(
            f"{where}: `{pattern}` names no uncovered model arm (stale or redundant entry)"
        )
    listed.update(justified)

missing = [arm for arm in uncovered if arm not in listed]

if mode == "list-uncovered":
    for arm in missing:
        print(arm)
    sys.exit(0)

for arm in missing:
    errors.append(
        f"arm `{arm}` is neither covered by an audit runner's covered_actions nor named in {not_covered_path.name}"
    )
if errors:
    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    sys.exit(
        f"MobMachine arm coverage gate failed: {len(errors)} problem(s); "
        f"{len(arms)} arms, {len(covered)} covered, {len(listed)} listed not covered"
    )
print(
    f"MobMachine arm coverage gate passed: {len(arms)} arms, {len(covered)} covered by audits, "
    f"{len(listed)} listed not covered with a reason"
)
PY
