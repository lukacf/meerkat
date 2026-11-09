"""Split the Forge backend suite into CI shards balanced on expected time.

Every shard of the installer workflow computes the same plan. Each installer
test file is one run. Heaviest first, each run goes to the shard with the
least expected time so far (ties go to the lower shard, equal runs in name
order), so the shards together run every collected test file exactly once. A
run weighs the seconds a hosted run recorded for its file in
installer_shard_seconds.json, or, without a record, the file's collected item
count at the recorded median seconds per item. Records only steer the
balance; a stale record never drops or repeats a run. A collection error, or
a collected file no plan can place, fails every shard: no shard runs a suite
that silently lost a file. Because each shard plans on its own, the backend
job then checks the shards' run lists together (verify).

Usage: installer_shards.py SHARD COUNT FILES_OUT writes the test files this
shard runs to FILES_OUT. installer_shards.py verify COUNT PLANS fails unless
the COUNT shards' run lists, one folder each under PLANS, ran every installer
test file once. installer_shards.py record JUNIT... rewrites
installer_shard_seconds.json from a complete run's junit files.
"""
from __future__ import annotations

from collections import Counter
import json
from pathlib import Path
import re
import subprocess
import sys

TESTS = Path(__file__).resolve().parent
INSTALLER = TESTS.parent
SECONDS = TESTS / "installer_shard_seconds.json"
COLLECTED = re.compile(r"tests/(test_[A-Za-z0-9_]+\.py): ([0-9]+)")
COLLECTED_FILE = re.compile(r"\S+\.py: [0-9]+")


def test_files():
    return sorted(path.name for path in TESTS.glob("test_*.py"))


def tracked_test_files():
    """Every installer test file pytest collects, at any depth under tests."""
    return sorted(path.relative_to(TESTS).as_posix() for path in TESTS.rglob("test_*.py"))


def collected_items(files):
    """Each test file's collected item count, from pytest's own collection."""
    result = subprocess.run([sys.executable, "-m", "pytest", "--collect-only", "-qq", "-p", "no:cacheprovider", "tests"],
                            cwd=INSTALLER, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        sys.stderr.write(result.stdout[-20000:] + result.stderr[-20000:])
        raise SystemExit(f"The Forge test collection failed (exit {result.returncode}); no shard runs a partial suite.")
    counts = dict.fromkeys(files, 0)
    for line in result.stdout.splitlines():
        match = COLLECTED.fullmatch(line.strip())
        if match is None and COLLECTED_FILE.fullmatch(line.strip()):
            raise SystemExit(f"The Forge test collection listed a file no shard can place: {line.strip()}")
        if match:
            if match[1] not in counts:
                raise SystemExit(f"The Forge test collection listed an unknown file: {match[1]}")
            counts[match[1]] = int(match[2])
    if not any(counts.values()):
        raise SystemExit("The Forge test collection listed no test items.")
    return counts


def recorded_seconds(path=SECONDS):
    if not path.exists():
        return {}
    value = json.loads(path.read_text())
    if not isinstance(value, dict) or not all(
            isinstance(name, str) and type(seconds) in (int, float) and seconds >= 0
            for name, seconds in value.items()):
        raise SystemExit(f"{path.name} must map test file names to recorded seconds.")
    return value


def weights(counts, recorded=None):
    """Each file's expected seconds for one run."""
    recorded = recorded_seconds() if recorded is None else recorded
    rates = sorted(recorded[name] / items for name, items in counts.items() if items and name in recorded)
    rate = rates[len(rates) // 2] if rates else 1.0
    return {name: recorded[name] if name in recorded else items * rate for name, items in counts.items()}


def plan(weighted, shards):
    """Each shard's files: every file once, balanced."""
    loads, assigned = [0] * shards, [[] for _ in range(shards)]
    for weight, name in sorted(((weight, name) for name, weight in weighted.items()),
                               key=lambda run: (-run[0], run[1])):
        shard = min(range(shards), key=lambda index: (loads[index], index))
        assigned[shard].append(name)
        loads[shard] += weight
    return [sorted(files) for files in assigned]


def record(junit_files, path=SECONDS):
    """Write each test file's total testcase seconds from complete junit files."""
    import xml.etree.ElementTree as ElementTree
    seconds = {}
    for junit in junit_files:
        for case in ElementTree.parse(junit).iter("testcase"):
            # tests.test_file, or tests.test_file.TestClass for a class's case.
            module = next((part for part in case.get("classname", "").split(".") if part.startswith("test_")), None)
            if module is not None:
                name = module + ".py"
                seconds[name] = seconds.get(name, 0.0) + float(case.get("time") or 0)
    path.write_text(json.dumps({name: round(value, 1) for name, value in sorted(seconds.items())}, indent=1) + "\n")


def verify(shards, plans, expected_files=None):
    """Fail unless the shards' run lists ran every test file once."""
    expected_files = tracked_test_files() if expected_files is None else expected_files
    folders = sorted(path for path in Path(plans).iterdir() if path.is_dir())
    problems = [] if len(folders) == shards else [f"{len(folders)} shard plans instead of {shards}"]
    ran = Counter()
    listing = "installer-shard-files.txt"
    for folder in folders:
        if not (folder / listing).is_file():
            problems.append(f"{folder.name} has no {listing}")
            continue
        ran.update(line.removeprefix("installer/tests/") for line in (folder / listing).read_text().splitlines())
    for kind, names in (("ran nowhere", sorted(set(expected_files) - set(ran))),
                        ("ran more than once", sorted(name for name, count in ran.items() if count > 1)),
                        ("ran but is not expected", sorted(set(ran) - set(expected_files)))):
        if names:
            problems.append(f"{kind}: {', '.join(names)}")
    if problems:
        raise SystemExit("The shards did not run the suite exactly once. " + "; ".join(problems))
    print(f"{len(folders)} shards ran {len(ran)} test files once.")


def main(argv):
    if argv[:1] == ["record"] and len(argv) > 1:
        return record(argv[1:])
    if argv[:1] == ["verify"] and len(argv) == 3:
        return verify(int(argv[1]), argv[2])
    if len(argv) != 3:
        raise SystemExit(__doc__)
    shard, shards = int(argv[0]), int(argv[1])
    if not 0 <= shard < shards:
        raise SystemExit(f"Shard {shard} is outside 0..{shards - 1}.")
    mine = plan(weights(collected_items(test_files())), shards)[shard]
    Path(argv[2]).write_text("".join(f"installer/tests/{name}\n" for name in mine))


if __name__ == "__main__":
    main(sys.argv[1:])
