#!/usr/bin/env python3
"""Run the actual packaging script without credentials or model calls."""

import os
from pathlib import Path
import shutil
import subprocess
import unittest
import uuid


HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent


class PackagingTests(unittest.TestCase):
    def test_signed_artifact_is_repeatable_and_does_not_execute_agents(self):
        output = subprocess.check_output(
            [REPO / "scripts/repo-cargo", "--print-env"], text=True, cwd=REPO
        )
        target = next(line.split("=", 1)[1] for line in output.splitlines()
                      if line.startswith("CARGO_TARGET_DIR="))
        binary = Path(os.environ.get("RKAT_TEST_BIN", f"{target}/debug/rkat")).resolve()
        self.assertTrue(binary.is_file(), "Build current rkat first, or set RKAT_TEST_BIN")
        work = HERE / ".work" / f"regression-{uuid.uuid4().hex}"
        work.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, work)
        example = work / "checkout with spaces"
        shutil.copytree(HERE, example, ignore=shutil.ignore_patterns(".work", "__pycache__"))
        home = work / "home"
        home.mkdir()
        env = {"PATH": os.environ["PATH"], "HOME": str(home), "RKAT": str(binary)}
        for _ in range(2):
            result = subprocess.run(["bash", example / "examples.sh"], cwd=REPO, env=env,
                                    capture_output=True, text=True, timeout=60)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("Packaging only: no members are spawned", result.stdout)
            self.assertIn("no incident prompt is executed", result.stdout)
            self.assertNotIn("deployed\t", result.stdout)
            self.assertGreater((example / ".work/release-triage.mobpack").stat().st_size, 0)
        roots = [item for name in ("state", "context", "user-config")
                 for item in (f"--{name}-root", str(example / ".work" /
                             {"state": "state", "context": "project", "user-config": "user"}[name]))]
        result = subprocess.run([binary, *roots, "session", "list"], cwd=REPO, env=env,
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("No sessions found", result.stdout)
        self.assertFalse((home / ".rkat/config.toml").exists())


if __name__ == "__main__":
    unittest.main()
