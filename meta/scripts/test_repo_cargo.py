"""Verify cache-root selection without resolving dependencies or invoking Cargo."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class RepoCargoCacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.repo = self.root / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        shutil.copyfile(
            Path(__file__).with_name("repo-cargo"), self.repo / "scripts" / "repo-cargo"
        )
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith("GIT_")
            and key not in ("CARGO_HOME", "CARGO_TARGET_DIR", "RUST_LANE_ID")
        }
        self.env["HOME"] = str(self.root / "home")
        self.env["XDG_CACHE_HOME"] = str(self.root / "cache")
        self.env["RUST_LANE_ID"] = "fixture-lane"
        subprocess.run(["git", "init", "-q", str(self.repo)], env=self.env, check=True)

    def run_wrapper(self, **overrides):
        result = subprocess.run(
            ["bash", str(self.repo / "scripts" / "repo-cargo"), "--print-env"],
            env={**self.env, **overrides},
            capture_output=True,
            text=True,
            check=True,
        )
        values = dict(line.split("=", 1) for line in result.stdout.splitlines())
        for key in ("CARGO_HOME", "CARGO_TARGET_DIR"):
            path = Path(values[key])
            self.assertTrue(path.is_relative_to(self.root), values)
            self.assertTrue(path.is_dir())
        return values

    def test_defaults_remain_repo_and_lane_scoped(self):
        values = self.run_wrapper()
        self.assertTrue(values["CARGO_HOME"].endswith(
            f"/{values['repo_key']}/cargo-home"
        ))
        self.assertTrue(values["CARGO_TARGET_DIR"].endswith(
            f"/{values['repo_key']}/targets/fixture-lane"
        ))

    def test_explicit_roots_are_preserved_independently(self):
        home = str(self.root / "explicit-home")
        target = str(self.root / "explicit-target")
        for overrides in (
            {"CARGO_HOME": home},
            {"CARGO_TARGET_DIR": target},
            {"CARGO_HOME": home, "CARGO_TARGET_DIR": target},
        ):
            with self.subTest(overrides=overrides):
                values = self.run_wrapper(**overrides)
                for key, expected in overrides.items():
                    self.assertEqual(values[key], expected)

    def test_empty_overrides_use_defaults(self):
        expected = self.run_wrapper()
        self.assertEqual(
            self.run_wrapper(CARGO_HOME="", CARGO_TARGET_DIR=""), expected
        )

    def test_doctor_checks_the_explicit_roots(self):
        env = {
            **self.env,
            "CARGO_HOME": str(self.root / "doctor-home"),
            "CARGO_TARGET_DIR": str(self.root / "doctor-target"),
        }
        result = subprocess.run(
            ["bash", str(self.repo / "scripts" / "repo-cargo"), "--doctor"],
            env=env, capture_output=True, text=True, check=True,
        )
        self.assertIn("doctor=ok", result.stdout)
        self.assertIn(f"CARGO_HOME={env['CARGO_HOME']}", result.stdout)
        self.assertIn(f"CARGO_TARGET_DIR={env['CARGO_TARGET_DIR']}", result.stdout)


if __name__ == "__main__":
    unittest.main()
