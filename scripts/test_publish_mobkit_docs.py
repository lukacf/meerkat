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
        publish = self.workflow.index("git add docs/mobkit")

        self.assertLess(docs_check, publish)
        self.assertLess(mint_check, publish)
        self.assertIn("grep -Ev '^docs/mobkit/'", self.workflow[publish:])
        self.assertIn("git push origin HEAD:main", self.workflow[publish:])


if __name__ == "__main__":
    unittest.main()
