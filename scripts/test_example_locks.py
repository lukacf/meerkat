#!/usr/bin/env python3
"""Offline contract checks using real Cargo resolution, without compilation."""

from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[1]
CARGO = REPO / "scripts/repo-cargo"
HELPER = REPO / "scripts/example-locks.py"


class ExampleLocksTests(unittest.TestCase):
    def run_command(self, command, root, expected=0):
        result = subprocess.run(
            command, cwd=root,
            env={**os.environ, "CARGO": str(CARGO), "RUST_LANE_ID": "example-lock-contract",
                 "CARGO_NET_OFFLINE": "true", "GITHUB_ACTIONS": "false"},
            text=True, capture_output=True, timeout=60, check=False,
        )
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result

    def test_release_drift_fails_gate_then_refreshes_and_stages_only_example_lock(self):
        with tempfile.TemporaryDirectory(prefix="example lock contract ") as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["shared"]\nresolver="2"\n')
            shared = root / "shared"
            (shared / "src").mkdir(parents=True)
            (shared / "src/lib.rs").write_text("")
            shared_manifest = shared / "Cargo.toml"
            manifest = '[package]\nname="example-shared"\nversion="{version}"\nedition="2021"\n'
            shared_manifest.write_text(manifest.format(version="0.1.0"))
            example = root / "examples/standalone with spaces"
            (example / "src").mkdir(parents=True)
            (example / "src/main.rs").write_text("fn main() {}\n")
            (example / "Cargo.toml").write_text(
                '[package]\nname="standalone-example"\nversion="0.1.0"\nedition="2021"\n'
                '[dependencies]\nexample-shared={path="../../shared"}\n[workspace]\n'
            )
            # A workspace-member-like manifest does not own an independent lock.
            member = root / "examples/member"
            member.mkdir()
            (member / "Cargo.toml").write_text(
                '[package]\nname="member"\nversion="0.1.0"\nedition="2021"\n'
            )
            helper = [sys.executable, str(HELPER)]
            root_metadata = [str(CARGO), "metadata", "--format-version", "1",
                             "--manifest-path", str(root / "Cargo.toml")]
            self.run_command(root_metadata, root)
            self.run_command([*helper, "refresh", "--root", str(root)], root)
            lock = example / "Cargo.lock"
            before = lock.read_bytes()
            self.run_command([*helper, "check", "--root", str(root)], root)
            shared_manifest.write_text(manifest.format(version="0.2.0"))
            self.run_command(root_metadata, root)
            root_lock = (root / "Cargo.lock").read_bytes()
            result = self.run_command(
                ["bash", str(REPO / "scripts/verify-lock-consistency.sh"), str(root)], root, 1,
            )
            self.assertIn("example lock check failed", result.stderr)
            self.assertEqual(lock.read_bytes(), before, "Checking must not heal a stale lock")
            self.run_command(["git", "init", "--quiet"], root)
            self.run_command([*helper, "refresh", "--stage", "--root", str(root)], root)
            self.assertNotEqual(lock.read_bytes(), before)
            self.assertEqual((root / "Cargo.lock").read_bytes(), root_lock)
            self.run_command([*helper, "check", "--root", str(root)], root)
            staged = self.run_command(["git", "diff", "--cached", "--name-only"], root)
            self.assertEqual(staged.stdout.strip(), "examples/standalone with spaces/Cargo.lock")
            lock.unlink()
            self.run_command([*helper, "check", "--root", str(root)], root, 1)
            self.assertFalse(lock.exists(), "A missing lock must not be silently created by check")

    def test_release_hook_routes_refresh_before_sentinel(self):
        hook = (REPO / "scripts/release-hook.sh").read_text()
        refresh = 'CARGO="$CARGO" "$PYTHON" "$ROOT/scripts/example-locks.py" refresh --stage --root "$ROOT"'
        self.assertEqual(hook.count(refresh), 1)
        self.assertLess(hook.index(refresh), hook.index('echo "$VERSION" > "$SENTINEL"'))


if __name__ == "__main__":
    unittest.main()
