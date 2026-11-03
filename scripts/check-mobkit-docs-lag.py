#!/usr/bin/env python3
"""Fail when the docs.rkat.ai MobKit mirror lags main.

docs/mobkit is a generated snapshot of MobKit's documentation (meta/docs in
this repository) at one main commit, recorded in docs/mobkit/_source.json as
`source_commit`, with the docs path as `source_path`. Every push to main that
changes meta/docs is supposed to regenerate the snapshot through the Publish
MobKit docs workflow, but a publication can fail (or wait on a pull request)
after main moved on. This ratchet counts the commits on the checked-out main
after the mirrored one that change the docs path, and fails when there are
more than `--max-lag` of them.

Why commits changing the docs path rather than all commits or wall-clock age:
any other commit on main changes nothing the mirror renders, so counting it
would page on a mirror that is in fact current; wall-clock age has the same
problem in reverse (a stale mirror looks fine on a quiet week). A
docs-changing commit that is not mirrored is exactly the defect.

The mirrored commit must be on the checked-out history (main with full
history): a mirror rendered from a commit main does not contain (a branch, a
squashed pull request) is a lag that cannot be measured, not a zero lag.

Usage (a full-history checkout of main):
    python3 scripts/check-mobkit-docs-lag.py [--manifest docs/mobkit/_source.json] [--max-lag 0]
"""

from __future__ import annotations

import argparse
import json
import subprocess
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "docs" / "mobkit" / "_source.json"
REPOSITORY = "https://github.com/lukacf/meerkat"
DEFAULT_DOCS_PATH = "meta/docs"


@dataclass(frozen=True)
class DocsLag:
    """Commits on the checked-out history after the mirrored one."""

    head_sha: str
    ahead_by: int
    docs_commits: tuple[str, ...]


class LagCheckError(Exception):
    """The lag could not be computed; this is not a zero lag."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="generated MobKit docs manifest (default: docs/mobkit/_source.json)",
    )
    parser.add_argument(
        "--repo",
        type=Path,
        default=ROOT,
        help="checkout whose HEAD is main (default: this repository)",
    )
    parser.add_argument(
        "--max-lag",
        type=int,
        default=0,
        help="largest number of unmirrored docs-changing commits tolerated (default: 0)",
    )
    return parser.parse_args()


def git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)


def manifest_source(manifest_path: Path) -> tuple[str, str]:
    """The mirrored commit and docs path, refusing a mirror of another repository."""
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise LagCheckError(f"{manifest_path} is unreadable: {error}") from error
    if not isinstance(manifest, dict):
        raise LagCheckError(f"{manifest_path} is not a JSON object")
    source_commit = manifest.get("source_commit")
    if not isinstance(source_commit, str) or not source_commit:
        raise LagCheckError(f"{manifest_path} has no source_commit")
    repository = manifest.get("source_repository")
    if repository != REPOSITORY:
        raise LagCheckError(
            f"{manifest_path} mirrors {repository!r}, not this repository: regenerate it from meta/docs "
            "(`make docs-sync-mobkit`)"
        )
    docs_path = manifest.get("source_path", DEFAULT_DOCS_PATH)
    if not isinstance(docs_path, str) or not docs_path or docs_path.startswith("/") or ".." in docs_path.split("/"):
        raise LagCheckError(f"{manifest_path} has an invalid source_path {docs_path!r}")
    return source_commit, docs_path


def docs_lag(repo: Path, source_commit: str, docs_path: str) -> DocsLag:
    head = git(repo, "rev-parse", "--verify", "HEAD^{commit}")
    if head.returncode != 0:
        raise LagCheckError(f"{repo} has no HEAD commit")
    resolved = git(repo, "rev-parse", "--verify", f"{source_commit}^{{commit}}")
    if resolved.returncode != 0:
        raise LagCheckError(
            f"mirrored commit {source_commit[:12]} is not in this history (a shallow checkout, or a commit "
            "main does not contain)"
        )
    if git(repo, "merge-base", "--is-ancestor", source_commit, "HEAD").returncode != 0:
        raise LagCheckError(
            f"mirrored commit {source_commit[:12]} is not an ancestor of HEAD; the mirror was generated "
            "from something other than main"
        )
    ahead = git(repo, "rev-list", "--count", f"{source_commit}..HEAD")
    changed = git(repo, "rev-list", "--abbrev=12", "--abbrev-commit", f"{source_commit}..HEAD", "--", docs_path)
    if ahead.returncode != 0 or changed.returncode != 0:
        raise LagCheckError(f"git rev-list failed: {(ahead.stderr or changed.stderr).strip()}")
    return DocsLag(
        head_sha=head.stdout.strip(),
        ahead_by=int(ahead.stdout.strip()),
        docs_commits=tuple(line for line in changed.stdout.split() if line),
    )


def main() -> int:
    args = parse_args()
    try:
        source_commit, docs_path = manifest_source(args.manifest)
        lag = docs_lag(args.repo, source_commit, docs_path)
    except LagCheckError as error:
        print(f"mobkit-docs-lag: cannot determine lag: {error}")
        return 2
    unmirrored = len(lag.docs_commits)
    print(
        f"mobkit-docs-lag: mirror renders {docs_path} at {source_commit[:12]}; HEAD is "
        f"{lag.head_sha[:12]}; {lag.ahead_by} commit(s) ahead, {unmirrored} changing {docs_path}, "
        f"tolerated {args.max_lag}"
    )
    if unmirrored > args.max_lag:
        print(
            f"mobkit-docs-lag: docs/mobkit misses {unmirrored} commit(s) changing {docs_path}: "
            f"{', '.join(lag.docs_commits)}. Run the Publish MobKit docs workflow (workflow_dispatch) "
            "or `make docs-sync-mobkit` on a clean main."
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
