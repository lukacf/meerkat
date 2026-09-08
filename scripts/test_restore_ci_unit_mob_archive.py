#!/usr/bin/env python3
import hashlib
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location(
    "restore_archive", Path(__file__).with_name("restore-ci-unit-mob-archive.py")
)
RESTORE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RESTORE)
COMMIT = "a" * 40


class ArchiveReuseTests(unittest.TestCase):
    def run_row(self):
        return {
            "id": 10,
            "head_sha": COMMIT,
            "event": "push",
            "status": "completed",
            "conclusion": "success",
            "path": ".github/workflows/ci.yml",
            "head_repository": {"full_name": "owner/repo"},
        }

    def artifact(self):
        return {
            "id": 20,
            "name": f"nextest-unit-mob-{COMMIT}",
            "expired": False,
            "digest": "sha256:" + "b" * 64,
            "workflow_run": {"id": 10, "head_sha": COMMIT},
        }

    def test_only_completed_successful_exact_commit_push_ci_is_reusable(self):
        row = self.run_row()
        self.assertEqual(
            RESTORE.candidate_runs([row], "owner/repo", COMMIT, 11), [10]
        )
        for field, value in [
            ("id", 11),
            ("id", -1),
            ("head_sha", "b" * 40),
            ("event", "pull_request"),
            ("status", "in_progress"),
            ("conclusion", "failure"),
            ("path", ".github/workflows/untrusted.yml"),
            ("head_repository", None),
            ("head_repository", {"full_name": "fork/repo"}),
        ]:
            with self.subTest(field=field, value=value):
                self.assertEqual(
                    RESTORE.candidate_runs(
                        [{**row, field: value}], "owner/repo", COMMIT, 11
                    ),
                    [],
                )

    def test_artifact_identity_digest_and_uniqueness_are_required(self):
        row = self.artifact()
        self.assertEqual(RESTORE.select_artifact([row], COMMIT, 10), row)
        self.assertIsNone(RESTORE.select_artifact([row, row], COMMIT, 10))
        for field, value in [
            ("name", "nextest-unit-mob-wrong"),
            ("expired", True),
            ("digest", None),
            ("digest", "sha256:bad"),
            ("id", True),
            ("workflow_run", {"id": 12, "head_sha": COMMIT}),
            ("workflow_run", {"id": 10, "head_sha": "b" * 40}),
        ]:
            with self.subTest(field=field):
                self.assertIsNone(
                    RESTORE.select_artifact([{**row, field: value}], COMMIT, 10)
                )

    def test_verified_zip_extracts_only_the_expected_nonempty_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            download = root / "download.zip"
            destination = root / "archive.tar.zst"
            with zipfile.ZipFile(download, "w") as archive:
                archive.writestr("nextest-unit-mob.tar.zst", b"exact archive")
            digest = "sha256:" + hashlib.sha256(download.read_bytes()).hexdigest()
            RESTORE.extract_verified_archive(download, digest, destination)
            self.assertEqual(destination.read_bytes(), b"exact archive")
            with self.assertRaisesRegex(ValueError, "digest"):
                RESTORE.extract_verified_archive(download, "sha256:" + "0" * 64, destination)
            self.assertEqual(destination.read_bytes(), b"exact archive")

    def test_unexpected_zip_paths_and_empty_archives_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, content in [
                ("../escape", b"bad"),
                ("nextest-unit-mob.tar.zst", b""),
            ]:
                with self.subTest(name=name):
                    download = root / "download.zip"
                    destination = root / "archive.tar.zst"
                    with zipfile.ZipFile(download, "w") as archive:
                        archive.writestr(name, content)
                    digest = "sha256:" + hashlib.sha256(download.read_bytes()).hexdigest()
                    with self.assertRaises(ValueError):
                        RESTORE.extract_verified_archive(download, digest, destination)
                    self.assertFalse(destination.exists())
                    self.assertFalse((root.parent / "escape").exists())

    def test_api_failure_is_not_reported_as_successful_reuse(self):
        with patch.object(
            RESTORE.subprocess,
            "run",
            side_effect=subprocess.CalledProcessError(1, ["gh", "api"]),
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                RESTORE.github_json("repos/owner/repo/actions/runs")


if __name__ == "__main__":
    unittest.main()
