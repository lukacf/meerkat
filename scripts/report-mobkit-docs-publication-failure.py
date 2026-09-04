#!/usr/bin/env python3
"""Make a failed MobKit docs publication actionable instead of silent.

publish-mobkit-docs.yml pushes the generated snapshot to a publication branch
and then opens a protected pull request with `gh pr create`. When the
repository forbids Actions from opening pull requests, or the dedicated token
is missing or expired, that step fails after the branch is already pushed: the
run turns red without saying what a human has to do, and the next release
repeats the same red run. Every publication run between 2026-08-24 and
2026-09-03 that reached that step died there (two 2026-08-29 runs failed
earlier, at `make docs-check`), and docs.rkat.ai served MobKit 0.8.22 for those
ten days while 0.8.23 through 0.8.30 shipped.

This script runs from the workflow's failure handler. It writes the pushed
branch and the exact recovery commands to the job summary and opens or updates
one tracking issue. The issue title is stable, so repeated failures append a
comment to the open issue instead of creating a new one per run.

Usage:
    python3 scripts/report-mobkit-docs-publication-failure.py \\
        --repository lukacf/meerkat --tag v0.8.30 \\
        --branch codex/publish-mobkit-docs-0.8.30-123 \\
        --run-url https://github.com/lukacf/meerkat/actions/runs/123 \\
        --summary "$GITHUB_STEP_SUMMARY" [--pull-request-url URL]
"""

from __future__ import annotations

import argparse
import json
import subprocess
from dataclasses import dataclass
from pathlib import Path


TRACKING_ISSUE_TITLE = "MobKit docs publication needs a human to open the pull request"


@dataclass(frozen=True)
class PublicationFailure:
    repository: str
    tag: str
    branch: str
    run_url: str
    pull_request_url: str | None

    def pull_request_title(self) -> str:
        return f"docs: publish MobKit {self.tag}"

    def recovery_commands(self) -> list[str]:
        """Commands a maintainer runs from any checkout to finish the publication."""
        if self.pull_request_url:
            return [
                f"gh pr merge --repo {self.repository} --auto --squash {self.pull_request_url}",
            ]
        return [
            (
                f"gh pr create --repo {self.repository} --base main --head {self.branch} "
                f'--title "{self.pull_request_title()}" '
                f'--body "Publish the immutable {self.tag} documentation snapshot '
                f'pushed by {self.run_url}."'
            ),
            f"gh pr merge --repo {self.repository} --auto --squash {self.branch}",
        ]

    def what_failed(self) -> str:
        if self.pull_request_url:
            return (
                f"The pull request {self.pull_request_url} was opened, but the CI "
                "dispatch or auto-merge that follows it failed."
            )
        return (
            f"The snapshot branch `{self.branch}` was pushed, but the workflow could "
            "not open the pull request."
        )

    def markdown(self, heading_level: int) -> str:
        heading = "#" * heading_level
        commands = "\n".join(self.recovery_commands())
        return (
            f"{heading} MobKit {self.tag} docs publication needs a human\n\n"
            f"{self.what_failed()}\n\n"
            f"- MobKit release: `{self.tag}`\n"
            f"- Pushed branch: `{self.branch}`\n"
            f"- Failed run: {self.run_url}\n\n"
            "Finish the publication by hand:\n\n"
            f"```bash\n{commands}\n```\n\n"
            "Stop this from repeating on the next release by doing one of:\n\n"
            '1. Enable "Allow GitHub Actions to create and approve pull requests" '
            f"under Settings > Actions > General for `{self.repository}`.\n"
            "2. Add a `MOBKIT_DOCS_PR_TOKEN` repository secret holding a fine-grained "
            f"token with Pull requests: read and write on `{self.repository}` "
            "(Metadata: read is implied; nothing else). `publish-mobkit-docs.yml` uses "
            "it only to open the pull request, prefers it over `github.token` when "
            "present, and enables auto-merge with its own token.\n"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, help="owner/name of this repository")
    parser.add_argument("--tag", required=True, help="MobKit release tag being published")
    parser.add_argument("--branch", required=True, help="publication branch that was pushed")
    parser.add_argument("--run-url", required=True, help="URL of the failed workflow run")
    parser.add_argument(
        "--summary",
        required=True,
        type=Path,
        help="path of the job summary file to append to (GITHUB_STEP_SUMMARY)",
    )
    parser.add_argument(
        "--pull-request-url",
        default="",
        help="pull request URL when creation succeeded but a later step failed",
    )
    return parser.parse_args()


def gh(*args: str, stdin: str | None = None) -> str:
    result = subprocess.run(
        ["gh", *args],
        input=stdin,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        command = " ".join(["gh", *args])
        raise SystemExit(
            f"`{command}` failed with exit status {result.returncode}: {result.stderr.strip()}"
        )
    return result.stdout.strip()


def find_tracking_issue(repository: str, title: str) -> int | None:
    """Return the number of the open issue whose title is exactly `title`."""
    listing = gh(
        "issue",
        "list",
        "--repo",
        repository,
        "--state",
        "open",
        "--search",
        f'"{title}" in:title',
        "--limit",
        "50",
        "--json",
        "number,title",
    )
    issues = json.loads(listing or "[]")
    matches = sorted(
        int(issue["number"])
        for issue in issues
        if isinstance(issue, dict) and issue.get("title") == title
    )
    return matches[0] if matches else None


def upsert_tracking_issue(failure: PublicationFailure) -> str:
    body = failure.markdown(heading_level=2)
    number = find_tracking_issue(failure.repository, TRACKING_ISSUE_TITLE)
    if number is not None:
        gh(
            "issue",
            "comment",
            str(number),
            "--repo",
            failure.repository,
            "--body-file",
            "-",
            stdin=body,
        )
        return f"updated tracking issue #{number}"
    url = gh(
        "issue",
        "create",
        "--repo",
        failure.repository,
        "--title",
        TRACKING_ISSUE_TITLE,
        "--body-file",
        "-",
        stdin=body,
    )
    return f"opened tracking issue {url}"


def main() -> int:
    args = parse_args()
    failure = PublicationFailure(
        repository=args.repository,
        tag=args.tag,
        branch=args.branch,
        run_url=args.run_url,
        pull_request_url=args.pull_request_url or None,
    )
    with args.summary.open("a", encoding="utf-8") as summary:
        summary.write(failure.markdown(heading_level=2))
    print(upsert_tracking_issue(failure))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
