#!/usr/bin/env python3
"""Tests for the MobKit docs publication failure reporter.

The reporter shells out to `gh`; these tests put a recording fake `gh` first on
PATH so the issue upsert and the recovery text are exercised end to end.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("report-mobkit-docs-publication-failure.py")
SPEC = importlib.util.spec_from_file_location("report_mobkit_docs_publication_failure", SCRIPT)
assert SPEC and SPEC.loader
report = importlib.util.module_from_spec(SPEC)
# Dataclasses resolve postponed annotations through sys.modules; register
# the module before executing it like a normal import would.
sys.modules[SPEC.name] = report
SPEC.loader.exec_module(report)

REPOSITORY = "lukacf/meerkat"
TAG = "v0.8.30"
BRANCH = "codex/publish-mobkit-docs-0.8.30-777"
RUN_URL = "https://github.com/lukacf/meerkat/actions/runs/777"
PULL_REQUEST_URL = "https://github.com/lukacf/meerkat/pull/5"

FAKE_GH = """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$FAKE_GH_LOG"
case "$1 $2" in
  "issue list")
    if [[ "${FAKE_GH_FAIL_LIST:-}" == "1" ]]; then
      echo "HTTP 403: Resource not accessible by integration" >&2
      exit 1
    fi
    printf '%s' "$FAKE_GH_ISSUES"
    ;;
  "issue create")
    cat > "$FAKE_GH_BODY_DIR/create.md"
    echo "https://github.com/lukacf/meerkat/issues/99"
    ;;
  "issue comment")
    cat > "$FAKE_GH_BODY_DIR/comment.md"
    echo "https://github.com/lukacf/meerkat/issues/$3#issuecomment-1"
    ;;
  *)
    echo "unexpected gh invocation: $*" >&2
    exit 1
    ;;
esac
"""


class ReportMobKitDocsPublicationFailureTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.addCleanup(self._temp.cleanup)
        self.root = Path(self._temp.name)
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        fake_gh = bin_dir / "gh"
        fake_gh.write_text(FAKE_GH, encoding="utf-8")
        fake_gh.chmod(0o755)
        self.body_dir = self.root / "bodies"
        self.body_dir.mkdir()
        self.log = self.root / "gh.log"
        self.summary = self.root / "summary.md"
        self.env = {
            **os.environ,
            "PATH": f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}",
            "FAKE_GH_LOG": str(self.log),
            "FAKE_GH_BODY_DIR": str(self.body_dir),
            "FAKE_GH_ISSUES": "[]",
        }

    def run_report(
        self,
        *,
        issues: list[dict[str, object]] | None = None,
        pull_request_url: str = "",
        fail_list: bool = False,
    ) -> subprocess.CompletedProcess[str]:
        env = dict(self.env)
        env["FAKE_GH_ISSUES"] = json.dumps(issues or [])
        if fail_list:
            env["FAKE_GH_FAIL_LIST"] = "1"
        command = [
            sys.executable,
            str(SCRIPT),
            "--repository",
            REPOSITORY,
            "--tag",
            TAG,
            "--branch",
            BRANCH,
            "--run-url",
            RUN_URL,
            "--summary",
            str(self.summary),
            "--pull-request-url",
            pull_request_url,
        ]
        return subprocess.run(command, env=env, capture_output=True, text=True)

    def gh_calls(self) -> list[str]:
        if not self.log.exists():
            return []
        return self.log.read_text(encoding="utf-8").splitlines()

    def test_tracking_issue_title_is_stable_across_releases(self) -> None:
        self.assertNotIn(TAG, report.TRACKING_ISSUE_TITLE)
        self.assertNotIn("0.8", report.TRACKING_ISSUE_TITLE)

    def test_summary_names_the_pushed_branch_and_the_manual_pull_request_command(self) -> None:
        result = self.run_report()
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = self.summary.read_text(encoding="utf-8")
        self.assertIn(f"Pushed branch: `{BRANCH}`", summary)
        self.assertIn(
            f"gh pr create --repo {REPOSITORY} --base main --head {BRANCH} "
            f'--title "docs: publish MobKit {TAG}"',
            summary,
        )
        self.assertIn(f"gh pr merge --repo {REPOSITORY} --auto --squash {BRANCH}", summary)
        self.assertIn(RUN_URL, summary)
        self.assertIn("Allow GitHub Actions to create and approve pull requests", summary)
        self.assertIn("MOBKIT_DOCS_PR_TOKEN", summary)
        # The token spec must be the one that actually suffices: the workflow
        # uses the dedicated token for `gh pr create` only, so a pull-request
        # scoped token is enough; auto-merge (which needs contents: write) runs
        # under the workflow's own token.
        self.assertIn("Pull requests: read and write", summary)
        self.assertIn("only to open the pull request", summary)

    def test_opens_the_tracking_issue_when_none_is_open(self) -> None:
        result = self.run_report(issues=[])
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.gh_calls()
        self.assertEqual(len(calls), 2, calls)
        self.assertTrue(calls[0].startswith(f"issue list --repo {REPOSITORY} --state open"), calls[0])
        self.assertIn(f'"{report.TRACKING_ISSUE_TITLE}" in:title', calls[0])
        self.assertEqual(
            calls[1],
            f"issue create --repo {REPOSITORY} --title {report.TRACKING_ISSUE_TITLE} --body-file -",
        )
        body = (self.body_dir / "create.md").read_text(encoding="utf-8")
        self.assertIn(BRANCH, body)
        self.assertIn("gh pr create", body)
        self.assertIn("opened tracking issue https://github.com/lukacf/meerkat/issues/99", result.stdout)

    def test_updates_the_open_tracking_issue_instead_of_duplicating_it(self) -> None:
        issues = [
            {"number": 7, "title": f"{report.TRACKING_ISSUE_TITLE} (old copy)"},
            {"number": 42, "title": report.TRACKING_ISSUE_TITLE},
        ]
        result = self.run_report(issues=issues)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.gh_calls()
        self.assertEqual(len(calls), 2, calls)
        self.assertEqual(calls[1], f"issue comment 42 --repo {REPOSITORY} --body-file -")
        self.assertFalse(any(call.startswith("issue create") for call in calls), calls)
        body = (self.body_dir / "comment.md").read_text(encoding="utf-8")
        self.assertIn(BRANCH, body)
        self.assertIn("updated tracking issue #42", result.stdout)

    def test_existing_pull_request_recovery_is_a_merge_not_a_create(self) -> None:
        result = self.run_report(pull_request_url=PULL_REQUEST_URL)
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = self.summary.read_text(encoding="utf-8")
        self.assertIn(f"gh pr merge --repo {REPOSITORY} --auto --squash {PULL_REQUEST_URL}", summary)
        self.assertNotIn("gh pr create", summary)
        self.assertIn(f"The pull request {PULL_REQUEST_URL} was opened", summary)

    def test_summary_is_written_before_the_issue_api_is_needed(self) -> None:
        result = self.run_report(fail_list=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("gh issue list", result.stderr)
        self.assertIn("Resource not accessible by integration", result.stderr)
        summary = self.summary.read_text(encoding="utf-8")
        self.assertIn(f"Pushed branch: `{BRANCH}`", summary)


if __name__ == "__main__":
    unittest.main()
