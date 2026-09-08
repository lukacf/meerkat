#!/usr/bin/env python3
"""Restore only a digest-verified unit archive from successful exact-commit CI."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile


def candidate_runs(rows, repository, commit, current_run):
    return [
        row["id"]
        for row in rows
        if type(row.get("id")) is int
        and 0 < row["id"] < current_run
        and row.get("head_sha") == commit
        and row.get("event") == "push"
        and row.get("status") == "completed"
        and row.get("conclusion") == "success"
        and row.get("path") == ".github/workflows/ci.yml"
        and (row.get("head_repository") or {}).get("full_name") == repository
    ]


def select_artifact(rows, commit, run_id):
    matches = [
        row
        for row in rows
        if row.get("name") == f"nextest-unit-mob-{commit}"
        and row.get("expired") is False
        and type(row.get("id")) is int
        and row["id"] > 0
        and re.fullmatch(r"sha256:[0-9a-f]{64}", row.get("digest") or "")
        and (row.get("workflow_run") or {}).get("id") == run_id
        and (row.get("workflow_run") or {}).get("head_sha") == commit
    ]
    return matches[0] if len(matches) == 1 else None


def extract_verified_archive(download, digest, destination):
    with download.open("rb") as source:
        actual = hashlib.file_digest(source, "sha256").hexdigest()
    if f"sha256:{actual}" != digest:
        raise ValueError("CI artifact ZIP digest does not match GitHub metadata")
    with zipfile.ZipFile(download) as archive:
        if archive.namelist() != ["nextest-unit-mob.tar.zst"]:
            raise ValueError("CI artifact must contain only the expected unit archive")
        with tempfile.TemporaryDirectory(dir=destination.parent) as directory:
            temporary = Path(directory) / "archive.tar.zst"
            with archive.open("nextest-unit-mob.tar.zst") as source:
                with temporary.open("wb") as target:
                    shutil.copyfileobj(source, target)
                    if target.tell() == 0:
                        raise ValueError("CI unit archive is empty")
            os.replace(temporary, destination)


def github_json(endpoint):
    result = subprocess.run(
        ["gh", "api", endpoint],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def restore(repository, commit, current_run, destination):
    runs = github_json(
        f"repos/{repository}/actions/workflows/ci.yml/runs"
        f"?event=push&status=success&head_sha={commit}&per_page=20"
    )
    for run_id in candidate_runs(
        runs["workflow_runs"], repository, commit, current_run
    ):
        artifacts = github_json(
            f"repos/{repository}/actions/runs/{run_id}/artifacts?per_page=100"
        )
        artifact = select_artifact(artifacts["artifacts"], commit, run_id)
        if artifact is None:
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=destination.parent) as directory:
            download = Path(directory) / "artifact.zip"
            with download.open("wb") as output:
                subprocess.run(
                    [
                        "gh",
                        "api",
                        f"repos/{repository}/actions/artifacts/{artifact['id']}/zip",
                    ],
                    stdout=output,
                    check=True,
                )
            extract_verified_archive(download, artifact["digest"], destination)
        print(f"Reusing exact-commit CI archive from run {run_id}", file=sys.stderr)
        print("reused=true")
        print(f"source_run_id={run_id}")
        print(f"source_artifact_id={artifact['id']}")
        return
    print("No successful exact-commit CI archive; building from source", file=sys.stderr)
    print("reused=false")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--current-run", required=True, type=int)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository):
        parser.error("invalid repository")
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit) or args.current_run <= 0:
        parser.error("invalid commit or run identity")
    restore(args.repository, args.commit, args.current_run, args.output)


if __name__ == "__main__":
    main()
