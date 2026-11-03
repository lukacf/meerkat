#!/usr/bin/env python3
"""Tests for the MobKit docs mirror lag ratchet (mirror vs main's meta/docs)."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check-mobkit-docs-lag.py")
SPEC = importlib.util.spec_from_file_location("check_mobkit_docs_lag", SCRIPT)
assert SPEC and SPEC.loader
lag = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = lag
SPEC.loader.exec_module(lag)


class Repo:
    """A throwaway repository with meta/docs, code and a docs/mobkit manifest."""

    def __init__(self, root: Path) -> None:
        self.root = root
        root.mkdir(parents=True)
        self.git("init", "-q", "-b", "main")
        self.commit({"meta/docs/introduction.mdx": "intro\n", "crates/a.rs": "fn a() {}\n"}, "base")

    def git(self, *args: str) -> str:
        identity = ["-c", "user.name=test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"]
        return subprocess.run(
            ["git", "-C", str(self.root), *identity, *args],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def commit(self, files: dict[str, str], message: str) -> str:
        for name, text in files.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD")

    def manifest(self, source_commit: str, **fields: object) -> Path:
        path = self.root.parent / "_source.json"
        path.write_text(
            json.dumps(
                {
                    "source_repository": lag.REPOSITORY,
                    "source_path": "meta/docs",
                    "source_commit": source_commit,
                    **fields,
                }
            ),
            encoding="utf-8",
        )
        return path


class DocsLagTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.repo = Repo(Path(self.temp.name) / "repo")
        self.mirrored = self.repo.git("rev-parse", "HEAD")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_mirror_at_head_has_no_lag(self) -> None:
        result = lag.docs_lag(self.repo.root, self.mirrored, "meta/docs")
        self.assertEqual(result.ahead_by, 0)
        self.assertEqual(result.docs_commits, ())

    def test_commits_outside_meta_docs_are_not_lag(self) -> None:
        self.repo.commit({"crates/a.rs": "fn a() { 1; }\n"}, "code")
        # The mirror publication itself changes docs/mobkit, not meta/docs.
        self.repo.commit({"docs/mobkit/introduction.mdx": "mirrored\n"}, "mirror")
        result = lag.docs_lag(self.repo.root, self.mirrored, "meta/docs")
        self.assertEqual(result.ahead_by, 2)
        self.assertEqual(result.docs_commits, ())

    def test_meta_docs_commits_after_the_mirror_are_lag(self) -> None:
        first = self.repo.commit({"meta/docs/introduction.mdx": "intro 2\n"}, "docs 1")
        self.repo.commit({"crates/a.rs": "fn a() { 2; }\n"}, "code")
        second = self.repo.commit({"meta/docs/guide.mdx": "guide\n"}, "docs 2")
        result = lag.docs_lag(self.repo.root, self.mirrored, "meta/docs")
        self.assertEqual(result.ahead_by, 3)
        self.assertEqual(result.docs_commits, (second[:12], first[:12]))

    def test_a_mirror_not_on_head_is_an_error_not_a_zero_lag(self) -> None:
        self.repo.git("checkout", "-q", "-b", "side")
        side = self.repo.commit({"meta/docs/introduction.mdx": "side\n"}, "side docs")
        self.repo.git("checkout", "-q", "main")
        with self.assertRaisesRegex(lag.LagCheckError, "not an ancestor of HEAD"):
            lag.docs_lag(self.repo.root, side, "meta/docs")
        with self.assertRaisesRegex(lag.LagCheckError, "not in this history"):
            lag.docs_lag(self.repo.root, "1" * 40, "meta/docs")

    def test_manifest_must_name_this_repository_and_a_relative_docs_path(self) -> None:
        good = self.repo.manifest(self.mirrored)
        self.assertEqual(lag.manifest_source(good), (self.mirrored, "meta/docs"))
        legacy = self.repo.manifest(self.mirrored, source_repository="https://github.com/lukacf/meerkat-meta")
        with self.assertRaisesRegex(lag.LagCheckError, "not this repository"):
            lag.manifest_source(legacy)
        for bad_path in ("/meta/docs", "../docs", ""):
            with self.subTest(path=bad_path):
                bad = self.repo.manifest(self.mirrored, source_path=bad_path)
                with self.assertRaisesRegex(lag.LagCheckError, "invalid source_path"):
                    lag.manifest_source(bad)
        missing = self.repo.manifest("")
        with self.assertRaisesRegex(lag.LagCheckError, "no source_commit"):
            lag.manifest_source(missing)

    def run_command(self, manifest: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--manifest", str(manifest), "--repo", str(self.repo.root), "--max-lag", "0"],
            capture_output=True,
            text=True,
        )

    def test_command_passes_while_only_other_paths_moved(self) -> None:
        self.repo.commit({"crates/a.rs": "fn a() { 3; }\n"}, "code")
        result = self.run_command(self.repo.manifest(self.mirrored))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("1 commit(s) ahead, 0 changing meta/docs", result.stdout)

    def test_command_fails_when_meta_docs_moved_after_the_mirror(self) -> None:
        moved = self.repo.commit({"meta/docs/introduction.mdx": "moved\n"}, "docs")
        result = self.run_command(self.repo.manifest(self.mirrored))
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(moved[:12], result.stdout)

    def test_command_distinguishes_unknown_lag_from_lag(self) -> None:
        result = self.run_command(self.repo.manifest("1" * 40))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("cannot determine lag", result.stdout)


class SourceAgreementTests(unittest.TestCase):
    def test_the_check_follows_the_repository_and_path_the_sync_script_records_for_meta(self) -> None:
        spec = importlib.util.spec_from_file_location("sync_mobkit_docs", SCRIPT.with_name("sync-mobkit-docs.py"))
        assert spec and spec.loader
        sync = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(sync)
        repository, _version, docs_path = sync.source_identity(SCRIPT.resolve().parents[1] / "meta")
        self.assertEqual((repository, docs_path), (lag.REPOSITORY, lag.DEFAULT_DOCS_PATH))


if __name__ == "__main__":
    unittest.main()
