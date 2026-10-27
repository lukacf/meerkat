"""Host-selected scratch directories must not change either observed tree."""
from pathlib import Path
import importlib.util
import os
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("readback_fixtures", Path(__file__).with_name("test_selected_reconciliation.py"))
R = importlib.util.module_from_spec(spec)
spec.loader.exec_module(R)
M = R.M


class SelectedScratchTests(unittest.TestCase):
    def fixture(self, python=True):
        helper = R.SelectedReadbackTests()
        self.addCleanup(helper.doCleanups)
        fixture, _ = helper.fixture(python)
        return fixture

    def test_host_scratch_is_used_and_removed_for_both_formats(self):
        for python in (True, False):
            fixture = self.fixture(python)
            with tempfile.TemporaryDirectory(prefix="toolkit host scratch ") as raw:
                scratch = Path(raw).resolve()
                calls = []

                def validator(argv, cwd):
                    self.assertEqual(cwd.parent, scratch)
                    self.assertEqual((cwd / "activation.json").read_bytes(), fixture.files["activation.json"])
                    self.assertEqual((cwd / "package.json").read_bytes(), fixture.files["package.json"])
                    calls.append(cwd)
                    return M.encoded(fixture.plan)

                before = R.snapshot(Path(fixture.args.destination))
                with patch.dict(os.environ, {"TMPDIR": str(scratch)}, clear=True), patch.object(M, "readback_validator", side_effect=validator):
                    self.assertTrue(M.verify_existing_selected(fixture.args)["verified"])
                self.assertEqual(len(calls), 1)
                self.assertFalse(calls[0].exists())
                self.assertEqual(list(scratch.iterdir()), [])
                self.assertEqual(R.snapshot(Path(fixture.args.destination)), before)

    def test_unset_scratch_keeps_canonical_system_default(self):
        fixture = self.fixture()

        def validator(argv, cwd):
            self.assertEqual(cwd.parent, Path("/tmp").resolve())
            return M.encoded(fixture.plan)

        with patch.dict(os.environ, {}, clear=True), patch.object(M, "readback_validator", side_effect=validator):
            self.assertTrue(M.verify_existing_selected(fixture.args)["verified"])

    def test_invalid_or_protected_scratch_refuses_before_creation(self):
        fixture = self.fixture()
        destination = Path(fixture.args.destination)
        with tempfile.TemporaryDirectory(prefix="toolkit scratch refusal ") as raw:
            root = Path(raw)
            ordinary_file = root / "ordinary-file"
            ordinary_file.write_text("not a directory")
            alias = root / "alias"
            alias.symlink_to(destination, target_is_directory=True)
            child = destination / "supplied-tmp"
            child.mkdir()
            values = ["", "relative", str(ordinary_file), str(root / "missing"),
                      str(destination), str(child), str(alias), str(fixture.bundle)]
            original = R.snapshot(fixture.bundle)
            before = R.snapshot(destination)
            for value in values:
                with self.subTest(value=value), patch.dict(os.environ, {"TMPDIR": value}, clear=True), \
                        patch.object(M.tempfile, "TemporaryDirectory") as create, patch.object(M, "readback_validator") as validator:
                    with self.assertRaises((M.InstallError, OSError)):
                        M.verify_existing_selected(fixture.args)
                    create.assert_not_called()
                    validator.assert_not_called()
                self.assertEqual(R.snapshot(fixture.bundle), original)
                self.assertEqual(R.snapshot(destination), before)


if __name__ == "__main__":
    unittest.main()
