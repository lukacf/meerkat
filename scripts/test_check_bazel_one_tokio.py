#!/usr/bin/env python3
"""Contract rows for scripts/check-bazel-one-tokio.sh.

The check must still refuse the defect it exists for: a release binary or
the meerkat-live test whose Rust closure holds two tokio libraries (one per
crate_universe repository). The rows feed it closures through its
`--closures` seam; no Bazel runs here.
"""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
CHECK = ROOT / "scripts" / "check-bazel-one-tokio.sh"
MAIN = "@@rules_rust++crate+crates__tokio-1.54.1//:tokio"
LIVE = "@@rules_rust++crate+meerkat_live_crates__tokio-1.54.1//:tokio"
OTHERS = [
    "@@rules_rust++crate+crates__tokio-util-0.7.20//:tokio_util",
    "@@rules_rust++crate+meerkat_live_crates__tokio-util-0.7.20//:tokio_util",
    "@@rules_rust++crate+crates__tokio-stream-0.1.20//:tokio_stream",
    "@@rules_rust++crate+meerkat_live_crates__webrtc-0.14.0//:webrtc",
    "//crates/meerkat-live:meerkat_live",
]


class OneTokioTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory(prefix="meerkat-one-tokio-")
        self.dir = pathlib.Path(self._tmp.name)
        self.targets = subprocess.run(
            [str(CHECK), "--print-targets"], capture_output=True, text=True, check=True
        ).stdout.split()

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def closures(self, per_target: dict[int, list[str]] | None = None) -> None:
        for index in range(1, len(self.targets) + 1):
            labels = (per_target or {}).get(index, [MAIN]) + OTHERS
            (self.dir / f"{index}.labels").write_text("\n".join(labels) + "\n", encoding="utf-8")

    def run_check(self) -> subprocess.CompletedProcess[str]:
        return subprocess.run([str(CHECK), "--closures", str(self.dir)], capture_output=True, text=True, check=False)

    def test_targets_are_the_release_roots_and_the_live_test(self) -> None:
        self.assertEqual(
            self.targets,
            [
                "//crates/meerkat-cli:rkat",
                "//crates/meerkat-rpc:rkat_rpc_bin",
                "//crates/meerkat-rest:rkat_rest_bin",
                "//crates/meerkat-mcp-server:rkat_mcp_bin",
                "//crates/meerkat-live:meerkat_live_unit_test",
            ],
        )

    def test_one_tokio_everywhere_passes(self) -> None:
        self.closures()
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("one tokio"), len(self.targets))

    def test_second_tokio_in_a_release_binary_fails(self) -> None:
        # The graph before the override: rkat_rpc_bin links the live stack,
        # which brought its own tokio.
        self.closures({2: [MAIN, LIVE]})
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("//crates/meerkat-rpc:rkat_rpc_bin links 2 tokio libraries", result.stderr)
        self.assertIn(LIVE, result.stderr)
        self.assertIn(MAIN, result.stderr)

    def test_second_tokio_in_the_live_test_fails(self) -> None:
        self.closures({len(self.targets): [MAIN, LIVE]})
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("//crates/meerkat-live:meerkat_live_unit_test links 2 tokio libraries", result.stderr)

    def test_closure_without_tokio_is_not_a_pass(self) -> None:
        self.closures({1: []})
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("//crates/meerkat-cli:rkat links no tokio library", result.stderr)

    def test_other_tokio_named_crates_do_not_count(self) -> None:
        # tokio-util and tokio-stream of both repositories are in every
        # closure above; only the tokio crate itself is counted.
        self.closures()
        self.assertEqual(self.run_check().returncode, 0)

    def test_missing_closure_is_an_error(self) -> None:
        self.closures()
        (self.dir / "3.labels").unlink()
        result = self.run_check()
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("no closure file", result.stderr)

    # The live repository's crates are compiled against the main one's
    # tokio: a different major version is refused from the lock.

    def checkout(self, lock: str) -> pathlib.Path:
        """A stand-in checkout: the real launcher and a lock of two lines."""
        root = self.dir / "checkout"
        (root / "scripts").mkdir(parents=True)
        shutil.copy(ROOT / "scripts" / "buildbuddy-bazel-poc", root / "scripts" / "buildbuddy-bazel-poc")
        (root / "MODULE.bazel.lock").write_text(lock, encoding="utf-8")
        return root

    def run_in(self, root: pathlib.Path) -> subprocess.CompletedProcess[str]:
        self.closures()
        return subprocess.run(
            [str(CHECK), "--root", str(root), "--closures", str(self.dir)], capture_output=True, text=True, check=False
        )

    def test_same_major_in_both_repositories_passes(self) -> None:
        result = self.run_in(self.checkout('"crates__tokio-1.54.1": {}\n"meerkat_live_crates__tokio-1.60.0": {}\n'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_different_major_in_the_two_repositories_fails(self) -> None:
        result = self.run_in(self.checkout('"crates__tokio-1.54.1": {}\n"meerkat_live_crates__tokio-2.0.0": {}\n'))
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("different major versions", result.stderr)

    def test_lock_without_both_tokio_repositories_is_an_error(self) -> None:
        result = self.run_in(self.checkout('"crates__tokio-1.54.1": {}\n'))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("does not render tokio for both", result.stderr)

    def test_repository_lock_renders_one_major(self) -> None:
        self.closures()
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
