#!/usr/bin/env python3
"""Behavioral tests for the crate publication gate.

Each test constructs a workspace whose defect the gate must catch, rather than
asserting on the gate's source. A gate is only worth its runtime if it goes red
on the thing it was written for, so every defect here is one that has actually
occurred or would fail a real release.
"""

import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

GATE = Path(__file__).resolve().parents[1] / "scripts/verify-crate-publication.py"


def build_workspace(root: Path, members: dict[str, str], release_yml: str) -> None:
    (root / "Cargo.toml").write_text(
        "[workspace]\nmembers = [%s]\n"
        % ", ".join(f'"{name}"' for name in members)
    )
    for name, manifest in members.items():
        (root / name).mkdir(parents=True, exist_ok=True)
        (root / name / "Cargo.toml").write_text(textwrap.dedent(manifest))
    workflows = root / ".github/workflows"
    workflows.mkdir(parents=True, exist_ok=True)
    (workflows / "release.yml").write_text(release_yml)


def run_gate(root: Path):
    target = root / "scripts"
    target.mkdir(exist_ok=True)
    (target / GATE.name).write_text(GATE.read_text())
    return subprocess.run(
        [sys.executable, str(target / GATE.name)], capture_output=True, text=True
    )


PUBLISHES_ONE = "  run: cargo publish -p alpha --locked\n"


class CratePublicationGateTests(unittest.TestCase):
    def gate(self, members, release_yml=PUBLISHES_ONE):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        build_workspace(root, members, release_yml)
        return run_gate(root)

    def test_an_explicitly_declared_workspace_passes(self):
        result = self.gate({
            "alpha": '[package]\nname = "alpha"\nversion = "0.1.0"\n',
            "beta": '[package]\nname = "beta"\nversion = "0.1.0"\npublish = false\n',
        })
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_member_that_is_neither_published_nor_withheld_fails(self):
        # The meta-store-conformance shape: no publish key, never published.
        result = self.gate({
            "alpha": '[package]\nname = "alpha"\nversion = "0.1.0"\n',
            "beta": '[package]\nname = "beta"\nversion = "0.1.0"\n',
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("beta", result.stderr)

    def test_a_published_crate_marked_publish_false_fails(self):
        result = self.gate({
            "alpha": '[package]\nname = "alpha"\nversion = "0.1.0"\npublish = false\n',
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("One of the two is wrong", result.stderr)

    def test_a_published_crate_with_a_versionless_path_dep_fails(self):
        # Exactly what cargo refuses, caught before a release reaches the
        # publish step rather than after every cross-compile has run.
        result = self.gate({
            "alpha": (
                '[package]\nname = "alpha"\nversion = "0.1.0"\n'
                '[dependencies]\nbeta = { path = "../beta" }\n'
            ),
            "beta": '[package]\nname = "beta"\nversion = "0.1.0"\npublish = false\n',
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("cargo publish will refuse", result.stderr)

    def test_a_versioned_path_dep_is_accepted(self):
        result = self.gate({
            "alpha": (
                '[package]\nname = "alpha"\nversion = "0.1.0"\n'
                '[dependencies]\nbeta = { path = "../beta", version = "0.1.0" }\n'
            ),
            "beta": '[package]\nname = "beta"\nversion = "0.1.0"\npublish = false\n',
        })
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_typo_in_the_release_publish_flag_fails(self):
        # `-p meerkat-metat` currently fails at the publish step, mid-release.
        result = self.gate(
            {"alpha": '[package]\nname = "alpha"\nversion = "0.1.0"\n'},
            release_yml="  run: cargo publish -p alphaa --locked\n",
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("not workspace members", result.stderr)

    def test_publication_disappearing_from_the_workflow_fails(self):
        # If publication moves somewhere this gate cannot see, the gate must say
        # so rather than pass an empty published set as "nothing to check".
        result = self.gate(
            {"alpha": '[package]\nname = "alpha"\nversion = "0.1.0"\npublish = false\n'},
            release_yml="  run: echo nothing here\n",
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("no `cargo publish -p NAME`", result.stderr)

    def test_in_the_monorepo_only_metas_members_of_the_root_workspace_count(self):
        # Meta under meta/ of a repository whose one workspace lists Meta's
        # crates by glob beside Core's: Core's crate, neither published by
        # Meta's release nor withheld, is not Meta's to declare.
        self.tmp = tempfile.TemporaryDirectory()
        repo = Path(self.tmp.name)
        (repo / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/core", "meta/crates/*"]\n')
        for directory, manifest in {
            "crates/core": '[package]\nname = "core"\nversion = "0.9.6"\n',
            "meta/crates/alpha": '[package]\nname = "alpha"\nversion = "0.9.5"\n',
            "meta/crates/beta": '[package]\nname = "beta"\nversion = "0.9.5"\npublish = false\n',
        }.items():
            (repo / directory).mkdir(parents=True)
            (repo / directory / "Cargo.toml").write_text(manifest)
        workflows = repo / "meta/.github/workflows"
        workflows.mkdir(parents=True)
        (workflows / "release.yml").write_text(PUBLISHES_ONE)
        result = run_gate(repo / "meta")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("2 workspace member(s)", result.stdout)
        # A Meta crate that declares neither still fails there.
        (repo / "meta/crates/beta/Cargo.toml").write_text('[package]\nname = "beta"\nversion = "0.9.5"\n')
        result = run_gate(repo / "meta")
        self.assertEqual(result.returncode, 1)
        self.assertIn("beta", result.stderr)
        self.assertNotIn("core", result.stderr)


if __name__ == "__main__":
    unittest.main()
