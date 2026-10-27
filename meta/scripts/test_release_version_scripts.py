#!/usr/bin/env python3
"""Regression tests for release version bump and staging ownership."""

import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).parent


class ReleaseVersionScriptsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / "scripts").mkdir()
        (self.root / "sdk/python").mkdir(parents=True)
        (self.root / "sdk/typescript").mkdir(parents=True)
        (self.root / "crates/meerkat-meta").mkdir(parents=True)
        (self.root / "crates/meta-store-conformance").mkdir(parents=True)
        (self.root / "docs/sdks").mkdir(parents=True)
        (self.root / "src").mkdir()

        for name in (
            "bump-sdk-versions.sh",
            "release-hook.sh",
            "verify-version-parity.sh",
        ):
            destination = self.root / "scripts" / name
            shutil.copy2(SCRIPTS / name, destination)
            destination.chmod(0o755)

        repo_cargo = self.root / "scripts/repo-cargo"
        repo_cargo.write_text('#!/usr/bin/env bash\nexec cargo "$@"\n')
        repo_cargo.chmod(0o755)

        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "meerkat-meta"\nversion = "0.8.1"\nedition = "2024"\n'
        )
        (self.root / "src/lib.rs").write_text("")
        (self.root / "sdk/python/pyproject.toml").write_text(
            '[project]\nname = "meerkat-meta"\nversion = "0.8.0"\n'
        )
        (self.root / "sdk/typescript/package.json").write_text(
            json.dumps({"name": "@rkat/meta-sdk", "version": "0.8.0"}, indent=2)
            + "\n"
        )
        (self.root / "sdk/typescript/package-lock.json").write_text(
            json.dumps(
                {
                    "name": "@rkat/meta-sdk",
                    "version": "0.8.0",
                    "packages": {"": {"version": "0.8.0"}},
                },
                indent=2,
            )
            + "\n"
        )
        (self.root / "crates/meerkat-meta/BUILD.bazel").write_text(
            'rustc_env = {"CARGO_PKG_VERSION": "0.8.0"}\n'
        )
        # A SECOND generated BUILD.bazel, because one is not a test of the
        # defect. bump-sdk-versions.sh used to sed meerkat-meta's file by
        # name and release-hook.sh used to stage it by name, so
        # crates/meta-store-conformance/BUILD.bazel drifted a release behind from
        # the moment that crate existed. With a single-crate fixture both the
        # broken and the fixed script pass.
        (self.root / "crates/meta-store-conformance").mkdir(parents=True, exist_ok=True)
        (self.root / "crates/meta-store-conformance/BUILD.bazel").write_text(
            'rustc_env = {"CARGO_PKG_VERSION": "0.8.0"}\n'
        )
        (self.root / "MODULE.bazel").write_text(
            'module(\n    name = "meerkat_meta",\n    version = "0.8.0",\n)\n'
        )
        (self.root / "docs/quickstart.mdx").write_text(
            'meerkat-meta = "0.8.0"\n'
        )
        (self.root / "docs/sdks/rust.mdx").write_text(
            'meerkat-meta = "0.8.0"\n'
        )

        self.run_command("git", "init", "-q")
        self.run_command("git", "config", "user.email", "test@example.com")
        self.run_command("git", "config", "user.name", "Release Script Test")
        self.run_command("git", "add", ".")
        self.run_command("git", "commit", "-qm", "fixture")

    def tearDown(self):
        self.temp.cleanup()

    def run_command(self, *args, check=True, extra_env=None):
        env = os.environ.copy()
        env["CARGO_TARGET_DIR"] = str(self.root / "target")
        # A None value removes the variable.
        for key, value in (extra_env or {}).items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
        result = subprocess.run(
            args,
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
        )
        if check and result.returncode != 0:
            self.fail(
                f"command failed ({result.returncode}): {' '.join(args)}\n"
                f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
            )
        return result

    def test_release_hook_bumps_and_stages_bazel_module_version(self):
        self.run_command(
            "scripts/release-hook.sh", "0.8.1", extra_env={"DRY_RUN": "false"}
        )

        module = (self.root / "MODULE.bazel").read_text()
        self.assertIn('version = "0.8.1"', module)

        staged = self.run_command(
            "git", "diff", "--cached", "--name-only"
        ).stdout.splitlines()
        self.assertIn("MODULE.bazel", staged)
        self.assertIn("docs/quickstart.mdx", staged)
        self.assertIn("docs/sdks/rust.mdx", staged)

        # EVERY generated BUILD.bazel, bumped and staged. The conformance crate
        # is the one the old hardcoded paths missed, so asserting it is what
        # makes this a test of the defect rather than of the happy path.
        for crate in ("meerkat-meta", "meta-store-conformance"):
            build_file = f"crates/{crate}/BUILD.bazel"
            self.assertIn(
                '"CARGO_PKG_VERSION": "0.8.1"',
                (self.root / build_file).read_text(),
                f"{build_file} was not bumped",
            )
            self.assertIn(build_file, staged, f"{build_file} was not staged")

        # And nothing generated may be left dirty: a release commit that omits
        # a regenerated file ships a version its build files disagree with.
        dirty = self.run_command(
            "git", "diff", "--name-only"
        ).stdout.splitlines()
        self.assertNotIn("crates/meta-store-conformance/BUILD.bazel", dirty)
        self.assertNotIn("crates/meerkat-meta/BUILD.bazel", dirty)
        self.assertIn(
            'meerkat-meta = "0.8.1"',
            (self.root / "docs/quickstart.mdx").read_text(),
        )

        self.run_command("scripts/verify-version-parity.sh")

    def test_release_hook_writes_nothing_in_a_dry_run(self):
        # cargo-release runs the pre-release hook in its dry run too, with
        # DRY_RUN=true (and DRY_RUN=false on --execute). The 0.9.2 cut's dry
        # run bumped the version files, then failed the parity check against
        # the unbumped Cargo version and left them modified.
        def worktree():
            return self.run_command(
                "git", "status", "--porcelain", "--ignored", "--untracked-files=all"
            ).stdout

        before = worktree()
        dry = self.run_command(
            "scripts/release-hook.sh", "0.8.1", extra_env={"DRY_RUN": "true"}
        )
        self.assertEqual(worktree(), before)
        self.assertEqual(
            self.run_command("git", "diff", "--cached", "--name-only").stdout, ""
        )
        self.assertFalse((self.root / ".release-hook-done").exists())
        self.assertIn('version = "0.8.0"', (self.root / "MODULE.bazel").read_text())
        self.assertIn("dry run", dry.stdout)

        # An execute still bumps and stages.
        self.run_command(
            "scripts/release-hook.sh", "0.8.1", extra_env={"DRY_RUN": "false"}
        )
        self.assertIn('version = "0.8.1"', (self.root / "MODULE.bazel").read_text())
        self.assertIn(
            "MODULE.bazel",
            self.run_command("git", "diff", "--cached", "--name-only").stdout.split(),
        )

    def test_release_hook_refuses_an_unset_or_unknown_dry_run(self):
        # cargo-release always passes DRY_RUN=true or DRY_RUN=false; anything
        # else (an unpinned tool, a direct run) is refused before any write.
        def worktree():
            return self.run_command(
                "git", "status", "--porcelain", "--ignored", "--untracked-files=all"
            ).stdout

        before = worktree()
        for value in (None, "", "yes", "TRUE", "1"):
            with self.subTest(dry_run=value):
                result = self.run_command(
                    "scripts/release-hook.sh",
                    "0.8.1",
                    check=False,
                    extra_env={"DRY_RUN": value},
                )
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn("DRY_RUN must be true or false", result.stderr)
                self.assertEqual(worktree(), before)
                self.assertFalse((self.root / ".release-hook-done").exists())

    def test_version_parity_rejects_unparsable_bazel_module_version(self):
        self.run_command("scripts/bump-sdk-versions.sh", "0.8.1")
        (self.root / "MODULE.bazel").write_text(
            'module(\n    name = "meerkat_meta",\n)\n'
        )

        result = self.run_command(
            "scripts/verify-version-parity.sh",
            check=False,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "MODULE.bazel module version is missing or unparsable",
            result.stdout,
        )

    def test_version_parity_rejects_nested_lockfile_root_drift(self):
        self.run_command("scripts/bump-sdk-versions.sh", "0.8.1")
        lock_path = self.root / "sdk/typescript/package-lock.json"
        lock = json.loads(lock_path.read_text())
        lock["packages"][""]["version"] = "0.8.0"
        lock_path.write_text(json.dumps(lock, indent=2) + "\n")

        result = self.run_command(
            "scripts/verify-version-parity.sh",
            check=False,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            'package-lock.json version mismatch (top-level=0.8.1, packages[""]=0.8.0, expected=0.8.1)',
            result.stdout,
        )

    def test_version_parity_rejects_stale_rust_install_docs(self):
        self.run_command("scripts/bump-sdk-versions.sh", "0.8.1")
        (self.root / "docs/sdks/rust.mdx").write_text(
            'meerkat-meta = "0.7.39"\n'
        )

        result = self.run_command(
            "scripts/verify-version-parity.sh",
            check=False,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "docs/sdks/rust.mdx install version mismatch (0.7.39 != 0.8.1)",
            result.stdout,
        )


if __name__ == "__main__":
    unittest.main()
