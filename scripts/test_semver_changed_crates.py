#!/usr/bin/env python3
"""Unit tests for the baseline-identical crate classifier.

They build a real git repository with a two-crate workspace, tag a baseline,
apply targeted changes, and assert which crates the release must re-measure.
"""

from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import sys
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name("semver_changed_crates.py")
SPEC = importlib.util.spec_from_file_location("semver_changed_crates", SCRIPT)
assert SPEC and SPEC.loader
classifier = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = classifier
SPEC.loader.exec_module(classifier)

ROOT_MANIFEST = """[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
version = "{version}"
edition = "2024"

[workspace.dependencies]
serde = "{serde}"
alpha = {{ path = "crates/alpha", version = "={version}" }}
"""

ALPHA_MANIFEST = """[package]
name = "alpha"
version.workspace = true
edition.workspace = true

[dependencies]
serde = {{ workspace = true }}
"""

BETA_MANIFEST = """[package]
name = "beta"
version.workspace = true
edition.workspace = true

[dependencies]
alpha = {{ workspace = true }}
"""


def git(repo: pathlib.Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True)


class ClassifierRepo:
    def __init__(self, root: pathlib.Path) -> None:
        self.root = root
        git(root, "init", "-q", "-b", "main")
        git(root, "config", "user.email", "test@example.com")
        git(root, "config", "user.name", "test")
        self.write_workspace(version="0.1.0", serde="1")
        for crate in ("alpha", "beta"):
            (root / "crates" / crate / "src").mkdir(parents=True)
            (root / "crates" / crate / "src" / "lib.rs").write_text("pub fn f() {}\n")
        self.commit("baseline")
        git(root, "tag", "v0.1.0")

    def write_workspace(self, *, version: str, serde: str) -> None:
        (self.root / "Cargo.toml").write_text(ROOT_MANIFEST.format(version=version, serde=serde))
        (self.root / "crates").mkdir(exist_ok=True)
        (self.root / "crates" / "alpha").mkdir(exist_ok=True)
        (self.root / "crates" / "beta").mkdir(exist_ok=True)
        (self.root / "crates" / "alpha" / "Cargo.toml").write_text(ALPHA_MANIFEST.format())
        (self.root / "crates" / "beta" / "Cargo.toml").write_text(BETA_MANIFEST.format())

    def commit(self, message: str) -> None:
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", message)

    def classify(self, crates: list[str]) -> classifier.Classification:
        return classifier.classify(self.root, "v0.1.0", "HEAD", crates)


class ClassifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.repo = ClassifierRepo(pathlib.Path(self._temp.name))

    def tearDown(self) -> None:
        self._temp.cleanup()

    def test_version_bump_alone_leaves_every_crate_unchanged(self) -> None:
        self.repo.write_workspace(version="0.1.1", serde="1")
        self.repo.commit("release 0.1.1")
        result = self.repo.classify(["alpha", "beta"])
        self.assertEqual(result.changed, [])
        self.assertEqual(result.unchanged, ["alpha", "beta"])
        self.assertEqual(result.first_publish, [])

    def test_source_change_marks_only_that_crate(self) -> None:
        (self.repo.root / "crates" / "alpha" / "src" / "lib.rs").write_text("pub fn g() {}\n")
        self.repo.write_workspace(version="0.1.1", serde="1")
        self.repo.commit("change alpha")
        result = self.repo.classify(["alpha", "beta"])
        self.assertEqual(result.changed, ["alpha"])
        self.assertEqual(result.unchanged, ["beta"])
        self.assertIn("source differs", result.reasons["alpha"])

    def test_generated_bazel_and_test_trees_do_not_count_as_api_changes(self) -> None:
        (self.repo.root / "crates" / "alpha" / "BUILD.bazel").write_text('version = "0.1.1"\n')
        tests = self.repo.root / "crates" / "beta" / "tests"
        tests.mkdir()
        (tests / "smoke.rs").write_text("#[test] fn t() {}\n")
        self.repo.write_workspace(version="0.1.1", serde="1")
        self.repo.commit("bazel + tests")
        result = self.repo.classify(["alpha", "beta"])
        self.assertEqual(result.changed, [])
        self.assertEqual(result.unchanged, ["alpha", "beta"])

    def test_build_script_change_counts(self) -> None:
        (self.repo.root / "crates" / "alpha" / "build.rs").write_text("fn main() {}\n")
        self.repo.commit("build script")
        result = self.repo.classify(["alpha", "beta"])
        self.assertEqual(result.changed, ["alpha"])

    def test_workspace_dependency_spec_change_marks_dependents(self) -> None:
        self.repo.write_workspace(version="0.1.1", serde="1.0.200")
        self.repo.commit("bump serde")
        result = self.repo.classify(["alpha", "beta"])
        self.assertEqual(result.changed, ["alpha"])
        self.assertEqual(result.unchanged, ["beta"])
        self.assertIn("serde", result.reasons["alpha"])

    def test_member_dependency_feature_change_still_marks_dependents(self) -> None:
        text = (self.repo.root / "Cargo.toml").read_text().replace(
            'alpha = { path = "crates/alpha", version = "=0.1.0" }',
            'alpha = { path = "crates/alpha", version = "=0.1.1", default-features = false }',
        )
        (self.repo.root / "Cargo.toml").write_text(text)
        self.repo.commit("alpha features")
        result = self.repo.classify(["alpha", "beta"])
        self.assertEqual(result.changed, ["beta"])
        self.assertIn("alpha", result.reasons["beta"])

    def test_workspace_package_field_change_marks_everything(self) -> None:
        text = (self.repo.root / "Cargo.toml").read_text().replace('edition = "2024"', 'edition = "2021"')
        (self.repo.root / "Cargo.toml").write_text(text)
        self.repo.commit("edition")
        result = self.repo.classify(["alpha", "beta"])
        self.assertEqual(result.changed, ["alpha", "beta"])

    def test_new_crate_is_first_publish(self) -> None:
        gamma = self.repo.root / "crates" / "gamma"
        (gamma / "src").mkdir(parents=True)
        (gamma / "Cargo.toml").write_text(
            '[package]\nname = "gamma"\nversion.workspace = true\nedition.workspace = true\n'
        )
        (gamma / "src" / "lib.rs").write_text("pub fn h() {}\n")
        self.repo.commit("add gamma")
        result = self.repo.classify(["alpha", "beta", "gamma"])
        self.assertEqual(result.first_publish, ["gamma"])
        self.assertEqual(result.unchanged, ["alpha", "beta"])

    def test_unknown_crate_is_measured_to_fail_closed(self) -> None:
        result = self.repo.classify(["alpha", "ghost"])
        self.assertEqual(result.changed, ["ghost"])
        self.assertIn("not a workspace member", result.reasons["ghost"])

    def test_cli_emits_json(self) -> None:
        completed = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--repo-root",
                str(self.repo.root),
                "--baseline-tag",
                "v0.1.0",
                "--release-crate",
                "alpha",
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertIn('"unchanged": [\n    "alpha"\n  ]', completed.stdout)


if __name__ == "__main__":
    unittest.main()
