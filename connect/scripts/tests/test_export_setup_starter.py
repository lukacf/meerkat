"""Starter projection checks use inert inputs; the real starter is used only for help."""
import contextlib
import hashlib
import io
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("export_setup_starter", ROOT / "scripts/export-setup-starter.py")
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


class ExportSetupStarterTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="setup-starter-")
        self.addCleanup(temporary.cleanup)
        self.temporary = Path(temporary.name).resolve()
        self.root = self.temporary / "checkout"
        self.source = self.root / "sdk/consumer-setup"
        self.source.mkdir(parents=True)
        self.manifest = self.root / "publication/candidate.json"
        self.manifest.parent.mkdir()
        self.manifest.write_bytes(b'{"schema_version":1,"assets":[]}\n')
        self.pointer = {"schema_version": 1, "manifest": "../../publication/candidate.json",
                        "manifest_sha256": hashlib.sha256(self.manifest.read_bytes()).hexdigest()}
        self.selector = self.source / "current-distribution.json"
        self.selector.write_text(json.dumps(self.pointer))
        for name in M.SOURCE_FILES:
            (self.source / name).write_bytes(("# inert " + name + "\n").encode())
        self.destination = self.temporary / "starter"
        self.root_patch = patch.object(M, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def test_export_copies_exact_inputs_and_only_relocates_pointer(self):
        result = M.export(self.destination)
        expected = {*M.SOURCE_FILES, "release-manifest.json", "current-distribution.json", "README.md"}
        self.assertEqual({path.name for path in self.destination.iterdir()}, expected)
        for name in M.SOURCE_FILES:
            self.assertEqual((self.destination / name).read_bytes(), (self.source / name).read_bytes())
        self.assertEqual((self.destination / "release-manifest.json").read_bytes(), self.manifest.read_bytes())
        self.assertEqual(json.loads((self.destination / "current-distribution.json").read_bytes()),
                         dict(self.pointer, manifest="release-manifest.json"))
        self.assertEqual(result["manifest_sha256"], self.pointer["manifest_sha256"])
        self.assertIn("python3.11 start.py --archive-dir /ABS/downloads", (self.destination / "README.md").read_text())

    def test_explicit_manifest_export_leaves_ordinary_selection_unchanged(self):
        selected = self.root / "private-release.json"
        selected.write_bytes(b'{"schema_version":1,"assets":[{"upload_name":"toolkit-library-private-macos-arm64.tar.gz","bytes":7,"sha256":"' + b"a" * 64 + b'"}]}\n')
        selected_raw = selected.read_bytes()
        selected_sha = hashlib.sha256(selected_raw).hexdigest()
        original_pointer = self.selector.read_bytes()
        original_manifest = self.manifest.read_bytes()
        with contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(M.main(["--destination", str(self.destination),
                "--manifest", str(selected), "--manifest-sha256", selected_sha]), 0)
        self.assertEqual(json.loads(output.getvalue())["manifest_sha256"], selected_sha)
        self.assertEqual((self.destination / "release-manifest.json").read_bytes(), selected_raw)
        self.assertEqual(json.loads((self.destination / "current-distribution.json").read_bytes()),
                         {"schema_version": 1, "manifest": "release-manifest.json", "manifest_sha256": selected_sha})
        for name in M.SOURCE_FILES:
            self.assertEqual((self.destination / name).read_bytes(), (self.source / name).read_bytes())
        self.assertEqual(self.selector.read_bytes(), original_pointer)
        self.assertEqual(self.manifest.read_bytes(), original_manifest)
        ordinary = self.temporary / "ordinary-starter"
        M.export(ordinary)
        self.assertEqual((ordinary / "release-manifest.json").read_bytes(), original_manifest)
        self.assertEqual(json.loads((ordinary / "current-distribution.json").read_bytes()),
                         dict(self.pointer, manifest="release-manifest.json"))

    def test_explicit_manifest_selection_refuses_before_output(self):
        selected = self.root / "private-release.json"
        selected.write_bytes(b'{"schema_version":1,"assets":[]}\n')
        digest = hashlib.sha256(selected.read_bytes()).hexdigest()
        cases = [dict(manifest=selected), dict(manifest_sha256=digest),
                 dict(manifest="relative.json", manifest_sha256=digest),
                 dict(manifest=selected, manifest_sha256=""),
                 dict(manifest=selected, manifest_sha256="A" * 64),
                 dict(manifest=selected, manifest_sha256="0" * 64)]
        for selection in cases:
            with self.subTest(selection=selection), self.assertRaises(M.ExportError):
                M.export(self.destination, **selection)
            self.assertFalse(self.destination.exists())
        linked = self.root / "linked-manifest.json"
        linked.symlink_to(selected)
        with self.assertRaises(OSError):
            M.export(self.destination, manifest=linked, manifest_sha256=digest)
        self.assertFalse(self.destination.exists())

    def test_tampered_manifest_refuses_before_output(self):
        self.manifest.write_bytes(self.manifest.read_bytes() + b" ")
        with self.assertRaisesRegex(M.ExportError, "manifest hash mismatch"):
            M.export(self.destination)
        self.assertFalse(self.destination.exists())

    def test_occupied_destination_is_preserved(self):
        self.destination.mkdir()
        marker = self.destination / "keep"
        marker.write_bytes(b"retained output")
        with self.assertRaises(FileExistsError):
            M.export(self.destination)
        self.assertEqual({path.name for path in self.destination.iterdir()}, {"keep"})
        self.assertEqual(marker.read_bytes(), b"retained output")

    def test_source_symlinks_refuse_before_output(self):
        for path in (self.selector, self.manifest, *(self.source / name for name in M.SOURCE_FILES)):
            with self.subTest(path=path.name):
                retained = path.with_name(path.name + ".original")
                path.rename(retained)
                path.symlink_to(retained)
                try:
                    with self.assertRaises(OSError):
                        M.export(self.destination)
                    self.assertFalse(self.destination.exists())
                finally:
                    path.unlink()
                    retained.rename(path)

    def test_source_directory_symlink_refuses_before_output(self):
        real = self.root / "real-setup"
        self.source.rename(real)
        self.source.symlink_to(real, target_is_directory=True)
        with self.assertRaises(OSError):
            M.export(self.destination)
        self.assertFalse(self.destination.exists())

    def test_selector_shape_and_missing_source_refuse_before_output(self):
        variants = [[], dict(self.pointer, schema_version=True), dict(self.pointer, extra=True),
                    dict(self.pointer, manifest=""), dict(self.pointer, manifest=str(self.manifest)),
                    dict(self.pointer, manifest_sha256="A" * 64)]
        for value in variants:
            with self.subTest(value=value):
                self.selector.write_text(json.dumps(value))
                with self.assertRaises(M.ExportError):
                    M.export(self.destination)
                self.assertFalse(self.destination.exists())
        self.selector.write_text(json.dumps(self.pointer))
        (self.source / "prepare_host.py").unlink()
        with self.assertRaises(FileNotFoundError):
            M.export(self.destination)
        self.assertFalse(self.destination.exists())

    def test_partial_write_failure_preserves_completed_output(self):
        original = M.write_new

        def fail_second(directory, name, raw):
            if name == "bootstrap_host.py":
                raise OSError("injected write failure")
            original(directory, name, raw)

        with patch.object(M, "write_new", side_effect=fail_second), self.assertRaises(OSError):
            M.export(self.destination)
        self.assertEqual({path.name for path in self.destination.iterdir()}, {"start.py"})
        self.assertEqual((self.destination / "start.py").read_bytes(), (self.source / "start.py").read_bytes())
        with self.assertRaises(FileExistsError):
            M.export(self.destination)

    def test_relative_destination_refuses_before_output(self):
        with self.assertRaisesRegex(M.ExportError, "absolute"):
            M.export(Path("starter"))

    def test_real_export_runs_help_outside_checkout_without_archive(self):
        with patch.object(M, "ROOT", ROOT):
            result = M.export(self.destination)
        self.assertEqual(result["status"], "exported")
        completed = subprocess.run([sys.executable, "-I", "-B", str(self.destination / "start.py"), "--help"],
                                   cwd=self.destination, env={"PATH": os.defpath},
                                   capture_output=True, text=True, timeout=15)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn("--archive-dir", completed.stdout)
        self.assertIn("--destination", completed.stdout)
        self.assertEqual(len(list(self.destination.iterdir())), 6)


if __name__ == "__main__":
    unittest.main()
