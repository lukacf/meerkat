"""Exercise changed-package selection with real Git history and a Cargo stub."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


class PrePushClippyDispatchTests(unittest.TestCase):
    def test_config_dispatches_rust_manifests_lockfiles_and_toolchains(self):
        config = (Path(__file__).resolve().parents[1] / ".pre-commit-config.yaml").read_text()
        hook = re.search(
            r"^      - id: cargo-clippy\n(?P<fields>(?:^        [^\n]*\n)+)",
            config, re.MULTILINE,
        )
        self.assertIsNotNone(hook, "cargo-clippy hook is missing")
        fields = dict(line.strip().split(": ", 1) for line in hook["fields"].splitlines())
        self.assertEqual(fields["entry"], "scripts/pre-push-clippy.sh")
        self.assertEqual(fields["stages"], "[manual]")
        # pre-commit combines files and type filters, so a Rust-only type
        # filter would still prevent manifests and lockfiles from dispatching.
        for key in ("types", "types_or", "exclude_types", "exclude"):
            self.assertNotIn(key, fields, f"Review the additional {key} dispatch filter")
        selector = re.compile(fields["files"])
        for path in (
            "crates/meerkat-meta/src/lib.rs", "src/main.rs", "build.rs",
            "Cargo.toml", "Cargo.lock", "crates/meerkat-meta/Cargo.toml",
            "nested/Cargo.lock", "rust-toolchain.toml", "rust-toolchain",
        ):
            with self.subTest(selected=path):
                self.assertIsNotNone(selector.search(path))
        for path in (
            "README.md", "console/src/main.ts", "scripts/test_pre_push_clippy.py",
            "pyproject.toml", "src/lib.rs.md", "NotCargo.lock", "Cargo.lock.backup",
            "rust-toolchain.toml.backup", "docs/rust-toolchain.md",
        ):
            with self.subTest(excluded=path):
                self.assertIsNone(selector.search(path))



def hook_stages(config: str) -> dict[str, str]:
    """Every hook id in .pre-commit-config.yaml and its stages field."""
    hooks = {}
    for match in re.finditer(r"(?m)^      - id: (\S+)\n((?:^        [^\n]*\n)*)", config):
        fields = dict(line.strip().split(": ", 1) for line in match[2].splitlines() if ": " in line)
        hooks[match[1]] = fields
    return hooks


class PushStageTests(unittest.TestCase):
    """A push runs only fast local checks. The workspace clippy and unit gate
    took 712.88 s and 974.16 s of a 28m19s push (2026-10-16); CI's clippy
    and test jobs run them on every pull request that touches Rust, so the
    hook keeps them only on demand (`pre-commit run --hook-stage manual`)."""

    def setUp(self):
        self.config = (Path(__file__).resolve().parents[1] / ".pre-commit-config.yaml").read_text()
        self.hooks = hook_stages(self.config)

    def test_the_push_stage_compiles_nothing(self):
        pushed = {name for name, fields in self.hooks.items() if "pre-push" in fields.get("stages", "")}
        self.assertEqual(pushed, {
            "gitleaks", "trailing-whitespace", "end-of-file-fixer", "check-yaml", "check-toml",
            "check-merge-conflict", "check-added-large-files", "cargo-fmt", "bazel-builds-fresh",
            "decorator-authority",
        })
        for name in pushed:
            entry = self.hooks[name].get("entry", "")
            with self.subTest(hook=name):
                self.assertNotRegex(entry, r"clippy|nextest|cargo (build|check|test)|pre-push-unit")

    def test_the_cargo_gates_stay_runnable_on_demand(self):
        for name, entry in (("cargo-clippy", "scripts/pre-push-clippy.sh"),
                            ("cargo-test", "scripts/pre-push-unit.sh")):
            with self.subTest(hook=name):
                self.assertEqual(self.hooks[name]["entry"], entry)
                self.assertEqual(self.hooks[name]["stages"], "[manual]")
        self.assertIn("pre-commit run --hook-stage manual", self.config)

    def test_the_stage_parser_reads_every_hook(self):
        # Control: the parser sees both the local and the third-party hooks.
        self.assertGreaterEqual(len(self.hooks), 13)
        self.assertEqual(self.hooks["gitleaks"]["stages"], "[pre-push]")
        self.assertNotIn("stages", self.hooks["cargo-fmt-fix"])


class PrePushClippyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.repo = self.root / "repo with spaces"
        self.repo.mkdir()
        self.log = self.root / "cargo-calls.jsonl"
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith("GIT_")
        }
        self.env.update({
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "CLIPPY_TEST_LOG": str(self.log),
            "CARGO_INCREMENTAL": "1",
        })
        self.git("init", "-q", "-b", "feature")
        self.git("config", "user.name", "Clippy hook test")
        self.git("config", "user.email", "clippy-hook@example.invalid")
        self.git("config", "core.hooksPath", str(self.root / "no-hooks"))
        self.write("Cargo.toml", "[workspace]\nmembers = []\n")
        self.write("Cargo.lock", "version = 4\n")
        (self.repo / "scripts").mkdir()
        shutil.copyfile(
            Path(__file__).with_name("pre-push-clippy.sh"),
            self.repo / "scripts/pre-push-clippy.sh",
        )
        self.write(
            "scripts/repo-cargo",
            f"#!{sys.executable}\n"
            "import json, os, sys\n"
            "with open(os.environ['CLIPPY_TEST_LOG'], 'a') as log:\n"
            "    log.write(json.dumps({'argv': sys.argv[1:], "
            "'incremental': os.environ.get('CARGO_INCREMENTAL')}) + '\\n')\n"
            "sys.exit(int(os.environ.get('CLIPPY_TEST_EXIT', '0')))\n",
        )
        (self.repo / "scripts/repo-cargo").chmod(0o755)
        self.commit("initial workspace")
        self.track_head()

    def git(self, *args):
        return subprocess.run(
            ["git", *args], cwd=self.repo, env=self.env,
            capture_output=True, text=True, check=True,
        ).stdout.strip()

    def write(self, path, content):
        destination = self.repo / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding="utf-8")

    def commit(self, message):
        self.git("add", "--all")
        self.git("commit", "-q", "-m", message)

    def track_head(self):
        self.git("config", "remote.origin.url", str(self.root / "unused-remote"))
        self.git("config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("branch", "--set-upstream-to=origin/main", "feature")

    def package(self, directory, name):
        self.write(
            f"{directory}/Cargo.toml",
            f'[package]\nname = "{name}"\nversion = "0.1.0"\n',
        )
        self.write(f"{directory}/src/lib.rs", "pub fn before() {}\n")
        self.commit(f"add {name}")
        self.track_head()

    def run_hook(self, *, exit_code=0):
        result = subprocess.run(
            ["bash", str(self.repo / "scripts/pre-push-clippy.sh")],
            cwd=self.repo,
            env={**self.env, "CLIPPY_TEST_EXIT": str(exit_code)},
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, exit_code, result.stdout + result.stderr)
        calls = (
            [json.loads(line) for line in self.log.read_text().splitlines()]
            if self.log.exists() else []
        )
        for call in calls:
            self.assertEqual(call["incremental"], "0")
        return calls

    def assert_packages(self, *packages, exit_code=0):
        flags = [flag for package in packages for flag in ("-p", package)]
        self.assertEqual(self.run_hook(exit_code=exit_code), [{
            "argv": ["clippy", *flags, "--all-targets", "--", "-D", "warnings"],
            "incremental": "0",
        }])

    def assert_workspace(self):
        self.assertEqual(self.run_hook(), [{
            "argv": ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
            "incremental": "0",
        }])

    def test_nested_crate_source_selects_manifest_package(self):
        self.package("crates/directory-name", "actual-package")
        self.write("crates/directory-name/src/lib.rs", "pub fn after() {}\n")
        self.commit("change nested source")
        self.assert_packages("actual-package")

    def test_top_level_crate_reads_package_table_not_first_target_name(self):
        self.package("top-level", "actual-package")
        self.write(
            "top-level/Cargo.toml",
            "[[bin]]\nname = 'cli-target'\npath = 'src/main.rs'\n"
            "[package]\nname = 'actual-package'\nversion = '0.1.0'\n",
        )
        self.commit("change top-level manifest")
        self.assert_packages("actual-package")

    def test_out_of_tree_example_source_falls_back_to_workspace(self):
        self.package("crates/owner", "example-owner")
        manifest = self.repo / "crates/owner/Cargo.toml"
        manifest.write_text(manifest.read_text() +
                            "[[example]]\nname = 'outside'\npath = '../../examples/outside.rs'\n")
        self.write("examples/outside.rs", "fn main() {}\n")
        self.commit("add out of tree example")
        self.track_head()
        self.write("examples/outside.rs", "fn main() { println!(\"changed\"); }\n")
        self.commit("change only out of tree example")
        self.assert_workspace()

    def test_unowned_source_with_known_package_still_runs_workspace(self):
        self.package("crates/owner", "known-package")
        self.write("crates/owner/src/lib.rs", "pub fn changed() {}\n")
        self.write("examples/support/helper.rs", "pub fn helper() {}\n")
        self.commit("change package and out of tree support source")
        self.assert_workspace()

    def test_deleted_unowned_source_falls_back_to_workspace(self):
        self.write("examples/removed.rs", "fn main() {}\n")
        self.commit("add source before deletion")
        self.track_head()
        (self.repo / "examples/removed.rs").unlink()
        self.commit("delete unowned source")
        self.assert_workspace()

    def test_lockfile_only_change_runs_workspace(self):
        self.write("Cargo.lock", "version = 4\n# dependency update\n")
        self.commit("change lockfile")
        self.assert_workspace()

    def test_root_manifest_change_runs_workspace(self):
        self.write("Cargo.toml", '[workspace]\nmembers = []\nresolver = "2"\n')
        self.commit("change workspace manifest")
        self.assert_workspace()

    def test_toolchain_toml_only_change_runs_workspace(self):
        self.write("rust-toolchain.toml", '[toolchain]\nchannel = "stable"\n')
        self.commit("change workspace toolchain")
        self.assert_workspace()

    def test_legacy_toolchain_only_change_runs_workspace(self):
        self.write("rust-toolchain", "stable\n")
        self.commit("change legacy workspace toolchain")
        self.assert_workspace()

    def test_unrelated_changes_skip_cargo(self):
        self.write("README.md", "Updated documentation.\n")
        self.write("console/src/main.ts", "export {};\n")
        self.commit("change documentation and console")
        self.assertEqual(self.run_hook(), [])

    def test_python_script_only_change_skips_cargo(self):
        self.write("scripts/test_fixture.py", "assert True\n")
        self.commit("change Python test")
        self.assertEqual(self.run_hook(), [])

    def test_multiple_changed_packages_are_deduplicated(self):
        self.package("crates/first", "zebra")
        self.package("crates/second", "alpha")
        self.write("crates/first/src/lib.rs", "pub fn after() {}\n")
        self.write("crates/first/src/helper.rs", "pub fn helper() {}\n")
        self.write("crates/second/src/lib.rs", "pub fn after() {}\n")
        self.commit("change two packages and three files")
        self.assert_packages("alpha", "zebra")

    def test_deleted_source_uses_surviving_ancestor_manifest(self):
        self.package("crates/deleted-source", "retained-package")
        (self.repo / "crates/deleted-source/src/lib.rs").unlink()
        (self.repo / "crates/deleted-source/src").rmdir()
        self.commit("delete the source directory")
        self.assert_packages("retained-package")

    def test_renamed_source_selects_both_owning_packages(self):
        self.package("crates/from", "source-package")
        self.package("crates/to", "destination-package")
        self.git("mv", "crates/from/src/lib.rs", "crates/to/src/moved.rs")
        self.commit("move source between packages")
        self.assert_packages("destination-package", "source-package")

    def test_tracking_scope_ignores_already_pushed_and_uncommitted_changes(self):
        self.package("crates/pushed", "pushed-package")
        self.package("crates/pending", "pending-package")
        self.write("crates/pending/src/lib.rs", "pub fn committed() {}\n")
        self.commit("change only pending package")
        self.write("crates/pushed/src/lib.rs", "pub fn uncommitted() {}\n")
        self.assert_packages("pending-package")

    def test_missing_remote_history_falls_back_to_workspace(self):
        self.git("config", "--unset", "branch.feature.remote")
        self.git("config", "--unset", "branch.feature.merge")
        self.git("update-ref", "-d", "refs/remotes/origin/main")
        self.assert_workspace()

    def test_cargo_failure_remains_a_failing_gate(self):
        self.package("top-level", "failing-package")
        self.write("top-level/src/lib.rs", "pub fn after() {}\n")
        self.commit("change failing package")
        self.assert_packages("failing-package", exit_code=23)


if __name__ == "__main__":
    unittest.main()
