"""Inert archive preflight tests; no runtime packaging or execution."""
from __future__ import annotations

import importlib.util
import io
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    "connect_extraction_package", Path(__file__).with_name("package-connect-resources.py"))
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)


class ExtractionPreflightTests(unittest.TestCase):
    def fixture(self, parent, entries):
        # macOS /var is an alias for /private/var. Use the same canonical root
        # as the production packager so old-source comparisons test link paths.
        parent = parent.resolve()
        archive = parent / "inert.tar"
        with tarfile.open(archive, "w") as source:
            for name, kind, content in entries:
                member = tarfile.TarInfo("bundle/" + name)
                member.mode = 0o644
                if kind == "link":
                    member.type = tarfile.SYMTYPE
                    member.linkname = content
                    source.addfile(member)
                elif kind == "directory":
                    member.type = tarfile.DIRTYPE
                    source.addfile(member)
                else:
                    member.size = len(content)
                    source.addfile(member, io.BytesIO(content))
        root = parent / "resources"
        root.mkdir()
        return archive, root

    def extract(self, archive, root, **kwargs):
        PACKAGE.extract(archive, root, lambda name: name.removeprefix("bundle/"), **kwargs)

    def test_final_member_cannot_exceed_single_or_aggregate_limit(self):
        for entries in ([("only", "file", b"123456789")],
                        [("first", "file", b"1234"), ("last", "file", b"56789")]):
            for target in ("macos-arm64", "linux-x86_64"):
                with self.subTest(entries=entries, target=target), tempfile.TemporaryDirectory() as scratch:
                    archive, root = self.fixture(Path(scratch), entries)
                    with mock.patch.object(PACKAGE, "MAX_RUNTIME_ARCHIVE_BYTES", 8):
                        with self.assertRaisesRegex(ValueError, "size limit"):
                            self.extract(archive, root, target=target)
                    self.assertEqual(list(root.iterdir()), [], "refuse before the first write")

    def test_exact_byte_and_entry_limits_allow_a_trailing_empty_file(self):
        with tempfile.TemporaryDirectory() as scratch:
            archive, root = self.fixture(Path(scratch), [
                ("first", "file", b"1234"), ("last", "file", b"5678"), ("empty", "file", b"")])
            with mock.patch.object(PACKAGE, "MAX_RUNTIME_ARCHIVE_BYTES", 8), \
                    mock.patch.object(PACKAGE, "MAX_RUNTIME_ARCHIVE_ENTRIES", 3):
                self.extract(archive, root)
            self.assertEqual({p.name: p.read_bytes() for p in root.iterdir()},
                             {"first": b"1234", "last": b"5678", "empty": b""})

    def test_extra_empty_entry_is_refused_before_output(self):
        with tempfile.TemporaryDirectory() as scratch:
            archive, root = self.fixture(Path(scratch), [
                ("one", "file", b""), ("two", "file", b""), ("three", "file", b"")])
            with mock.patch.object(PACKAGE, "MAX_RUNTIME_ARCHIVE_ENTRIES", 2):
                with self.assertRaisesRegex(ValueError, "size limit"):
                    self.extract(archive, root)
            self.assertEqual(list(root.iterdir()), [])

    def test_real_oversized_header_is_refused_without_reading_payload(self):
        with tempfile.TemporaryDirectory() as scratch:
            parent = Path(scratch)
            member = tarfile.TarInfo("bundle/oversized")
            member.size = 1024 * 1024 * 1024 + 1
            archive = parent / "oversized.tar"
            archive.write_bytes(member.tobuf() + b"\0" * 1024)
            root = parent / "resources"
            root.mkdir()
            with self.assertRaisesRegex(ValueError, "size limit"):
                self.extract(archive, root)
            self.assertEqual(list(root.iterdir()), [])

    def test_alias_duplicates_are_refused_before_output(self):
        for alias in ("./same", "dir//same", "dir/./same"):
            canonical = "same" if alias == "./same" else "dir/same"
            with self.subTest(alias=alias), tempfile.TemporaryDirectory() as scratch:
                archive, root = self.fixture(Path(scratch), [
                    (canonical, "file", b"first"), (alias, "file", b"last")])
                with self.assertRaisesRegex(ValueError, "path collision"):
                    self.extract(archive, root)
                self.assertEqual(list(root.iterdir()), [])

    def test_mapped_paths_cannot_escape_or_replace_root(self):
        for mapped in ("", ".", "./", "../outside", "/outside", "dir/../outside",
                       "dir\\outside", "nul\0name"):
            with self.subTest(mapped=mapped), tempfile.TemporaryDirectory() as scratch:
                parent = Path(scratch)
                archive, root = self.fixture(parent, [("input", "file", b"inert")])
                with self.assertRaisesRegex(ValueError, "invalid mapped archive path"):
                    PACKAGE.extract(archive, root, lambda _: mapped)
                self.assertEqual(list(root.iterdir()), [])
                self.assertFalse((parent / "outside").exists())

    def test_file_and_link_parents_refuse_in_either_archive_order(self):
        for kind, content in (("file", b"inert"), ("link", "elsewhere")):
            for parent, child in (("dir", "dir/child"), ("DIR", "dir/child"),
                                  ("\u00e9", "e\u0301/child")):
                entries = [(parent, kind, content), (child, "file", b"child")]
                for order in (entries, entries[::-1]):
                    with self.subTest(kind=kind, parent=parent, order=order), tempfile.TemporaryDirectory() as scratch:
                        archive, root = self.fixture(Path(scratch), order)
                        with self.assertRaisesRegex(ValueError, "non-directory"):
                            self.extract(archive, root)
                        self.assertEqual(list(root.iterdir()), [])

    def test_existing_parent_link_cannot_redirect_writes(self):
        with tempfile.TemporaryDirectory() as scratch:
            parent = Path(scratch)
            archive, root = self.fixture(parent, [("dir/child", "file", b"inert")])
            outside = parent / "outside"
            outside.mkdir()
            (root / "dir").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "non-directory"):
                self.extract(archive, root)
            self.assertEqual(list(outside.iterdir()), [])
            self.assertEqual(os.readlink(root / "dir"), str(outside))

    def test_existing_file_conflict_does_not_write_earlier_entries(self):
        with tempfile.TemporaryDirectory() as scratch:
            archive, root = self.fixture(Path(scratch), [
                ("first", "file", b"new"), ("existing", "file", b"replacement")])
            (root / "existing").write_bytes(b"retained")
            with self.assertRaisesRegex(ValueError, "existing resources"):
                self.extract(archive, root)
            self.assertEqual({p.name: p.read_bytes() for p in root.iterdir()}, {"existing": b"retained"})

    def test_planned_link_chain_cannot_escape_before_links_exist(self):
        for spelling in ("deep/up", "deep/UP"):
            for target in ("macos-arm64", "linux-x86_64"):
                if spelling.endswith("UP") and target == "linux-x86_64":
                    continue  # Distinct Linux paths are covered by the existing target tests.
                entries = [("deep/up", "link", "."), ("escape", "link", spelling + "/../../outside")]
                for order in (entries, entries[::-1]):
                    with self.subTest(spelling=spelling, target=target, order=order), tempfile.TemporaryDirectory() as scratch:
                        archive, root = self.fixture(Path(scratch), order)
                        with self.assertRaisesRegex(ValueError, "link escapes"):
                            self.extract(archive, root, target=target)
                        self.assertEqual(list(root.iterdir()), [])

    def test_link_cycles_are_refused_before_any_output(self):
        for entries in ([("self", "link", "self")],
                        [("one", "link", "two"), ("two", "link", "one")]):
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as scratch:
                archive, root = self.fixture(Path(scratch), entries)
                with self.assertRaisesRegex(ValueError, "link cycle or depth limit"):
                    self.extract(archive, root)
                self.assertEqual(list(root.iterdir()), [])

    def test_link_depth_boundary_is_checked_before_output(self):
        for links, accepted in ((41, True), (42, False)):
            with self.subTest(links=links), tempfile.TemporaryDirectory() as scratch:
                entries = [(f"link{index}", "link",
                            f"link{index + 1}" if index + 1 < links else "payload")
                           for index in range(links)]
                entries.append(("payload", "file", b"inert"))
                archive, root = self.fixture(Path(scratch), entries)
                if accepted:
                    self.extract(archive, root)
                    self.assertEqual(len(list(root.iterdir())), links + 1)
                    self.assertEqual(os.readlink(root / "link0"), "link1")
                else:
                    with self.assertRaisesRegex(ValueError, "link cycle or depth limit"):
                        self.extract(archive, root)
                    self.assertEqual(list(root.iterdir()), [])

    def test_unselected_files_do_not_spend_the_mapped_content_budget(self):
        with tempfile.TemporaryDirectory() as scratch:
            archive, root = self.fixture(Path(scratch), [
                ("skip", "file", b"unselected bytes"), ("selected", "file", b"12345678")])
            with mock.patch.object(PACKAGE, "MAX_RUNTIME_ARCHIVE_BYTES", 8), \
                    mock.patch.object(PACKAGE, "MAX_RUNTIME_ARCHIVE_ENTRIES", 1):
                PACKAGE.extract(archive, root, lambda name: "selected" if name.endswith("/selected") else None)
            self.assertEqual({p.name: p.read_bytes() for p in root.iterdir()}, {"selected": b"12345678"})

    def test_safe_reverse_order_links_and_shared_directories_are_preserved(self):
        with tempfile.TemporaryDirectory() as scratch:
            archive, root = self.fixture(Path(scratch), [
                ("alias", "link", "lib/link"), ("lib/link", "link", "./payload"),
                ("lib/payload", "file", b"exact inert bytes"), ("lib", "directory", None),
                ("directory-link", "link", "lib"), ("optional", "link", "absent")])
            (root / "lib").mkdir()
            (root / "lib/retained").write_bytes(b"retained")
            self.extract(archive, root)
            self.assertEqual((root / "alias").read_bytes(), b"exact inert bytes")
            self.assertEqual((root / "directory-link/payload").read_bytes(), b"exact inert bytes")
            self.assertEqual((root / "lib/retained").read_bytes(), b"retained")
            self.assertEqual(os.readlink(root / "lib/link"), "./payload")
            self.assertEqual(os.readlink(root / "optional"), "absent")


if __name__ == "__main__":
    unittest.main()
