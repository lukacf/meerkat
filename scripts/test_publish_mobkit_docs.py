#!/usr/bin/env python3
"""Contract tests for release-driven MobKit documentation publication."""

from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "publish-mobkit-docs.yml"


class PublishMobKitDocsWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_accepts_release_event_and_manual_recovery(self) -> None:
        self.assertIn("repository_dispatch:", self.workflow)
        self.assertIn("types: [mobkit-release-published]", self.workflow)
        self.assertIn("workflow_dispatch:", self.workflow)
        self.assertIn("release_tag:", self.workflow)

    def test_rejects_stale_or_unpublished_release(self) -> None:
        self.assertIn("releases/latest", self.workflow)
        self.assertIn("latest public release", self.workflow)
        self.assertIn("verify-published-registries.py --version", self.workflow)
        self.assertIn("EXPECTED_SHA", self.workflow)
        self.assertIn("EXPECTED_VERSION", self.workflow)

    def test_generates_from_clean_immutable_source(self) -> None:
        self.assertIn("scripts/sync-mobkit-docs.py _mobkit-release", self.workflow)
        self.assertIn('--source-ref "$MOBKIT_TAG"', self.workflow)
        self.assertIn("--require-clean", self.workflow)

    def test_validates_then_publishes_only_generated_paths(self) -> None:
        docs_check = self.workflow.index("run: make docs-check")
        mint_check = self.workflow.index("mint@4.2.728 broken-links")
        publish = self.workflow.index("git add docs/mobkit docs/docs.json")

        self.assertLess(docs_check, publish)
        self.assertLess(mint_check, publish)
        self.assertIn("grep -Ev '^docs/(mobkit/|docs\\.json$)'", self.workflow[publish:])
        self.assertIn("pull-requests: write", self.workflow)
        self.assertIn("actions: write", self.workflow)
        self.assertIn('git push origin "HEAD:refs/heads/${pr_branch}"', self.workflow[publish:])
        self.assertIn("gh pr create", self.workflow[publish:])
        self.assertIn('gh workflow run ci.yml --ref "${pr_branch}"', self.workflow[publish:])
        self.assertIn('gh pr merge "${pr_url}" --auto --squash', self.workflow[publish:])
        self.assertNotIn("git push origin HEAD:main", self.workflow[publish:])

    def test_pull_request_step_prefers_a_dedicated_token_over_the_workflow_token(self) -> None:
        push = self.workflow.index('git push origin "HEAD:refs/heads/${pr_branch}"')
        pull_request = self.workflow.index("GH_TOKEN: ${{ secrets.MOBKIT_DOCS_PR_TOKEN || github.token }}")
        create = self.workflow.index("gh pr create", push)
        self.assertLess(push, pull_request)
        self.assertLess(pull_request, create)
        # The dedicated token is scoped to pull requests; CI dispatch keeps the
        # workflow token, which is the one holding actions: write.
        self.assertIn(
            'GH_TOKEN="${ACTIONS_TOKEN}" gh workflow run ci.yml --ref "${pr_branch}"',
            self.workflow[pull_request:],
        )
        self.assertIn("ACTIONS_TOKEN: ${{ github.token }}", self.workflow[pull_request:create])
        # Enabling auto-merge needs contents: write, which the dedicated
        # pull-request token is not required to hold; it runs under the
        # workflow token too, so the documented token spec (Pull requests:
        # read and write) is sufficient for what the token is asked to do.
        self.assertIn(
            'GH_TOKEN="${ACTIONS_TOKEN}" gh pr merge "${pr_url}" --auto --squash',
            self.workflow[pull_request:],
        )
        # The token spec sits in the step's env comment, between the push and
        # the create command.
        self.assertIn("Pull requests: read and write", self.workflow[push:create])

    def test_pull_request_is_skipped_when_nothing_was_pushed(self) -> None:
        stage = self.workflow.index("id: stage")
        self.assertIn('echo "pushed_branch=${pr_branch}" >> "${GITHUB_OUTPUT}"', self.workflow[stage:])
        self.assertLess(
            self.workflow.index('git push origin "HEAD:refs/heads/${pr_branch}"'),
            self.workflow.index('echo "pushed_branch=${pr_branch}"'),
        )
        self.assertIn("if: steps.stage.outputs.pushed_branch != ''", self.workflow[stage:])

    def test_failed_publication_is_reported_after_the_branch_is_pushed(self) -> None:
        merge = self.workflow.index('gh pr merge "${pr_url}" --auto --squash')
        handler = self.workflow.index("if: failure() && steps.stage.outputs.pushed_branch != ''")
        self.assertLess(merge, handler)
        report = self.workflow[handler:]
        self.assertIn("python3 scripts/report-mobkit-docs-publication-failure.py", report)
        self.assertIn('--branch "${PR_BRANCH}"', report)
        self.assertIn('--pull-request-url "${PR_URL}"', report)
        self.assertIn('--summary "${GITHUB_STEP_SUMMARY}"', report)
        self.assertIn("PR_URL: ${{ steps.pull_request.outputs.pr_url }}", report)
        self.assertIn('echo "pr_url=${pr_url}" >> "${GITHUB_OUTPUT}"', self.workflow[:merge])
        self.assertIn("issues: write", self.workflow[: self.workflow.index("jobs:")])

    def test_workflow_structure_when_yaml_is_available(self) -> None:
        try:
            import yaml
        except ImportError:  # pragma: no cover - PyYAML is optional on hosted runners
            self.skipTest("PyYAML is not installed")
        parsed = yaml.safe_load(self.workflow)
        steps = {step.get("id", step["name"]): step for step in parsed["jobs"]["publish"]["steps"]}
        self.assertEqual(parsed["permissions"]["issues"], "write")
        self.assertEqual(steps["pull_request"]["if"], "steps.stage.outputs.pushed_branch != ''")
        self.assertEqual(
            steps["pull_request"]["env"]["GH_TOKEN"],
            "${{ secrets.MOBKIT_DOCS_PR_TOKEN || github.token }}",
        )
        handler = steps["Report a publication that needs a human"]
        self.assertEqual(handler["if"], "failure() && steps.stage.outputs.pushed_branch != ''")
        self.assertEqual(handler["env"]["GH_TOKEN"], "${{ github.token }}")
        names = [step["name"] for step in parsed["jobs"]["publish"]["steps"]]
        self.assertEqual(names[-3:], [
            "Stage the generated snapshot on a publication branch",
            "Publish generated snapshot through a protected pull request",
            "Report a publication that needs a human",
        ])


if __name__ == "__main__":
    unittest.main()
