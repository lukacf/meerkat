"""Archive selection stays bound across pathname changes; inert fixtures only."""
from __future__ import annotations

from contextlib import ExitStack
import copy
import hashlib
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock

import test_package_connect_resources as shared

PACKAGE = shared.PACKAGE


class ArchiveCaptureTests(unittest.TestCase):
    def test_verified_copy_survives_original_replacement_and_is_removed_on_exit(self):
        for failed_consumer in (False, True):
            with self.subTest(failed_consumer=failed_consumer), tempfile.TemporaryDirectory() as scratch:
                original = Path(scratch) / "archive.tar"
                raw = b"selected inert bytes"
                original.write_bytes(raw)
                seen = []
                try:
                    with PACKAGE.captured_archive(original, hashlib.sha256(raw).hexdigest(), len(raw)) as captured:
                        seen.append(captured)
                        original.unlink()
                        original.symlink_to("missing-replacement")
                        self.assertEqual(captured.read_bytes(), raw)
                        self.assertNotEqual(captured, original)
                        self.assertEqual(captured.stat().st_mode & 0o777, 0o400)
                        if failed_consumer:
                            raise RuntimeError("inert consumer failure")
                except RuntimeError as error:
                    self.assertTrue(failed_consumer)
                    self.assertEqual(str(error), "inert consumer failure")
                self.assertEqual(len(seen), 1)
                self.assertFalse(seen[0].parent.exists())

    def test_changed_or_nonregular_input_never_reaches_consumer(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            original = root / "archive"
            raw = b"selected"
            original.write_bytes(raw)
            link = root / "link"; link.symlink_to(original)
            fifo = root / "fifo"; os.mkfifo(fifo)
            entered = []
            cases = [(original, "0" * 64, len(raw)), (original, hashlib.sha256(raw).hexdigest(), len(raw) + 1),
                     (link, hashlib.sha256(raw).hexdigest(), len(raw)), (fifo, "0" * 64, 8),
                     (root, "0" * 64, 8)]
            for path, sha, size in cases:
                with self.subTest(path=path, sha=sha, size=size):
                    with self.assertRaises((OSError, ValueError)):
                        with PACKAGE.captured_archive(path, sha, size):
                            entered.append(path)
            self.assertEqual(entered, [])

    def test_invalid_bounds_refuse_before_input_open(self):
        for size in (False, 0, -1, 512 * 1024 * 1024 + 1):
            with self.subTest(size=size), mock.patch.object(PACKAGE.os, "open") as opened:
                with self.assertRaisesRegex(ValueError, "capture bound"):
                    with PACKAGE.captured_archive(Path("/inert/not-opened"), "0" * 64, size):
                        self.fail("invalid bound reached consumer")
                opened.assert_not_called()

    def test_in_place_changes_during_streaming_never_reach_consumer(self):
        for change in ("rewrite", "grow", "truncate"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as scratch:
                original = Path(scratch) / "archive"
                raw = b"a" * (2 * 1024 * 1024)
                original.write_bytes(raw)
                fdopen = PACKAGE.os.fdopen

                class ChangedStream:
                    def __init__(self, stream):
                        self.stream, self.changed = stream, False

                    def __enter__(self):
                        return self

                    def __exit__(self, *args):
                        self.stream.close()

                    def fileno(self):
                        return self.stream.fileno()

                    def read(self, size):
                        block = self.stream.read(size)
                        if not self.changed:
                            self.changed = True
                            with original.open("r+b") as writer:
                                if change == "truncate":
                                    writer.truncate(1024 * 1024)
                                elif change == "grow":
                                    writer.seek(0, 2); writer.write(b"extra")
                                else:
                                    writer.seek(1024 * 1024); writer.write(b"b" * (1024 * 1024))
                        return block

                with mock.patch.object(PACKAGE.os, "fdopen", side_effect=lambda *a, **kw: ChangedStream(fdopen(*a, **kw))):
                    with self.assertRaisesRegex(ValueError, "archive capture"):
                        with PACKAGE.captured_archive(original, hashlib.sha256(raw).hexdigest(), len(raw)):
                            self.fail("changed bytes reached consumer")


class PackagingCaptureTests(unittest.TestCase):
    # Reuse only fixture construction, without inheriting or rerunning its tests.
    setUp = shared.ExplicitResourceSelectionTests.setUp
    repin_manifest = shared.ExplicitResourceSelectionTests.repin_manifest
    kwargs = shared.ExplicitResourceSelectionTests.kwargs

    def arrange(self, *, legacy=False):
        self.upstream = self.root / "upstream"; self.upstream.mkdir()
        self.selected = copy.deepcopy(PACKAGE.TARGETS)
        self.inputs = {}
        for kind in ("python", "node"):
            name = self.selected["macos-arm64"][kind][0]
            path = self.upstream / name
            raw = ("selected inert " + kind).encode()
            path.write_bytes(raw)
            self.selected["macos-arm64"][kind] = (name, hashlib.sha256(raw).hexdigest(), len(raw))
            self.inputs[kind] = (path, raw)
        self.pip = self.upstream / "pip.pyz"; self.pip.write_bytes(b"inert pip")
        self.destination = self.root / "output"
        self.build_args = self.kwargs()
        self.starters = PACKAGE.STARTERS
        if legacy:
            directory = self.root / "dist/legacy-library"; directory.mkdir(parents=True)
            moved = directory / self.archive.name
            shutil.copyfile(self.archive, moved); self.archive = moved
            self.selected["macos-arm64"].update(
                library=(moved.name, PACKAGE.digest(moved), moved.stat().st_size),
                library_dir="legacy-library")
            export = self.selected["macos-arm64"]["starter"]
            starter = self.root / "dist" / export
            shutil.copytree(self.starter, starter); self.starter = starter
            self.starters = {export: {p.name: PACKAGE.digest(p) for p in starter.iterdir()}}
            self.build_args = {}
        self.inspected = []

    def decompressor(self, argv, **kwargs):
        self.assertEqual(argv[:3], ["/inert/zstd", "-d", "-c"])
        path = Path(argv[3])
        self.assertEqual(path.read_bytes(), self.inputs["python"][1])
        self.inspected.append(path)
        kwargs["stdout"].write(b"inert decompressed marker")

    def unpack(self, archive, resources, mapping, **kwargs):
        if archive.name == "python.tar":
            self.assertEqual(archive.read_bytes(), b"inert decompressed marker")
            shared.RuntimeCompatibilityTests().resources(resources, "macos-arm64", node="24.19.0", npm="11.17.0")
            cli = resources / "runtime/node/lib/node_modules/npm/bin/npm-cli.js"
            cli.parent.mkdir(parents=True); cli.write_bytes(b"inert npm cli")
        else:
            self.assertEqual(archive.read_bytes(), self.inputs["node"][1])
            self.inspected.append(archive)

    def patches(self, *, decompressor=None, unpack=None):
        stack = ExitStack()
        stack.enter_context(mock.patch.object(PACKAGE, "TARGETS", self.selected))
        stack.enter_context(mock.patch.object(PACKAGE, "STARTERS", self.starters))
        stack.enter_context(mock.patch.object(PACKAGE, "PIP",
            (self.pip.name, PACKAGE.digest(self.pip), self.pip.stat().st_size)))
        stack.enter_context(mock.patch.object(PACKAGE.subprocess, "run", side_effect=decompressor or self.decompressor))
        stack.enter_context(mock.patch.object(PACKAGE, "extract", side_effect=unpack or self.unpack))
        return stack

    def build(self):
        return PACKAGE.build(self.destination, self.upstream, Path("/inert/zstd"), **self.build_args)

    def assert_verified(self, result):
        manifest, sha = shared.PREPARE.verify_resources(Path(result["resources"]))
        self.assertEqual(sha, result["resources_sha256"])
        for kind in ("python", "node"):
            name, expected_sha, size = self.selected["macos-arm64"][kind]
            source = next(row for row in manifest["sources"] if row["name"] == name)
            self.assertEqual((source["sha256"], source["size"]), (expected_sha, size))
        self.assertEqual(len(self.inspected), 2)
        self.assertTrue(all(not path.exists() for path in self.inspected), "private captures are removed")

    def test_python_decompression_reads_verified_capture_after_original_changes(self):
        self.arrange()
        def decompress(argv, **kwargs):
            self.inputs["python"][0].write_bytes(b"substituted python")
            return self.decompressor(argv, **kwargs)
        with self.patches(decompressor=decompress):
            result = self.build()
        self.assert_verified(result)

    def test_node_extraction_reads_verified_capture_after_original_changes(self):
        self.arrange()
        def unpack(archive, resources, mapping, **kwargs):
            if archive.name != "python.tar":
                self.inputs["node"][0].write_bytes(b"substituted node")
            return self.unpack(archive, resources, mapping, **kwargs)
        with self.patches(unpack=unpack):
            result = self.build()
        self.assert_verified(result)

    def test_changed_input_after_initial_pin_refuses_before_decompression(self):
        self.arrange()
        original = PACKAGE.runpy.run_path
        def changed(path):
            self.inputs["python"][0].write_bytes(b"changed after preflight")
            return original(path)
        with self.patches(), mock.patch.object(PACKAGE.runpy, "run_path", side_effect=changed):
            with self.assertRaisesRegex(ValueError, "archive capture"):
                self.build()
            PACKAGE.subprocess.run.assert_not_called()
            PACKAGE.extract.assert_not_called()
        self.assertFalse((self.destination / "build.json").exists())
        self.assertFalse(list(self.destination.glob("*.tar.gz")))

    def test_legacy_library_inspection_and_copy_keep_the_same_selected_bytes(self):
        self.arrange(legacy=True)
        original = self.archive.read_bytes()
        inspect = PACKAGE.runtime_compatibility
        def changed(archive, *args, **kwargs):
            shared.RuntimeCompatibilityTests().library(self.archive, {"schema_version": 3})
            self.assertEqual(archive.read_bytes(), original)
            return inspect(archive, *args, **kwargs)
        with self.patches(), mock.patch.object(PACKAGE, "runtime_compatibility", side_effect=changed):
            result = self.build()
        self.assert_verified(result)
        resources = Path(result["resources"])
        manifest, _ = shared.PREPARE.verify_resources(resources)
        self.assertEqual((resources / manifest["archive"]).read_bytes(), original)
        self.assertEqual(result["runtime_compatibility"]["library_schema_version"], 1)

    def test_legacy_starter_copy_is_rechecked_before_success(self):
        self.arrange(legacy=True)
        copyfile = PACKAGE.shutil.copyfile
        def changed(source, destination, *args, **kwargs):
            if source == self.starter / "start.py":
                source.write_bytes(b"changed legacy starter")
            return copyfile(source, destination, *args, **kwargs)
        with self.patches(), mock.patch.object(PACKAGE.shutil, "copyfile", side_effect=changed):
            with self.assertRaisesRegex(ValueError, "packaging input does not match"):
                self.build()
        self.assertFalse((self.destination / "build.json").exists())
        self.assertFalse(list(self.destination.glob("*.tar.gz")))


if __name__ == "__main__":
    unittest.main()
