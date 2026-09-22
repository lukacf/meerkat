#!/usr/bin/env python3
"""Contract tests for main-tracking MobKit documentation publication."""

from __future__ import annotations

import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "publish-mobkit-docs.yml"


class PublishMobKitDocsWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")

    def test_mirrors_main_on_docs_push_manual_dispatch_and_nightly_catch_up(self) -> None:
        self.assertIn("repository_dispatch:", self.workflow)
        self.assertIn("types: [mobkit-docs-updated]", self.workflow)
        self.assertIn("workflow_dispatch:", self.workflow)
        self.assertIn("source_sha:", self.workflow)
        self.assertIn("schedule:", self.workflow)
        self.assertIn("cron:", self.workflow)
        # Releases no longer publish documentation.
        self.assertNotIn("mobkit-release-published", self.workflow)
        self.assertNotIn("release_tag", self.workflow)
        self.assertNotIn("releases/latest", self.workflow)
        self.assertNotIn("verify-published-registries.py", self.workflow)

    def test_only_commits_on_mobkit_main_can_be_mirrored(self) -> None:
        self.assertIn("repository: lukacf/meerkat-mobkit", self.workflow)
        self.assertIn("ref: main", self.workflow[self.workflow.index("repository: lukacf/meerkat-mobkit"):])
        verify = self.workflow.index("Resolve and verify the source commit")
        generate = self.workflow.index("scripts/sync-mobkit-docs.py _mobkit")
        self.assertLess(verify, generate)
        self.assertIn('if [[ "$REQUESTED_REF" != "main" ]]', self.workflow[verify:generate])
        self.assertIn(
            'git -C _mobkit merge-base --is-ancestor "$source_sha" origin/main',
            self.workflow[verify:generate],
        )
        self.assertIn('git -C _mobkit checkout --quiet --detach "$source_sha"', self.workflow[verify:generate])

    def test_generates_from_clean_main_source(self) -> None:
        self.assertIn("scripts/sync-mobkit-docs.py _mobkit", self.workflow)
        self.assertIn("--source-ref main", self.workflow)
        self.assertIn("--require-clean", self.workflow)

    def test_validates_then_stages_only_generated_paths(self) -> None:
        docs_check = self.workflow.index("run: make docs-check")
        mint_check = self.workflow.index("mint@4.2.728 broken-links")
        stage = self.workflow.index("git add docs/mobkit docs/docs.json")
        self.assertLess(docs_check, stage)
        self.assertLess(mint_check, stage)
        self.assertIn("grep -Ev '^docs/(mobkit/|docs\\.json$)'", self.workflow[stage:])
        self.assertIn('git commit -m "docs: mirror MobKit main ${SOURCE_SHORT}"', self.workflow[stage:])
        self.assertIn('echo "changed=true" >> "${GITHUB_OUTPUT}"', self.workflow[stage:])

    def test_direct_publication_needs_the_admin_token_and_follows_the_path_guard(self) -> None:
        stage = self.workflow.index("git add docs/mobkit docs/docs.json")
        direct = self.workflow.index("id: direct")
        self.assertLess(stage, direct)
        self.assertIn(
            "if: steps.stage.outputs.changed == 'true' && env.MOBKIT_DOCS_PR_TOKEN != ''",
            self.workflow[direct:],
        )
        self.assertIn("MOBKIT_DOCS_PR_TOKEN: ${{ secrets.MOBKIT_DOCS_PR_TOKEN }}", self.workflow[direct:])
        self.assertIn("x-access-token:${MOBKIT_DOCS_PR_TOKEN}@github.com/${GITHUB_REPOSITORY}.git\" HEAD:main", self.workflow[direct:])
        # The direct push must never run under the workflow token, which the
        # ruleset does not exempt.
        self.assertNotIn("git push origin HEAD:main", self.workflow)

    def test_pull_request_path_approves_its_own_ci_run_and_auto_merges_with_a_merge_commit(self) -> None:
        branch = self.workflow.index("id: branch")
        pull_request = self.workflow.index("id: pull_request")
        self.assertLess(branch, pull_request)
        self.assertIn(
            "if: steps.stage.outputs.changed == 'true' && env.MOBKIT_DOCS_PR_TOKEN == ''",
            self.workflow[branch:pull_request],
        )
        self.assertIn('git push origin "HEAD:refs/heads/${pr_branch}"', self.workflow[branch:pull_request])
        self.assertIn("if: steps.branch.outputs.pushed_branch != ''", self.workflow[pull_request:])
        self.assertIn("GH_TOKEN: ${{ github.token }}", self.workflow[pull_request:])
        self.assertIn("gh pr create", self.workflow[pull_request:])
        self.assertIn("event=pull_request", self.workflow[pull_request:])
        # GitHub reports a run awaiting approval as status "completed" with
        # conclusion "action_required"; the check must read the conclusion,
        # not only the status, and must confirm the approval took.
        self.assertIn('(.status // "") + "/" + (.conclusion // "")', self.workflow[pull_request:])
        self.assertIn('if [[ "$run_state" == *action_required* ]]', self.workflow[pull_request:])
        self.assertIn("still awaits approval after the approve call", self.workflow[pull_request:])
        self.assertIn(
            'gh api --method POST "repos/${GITHUB_REPOSITORY}/actions/runs/${run_id}/approve"',
            self.workflow[pull_request:],
        )
        self.assertIn('gh pr merge "${pr_url}" --auto --merge', self.workflow[pull_request:])
        self.assertNotIn("--auto --squash", self.workflow)
        self.assertIn("actions: write", self.workflow)
        self.assertIn("pull-requests: write", self.workflow)

    def test_failed_publication_is_reported_with_the_mirrored_source(self) -> None:
        merge = self.workflow.index('gh pr merge "${pr_url}" --auto --merge')
        handler = self.workflow.index("if: failure() && steps.stage.outputs.changed == 'true'")
        self.assertLess(merge, handler)
        report = self.workflow[handler:]
        self.assertIn("python3 scripts/report-mobkit-docs-publication-failure.py", report)
        self.assertIn('--source "main@${SOURCE_SHORT}"', report)
        self.assertIn('--branch "${PR_BRANCH:-unpushed}"', report)
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
        triggers = parsed[True]
        self.assertEqual(triggers["repository_dispatch"]["types"], ["mobkit-docs-updated"])
        self.assertIn("source_sha", triggers["workflow_dispatch"]["inputs"])
        self.assertFalse(triggers["workflow_dispatch"]["inputs"]["source_sha"]["required"])
        self.assertEqual(len(triggers["schedule"]), 1)
        steps = {step.get("id", step["name"]): step for step in parsed["jobs"]["publish"]["steps"]}
        self.assertEqual(parsed["permissions"]["issues"], "write")
        self.assertEqual(steps["direct"]["if"], "steps.stage.outputs.changed == 'true' && env.MOBKIT_DOCS_PR_TOKEN != ''")
        self.assertEqual(steps["branch"]["if"], "steps.stage.outputs.changed == 'true' && env.MOBKIT_DOCS_PR_TOKEN == ''")
        self.assertEqual(steps["pull_request"]["if"], "steps.branch.outputs.pushed_branch != ''")
        self.assertEqual(steps["pull_request"]["env"]["GH_TOKEN"], "${{ github.token }}")
        handler = steps["Report a publication that needs a human"]
        self.assertEqual(handler["if"], "failure() && steps.stage.outputs.changed == 'true'")
        names = [step["name"] for step in parsed["jobs"]["publish"]["steps"]]
        self.assertEqual(names[-5:], [
            "Stage the generated snapshot",
            "Publish directly to main with the admin token",
            "Push a publication branch",
            "Publish through a protected pull request",
            "Report a publication that needs a human",
        ])


if __name__ == "__main__":
    unittest.main()
