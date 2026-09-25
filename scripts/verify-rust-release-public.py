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


def wait_until_public(
    crates: list[str], version: str, deadline_seconds: int
) -> dict[str, dict[str, str]]:
    """Poll crates.io until every crate is public; return per-crate observations."""
    deadline = time.monotonic() + deadline_seconds
    pending = set(crates)
    observations: dict[str, dict[str, str]] = {}
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
            observations[crate] = {
                "published_at": parse_instant(created_at).isoformat(),
                "observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            }
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
    return observations


def load_observations(
    path: pathlib.Path, version: str, crates: list[str]
) -> dict[str, dict[str, str]]:
    recorded = json.loads(path.read_text())
    if recorded.get("version") != version:
        raise SystemExit(
            f"observations in {path} are for {recorded.get('version')}, expected {version}"
        )
    observations = recorded.get("crates", {})
    missing = sorted(set(crates) - set(observations))
    if missing:
        raise SystemExit(f"observations in {path} lack crate(s): {', '.join(missing)}")
    return observations


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=pathlib.Path, default=pathlib.Path.cwd())
    parser.add_argument("--deadline-seconds", type=int, default=900)
    parser.add_argument(
        "--readback-only",
        action="store_true",
        help="verify every crate is public and stop; no SLO evaluation",
    )
    parser.add_argument(
        "--observations-out",
        type=pathlib.Path,
        help="write the per-crate publication observations to this JSON file",
    )
    parser.add_argument(
        "--observations-in",
        type=pathlib.Path,
        help="evaluate the SLO on observations recorded by an earlier readback",
    )
    parser.add_argument(
        "--window-started-at",
        help="start of the SLO window (the moment crate publication began)",
    )
    parser.add_argument(
        "--tag-pushed-at",
        help="tag push time; reported for information, and the window start "
        "when --window-started-at is not given",
    )
    parser.add_argument("--slo-seconds", type=int, default=1800)
    args = parser.parse_args()

    root = args.repo_root.resolve()
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    version = workspace["workspace"]["package"]["version"]
    crates = release_crates(root)

    if args.observations_in:
        observations = load_observations(args.observations_in, version, crates)
    else:
        observations = wait_until_public(crates, version, args.deadline_seconds)
    if args.observations_out:
        args.observations_out.write_text(
            json.dumps({"version": version, "crates": observations}, indent=2) + "\n"
        )

    if args.readback_only:
        print(f"all {len(crates)} crates public, checksummed and not yanked at {version}")
        return 0

    window_start_raw = args.window_started_at or args.tag_pushed_at
    if not window_start_raw:
        raise SystemExit("the SLO needs --window-started-at or --tag-pushed-at")
    window_start = parse_instant(window_start_raw)
    final_publication = max(parse_instant(o["published_at"]) for o in observations.values())
    final_observation = max(parse_instant(o["observed_at"]) for o in observations.values())
    registry_elapsed = max(0, int((final_publication - window_start).total_seconds()))
    observed_elapsed = max(0, int((final_observation - window_start).total_seconds()))
    window_label = "publication start" if args.window_started_at else "tag push"
    print(
        f"all {len(crates)} crates observed public {observed_elapsed}/{args.slo_seconds} "
        f"seconds after {window_label} (registry-created component: {registry_elapsed}s)"
    )
    if args.window_started_at and args.tag_pushed_at:
        tag_elapsed = max(
            0, int((final_observation - parse_instant(args.tag_pushed_at)).total_seconds())
        )
        print(f"tag-to-public latency (information): {tag_elapsed}s")
    if observed_elapsed > args.slo_seconds:
        raise SystemExit(
            f"crates.io publication SLO exceeded: {observed_elapsed}s > "
            f"{args.slo_seconds}s after {window_label}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
