#!/usr/bin/env python3
"""Wait until every canonical Meerkat release crate is public and non-yanked."""

from __future__ import annotations

import argparse
import datetime
import json
import pathlib
import subprocess
import time
import urllib.error
import urllib.request

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 local fallback
    import tomli as tomllib


USER_AGENT = "meerkat-release-verifier (https://github.com/lukacf/meerkat)"


def release_crates(root: pathlib.Path) -> list[str]:
    output = subprocess.check_output(
        [str(root / "scripts" / "release-rust-crates.sh")], text=True
    )
    return [line.strip() for line in output.splitlines() if line.strip()]


def fetch_version(crate: str, version: str) -> dict | None:
    url = f"https://crates.io/api/v1/crates/{crate}/{version}"
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response)["version"]
    except urllib.error.HTTPError as error:
        if error.code in (404, 429, 500, 502, 503, 504):
            return None
        raise
    except (TimeoutError, urllib.error.URLError):
        return None


def parse_instant(value: str) -> datetime.datetime:
    return datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=pathlib.Path, default=pathlib.Path.cwd())
    parser.add_argument("--deadline-seconds", type=int, default=900)
    parser.add_argument("--tag-pushed-at", required=True)
    parser.add_argument("--slo-seconds", type=int, default=1800)
    args = parser.parse_args()

    root = args.repo_root.resolve()
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    version = workspace["workspace"]["package"]["version"]
    crates = release_crates(root)
    deadline = time.monotonic() + args.deadline_seconds
    pending = set(crates)
    published_at: dict[str, datetime.datetime] = {}
    observed_at: dict[str, datetime.datetime] = {}

    while pending:
        for crate in sorted(pending):
            published = fetch_version(crate, version)
            if not published:
                continue
            if published.get("yanked"):
                raise SystemExit(f"{crate} {version} is public but yanked")
            if not published.get("checksum"):
                raise SystemExit(f"{crate} {version} has no registry checksum")
            created_at = published.get("created_at")
            if not created_at:
                raise SystemExit(f"{crate} {version} has no registry creation timestamp")
            published_at[crate] = parse_instant(created_at)
            observed_at[crate] = datetime.datetime.now(datetime.timezone.utc)
            pending.remove(crate)
            print(f"public {crate} {version} {published['checksum']}", flush=True)
        if not pending:
            break
        if time.monotonic() >= deadline:
            raise SystemExit(
                f"timed out waiting for {len(pending)} crate(s): {', '.join(sorted(pending))}"
            )
        print(f"waiting for {len(pending)} crate(s)", flush=True)
        time.sleep(10)

    tag_pushed_at = parse_instant(args.tag_pushed_at)
    final_publication = max(published_at.values())
    final_observation = max(observed_at.values())
    registry_elapsed = max(
        0, int((final_publication - tag_pushed_at).total_seconds())
    )
    observed_elapsed = max(
        0,
        int((final_observation - tag_pushed_at).total_seconds()),
    )
    print(
        f"all {len(crates)} crates observed public {observed_elapsed}/{args.slo_seconds} "
        f"seconds after tag push (registry-created component: {registry_elapsed}s)"
    )
    if observed_elapsed > args.slo_seconds:
        raise SystemExit(
            f"tag-to-crates.io SLO exceeded: {observed_elapsed}s > "
            f"{args.slo_seconds}s"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
