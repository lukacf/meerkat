#!/usr/bin/env python3
"""The census workflow's jobs: which release crates each job tests.

The census (`published_crates_standalone.py census`) tests its crates one
after another and writes its report once, after the last. A runner lost
under one crate's check takes the finished crates' verdicts with it. The
workflow therefore gives the crate `--isolate` names a job of its own and
leaves every other crate in the job the census's own stride puts it in:

  published_crates_standalone_jobs.py run --job K --partitions N
      --isolate NAME --out DIR
      The Kth of the N partitions of the release list (every Nth crate
      starting at the Kth of scripts/release-rust-crates.sh, the stride
      `census --shard K/N` applies), without NAME.

  published_crates_standalone_jobs.py run --job isolated --partitions N
      --isolate NAME --out DIR
      NAME alone. NAME must be a release crate: a name the list no longer
      has fails this job, so the stale name is seen; the partitions are
      then whole and still test every crate once.

  published_crates_standalone_jobs.py plan --partitions N --isolate NAME
      Print every job's crates, one job per line. It runs nothing.

`run` hands the job's crates to `make published-crates-standalone-census`
as `--crate` arguments and passes no `--shard`: the stride is applied here,
once, over the whole list. A job without a crate (NAME was the only crate of
its partition, or the list is shorter than N) does not start the census,
because a census without `--crate` tests every release crate: it says that
it has nothing to test and exits 0, as a census of an empty partition does.

Exit: `run` returns the census's exit code (0 without a crate); 2 = usage
error (an unknown job, a NAME that is not a crate name, a release list with
a repeated or malformed name).
"""

from __future__ import annotations

import argparse
import importlib.util
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
ISOLATED = "isolated"
CRATE_NAME = re.compile(r"[a-z][a-z0-9-]*")


class UsageError(Exception):
    pass


def census_module():
    """The census script, loaded as a module: its `release_crates` is the one
    reader of the release list."""
    path = pathlib.Path(__file__).resolve().parent / "published_crates_standalone.py"
    spec = importlib.util.spec_from_file_location("published_crates_standalone", path)
    if spec is None or spec.loader is None:
        raise UsageError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def checked_list(crates: list[str], isolate: str) -> list[str]:
    """The release list, refused unless every name is a crate name and none
    repeats; `isolate` must be a crate name too (it need not be listed)."""
    if not CRATE_NAME.fullmatch(isolate):
        raise UsageError(f"--isolate must name a crate, got {isolate!r}")
    malformed = [name for name in crates if not CRATE_NAME.fullmatch(name)]
    if malformed:
        raise UsageError(f"not crate names in the release list: {malformed}")
    repeated = sorted({name for name in crates if crates.count(name) > 1})
    if repeated:
        raise UsageError(f"repeated in the release list: {', '.join(repeated)}")
    return crates


def partition(crates: list[str], k: int, n: int) -> list[str]:
    """The census's stride: what `census --shard K/N` tests of `crates`."""
    return crates[k - 1 :: n]


def job_crates(crates: list[str], job: str, partitions: int, isolate: str) -> list[str]:
    """The crates the job tests, in list order."""
    crates = checked_list(crates, isolate)
    if partitions < 1:
        raise UsageError(f"--partitions must be at least 1, got {partitions}")
    if job == ISOLATED:
        if isolate not in crates:
            raise UsageError(f"{isolate} is not a release crate: the isolated job has nothing to test")
        return [isolate]
    if not re.fullmatch(r"[1-9][0-9]*", job) or int(job) > partitions:
        raise UsageError(f"--job must be {ISOLATED} or K with 1 <= K <= {partitions}, got {job!r}")
    return [name for name in partition(crates, int(job), partitions) if name != isolate]


def jobs(partitions: int) -> list[str]:
    return [str(k) for k in range(1, partitions + 1)] + [ISOLATED]


def census_arguments(names: list[str]) -> list[str]:
    """The census's selection of exactly `names`. Never empty: a census
    without `--crate` tests every release crate."""
    if not names:
        raise UsageError("a census needs at least one crate to select")
    return [word for name in names for word in ("--crate", name)]


def census_command(names: list[str], out: pathlib.Path) -> list[str]:
    return [
        "make",
        "published-crates-standalone-census",
        f"STANDALONE_CENSUS_OUT={out}",
        f"STANDALONE_CENSUS_ARGS={' '.join(census_arguments(names))}",
    ]


def start_census(command: list[str]) -> int:
    return subprocess.run(command, cwd=ROOT).returncode


def run(crates: list[str], job: str, partitions: int, isolate: str, out: pathlib.Path) -> int:
    names = job_crates(crates, job, partitions, isolate)
    if not names:
        print(
            f"job {job} of {partitions}: no crate in this partition to test; "
            "the census is not started, since without --crate it tests every release crate",
            flush=True,
        )
        return 0
    print(f"job {job} of {partitions}: {' '.join(names)}", flush=True)
    return start_census(census_command(names, out))


def plan(crates: list[str], partitions: int, isolate: str) -> int:
    for job in jobs(partitions):
        try:
            names = " ".join(job_crates(crates, job, partitions, isolate)) or "(no crate)"
        except UsageError as err:
            names = f"(fails: {err})"
        print(f"{job}: {names}")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("run", "plan"):
        command = sub.add_parser(name)
        command.add_argument("--partitions", required=True, type=int)
        command.add_argument("--isolate", required=True)
        if name == "run":
            command.add_argument("--job", required=True)
            command.add_argument("--out", required=True, type=pathlib.Path)
    args = parser.parse_args(argv)
    try:
        crates = census_module().release_crates(ROOT)
        if args.command == "plan":
            return plan(crates, args.partitions, args.isolate)
        return run(crates, args.job, args.partitions, args.isolate, args.out)
    except UsageError as err:
        print(f"published_crates_standalone_jobs: {err}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
