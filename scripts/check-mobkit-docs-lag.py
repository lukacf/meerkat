#!/usr/bin/env python3
"""Fail when the docs.rkat.ai MobKit mirror lags MobKit main.

docs/mobkit is a generated snapshot of one MobKit main commit, recorded in
docs/mobkit/_source.json as `source_commit`. Every push to MobKit main that
touches docs/ is supposed to regenerate the snapshot through the Publish
MobKit docs workflow, but a dispatch can be lost or the publication can fail
after the source moved on. This ratchet compares the mirrored commit with the
head of MobKit main and fails when more than `--max-lag` commits that touch
docs/ were pushed to main after the mirrored one.

Why commits touching docs/ rather than all commits or wall-clock age: a code
commit on MobKit main changes nothing the mirror renders, so counting it would
page on a mirror that is in fact current; wall-clock age has the same problem
in reverse (a stale mirror looks fine on a quiet week). A doc-touching commit
that is not mirrored is exactly the defect.

Usage:
    gh api "repos/lukacf/meerkat-mobkit/compare/<mirrored>...main" > compare.json
    python3 scripts/check-mobkit-docs-lag.py --compare compare.json
"""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "docs" / "mobkit" / "_source.json"
DOCS_PREFIX = "docs/"


@dataclass(frozen=True)
class DocsLag:
    """Commits on MobKit main after the mirrored commit, split by docs impact."""

    ahead_by: int
    docs_commits: tuple[str, ...]
    head_sha: str


class LagCheckError(Exception):
    """The lag could not be computed; this is not a zero lag."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--compare",
        required=True,
        type=Path,
        help=(
            "JSON object as returned by "
            "`gh api repos/<owner>/<repo>/compare/<mirrored>...main`"
        ),
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="generated MobKit docs manifest (default: docs/mobkit/_source.json)",
    )
    parser.add_argument(
        "--max-lag",
        type=int,
        default=0,
        help="largest number of unmirrored docs-touching main commits tolerated (default: 0)",
    )
    return parser.parse_args()


def manifest_source_commit(manifest_path: Path) -> str:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    source_commit = manifest.get("source_commit") if isinstance(manifest, dict) else None
    if not isinstance(source_commit, str) or not source_commit:
        raise LagCheckError(f"{manifest_path} has no source_commit")
    return source_commit


def docs_lag(compare: object) -> DocsLag:
    """Read the compare payload for `<mirrored>...main`.

    The compare endpoint reports the commits reachable from main but not from
    the mirrored commit, and the files changed across that range. A commit
    counts toward the lag only when it touches docs/; the per-commit file
    lists are not in this payload, so each commit is attributed through the
    range-level file list: when the range touches docs/ at all, every commit
    in the range is listed as a candidate and the range-level count is what
    the tolerance is compared with. This overstates a mixed range and never
    understates a docs-only one, which is the safe direction for a ratchet.
    """
    if not isinstance(compare, dict):
        raise LagCheckError("compare payload is not a JSON object")
    status = compare.get("status")
    if status not in {"identical", "ahead", "behind", "diverged"}:
        raise LagCheckError(f"compare payload has unknown status {status!r}")
    if status in {"behind", "diverged"}:
        raise LagCheckError(
            f"mirrored MobKit commit is not an ancestor of main (status {status}); "
            "the mirror was generated from something other than main"
        )
    ahead_by = compare.get("ahead_by")
    if not isinstance(ahead_by, int):
        raise LagCheckError("compare payload has no ahead_by count")
    commits = compare.get("commits")
    if not isinstance(commits, list):
        raise LagCheckError("compare payload has no commits array")
    files = compare.get("files")
    if not isinstance(files, list):
        raise LagCheckError("compare payload has no files array")
    head_sha = (
        compare.get("base_commit", {}).get("sha") if status == "identical" else None
    )
    if commits:
        head_sha = commits[-1].get("sha")
    if not isinstance(head_sha, str):
        raise LagCheckError("compare payload names no head commit")
    touches_docs = any(
        isinstance(entry, dict)
        and isinstance(entry.get("filename"), str)
        and (
            entry["filename"].startswith(DOCS_PREFIX)
            or (
                isinstance(entry.get("previous_filename"), str)
                and entry["previous_filename"].startswith(DOCS_PREFIX)
            )
        )
        for entry in files
    )
    docs_commits = (
        tuple(str(commit.get("sha", ""))[:12] for commit in commits if isinstance(commit, dict))
        if touches_docs
        else ()
    )
    return DocsLag(ahead_by=ahead_by, docs_commits=docs_commits, head_sha=head_sha)


def main() -> int:
    args = parse_args()
    try:
        source_commit = manifest_source_commit(args.manifest)
        lag = docs_lag(json.loads(args.compare.read_text(encoding="utf-8")))
    except LagCheckError as error:
        print(f"mobkit-docs-lag: cannot determine lag: {error}")
        return 2
    unmirrored = len(lag.docs_commits)
    print(
        f"mobkit-docs-lag: mirror renders MobKit main at {source_commit[:12]}; main head is "
        f"{lag.head_sha[:12]}; {lag.ahead_by} commit(s) ahead, {unmirrored} touching docs/, "
        f"tolerated {args.max_lag}"
    )
    if unmirrored > args.max_lag:
        print(
            f"mobkit-docs-lag: docs/mobkit misses {unmirrored} docs-touching commit(s) on MobKit "
            f"main: {', '.join(lag.docs_commits)}. Run the Publish MobKit docs workflow "
            "(workflow_dispatch) or `make docs-sync-mobkit` from a clean MobKit main checkout."
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
