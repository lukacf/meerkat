"""One-wrapper npm normalization, custody and retained compatibility controls.

Set TOOLKIT_NAMED_ROOT_ARCHIVE to the exact recorded public tarball to include
its inert byte checks. No test here installs or executes package payloads.
"""
from __future__ import annotations

import copy
import io
import json
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import test_selected_node_install as fixtures


MODULE = fixtures.MODULE
RETAINED_SHA256 = "ccb53979d69626fd35a1ed09390decbc5c2e4b96d8173cf1caa77e10da49c81d"


def wrapped(wrapper, name="@outside/server", version="1.0.0", payload=None,
            dependencies=None, root_directory=False):
    meta = {"name": name, "version": version}
    if dependencies:
        meta["dependencies"] = dependencies
    files = {"package.json": MODULE.encoded(meta),
             **(payload or {"dist/main.js": b"// Inert selected payload.\n"})}
    entries = [(wrapper + "/", b"", tarfile.DIRTYPE, "")] if root_directory else []
    entries += [(wrapper + "/" + path, data, tarfile.REGTYPE, "")
                for path, data in files.items()]
    return fixtures.tarball(entries), files


def closure(component, dependencies=()):
    """Supply explicit retained-byte lock rows; never infer identity from roots."""
    primary = MODULE.npm_archive(component)
    project = {"name": "named-root-test", "version": "1.0.0", "private": True,
               "dependencies": {primary["name"]: "file:./artifact.tgz"}}
    rows = {"": {"name": project["name"], "version": project["version"],
                 "dependencies": dict(project["dependencies"])},
            "node_modules/" + primary["name"]: {
                "version": primary["version"], "resolved": "file:artifact.tgz",
                "integrity": fixtures.integrity(component)}}
    files = {"artifact.tgz": component, "closure/package-json": MODULE.encoded(project)}
    primary_metadata = json.loads(primary["files"]["package.json"])
    if primary_metadata.get("dependencies"):
        rows["node_modules/" + primary["name"]]["dependencies"] = primary_metadata["dependencies"]
    for index, raw in enumerate(dependencies):
        archive = MODULE.npm_archive(raw)
        files["closure/dependency-" + str(index)] = raw
        rows["node_modules/" + archive["name"]] = {
            "version": archive["version"], "integrity": fixtures.integrity(raw),
            "resolved": "https://registry.npmjs.org/retained/-/dependency-" + str(index) + ".tgz"}
    files["closure/package-lock"] = MODULE.encoded({
        "name": project["name"], "version": project["version"],
        "lockfileVersion": 3, "packages": rows})
    return files


class NamedRootTests(unittest.TestCase):
    def test_single_wrapper_normalizes_only_paths_and_never_package_identity(self):
        for wrapper, explicit in (("package", False), ("aws-lambda", True),
                                  ("unrelated-wrapper", False), ("@wrapper", True)):
            with self.subTest(wrapper=wrapper):
                raw, expected = wrapped(wrapper, root_directory=explicit)
                actual = MODULE.npm_archive(raw)
                self.assertEqual(actual["files"], expected)
                self.assertEqual((actual["name"], actual["version"]), ("@outside/server", "1.0.0"))
                self.assertEqual(actual["integrity"], fixtures.integrity(raw))

    def test_multiple_or_case_variant_roots_and_root_files_refuse(self):
        meta = MODULE.encoded({"name": "server", "version": "1.0.0"})
        good = [("named/package.json", meta, tarfile.REGTYPE, "")]
        for extra in (("other/file", b"x", tarfile.REGTYPE, ""),
                      ("other/", b"", tarfile.DIRTYPE, ""),
                      ("Named/file", b"x", tarfile.REGTYPE, ""),
                      ("named", b"x", tarfile.REGTYPE, "")):
            for entries in (good + [extra], [extra] + good):
                with self.subTest(extra=extra, first=entries[0][0]), self.assertRaises(ValueError):
                    MODULE.npm_archive(fixtures.tarball(entries))
        with self.assertRaises(ValueError):
            MODULE.npm_archive(fixtures.tarball([("package.json", meta, tarfile.REGTYPE, "")]))

    def test_named_root_preserves_traversal_alias_and_bundled_payload_refusal(self):
        meta = MODULE.encoded({"name": "server", "version": "1.0.0"})
        for path in ("../named/file", "/named/file", "named/../file", "named/./file",
                     "named//file", "named\\file", "named/node_modules/child/index.js",
                     "named/package.json", "named/PACKAGE.JSON"):
            entries = [("named/package.json", meta, tarfile.REGTYPE, ""),
                       (path, b"x", tarfile.REGTYPE, "")]
            with self.subTest(path=path), self.assertRaises(ValueError):
                MODULE.npm_archive(fixtures.tarball(entries))

    def test_links_special_and_sparse_members_still_refuse(self):
        meta = MODULE.encoded({"name": "server", "version": "1.0.0"})
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE,
                     tarfile.CHRTYPE, tarfile.BLKTYPE, tarfile.GNUTYPE_SPARSE):
            entries = [("named/package.json", meta, tarfile.REGTYPE, ""),
                       ("named/unsafe", b"", kind, "named/package.json")]
            with self.subTest(kind=kind), self.assertRaises((ValueError, tarfile.TarError)):
                MODULE.npm_archive(fixtures.tarball(entries))

    def test_named_archive_preserves_expansion_member_and_file_limits(self):
        raw, _ = wrapped("named")
        for field, limit in (("MAX_NPM_EXPANDED", 100), ("MAX_NPM_MEMBERS", 1),
                             ("MAX_FILE", 1)):
            with self.subTest(field=field), patch.object(MODULE, field, limit), self.assertRaises(ValueError):
                MODULE.npm_archive(raw)

    def test_named_primary_and_dependency_share_exact_selected_installed_map(self):
        dependency, depfiles = wrapped("not-the-dependency-name", "dependency")
        component, primaryfiles = wrapped("not-the-primary-name", dependencies={"dependency": "1.0.0"})
        files = closure(component, [dependency])
        original = copy.deepcopy(files)
        graph = MODULE.selected_npm_closure(files, {"entrypoint": {"executable": "dist/main.js"}})
        self.assertEqual(files, original)
        self.assertEqual(graph["rows"]["node_modules/dependency"][1]["files"], depfiles)
        self.assertEqual(graph["rows"]["node_modules/@outside/server"][1]["files"], primaryfiles)
        with tempfile.TemporaryDirectory(prefix="named-root-readback-") as directory:
            destination = Path(directory).resolve()
            for path, (_, archive) in graph["rows"].items():
                for name, data in archive["files"].items():
                    target = destination / path / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(data)
            MODULE.verify_selected_npm(destination, graph)
            target = destination / "node_modules/dependency/dist/main.js"
            target.write_bytes(b"modified")
            with self.assertRaisesRegex(ValueError, "payload differs"):
                MODULE.verify_selected_npm(destination, graph)
            target.write_bytes(depfiles["dist/main.js"])
            (target.parent / "extra.js").write_bytes(b"extra")
            with self.assertRaisesRegex(ValueError, "file graph differs"):
                MODULE.verify_selected_npm(destination, graph)

    def test_wrapper_does_not_override_lock_identity_version_or_integrity(self):
        component, _ = wrapped("dependency", dependencies={"dependency": "1.0.0"})
        dependency, _ = wrapped("server", "dependency")
        files = closure(component, [dependency])
        for field, value in (("version", "9.0.0"), ("integrity", fixtures.integrity(b"wrong"))):
            lock = json.loads(files["closure/package-lock"])
            lock["packages"]["node_modules/dependency"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                MODULE.npm_archive_closure({**files, "closure/package-lock": MODULE.encoded(lock)})
        lock = json.loads(files["closure/package-lock"])
        lock["packages"]["node_modules/server"] = lock["packages"].pop("node_modules/dependency")
        with self.assertRaisesRegex(ValueError, "name/version differs"):
            MODULE.npm_archive_closure({**files, "closure/package-lock": MODULE.encoded(lock)})


@unittest.skipUnless(os.environ.get("TOOLKIT_NAMED_ROOT_ARCHIVE"), "exact retained public archive not supplied")
class RetainedNamedRootTests(unittest.TestCase):
    def setUp(self):
        self.raw = Path(os.environ["TOOLKIT_NAMED_ROOT_ARCHIVE"]).read_bytes()
        self.assertEqual(MODULE.sha(self.raw), RETAINED_SHA256)

    def test_recorded_archive_keeps_all_exact_files_under_its_metadata_identity(self):
        with tarfile.open(fileobj=io.BytesIO(self.raw), mode="r:gz") as archive:
            expected = {m.name[len("aws-lambda/"):]: archive.extractfile(m).read()
                        for m in archive if m.isfile()}
        parsed = MODULE.npm_archive(self.raw)
        self.assertEqual((parsed["name"], parsed["version"]), ("@types/aws-lambda", "8.10.164"))
        self.assertEqual(len(expected), 65)
        self.assertEqual(parsed["files"], expected)
        graph = MODULE.npm_archive_closure(closure(self.raw))
        self.assertEqual(graph["component_path"], "node_modules/@types/aws-lambda")
        self.assertEqual(graph["rows"][graph["component_path"]][1]["files"], expected)

    def test_recorded_archive_is_accepted_as_an_exact_pinned_dependency(self):
        component, _ = wrapped("component", dependencies={"@types/aws-lambda": "8.10.164"})
        files = closure(component, [self.raw])
        graph = MODULE.selected_npm_closure(files, {"entrypoint": {"executable": "dist/main.js"}})
        self.assertEqual(set(graph["rows"]), {"node_modules/@outside/server", "node_modules/@types/aws-lambda"})
        self.assertEqual(graph["rows"]["node_modules/@types/aws-lambda"][1]["integrity"], fixtures.integrity(self.raw))


if __name__ == "__main__":
    unittest.main()
