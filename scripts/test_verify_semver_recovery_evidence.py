#!/usr/bin/env python3
"""Unit tests for exact semver measurement recovery evidence."""

from __future__ import annotations

import copy
import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify_semver_recovery_evidence.py")
SPEC = importlib.util.spec_from_file_location("verify_semver_recovery_evidence", SCRIPT)
assert SPEC and SPEC.loader
gate = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = gate
SPEC.loader.exec_module(gate)

RUN_ID = 33123992541
JOB_ID = 98697718256
TAG = "v0.8.30"
SHA = "5e229e0b8379ac162a6b3c69187d186b570535e9"
RUN = {
    "id": RUN_ID,
    "event": "push",
    "head_branch": TAG,
    "head_sha": SHA,
    "path": ".github/workflows/release.yml",
}
JOB = {
    "id": JOB_ID,
    "run_id": RUN_ID,
    "workflow_name": "Release",
    "name": "Breaks declared and notes stamped",
    "head_branch": TAG,
    "head_sha": SHA,
    "status": "completed",
    "conclusion": "failure",
    "steps": [
        {"name": "Set up job", "conclusion": "success"},
        {"name": "Checkout", "conclusion": "success"},
        {"name": "Install Rust", "conclusion": "success"},
        {"name": "Cache cargo", "conclusion": "success"},
        {"name": "Install cargo-semver-checks", "conclusion": "success"},
        {
            "name": "Verify every reported break is named and the notes are stamped",
            "conclusion": "failure",
        },
    ],
}
LOG = """\
2026-08-27T22:51:01.0000000Z semver-breaks: analyser self-test passed
2026-08-28T00:25:42.0000000Z semver-breaks: FAILED
2026-08-28T00:25:42.0000000Z   - [meerkat-mob] enum_variant_added: `variant MobError:LifecycleOperationProgressStalled` is not named under `### Breaking` (missing: `MobError`, `LifecycleOperationProgressStalled`)
2026-08-28T00:25:42.0000000Z   - [meerkat-mob] trait_method_added: `trait method meerkat_mob::MobSessionService::enqueue_committed_parent_session_boundary_after_runtime_turn` is not named under `### Breaking` (missing: `MobSessionService`, `enqueue_committed_parent_session_boundary_after_runtime_turn`)
2026-08-28T00:25:42.0000000Z
2026-08-28T00:25:42.0000000Z Policy (M3): 0.x patch releases may break public API, but every break must be
2026-08-28T00:25:42.0000000Z declared under `### Breaking` in the pending release section, naming the changed
2026-08-28T00:25:42.0000000Z signatures, so exact-pinned downstreams can plan the bump.
2026-08-28T00:25:42.0000000Z
2026-08-28T00:25:42.0000000Z cargo-semver-checks exited 100; last 40 report lines:
"""
CHANGELOG = """\
# Changelog

## [Unreleased]

## [0.8.30] - 2026-08-26

### Breaking

- `MobError::LifecycleOperationProgressStalled` was added and
  `MobSessionService::enqueue_committed_parent_session_boundary_after_runtime_turn` is required.
"""


class EvidenceTests(unittest.TestCase):
    def test_accepts_exact_completed_measurement_with_amended_declarations(self) -> None:
        gate.verify_metadata(
            RUN,
            JOB,
            evidence_run_id=RUN_ID,
            evidence_job_id=JOB_ID,
            release_tag=TAG,
            release_sha=SHA,
        )
        findings = gate.parse_missing_declarations(LOG)
        self.assertEqual(len(findings), 2)
        self.assertEqual(
            findings[0].symbols,
            ("MobError", "LifecycleOperationProgressStalled"),
        )

    def test_rejects_non_tag_dispatch_evidence(self) -> None:
        run = copy.deepcopy(RUN)
        run["event"] = "workflow_dispatch"
        with self.assertRaisesRegex(gate.EvidenceError, "run event"):
            gate.verify_metadata(
                run,
                JOB,
                evidence_run_id=RUN_ID,
                evidence_job_id=JOB_ID,
                release_tag=TAG,
                release_sha=SHA,
            )

    def test_rejects_wrong_release_sha(self) -> None:
        with self.assertRaisesRegex(gate.EvidenceError, "run SHA"):
            gate.verify_metadata(
                RUN,
                JOB,
                evidence_run_id=RUN_ID,
                evidence_job_id=JOB_ID,
                release_tag=TAG,
                release_sha="0" * 40,
            )

    def test_rejects_measurement_setup_failure(self) -> None:
        job = copy.deepcopy(JOB)
        job["steps"][4]["conclusion"] = "failure"
        with self.assertRaisesRegex(gate.EvidenceError, "Install cargo-semver-checks"):
            gate.verify_metadata(
                RUN,
                job,
                evidence_run_id=RUN_ID,
                evidence_job_id=JOB_ID,
                release_tag=TAG,
                release_sha=SHA,
            )

    def test_rejects_any_analyzer_failure_beyond_missing_declarations(self) -> None:
        bad = LOG.replace(
            "2026-08-28T00:25:42.0000000Z Policy (M3):",
            "2026-08-28T00:25:42.0000000Z   - the report never reached these publishable crates: meerkat-runtime\n"
            "2026-08-28T00:25:42.0000000Z Policy (M3):",
        )
        with self.assertRaisesRegex(gate.EvidenceError, "unexpected analyzer failure line"):
            gate.parse_missing_declarations(bad)

    def test_rejects_missing_exit_attestation(self) -> None:
        with self.assertRaisesRegex(gate.EvidenceError, "exit line"):
            gate.parse_missing_declarations(
                LOG.replace("cargo-semver-checks exited 100", "cargo-semver-checks completed")
            )

    def test_changelog_must_name_every_missing_symbol(self) -> None:
        import tempfile

        findings = gate.parse_missing_declarations(LOG)
        with tempfile.TemporaryDirectory() as directory:
            changelog = Path(directory) / "CHANGELOG.md"
            changelog.write_text(CHANGELOG, encoding="utf-8")
            gate.verify_changelog(changelog, TAG, findings)
            changelog.write_text(
                CHANGELOG.replace("LifecycleOperationProgressStalled", "LifecycleStalled"),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(gate.EvidenceError, "LifecycleOperationProgressStalled"):
                gate.verify_changelog(changelog, TAG, findings)


if __name__ == "__main__":
    unittest.main()
