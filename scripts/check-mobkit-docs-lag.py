#!/usr/bin/env python3
"""Fail when the docs.rkat.ai MobKit mirror lags the public MobKit releases.

docs/mobkit is a generated snapshot of one MobKit release, recorded in
docs/mobkit/_source.json as `source_ref`. Publication of a newer release can
fail after the release itself succeeded (the publish workflow could not open
its pull request for ten days in 2026-08/09), and nothing else looked at the
gap. This ratchet compares the mirrored ref with the published, non-draft,
non-prerelease MobKit releases and fails when more than `--max-lag` releases
were published after the mirrored one.

Usage:
    gh api "repos/lukacf/meerkat-mobkit/releases?per_page=100" > releases.json
    python3 scripts/check-mobkit-docs-lag.py --releases releases.json
"""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "docs" / "mobkit" / "_source.json"


@dataclass(frozen=True, order=True)
class Release:
    published_at: datetime
    tag_name: str


class LagCheckError(Exception):
    """The lag could not be computed; this is not a zero lag."""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--releases",
        required=True,
        type=Path,
        help="JSON array as returned by `gh api repos/<owner>/<repo>/releases`",
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
        default=1,
        help="largest number of newer published releases tolerated (default: 1)",
    )
    return parser.parse_args()


def published_releases(releases: object) -> list[Release]:
    """Published, non-draft, non-prerelease releases, newest first.

    The releases API lists drafts (with a null `published_at`) and does not
    order strictly by publication date, so the list order is not the lag.
    """
    if not isinstance(releases, list):
        raise LagCheckError("releases payload is not a JSON array")
    published: list[Release] = []
    for entry in releases:
        if not isinstance(entry, dict):
            raise LagCheckError("releases payload contains a non-object entry")
        if entry.get("draft") is True or entry.get("prerelease") is True:
            continue
        tag_name = entry.get("tag_name")
        published_at = entry.get("published_at")
        if not isinstance(tag_name, str) or not isinstance(published_at, str):
            continue
        published.append(
            Release(
                published_at=datetime.fromisoformat(published_at.replace("Z", "+00:00")),
                tag_name=tag_name,
            )
        )
    if not published:
        raise LagCheckError("releases payload contains no published releases")
    return sorted(published, reverse=True)


def mirror_lag(source_ref: str, releases: list[Release]) -> int:
    """Number of published releases newer than the mirrored release."""
    for index, release in enumerate(releases):
        if release.tag_name == source_ref:
            return index
    raise LagCheckError(
        f"mirrored MobKit ref {source_ref} is not among the published releases "
        f"(latest is {releases[0].tag_name})"
    )


def manifest_source_ref(manifest_path: Path) -> str:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    source_ref = manifest.get("source_ref") if isinstance(manifest, dict) else None
    if not isinstance(source_ref, str) or not source_ref:
        raise LagCheckError(f"{manifest_path} has no source_ref")
    return source_ref


def main() -> int:
    args = parse_args()
    try:
        source_ref = manifest_source_ref(args.manifest)
        releases = published_releases(json.loads(args.releases.read_text(encoding="utf-8")))
        lag = mirror_lag(source_ref, releases)
    except LagCheckError as error:
        print(f"mobkit-docs-lag: cannot determine lag: {error}")
        return 2
    latest = releases[0]
    print(
        f"mobkit-docs-lag: mirror documents {source_ref}; latest published release is "
        f"{latest.tag_name} ({latest.published_at.isoformat()}); lag {lag} release(s), "
        f"tolerated {args.max_lag}"
    )
    if lag > args.max_lag:
        newer = ", ".join(release.tag_name for release in releases[:lag])
        print(
            f"mobkit-docs-lag: docs/mobkit is {lag} releases behind; published after "
            f"{source_ref}: {newer}. Run the Publish MobKit docs workflow for "
            f"{latest.tag_name} (or `make docs-sync-mobkit` from its clean tag)."
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
