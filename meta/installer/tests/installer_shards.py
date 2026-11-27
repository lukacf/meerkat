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

A run can have more than one attempt: a rerun of some jobs runs those jobs
again and leaves the others as they were. A shard that is not rerun keeps the
plan and the toolchain file of the attempt it ran in, under that attempt's
artifact names, and the newer attempt's job list repeats its result under the
newer attempt's number, so neither a name pattern for one attempt nor the job
list says where a shard's files are. Each shard therefore writes its identity
beside its run list (installer-shard-identity.json: the run, the attempt, the
tested commit, the shard, and the collection the plan was computed from) and
again beside its toolchain file, which it writes only after its tests passed.
The backend job collects the plans and toolchain files of every attempt of
its run and takes, for each shard, the newest execution by those identities.
It refuses plans that are not of its run, of one tested commit and of one
collection, a shard without a plan, a taken plan without its own toolchain
file, and any folder it is given that is not one execution's.

What this script cannot see: an execution that failed before it uploaded a
plan leaves nothing, so "newest" here is the newest execution that planned.
If such a failed execution were a shard's newest, the script alone would take
the older one. The workflow's job graph refuses that case, not this script:
the backend job needs every shard job and has no condition of its own, so it
does not run while any shard's newest result is not a success. Keep it so
(meta/scripts/test_installer_release.py pins it).

Usage: installer_shards.py SHARD COUNT FILES_OUT writes the test files this
shard runs to FILES_OUT and its identity beside it. installer_shards.py verify
COUNT PLANS fails unless the newest run lists of the COUNT shards, one folder
per execution under PLANS, ran every installer test file once.
installer_shards.py toolchain COUNT PLANS TOOLCHAINS prints the one toolchain
the executions taken from PLANS wrote, one folder per execution under
TOOLCHAINS. installer_shards.py record JUNIT... rewrites
installer_shard_seconds.json from a complete run's junit files.
"""
from __future__ import annotations

from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

TESTS = Path(__file__).resolve().parent
INSTALLER = TESTS.parent
SECONDS = TESTS / "installer_shard_seconds.json"
COLLECTED = re.compile(r"tests/(test_[A-Za-z0-9_]+\.py): ([0-9]+)")
COLLECTED_FILE = re.compile(r"\S+\.py: [0-9]+")
LISTING = "installer-shard-files.txt"
IDENTITY = "installer-shard-identity.json"
TOOLCHAIN = "installer-toolchain.json"
# What one suite shares: the run, the commit it tested, the collection its
# plan was computed from. With the environment variable that names this run's.
SUITE = (("run_id", "run id", "GITHUB_RUN_ID"), ("commit", "tested commit", "GITHUB_SHA"),
         ("collection", "collection", None))


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


def identity(shard, shards, counts, environ=None):
    """This shard execution's identity: where it ran and what it planned from."""
    environ = os.environ if environ is None else environ
    unset = [name for name in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_SHA") if not environ.get(name)]
    if unset and environ.get("GITHUB_ACTIONS") == "true":
        raise SystemExit(f"Inside Actions without {', '.join(unset)}: this execution cannot say what it is.")
    collected = "".join(f"{name} {items}\n" for name, items in sorted(counts.items()))
    return {"run_id": environ.get("GITHUB_RUN_ID") or "local",
            "run_attempt": int(environ.get("GITHUB_RUN_ATTEMPT") or 1),
            "commit": environ.get("GITHUB_SHA") or "local",
            "shard": shard, "shards": shards,
            "collection": hashlib.sha256(collected.encode()).hexdigest()}


def read_identity(folder):
    """A folder's identity, or why it has none that can be used."""
    path = folder / IDENTITY
    if not path.is_file():
        return None, f"{folder.name} has no {IDENTITY}"
    try:
        value = json.loads(path.read_text())
    except ValueError:
        value = None
    if not (isinstance(value, dict)
            and set(value) == {"run_id", "run_attempt", "commit", "shard", "shards", "collection"}
            and all(type(value[key]) is int for key in ("run_attempt", "shard", "shards"))
            and value["run_attempt"] >= 1 and value["shards"] >= 1 and value["shard"] >= 0
            and all(isinstance(value[key], str) and value[key] for key in ("run_id", "commit", "collection"))):
        return None, f"{folder.name} has an unreadable {IDENTITY}"
    return value, None


def executions(folder, prefix, problems):
    """Every execution folder under FOLDER with its identity. A folder is one
    execution's when it is named PREFIX-<shard>-<attempt> as its own identity
    says; whatever else FOLDER holds is added to PROBLEMS."""
    entries, found = sorted(Path(folder).iterdir()), []
    loose = [path.name for path in entries if not path.is_dir()]
    if loose:
        problems.append(f"{Path(folder).name} holds {', '.join(loose)} outside a folder: "
                        "one folder per execution is expected")
    for path in entries:
        if not path.is_dir():
            continue
        value, problem = read_identity(path)
        if problem is None and path.name != f"{prefix}-{value['shard']}-{value['run_attempt']}":
            problem = f"{path.name} says it is shard {value['shard']} of attempt {value['run_attempt']}"
        if problem is None:
            found.append((path, value))
        else:
            problems.append(problem)
    return found


def select(shards, plans, environ=None):
    """Each shard's newest plan under PLANS, the plans it supersedes, and
    what stops the newest ones from being one suite of this run."""
    environ = os.environ if environ is None else environ
    problems, newest, superseded = [], {}, []
    for folder, value in executions(plans, "installer-shard-plan", problems):
        if not (value["shards"] == shards and value["shard"] < shards):
            problems.append(f"{folder.name} is shard {value['shard']} of {value['shards']}, not one of {shards}")
            continue
        held = newest.get(value["shard"])
        if held is not None and held[1]["run_attempt"] > value["run_attempt"]:
            superseded.append((folder, value))
            continue
        if held is not None:
            superseded.append(held)
        newest[value["shard"]] = (folder, value)
    chosen = [newest[shard] for shard in sorted(newest)]
    if len(chosen) != shards:
        missing = ", ".join(str(shard) for shard in range(shards) if shard not in newest)
        problems.insert(0, f"{len(chosen)} shard plans instead of {shards}")
        problems.insert(1, f"no plan of shard {missing}")
    for field, label, variable in SUITE:
        said = {}
        for folder, value in chosen:
            said.setdefault(value[field], []).append(folder.name)
        if len(said) > 1:
            most = max(said, key=lambda value: (len(said[value]), value))
            others = "; ".join(f"{', '.join(names)} has {value[:12]}" for value, names in sorted(said.items())
                               if value != most)
            problems.append(f"plans of more than one {label}: {others}; the other {len(said[most])} have {most[:12]}")
        here = environ.get(variable) if variable else None
        if variable and not here and environ.get("GITHUB_ACTIONS") == "true":
            # A hosted job always has both. Without one the comparison with
            # this run would be skipped, so it is refused.
            problems.append(f"inside Actions without {variable}: this run's {label} is unknown")
        strangers = [folder.name for folder, value in chosen if here and value[field] != here]
        if strangers:
            problems.append(f"not this run's {label} {here[:12]}: {', '.join(strangers)}")
    return chosen, sorted(superseded, key=lambda held: held[0].name), problems


def verify(shards, plans, expected_files=None, environ=None):
    """Fail unless the shards' newest run lists ran every test file once."""
    expected_files = tracked_test_files() if expected_files is None else expected_files
    chosen, superseded, problems = select(shards, plans, environ)
    ran = Counter()
    for folder, _ in chosen:
        if not (folder / LISTING).is_file():
            problems.append(f"{folder.name} has no {LISTING}")
            continue
        ran.update(line.removeprefix("installer/tests/") for line in (folder / LISTING).read_text().splitlines())
    for kind, names in (("ran nowhere", sorted(set(expected_files) - set(ran))),
                        ("ran more than once", sorted(name for name, count in ran.items() if count > 1)),
                        ("ran but is not expected", sorted(set(ran) - set(expected_files)))):
        if names:
            problems.append(f"{kind}: {', '.join(names)}")
    if problems:
        raise SystemExit("The shards did not run the suite exactly once. " + "; ".join(problems))
    attempts = {}
    for _, value in chosen:
        attempts.setdefault(value["run_attempt"], []).append(str(value["shard"]))
    taken = "; ".join(f"attempt {attempt}: shard{'s' if len(mine) > 1 else ''} {', '.join(mine)}"
                      for attempt, mine in sorted(attempts.items()))
    print(f"{len(chosen)} shards ran {len(ran)} test files once. Plans taken: {taken}."
          + (f" Not counted, superseded: {', '.join(folder.name for folder, _ in superseded)}." if superseded else ""))


def toolchain(shards, plans, toolchains, environ=None):
    """Print the one toolchain the executions taken from PLANS wrote.

    A shard writes its toolchain file after its tests passed, with its
    identity beside it. The file taken for a shard is the one whose identity
    is the taken plan's; an older file of the shard is never taken in its
    place. Every folder under TOOLCHAINS must be an execution that planned in
    this run, taken or superseded: nothing else is passed over in silence."""
    chosen, superseded, problems = select(shards, plans, environ)
    if problems:
        raise SystemExit("The shards' plans are not one suite of this run. " + "; ".join(problems))
    planned = {json.dumps(value, sort_keys=True) for _, value in chosen + superseded}
    held = {}
    for folder, value in executions(toolchains, "installer-toolchain-backend-shard", problems):
        key = json.dumps(value, sort_keys=True)
        if not (folder / TOOLCHAIN).is_file():
            problems.append(f"{folder.name} has no {TOOLCHAIN}")
        elif key not in planned:
            problems.append(f"{folder.name} is of no plan of this run")
        else:
            held.setdefault(key, []).append(folder / TOOLCHAIN)
    if problems:
        raise SystemExit("The shards' toolchain folders are not this run's executions. " + "; ".join(problems))
    values, missing = [], []
    for _, value in chosen:
        mine = held.get(json.dumps(value, sort_keys=True), [])
        if len(mine) == 1:
            values.append(mine[0].read_text())
        else:
            missing.append(f"shard {value['shard']} (attempt {value['run_attempt']})")
    if missing:
        raise SystemExit(f"No toolchain file of the execution that planned: {', '.join(missing)}. "
                         "A shard writes it only after its tests passed.")
    if len(values) != shards or len(set(values)) != 1:
        raise SystemExit(f"Expected {shards} identical shard toolchains, found {len(values)} records of "
                         f"{len(set(values))} kinds.")
    print(values[0], end="")


def main(argv):
    if argv[:1] == ["record"] and len(argv) > 1:
        return record(argv[1:])
    if argv[:1] == ["verify"] and len(argv) == 3:
        return verify(int(argv[1]), argv[2])
    if argv[:1] == ["toolchain"] and len(argv) == 4:
        return toolchain(int(argv[1]), argv[2], argv[3])
    if len(argv) != 3:
        raise SystemExit(__doc__)
    shard, shards = int(argv[0]), int(argv[1])
    if not 0 <= shard < shards:
        raise SystemExit(f"Shard {shard} is outside 0..{shards - 1}.")
    counts = collected_items(test_files())
    mine, own = plan(weights(counts), shards)[shard], identity(shard, shards, counts)
    Path(argv[2]).write_text("".join(f"installer/tests/{name}\n" for name in mine))
    Path(argv[2]).with_name(IDENTITY).write_text(json.dumps(own, sort_keys=True) + "\n")


if __name__ == "__main__":
    main(sys.argv[1:])
