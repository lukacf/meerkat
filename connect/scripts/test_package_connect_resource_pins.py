"""Pinned public inputs and the macOS library contract; inert fixtures only.

The pin file and the fetched directory are another step's output: a committed
list of public files (kind, file, digest) and one directory per kind. These
tests never fetch, and never package or execute a real runtime.
"""
from __future__ import annotations

import base64
import contextlib
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import tempfile
import unittest
from unittest import mock

import test_package_connect_resources as shared

PACKAGE = shared.PACKAGE
PREPARE = shared.PREPARE
CHECK_SCRIPT = Path(__file__).with_name("check-connect-resource-pins.py")


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def sha512(raw):
    return hashlib.sha512(raw).hexdigest()


class PinnedFixture(unittest.TestCase):
    """One explicit macOS selection plus a fetched directory and its pin file."""
    # Reuse only fixture construction, without inheriting or rerunning its tests.
    setUp = shared.ExplicitResourceSelectionTests.setUp
    repin_manifest = shared.ExplicitResourceSelectionTests.repin_manifest
    kwargs = shared.ExplicitResourceSelectionTests.kwargs
    npm_inputs = shared.ExplicitResourceSelectionTests.npm_inputs

    def arrange(self):
        acquisition = self.npm_inputs()
        self.directory = self.root / "pins"
        (self.directory / "release-download").mkdir(parents=True)
        (self.directory / "npm-tarball").mkdir()
        self.selected = copy.deepcopy(PACKAGE.TARGETS)
        row = self.selected["macos-arm64"]
        self.files = {
            "python": self.directory / "release-download" / row["python"][0],
            "node": self.directory / "release-download" / row["node"][0],
            "pip": self.directory / "release-download" / "pip-26.2.1.pyz",
            "npm": self.directory / "npm-tarball" / "npm-12.2.0.tgz",
        }
        self.files["python"].write_bytes(b"inert selected python archive")
        # The acquisition fixture is a real inert Node tree; the same archive
        # is the runtime Node of this target.
        shutil.copyfile(acquisition["acquisition_node_archive"], self.files["node"])
        self.files["pip"].write_bytes(b"inert pip source")
        shutil.copyfile(acquisition["npm_archive"], self.files["npm"])
        for kind in ("python", "node"):
            raw = self.files[kind].read_bytes()
            row[kind] = (row[kind][0], sha256(raw), len(raw))
        pip, npm = self.files["pip"].read_bytes(), self.files["npm"].read_bytes()
        self.pip = ("pip.pyz", sha256(pip), len(pip))
        self.npm = ("npm-12.2.0.tgz", sha256(npm), len(npm), sha512(npm))
        self.rows = {
            role: {"kind": path.parent.name, "name": role, "version": "fixture", "file": path.name,
                   "url": "https://example.invalid/" + path.name,
                   "sha256": None if role == "npm" else sha256(path.read_bytes()),
                   "sha512": sha512(path.read_bytes()) if role == "npm" else None,
                   "digest_source": "fixture", "use": "fixture " + role}
            for role, path in self.files.items()}
        self.other = {"kind": "pypi-wheel", "name": "unrelated", "version": "1", "file": "unrelated-1-py3-none-any.whl",
                      "url": "https://example.invalid/unrelated", "sha256": "1" * 64, "sha512": None,
                      "digest_source": "fixture", "use": "another reader"}
        self.pins = self.root / "library-pins-macos-arm64.json"
        self.write_pins()
        self.destination = self.root / "output"

    def write_pins(self, rows=None, **document):
        value = {"schema": "connect-library-pins/1", "target": "macos-arm64",
                 "pins": [self.other, *self.rows.values()] if rows is None else rows} | document
        self.pins.write_text(json.dumps(value, indent=2) + "\n")

    def unpack(self, archive, resources, mapping, **kwargs):
        if archive.name == "python.tar":
            shared.RuntimeCompatibilityTests().resources(resources, "macos-arm64", node="24.19.0", npm="11.17.0")
            cli = resources / "runtime/node/lib/node_modules/npm/bin/npm-cli.js"
            cli.parent.mkdir(parents=True)
            cli.write_bytes(b"// inert library npm cli")
            # One link to a file and one to a directory, as the real runtimes have.
            (resources / "runtime/python/bin/python3").symlink_to("python3.11")
            (resources / "runtime/python/current").symlink_to("bin")
        elif "acquisition-unpack-" in str(archive.parent):
            self.real_extract(archive, resources, mapping, **kwargs)

    @contextlib.contextmanager
    def patches(self):
        self.real_extract = PACKAGE.extract
        with contextlib.ExitStack() as stack:
            stack.enter_context(mock.patch.object(PACKAGE, "TARGETS", self.selected))
            stack.enter_context(mock.patch.object(PACKAGE, "PIP", self.pip))
            stack.enter_context(mock.patch.object(PACKAGE, "NPM_ACQUISITION", self.npm, create=True))
            self.zstd = stack.enter_context(mock.patch.object(PACKAGE.subprocess, "run"))
            stack.enter_context(mock.patch.object(PACKAGE, "extract", side_effect=self.unpack))
            yield

    def build(self, **changed):
        options = {"pins": self.pins, "pinned_inputs": self.directory} | self.kwargs() | changed
        target = options.pop("target", "macos-arm64")
        upstream = options.pop("upstream", None)
        with self.patches():
            return PACKAGE.build(self.destination, upstream, Path("/inert/zstd"), target=target, **options)

    def assert_no_output(self):
        self.assertFalse((self.destination / "build.json").exists())
        self.assertFalse(list(self.destination.glob("*.tar.gz")))


class PinnedInputTests(PinnedFixture):
    def test_pinned_build_packages_the_four_rows_without_upstream_or_acquisition_arguments(self):
        self.arrange()
        result = self.build()
        self.zstd.assert_called_once()  # Controlled zstd only; no fetched file is executed.
        self.assertEqual(self.zstd.call_args.args[0][:3], ["/inert/zstd", "-d", "-c"])
        resources = Path(result["resources"])
        manifest, digest = PREPARE.verify_resources(resources)
        self.assertEqual(digest, result["resources_sha256"])
        self.assertEqual(json.loads((self.destination / "build.json").read_text()), result)
        self.assertEqual((resources / "runtime/pip.pyz").read_bytes(), self.files["pip"].read_bytes())
        group = manifest["npm_acquisition"]
        self.assertEqual(group["node"], "runtime/acquisition-node/bin/node")
        self.assertEqual((resources / group["npm_archive"]).read_bytes(), self.files["npm"].read_bytes())
        self.assertEqual((resources / group["node"]).read_bytes(), (resources / manifest["node"]).read_bytes())
        self.assertEqual({row["name"]: (row["sha256"], row["size"]) for row in manifest["sources"]}, {
            "pip-26.2.1.pyz": self.pip[1:], **{self.selected["macos-arm64"][kind][0]:
                                              self.selected["macos-arm64"][kind][1:] for kind in ("python", "node")}})
        raw = self.pins.read_bytes()
        self.assertEqual(result["pinned_inputs"], {
            "pins": {"path": str(self.pins), "sha256": sha256(raw), "bytes": len(raw)},
            "directory": str(self.directory),
            "files": {role: {"kind": path.parent.name, "file": path.name,
                             "sha256": sha256(path.read_bytes()), "bytes": path.stat().st_size}
                      for role, path in self.files.items()},
            "scope": "pin_file_rows_equal_packager_pins_and_fetched_bytes_checked_at_use"})
        inputs = result["npm_acquisition_inputs"]
        self.assertEqual(inputs["node_archive"], {"path": str(self.files["node"]),
            "sha256": self.rows["node"]["sha256"], "bytes": self.files["node"].stat().st_size})
        self.assertEqual(inputs["npm_archive"], {"path": str(self.files["npm"]),
            "sha256": self.npm[1], "bytes": self.npm[2]})
        self.assertEqual(result["target"], "macos-arm64")

    def test_pin_rows_must_equal_the_packager_pins_before_any_output(self):
        self.arrange()
        for role, field, value in (("python", "sha256", "0" * 64), ("node", "sha256", "0" * 64),
                                   ("pip", "sha256", "0" * 64), ("pip", "sha256", None),
                                   ("npm", "sha512", "0" * 128), ("npm", "sha512", None),
                                   ("npm", "sha256", "0" * 64)):
            with self.subTest(role=role, field=field, value=value):
                rows = copy.deepcopy(self.rows)
                rows[role][field] = value
                self.write_pins(list(rows.values()))
                with self.assertRaisesRegex(ValueError, "differs from the packager pin"):
                    self.build()
                self.zstd.assert_not_called()
                self.assertFalse(self.destination.exists())
        # The registry's own optional sha256 of the npm archive is accepted when equal.
        rows = copy.deepcopy(self.rows)
        rows["npm"]["sha256"] = self.npm[1]
        self.write_pins(list(rows.values()))
        self.assertEqual(self.build()["pinned_inputs"]["files"]["npm"]["sha256"], self.npm[1])

    def test_missing_repeated_or_malformed_pin_file_refuses_before_any_output(self):
        self.arrange()
        rows = list(self.rows.values())
        cases = {
            "missing row": lambda: self.write_pins(rows[1:]),
            "repeated row": lambda: self.write_pins([*rows, rows[0]]),
            "row is not an object": lambda: self.write_pins([*rows, "release-download"]),
            "other schema": lambda: self.write_pins(schema="connect-library-pins/2"),
            "other target": lambda: self.write_pins(target="linux-x86_64"),
            "rows are not a list": lambda: self.write_pins({"rows": rows}),
            "not an object": lambda: self.pins.write_text("[]\n"),
            "duplicate key": lambda: self.pins.write_text(
                '{"schema":"connect-library-pins/1","target":"macos-arm64","pins":[],"pins":[]}'),
            "not JSON": lambda: self.pins.write_bytes(b"\xff"),
            "empty": lambda: self.pins.write_bytes(b""),
        }
        for name, change in cases.items():
            with self.subTest(case=name):
                change()
                with self.assertRaises(ValueError):
                    self.build()
                self.zstd.assert_not_called()
                self.assertFalse(self.destination.exists())
        self.write_pins()
        moved = self.root / "moved-pins.json"
        self.pins.rename(moved)
        self.pins.symlink_to(moved)
        with self.assertRaises((OSError, ValueError)):
            self.build()
        self.assertFalse(self.destination.exists())

    def test_fetched_file_that_differs_from_its_pin_never_reaches_a_closure(self):
        for role in ("python", "node", "pip", "npm"):
            for change in ("bytes", "link", "missing"):
                with self.subTest(role=role, change=change):
                    shared.ExplicitResourceSelectionTests.setUp(self)
                    self.arrange()
                    path = self.files[role]
                    original = path.read_bytes()
                    path.unlink()
                    if change == "bytes":
                        path.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
                    elif change == "link":
                        (self.root / "elsewhere").write_bytes(original)
                        path.symlink_to(self.root / "elsewhere")
                    with self.assertRaises((OSError, ValueError)):
                        self.build()
                    self.zstd.assert_not_called()
                    self.assert_no_output()
                    self.doCleanups()

    def test_npm_archive_must_also_match_the_registry_sha512(self):
        self.arrange()
        # Both committed pins agree on a sha512 that is not this archive's.
        self.npm = (*self.npm[:3], "a" * 128)
        self.rows["npm"]["sha512"] = "a" * 128
        self.write_pins()
        with self.assertRaisesRegex(ValueError, "registry sha512"):
            self.build()
        self.zstd.assert_not_called()
        self.assert_no_output()

    def test_pair_replaces_upstream_and_acquisition_arguments_and_needs_the_explicit_selection(self):
        self.arrange()
        acquisition = {"acquisition_node_archive": self.files["node"],
                       "acquisition_node_sha256": self.rows["node"]["sha256"],
                       "acquisition_node_bytes": self.files["node"].stat().st_size,
                       "npm_archive": self.files["npm"], "npm_archive_sha256": self.npm[1],
                       "npm_archive_bytes": self.npm[2]}
        for name, changed in (("pin file alone", {"pinned_inputs": None}),
                              ("directory alone", {"pins": None}),
                              ("with an upstream directory", {"upstream": self.directory / "release-download"}),
                              ("with acquisition arguments", acquisition),
                              ("with one acquisition argument", {"npm_archive": self.files["npm"]}),
                              ("relative pin file", {"pins": Path("library-pins-macos-arm64.json")}),
                              ("relative directory", {"pinned_inputs": Path("pins")}),
                              ("directory is a file", {"pinned_inputs": self.pins}),
                              ("text paths", {"pins": str(self.pins), "pinned_inputs": str(self.directory)})):
            with self.subTest(case=name):
                with self.assertRaisesRegex(ValueError, "pinned inputs"):
                    self.build(**changed)
                self.zstd.assert_not_called()
                self.assertFalse(self.destination.exists())
        with self.patches():
            with self.assertRaisesRegex(ValueError, "pinned inputs"):
                PACKAGE.build(self.destination, None, Path("/inert/zstd"),
                              pins=self.pins, pinned_inputs=self.directory)
            with self.assertRaisesRegex(ValueError, "upstream directory required"):
                PACKAGE.build(self.destination, None, Path("/inert/zstd"), **self.kwargs())
        link = self.root / "pins-link"
        link.symlink_to(self.directory, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "pinned inputs"):
            self.build(pinned_inputs=link)
        self.assertFalse(self.destination.exists())

    def test_linux_target_has_no_pinned_inputs_and_keeps_its_explicit_arguments(self):
        self.arrange()
        with mock.patch.object(PACKAGE, "require_case_sensitive") as case_check:
            with self.assertRaisesRegex(ValueError, "not defined for this packaging target"):
                self.build(target="linux-x86_64")
        case_check.assert_not_called()
        self.zstd.assert_not_called()
        self.assertFalse(self.destination.exists())
        self.assertEqual(PACKAGE.PINNED_TARGETS, ("macos-arm64",))

    def test_cli_forwards_the_pair_without_upstream(self):
        values = {"library": Path("/unused/library.tar.gz"), "library_sha256": "a" * 64, "library_bytes": 7,
                  "starter": Path("/unused/starter"), "release_manifest_sha256": "b" * 64,
                  "pins": Path("/unused/pins.json"), "pinned_inputs": Path("/unused/pins")}
        argv = ["package", "--destination", "/unused/output", "--zstd", "/unused/zstd"]
        for key, value in values.items():
            argv.extend(["--" + key.replace("_", "-"), str(value)])
        with mock.patch("sys.argv", argv), mock.patch.object(PACKAGE, "build", return_value={}) as build:
            with contextlib.redirect_stdout(io.StringIO()):
                PACKAGE.main()
        self.assertEqual(build.call_args.args, (Path("/unused/output"), None, Path("/unused/zstd")))
        self.assertEqual(build.call_args.kwargs, {"target": "macos-arm64", **values})

    def test_cli_still_requires_upstream_without_the_pair(self):
        argv = ["package", "--destination", "/unused/output", "--zstd", "/unused/zstd"]
        errors = io.StringIO()
        with mock.patch("sys.argv", argv), mock.patch.object(PACKAGE, "build", return_value={}) as build:
            with contextlib.redirect_stderr(errors), self.assertRaises(SystemExit) as stopped:
                PACKAGE.main()
        self.assertEqual(stopped.exception.code, 2)
        self.assertIn("the following arguments are required: --upstream", errors.getvalue())
        build.assert_not_called()

    def test_committed_pins_are_one_public_file_each_and_one_node_serves_both_uses(self):
        self.assertEqual(PACKAGE.PIP_SOURCE, "pip-26.2.1.pyz")
        name, digest, size, registry = PACKAGE.NPM_ACQUISITION
        self.assertEqual(name, "npm-12.2.0.tgz")
        self.assertRegex(digest, r"\A[0-9a-f]{64}\Z")
        self.assertRegex(registry, r"\A[0-9a-f]{128}\Z")
        self.assertTrue(type(size) is int and 0 < size <= 128 * 1024 * 1024)
        owner = PACKAGE.runpy.run_path(str(PACKAGE.PREPARE.parent.parent / "node_acquisition.py"))
        for target in PACKAGE.PINNED_TARGETS:
            with self.subTest(target=target):
                node = PACKAGE.TARGETS[target]["node"]
                platform = {"macos-arm64": "darwin-arm64"}[target]
                found = re.fullmatch(r"node-(v[0-9]+\.[0-9]+\.[0-9]+)-" + platform + r"\.tar\.gz", node[0])
                self.assertIsNotNone(found)
                # The npm owner must accept the runtime Node as the acquisition Node.
                self.assertTrue(owner["_runtime_matches"](
                    {"version": found[1], "platform": "darwin", "arch": "arm64"}, target))
                self.assertLessEqual(node[2], 128 * 1024 * 1024)
                rows = PACKAGE.pinned_rows(target)
                self.assertEqual(set(rows), {"python", "node", "pip", "npm"})
                self.assertEqual(rows["pip"], ("release-download", "pip-26.2.1.pyz", PACKAGE.PIP[1], PACKAGE.PIP[2], None))
                self.assertEqual(rows["npm"], ("npm-tarball", name, digest, size, registry))
                self.assertEqual(rows["node"], ("release-download", *node, None))
                self.assertEqual(rows["python"], ("release-download", *PACKAGE.TARGETS[target]["python"], None))


class LinkModeTests(PinnedFixture):
    def links(self, result):
        manifest = json.loads((Path(result["resources"]) / "resources.json").read_bytes())
        return {row["path"]: row["mode"] for row in manifest["entries"] if row["kind"] == "symlink"}

    def test_macos_closure_asks_the_host_for_0755_on_every_link_before_the_inventory(self):
        self.arrange()
        with mock.patch.object(PACKAGE.os, "lchmod", create=True) as change:
            result = self.build()
        links = self.links(result)
        self.assertEqual(set(links), {"runtime/python/bin/python3", "runtime/python/current"})
        resources = Path(result["resources"])
        self.assertEqual(sorted((str(call.args[0]), call.args[1]) for call in change.call_args_list),
                         sorted((str(resources / name), 0o755) for name in links))

    def test_only_a_macos_closure_sets_link_modes_and_only_on_links(self):
        tree = self.root / "tree"
        (tree / "lib").mkdir(parents=True)
        (tree / "lib/payload").write_bytes(b"inert")
        (tree / "lib/file-link").symlink_to("payload")
        (tree / "directory-link").symlink_to("lib")
        for target, expected in (("macos-arm64", [(tree / "directory-link", 0o755), (tree / "lib/file-link", 0o755)]),
                                 ("linux-x86_64", [])):
            with self.subTest(target=target), mock.patch.object(PACKAGE.os, "lchmod", create=True) as change:
                PACKAGE.set_link_modes(tree, target)
                self.assertEqual(sorted((call.args[0], call.args[1]) for call in change.call_args_list), expected)

    @unittest.skipUnless(hasattr(os, "lchmod"), "host links have no mode of their own")
    def test_macos_link_modes_do_not_follow_the_callers_umask(self):
        digests = set()
        for mask in (0o077, 0o022, 0o000):
            with self.subTest(umask=oct(mask)):
                shared.ExplicitResourceSelectionTests.setUp(self)
                self.arrange()
                with shared.umask(mask):
                    result = self.build()
                self.assertEqual(set(self.links(result).values()), {0o755})
                resources = Path(result["resources"])
                for name in self.links(result):
                    self.assertEqual(stat.S_IMODE((resources / name).lstat().st_mode), 0o755)
                PREPARE.verify_resources(resources)
                digests.add(json.dumps(sorted(self.links(result).items())))
        self.assertEqual(len(digests), 1)


class MacLibraryContractTests(unittest.TestCase):
    """The macOS library is CONTENTS schema 1: the shipped installers bind an
    exact runtime_target only for Linux, so a darwin schema 2 installs nowhere."""
    DARWIN = {"python": {"implementation": "cpython", "version": "3.11.16", "platform": "darwin",
                         "machine": "arm64", "pointer_bits": 64},
              "node": {"version": "v24.19.0", "platform": "darwin", "arch": "arm64", "npm_version": "11.17.0"}}

    def check(self, contents, node="24.19.0"):
        fixture = shared.RuntimeCompatibilityTests()
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            fixture.resources(root, "macos-arm64", node=node, npm="11.17.0")
            archive = root / "library.tar.gz"
            fixture.library(archive, contents)
            return fixture.check(archive, root, "macos-arm64")

    def test_pinned_macos_node_satisfies_the_schema_1_installer_predicate(self):
        found = re.fullmatch(r"node-v([0-9]+\.[0-9]+\.[0-9]+)-darwin-arm64\.tar\.gz",
                             PACKAGE.TARGETS["macos-arm64"]["node"][0])
        result = self.check({"schema_version": 1, "scope": "consumer_export_only_no_installation_or_activation",
                             "accepted_inputs": {}, "files": {}}, node=found[1])
        self.assertEqual(result["library_schema_version"], 1)
        self.assertEqual(result["supported_host"], {"platform": "darwin", "machine": "arm64", "pointer_bits": 64})
        self.assertEqual(result["runtime_metadata"]["node"]["version"], "v" + found[1])

    def test_darwin_runtime_target_is_refused_under_either_schema(self):
        with self.assertRaisesRegex(ValueError, "unsupported selected library runtime contract"):
            self.check({"schema_version": 2, "runtime_target": self.DARWIN})
        with self.assertRaisesRegex(ValueError, "unsupported selected library runtime contract"):
            self.check({"schema_version": 2})
        with self.assertRaisesRegex(ValueError, "legacy Mac"):
            self.check({"schema_version": 1, "runtime_target": self.DARWIN})


class PublisherCheckTests(PinnedFixture):
    """The pin check compares the packager's pins with publisher documents."""

    def load(self):
        self.assertTrue(CHECK_SCRIPT.is_file(), "the pin check script exists")
        check = shared.load("connect_resource_pin_check", CHECK_SCRIPT)
        # One packager module for this test and the check, so fixture pins reach both.
        check.PACKAGE = PACKAGE
        return check

    def documents(self, check, target="macos-arm64", **changed):
        rows = check.PACKAGE.pinned_rows(target)
        directory = Path(tempfile.mkdtemp(prefix="publisher-documents-", dir=self.root))
        values = {"python": rows["python"][2], "node": rows["node"][2], "npm": rows["npm"][4]} | changed
        names = check.document_names(target)
        (directory / names["python"]).write_text(
            "0" * 64 + "  some-other-build.tar.zst\n" + values["python"] + "  " + rows["python"][1] + "\n")
        (directory / names["node"]).write_text(
            values["node"] + "  " + rows["node"][1] + "\n" + "1" * 64 + "  node-v0.0.0-linux-x64.tar.gz\n")
        integrity = "sha512-" + base64.b64encode(bytes.fromhex(values["npm"])).decode()
        (directory / names["npm"]).write_text(json.dumps({"name": "npm", "version": "12.2.0",
            "dist": {"integrity": integrity, "tarball": "https://registry.npmjs.org/npm/-/" + rows["npm"][1]}}))
        return directory

    def results(self, report):
        return {(row["subject"], row["against"]): row["result"] for row in report["checks"]}

    def test_committed_pins_equal_consistent_publisher_documents_and_pip_is_reported_as_own_hash(self):
        check = self.load()
        with mock.patch.object(check.urllib.request, "urlopen") as fetch:
            report = check.check("macos-arm64", self.documents(check))
        fetch.assert_not_called()
        self.assertTrue(report["ok"])
        self.assertEqual(report["target"], "macos-arm64")
        self.assertEqual(self.results(report), {
            ("python", "publisher checksum file"): "equal", ("node", "publisher checksum file"): "equal",
            ("npm", "registry integrity"): "equal", ("pip", "publisher"): "own hash: no publisher checksum"})
        self.assertEqual(check.document_names("macos-arm64"),
                         {"python": "SHA256SUMS", "node": "SHASUMS256.txt", "npm": "npm-12.2.0.json"})
        self.assertEqual(check.document_urls("macos-arm64"), {
            "python": "https://github.com/astral-sh/python-build-standalone/releases/download/20260901/SHA256SUMS",
            "node": "https://nodejs.org/dist/v24.19.0/SHASUMS256.txt",
            "npm": "https://registry.npmjs.org/npm/12.2.0"})

    def test_each_publisher_difference_or_missing_line_fails_the_check(self):
        check = self.load()
        for role, value in (("python", "2" * 64), ("node", "2" * 64), ("npm", "2" * 128)):
            with self.subTest(role=role):
                report = check.check("macos-arm64", self.documents(check, **{role: value}))
                self.assertFalse(report["ok"])
                self.assertEqual([key[0] for key, result in self.results(report).items() if result == "differs"], [role])
        directory = self.documents(check)
        (directory / "SHASUMS256.txt").write_text("1" * 64 + "  node-v0.0.0-linux-x64.tar.gz\n")
        report = check.check("macos-arm64", directory)
        self.assertFalse(report["ok"])
        self.assertEqual(self.results(report)[("node", "publisher checksum file")], "missing")
        directory = self.documents(check)
        (directory / "SHA256SUMS").unlink()
        with self.assertRaises((OSError, ValueError)):
            check.check("macos-arm64", directory)
        with self.assertRaisesRegex(ValueError, "not defined for this packaging target"):
            check.check("linux-x86_64", self.documents(check))

    def test_pin_file_and_fetched_bytes_are_compared_with_the_same_pins(self):
        check = self.load()
        self.arrange()
        with self.patches():
            documents = self.documents(check)
            report = check.check("macos-arm64", documents, pins=self.pins, pinned_inputs=self.directory)
            self.assertTrue(report["ok"])
            results = self.results(report)
            self.assertEqual(results[("pin file", "packager pins")], "equal")
            for role in ("python", "node", "pip", "npm"):
                self.assertEqual(results[(role, "fetched bytes")], "equal")
            self.files["pip"].chmod(0o644)
            self.files["pip"].write_bytes(b"other pip bytes!")
            report = check.check("macos-arm64", documents, pins=self.pins, pinned_inputs=self.directory)
            self.assertFalse(report["ok"])
            self.assertEqual(self.results(report)[("pip", "fetched bytes")], "differs")
            self.rows["node"]["sha256"] = "0" * 64
            self.write_pins()
            report = check.check("macos-arm64", documents, pins=self.pins)
            self.assertFalse(report["ok"])
            self.assertEqual(self.results(report)[("pin file", "packager pins")], "differs")
        self.zstd.assert_not_called()

    def test_cli_reports_json_and_exit_status_and_fetches_only_the_three_documents(self):
        check = self.load()
        directory = self.documents(check)
        output = io.StringIO()
        with contextlib.redirect_stdout(output), mock.patch.object(check.urllib.request, "urlopen") as fetch:
            status = check.main(["--target", "macos-arm64", "--documents", str(directory)])
        fetch.assert_not_called()
        self.assertEqual(status, 0)
        self.assertTrue(json.loads(output.getvalue())["ok"])
        changed = self.documents(check, python="3" * 64)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(check.main(["--target", "macos-arm64", "--documents", str(changed)]), 1)
        self.assertFalse(json.loads(output.getvalue())["ok"])
        # --fetch writes exactly the three publisher documents into a NEW directory.
        names, urls = check.document_names("macos-arm64"), check.document_urls("macos-arm64")
        served = {urls[role]: (directory / names[role]).read_bytes() for role in ("python", "node", "npm")}
        opened = []

        def serve(request, timeout):
            opened.append(request.full_url)
            return contextlib.closing(io.BytesIO(served[request.full_url]))

        fresh = self.root / "fetched-documents"
        with contextlib.redirect_stdout(io.StringIO()), mock.patch.object(
                check.urllib.request, "urlopen", side_effect=serve):
            self.assertEqual(check.main(["--target", "macos-arm64", "--documents", str(fresh), "--fetch"]), 0)
            # An existing directory is never fetched into.
            with self.assertRaises((OSError, ValueError)):
                check.main(["--target", "macos-arm64", "--documents", str(fresh), "--fetch"])
        self.assertEqual(sorted(opened), sorted(served))
        self.assertEqual(sorted(path.name for path in fresh.iterdir()), sorted(names.values()))


if __name__ == "__main__":
    unittest.main()
