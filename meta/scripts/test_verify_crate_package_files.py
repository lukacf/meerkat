#!/usr/bin/env python3
"""Behavioral tests for the crate package-files gate.

Each test builds a workspace and a fake cargo whose `package --list` output
carries the defect the gate must catch: a missing license file, a file present
only below the crate root, or a listing cargo cannot produce.
"""

import os
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
GATE = SCRIPTS / "verify-crate-package-files.py"
PUBLICATION_GATE = SCRIPTS / "verify-crate-publication.py"

ALL_FILES = "LICENSE-APACHE\nLICENSE-MIT\n"


def build_workspace(root: Path, listing: str, exit_code: int = 0) -> Path:
    (root / "Cargo.toml").write_text('[workspace]\nmembers = ["alpha"]\n')
    (root / "alpha").mkdir()
    (root / "alpha/Cargo.toml").write_text('[package]\nname = "alpha"\nversion = "0.0.0"\n')
    workflows = root / ".github/workflows"
    workflows.mkdir(parents=True)
    (workflows / "release.yml").write_text("  run: cargo publish -p alpha --locked\n")
    scripts = root / "scripts"
    scripts.mkdir()
    for gate in (GATE, PUBLICATION_GATE):
        (scripts / gate.name).write_text(gate.read_text())
    cargo = root / "fake-cargo"
    cargo.write_text(
        textwrap.dedent(
            f"""\
            #!/usr/bin/env bash
            printf 'Cargo.toml\\nsrc/lib.rs\\n'
            printf '%b' {listing!r}
            if [ {exit_code} -ne 0 ]; then echo "error: could not list" >&2; fi
            exit {exit_code}
            """
        )
    )
    cargo.chmod(cargo.stat().st_mode | stat.S_IEXEC)
    return cargo


class CratePackageFilesGateTests(unittest.TestCase):
    def gate(self, listing: str, exit_code: int = 0):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        cargo = build_workspace(root, listing, exit_code)
        return subprocess.run(
            [sys.executable, str(root / "scripts" / GATE.name)],
            capture_output=True,
            text=True,
            env={**os.environ, "CARGO": str(cargo)},
        )

    def test_passes_when_every_file_is_packaged(self):
        result = self.gate(ALL_FILES)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_fails_naming_a_missing_license_file(self):
        result = self.gate(ALL_FILES.replace("LICENSE-APACHE\n", ""))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("alpha: package lacks LICENSE-APACHE", result.stderr)

    def test_fails_naming_every_missing_file(self):
        result = self.gate("README.md\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("alpha: package lacks LICENSE-MIT, LICENSE-APACHE", result.stderr)

    def test_files_below_the_crate_root_do_not_count(self):
        nested = "".join(f"docs/{line}\n" for line in ALL_FILES.splitlines())
        result = self.gate(nested)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("package lacks LICENSE-MIT", result.stderr)

    def test_fails_closed_when_cargo_cannot_list(self):
        result = self.gate(ALL_FILES, exit_code=101)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cargo package --list failed", result.stderr)


if __name__ == "__main__":
    unittest.main()
