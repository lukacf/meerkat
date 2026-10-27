"""Split the Forge backend suite into CI shards balanced on expected time.

Every forge.yml shard computes the same plan. Each installer test file is one
run, and each file checkout_sdk_startup_tests.txt lists is one more run with
this checkout's Meta SDK. Heaviest first, each run goes to the shard with the
least expected time so far (ties go to the lower shard, equal runs in name
order, a file's own run before its rerun), so the shards together run every
collected test file exactly once and rerun every listed file exactly once. A
run weighs the seconds a hosted run recorded for its file in
forge_shard_seconds.json, or, without a record, the file's collected item
count at the recorded median seconds per item. Records only steer the
balance; a stale record never drops or repeats a run. A collection error, or
a collected file no plan can place, fails every shard: no shard runs a suite
that silently lost a file. Because each shard plans on its own, the backend
job then checks the shards' run lists together (verify).

Usage: forge_shards.py SHARD COUNT FILES_OUT STARTUP_OUT writes the test
files this shard runs to FILES_OUT and the listed files it reruns with this
checkout's Meta SDK to STARTUP_OUT. forge_shards.py verify COUNT PLANS fails
unless the COUNT shards' run lists, one folder each under PLANS, ran every
installer test file once and reran every listed file once.
forge_shards.py record JUNIT... rewrites forge_shard_seconds.json from a
complete run's junit files.
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
STARTUP_LIST = TESTS / "checkout_sdk_startup_tests.txt"
SECONDS = TESTS / "forge_shard_seconds.json"
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


def plan(weighted, shards, reruns=()):
    """Each shard's (files, reruns): every file once, every rerun once, balanced."""
    runs = [(weight, name, False) for name, weight in weighted.items()]
    runs += [(weighted[name], name, True) for name in reruns if name in weighted]
    loads, assigned = [0] * shards, [([], []) for _ in range(shards)]
    for weight, name, rerun in sorted(runs, key=lambda run: (-run[0], run[1], run[2])):
        shard = min(range(shards), key=lambda index: (loads[index], index))
        assigned[shard][rerun].append(name)
        loads[shard] += weight
    return [(sorted(files), sorted(again)) for files, again in assigned]


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


def verify(shards, plans, expected_files=None, expected_reruns=None):
    """Fail unless the shards' run lists ran every test file once and reran every listed file once."""
    expected_files = tracked_test_files() if expected_files is None else expected_files
    expected_reruns = startup_files() if expected_reruns is None else expected_reruns
    folders = sorted(path for path in Path(plans).iterdir() if path.is_dir())
    problems = [] if len(folders) == shards else [f"{len(folders)} shard plans instead of {shards}"]
    ran, reran = Counter(), Counter()
    for folder in folders:
        for runs, name in ((ran, "forge-shard-files.txt"), (reran, "forge-shard-startup.txt")):
            if not (folder / name).is_file():
                problems.append(f"{folder.name} has no {name}")
                continue
            runs.update(line.removeprefix("installer/tests/") for line in (folder / name).read_text().splitlines())
    for verb, runs, expected in (("ran", ran, expected_files), ("reran", reran, expected_reruns)):
        for kind, names in ((f"{verb} nowhere", sorted(set(expected) - set(runs))),
                            (f"{verb} more than once", sorted(name for name, count in runs.items() if count > 1)),
                            (f"{verb} but is not expected", sorted(set(runs) - set(expected)))):
            if names:
                problems.append(f"{kind}: {', '.join(names)}")
    if problems:
        raise SystemExit("The shards did not run the suite exactly once. " + "; ".join(problems))
    print(f"{len(folders)} shards ran {len(ran)} test files once and reran {len(reran)} listed files once.")


def startup_files():
    names = []
    for line in STARTUP_LIST.read_text().splitlines():
        entry = line.split("#", 1)[0].strip()
        if entry:
            names.append(entry)
    return names


def main(argv):
    if argv[:1] == ["record"] and len(argv) > 1:
        return record(argv[1:])
    if argv[:1] == ["verify"] and len(argv) == 3:
        return verify(int(argv[1]), argv[2])
    if len(argv) != 4:
        raise SystemExit(__doc__)
    shard, shards = int(argv[0]), int(argv[1])
    if not 0 <= shard < shards:
        raise SystemExit(f"Shard {shard} is outside 0..{shards - 1}.")
    files = test_files()
    startup = startup_files()
    unknown = sorted(set(startup) - set(files))
    if unknown:
        raise SystemExit(f"{STARTUP_LIST.name} lists missing test files: {', '.join(unknown)}")
    mine, again = plan(weights(collected_items(files)), shards, set(startup))[shard]
    Path(argv[2]).write_text("".join(f"installer/tests/{name}\n" for name in mine))
    Path(argv[3]).write_text("".join(f"installer/tests/{name}\n" for name in again))


if __name__ == "__main__":
    main(sys.argv[1:])
