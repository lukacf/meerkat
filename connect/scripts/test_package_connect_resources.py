"""Inert target checks. Never package or execute supplied runtime binaries."""
from __future__ import annotations

import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import tarfile
import tempfile
import threading
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PACKAGE = load("connect_resource_package", ROOT / "scripts/package-connect-resources.py")
PREPARE = load("connect_resource_prepare", ROOT / "sdk/consumer-setup/product/prepare.py")


class BeforeMaterialization(Exception):
    pass


@contextlib.contextmanager
def umask(value):
    previous = os.umask(value)
    try:
        yield
    finally:
        os.umask(previous)


class SymlinkModeTests(unittest.TestCase):
    def fixture(self, parent):
        archive = parent / "inert.tar"
        with tarfile.open(archive, "w") as tar:
            directory = tarfile.TarInfo("bundle/lib")
            directory.type = tarfile.DIRTYPE
            directory.mode = 0o755
            tar.addfile(directory)
            data = b"inert notice; never executable"
            member = tarfile.TarInfo("bundle/lib/notice")
            member.mode = 0o644
            member.size = len(data)
            tar.addfile(member, io.BytesIO(data))
            for name, target in (("file-link", "lib/notice"), ("dir-link", "lib")):
                member = tarfile.TarInfo("bundle/" + name)
                member.type = tarfile.SYMTYPE
                member.mode = 0o700
                member.linkname = target
                tar.addfile(member)
        root = parent / "resources"
        root.mkdir()
        return archive, root

    def extract(self, archive, root, **kwargs):
        PACKAGE.extract(archive, root, lambda name: name.removeprefix("bundle/"), **kwargs)

    def test_linux_links_have_actual_manifest_and_tar_mode_0777_despite_umask(self):
        for mask in (0o077, 0o022):
            with self.subTest(umask=oct(mask)), tempfile.TemporaryDirectory() as scratch, umask(mask):
                archive, root = self.fixture(Path(scratch).resolve())
                self.extract(archive, root, target="linux-x86_64")
                entries = {row["path"]: row for row in PREPARE.tree_entries(root)}
                archived = io.BytesIO()
                with tarfile.open(fileobj=archived, mode="w") as tar:
                    for name, expected_target in (("file-link", "lib/notice"), ("dir-link", "lib")):
                        path = root / name
                        self.assertTrue(path.is_symlink())
                        self.assertEqual(os.readlink(path), expected_target)
                        self.assertEqual(stat.S_IMODE(path.lstat().st_mode), 0o777)
                        self.assertEqual(entries[name]["mode"], 0o777)
                        tar.addfile(tar.gettarinfo(str(path), arcname=name))
                with tarfile.open(fileobj=io.BytesIO(archived.getvalue()), mode="r:") as tar:
                    self.assertEqual(tar.getmember("file-link").mode, 0o777)
                    self.assertEqual(tar.getmember("dir-link").mode, 0o777)
                self.assertEqual((root / "lib/notice").read_bytes(), b"inert notice; never executable")
                self.assertEqual(stat.S_IMODE((root / "lib/notice").stat().st_mode), 0o644)

    def test_mac_default_and_explicit_target_preserve_host_link_behavior(self):
        for kwargs in ({}, {"target": "macos-arm64"}):
            with self.subTest(target=kwargs), tempfile.TemporaryDirectory() as scratch, umask(0o077):
                parent = Path(scratch).resolve()
                archive, root = self.fixture(parent)
                control = parent / "control-link"
                control.symlink_to("resources/lib/notice")
                expected = stat.S_IMODE(control.lstat().st_mode)
                self.extract(archive, root, **kwargs)
                self.assertEqual(stat.S_IMODE((root / "file-link").lstat().st_mode), expected)
                self.assertEqual(stat.S_IMODE((root / "dir-link").lstat().st_mode), expected)

    @unittest.skipUnless(hasattr(os, "lchmod"), "host cannot create a non-0777 link for refusal controls")
    def test_linux_refuses_if_no_follow_mode_change_cannot_be_applied(self):
        for failure in (OSError("inert permission refusal"), None):
            with self.subTest(failure=type(failure).__name__), tempfile.TemporaryDirectory() as scratch, umask(0o077):
                archive, root = self.fixture(Path(scratch).resolve())
                original = os.chmod

                def refuse_link(path, mode, **kwargs):
                    if kwargs.get("follow_symlinks") is False:
                        if failure:
                            raise failure
                        return
                    return original(path, mode, **kwargs)

                with mock.patch.object(PACKAGE.os, "chmod", side_effect=refuse_link):
                    with self.assertRaises((OSError, ValueError)):
                        self.extract(archive, root, target="linux-x86_64")
                self.assertEqual(stat.S_IMODE((root / "file-link").lstat().st_mode), 0o700)
                self.assertEqual(stat.S_IMODE((root / "lib/notice").stat().st_mode), 0o644)

    @unittest.skipUnless(hasattr(os, "lchmod"), "host cannot alter link mode for exact verification control")
    def test_changed_link_mode_is_still_rejected_by_installed_verifier(self):
        with tempfile.TemporaryDirectory() as scratch:
            archive, root = self.fixture(Path(scratch).resolve())
            self.extract(archive, root, target="linux-x86_64")
            manifest = {"schema_version": 1, "kind": "meerkat_connect_resources", "target": "linux-x86_64",
                "prepare": "lib/notice", "python": "file-link", "node": "file-link", "npm_cli": "lib/notice",
                "starter": "lib/notice", "archive": "lib/notice", "entries": PREPARE.tree_entries(root)}
            (root / "resources.json").write_text(json.dumps(manifest))
            self.assertEqual(PREPARE.verify_resources(root)[0], manifest)
            os.chmod(root / "file-link", 0o700, follow_symlinks=False)
            with self.assertRaisesRegex(PREPARE.ResourceError, "resource_closure_changed"):
                PREPARE.verify_resources(root)


class CasePathTests(unittest.TestCase):
    def fixture(self, parent, names):
        archive = parent / "inert.tar"
        with tarfile.open(archive, "w") as tar:
            for index, name in enumerate(names):
                data = ("inert payload " + str(index)).encode()
                member = tarfile.TarInfo("bundle/" + name)
                member.mode = 0o644
                member.size = len(data)
                tar.addfile(member, io.BytesIO(data))
        root = parent / "resources"
        root.mkdir()
        return archive, root

    def test_linux_refuses_actual_insensitive_staging_before_inputs_or_destination(self):
        with tempfile.TemporaryDirectory() as scratch:
            parent = Path(scratch).resolve()
            probe = parent / "case-probe"
            probe.write_bytes(b"inert")
            insensitive = (parent / "CASE-PROBE").exists()
            probe.unlink()
            if not insensitive:
                self.skipTest("host staging filesystem is case-sensitive")
            with mock.patch.object(PACKAGE, "pin") as pin, mock.patch.object(
                    PACKAGE.subprocess, "run") as run:
                with self.assertRaisesRegex(ValueError, "case-sensitive staging filesystem"):
                    PACKAGE.build(parent / "output", parent / "missing-inputs",
                                  parent / "unused-zstd", target="linux-x86_64")
            pin.assert_not_called()
            run.assert_not_called()
            self.assertEqual(list(parent.iterdir()), [])

    def test_linux_preserves_distinct_case_names_on_actual_sensitive_staging(self):
        with tempfile.TemporaryDirectory() as scratch:
            parent = Path(scratch).resolve()
            try:
                PACKAGE.require_case_sensitive(parent)
            except ValueError as error:
                self.assertEqual(str(error), "Linux resources require a case-sensitive staging filesystem")
                self.assertEqual(list(parent.iterdir()), [])
                self.skipTest("no case-sensitive staging filesystem in this host test directory")
            self.assertEqual(list(parent.iterdir()), [])
            names = ("terminfo/E/Eterm", "terminfo/e/eterm", "terminfo/2/2621A", "terminfo/2/2621a")
            archive, root = self.fixture(parent, names)
            PACKAGE.extract(archive, root, lambda name: name.removeprefix("bundle/"), target="linux-x86_64")
            files = {row["path"] for row in PREPARE.tree_entries(root) if row["kind"] == "file"}
            self.assertEqual(files, set(names))
            for index, name in enumerate(names):
                self.assertEqual((root / name).read_bytes(), ("inert payload " + str(index)).encode())

    def test_mac_default_and_explicit_target_still_reject_casefold_collisions(self):
        for kwargs in ({}, {"target": "macos-arm64"}):
            with self.subTest(target=kwargs), tempfile.TemporaryDirectory() as scratch:
                archive, root = self.fixture(Path(scratch).resolve(), ("2621A", "2621a"))
                with self.assertRaisesRegex(ValueError, "upstream path collision"):
                    PACKAGE.extract(archive, root, lambda name: name.removeprefix("bundle/"), **kwargs)
                self.assertEqual(list(root.iterdir()), [])

    def test_both_targets_still_reject_exact_and_nfc_duplicate_paths(self):
        for target in ("macos-arm64", "linux-x86_64"):
            for names in (("same", "same"), ("\u00e9", "e\u0301")):
                with self.subTest(target=target, names=names), tempfile.TemporaryDirectory() as scratch:
                    archive, root = self.fixture(Path(scratch).resolve(), names)
                    with self.assertRaisesRegex(ValueError, "upstream path collision"):
                        PACKAGE.extract(archive, root, lambda name: name.removeprefix("bundle/"), target=target)
                    self.assertEqual(list(root.iterdir()), [])


class PackagingTargetTests(unittest.TestCase):
    def test_default_cli_and_explicit_linux_select_maintainer_target(self):
        for extra, expected in (([], "macos-arm64"), (["--target", "linux-x86_64"], "linux-x86_64")):
            with self.subTest(target=expected), mock.patch.object(PACKAGE, "build", return_value={}) as build:
                with mock.patch("sys.argv", ["package", "--destination", "/unused/output",
                        "--upstream", "/unused/inputs", "--zstd", "/unused/zstd", *extra]):
                    with contextlib.redirect_stdout(io.StringIO()):
                        PACKAGE.main()
                self.assertEqual(build.call_args.kwargs, {"target": expected})

    def test_preflight_selects_exact_target_inputs_and_same_starter(self):
        expected = {
            "macos-arm64": ("cpython-3.11.16+20260901-aarch64-apple-darwin-pgo+lto-full.tar.zst",
                "node-v24.19.0-darwin-arm64.tar.gz", "toolkit-library-20261009-library-support-selection-macos-arm64",
                "toolkit-library-20261009-library-support-selection-macos-arm64.tar.gz",
                "49482af8016a76de90826a1362030319f27063b92f9ee0c87de970801f04cb6b"),
            "linux-x86_64": ("cpython-3.11.16+20260901-x86_64-unknown-linux-gnu-pgo+lto-full.tar.zst",
                "node-v22.23.2-linux-x64.tar.gz", "toolkit-library-20261009-installed-entry-recovery-linux-x86_64",
                "toolkit-library-20261009-installed-entry-recovery-linux-x86_64.tar.gz",
                "50b5ee1e384ae24f7a4a0511efaca781025eeaa8489414d71fd825557e48f146"),
        }
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            starter = root / "dist/toolkit-setup-20261010-resource-selection-export"
            starter.mkdir(parents=True)
            for name in PACKAGE.STARTER:
                (starter / name).write_bytes(b"inert fixture, never executed")
            for target, (python, node, directory, library, library_sha) in expected.items():
                with self.subTest(target=target), mock.patch.object(PACKAGE, "ROOT", root):
                    with mock.patch.object(PACKAGE, "pin") as pin, mock.patch.object(
                            PACKAGE.runpy, "run_path", side_effect=BeforeMaterialization), mock.patch.object(
                            PACKAGE, "require_case_sensitive") as case_check:
                        # Omitting target exercises the historical Python call default too.
                        kwargs = {} if target == "macos-arm64" else {"target": target}
                        with self.assertRaises(BeforeMaterialization):
                            PACKAGE.build(root / "output", root / "inputs", root / "zstd", **kwargs)
                    if target == "linux-x86_64":
                        case_check.assert_called_once_with((root / "output").resolve().parent)
                    else:
                        case_check.assert_not_called()
                    self.assertEqual([row.args[0] for row in pin.call_args_list[:4]],
                        [root / "inputs" / python, root / "inputs" / node,
                         root / "inputs/pip.pyz", root / "dist" / directory / library])
                    self.assertEqual(pin.call_args_list[2].args[1:], (PACKAGE.PIP[1], PACKAGE.PIP[2]))
                    self.assertEqual(pin.call_args_list[3].args[1], library_sha)
                    self.assertEqual([row.args for row in pin.call_args_list[4:-1]],
                        [(starter / name, sha) for name, sha in PACKAGE.STARTER.items()])
                    self.assertEqual(pin.call_args_list[-1].args,
                        (PACKAGE.PREPARE.parent / "notices/pip-LICENSE.txt", PACKAGE.PIP_LICENSE))
                    self.assertFalse((root / "output").exists())

    def test_missing_changed_or_linked_pip_refuses_before_materialization(self):
        for kind in ("missing", "changed", "symlink"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch)
                pip = root / "pip.pyz"
                if kind == "changed":
                    pip.write_bytes(b"unselected pip")
                elif kind == "symlink":
                    source = root / "unselected"
                    source.write_bytes(b"unselected pip")
                    pip.symlink_to(source)
                actual_pin = PACKAGE.pin
                def only_pip(path, expected, size=None):
                    if path == pip:
                        actual_pin(path, expected, size)
                with mock.patch.object(PACKAGE, "pin", side_effect=only_pip), mock.patch.object(
                        PACKAGE.subprocess, "run") as run:
                    with self.assertRaisesRegex(ValueError, "packaging input does not match"):
                        PACKAGE.build(root / "output", root, root / "unused-zstd")
                    run.assert_not_called()
                self.assertFalse((root / "output").exists())

    def test_unknown_target_refuses_before_input_read_or_destination_creation(self):
        with tempfile.TemporaryDirectory() as scratch, mock.patch.object(PACKAGE, "pin") as pin:
            destination = Path(scratch) / "output"
            with self.assertRaisesRegex(ValueError, "unsupported packaging target"):
                PACKAGE.build(destination, Path(scratch), Path("/unused/zstd"), target="linux-arm64")
            pin.assert_not_called()
            self.assertFalse(destination.exists())


class ExplicitResourceSelectionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="connect-selected-resource-control-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        # Restore only this test's copied read-only resource tree before cleanup.
        def writable():
            for directory, _, files in os.walk(self.root):
                Path(directory).chmod(0o755)
                for name in files:
                    path = Path(directory) / name
                    if not path.is_symlink(): path.chmod(0o644)
        self.addCleanup(writable)
        patch = mock.patch.object(PACKAGE, "ROOT", self.root)
        patch.start(); self.addCleanup(patch.stop)
        self.starter = self.root / "selected-starter"
        self.starter.mkdir()
        for name in ("start.py", "bootstrap_host.py", "prepare_host.py"):
            data = ("inert selected source: " + name).encode()
            source = self.root / "sdk/consumer-setup" / name
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_bytes(data)
            (self.starter / name).write_bytes(data)
        (self.starter / "README.md").write_text("Inert retained starter explanation.\n")
        self.archive = self.root / "toolkit-library-controlled-macos-arm64.tar.gz"
        RuntimeCompatibilityTests().library(self.archive, {"schema_version": 1})
        self.manifest = {"schema_version": 1, "status": "controlled_packaging_input_only",
            "assets": [{"upload_name": self.archive.name, "sha256": PACKAGE.digest(self.archive),
                        "bytes": self.archive.stat().st_size}]}
        self.repin_manifest()

    def repin_manifest(self):
        raw = (json.dumps(self.manifest) + "\n").encode()
        (self.starter / "release-manifest.json").write_bytes(raw)
        self.manifest_sha = hashlib.sha256(raw).hexdigest()
        (self.starter / "current-distribution.json").write_text(json.dumps({
            "schema_version": 1, "manifest": "release-manifest.json", "manifest_sha256": self.manifest_sha}))

    def kwargs(self):
        return {"library": self.archive, "library_sha256": PACKAGE.digest(self.archive),
                "library_bytes": self.archive.stat().st_size, "starter": self.starter,
                "release_manifest_sha256": self.manifest_sha}

    def selected(self, **changed):
        with mock.patch.object(PACKAGE.subprocess, "run") as run, mock.patch.object(PACKAGE.runpy, "run_path") as load:
            try:
                return PACKAGE.selected_resource_inputs("macos-arm64", **(self.kwargs() | changed))
            finally:
                run.assert_not_called(); load.assert_not_called()

    def test_exact_selection_checks_current_sources_and_retains_manifest_identity(self):
        archive, library, starter, files, record = self.selected()
        self.assertEqual(archive, self.archive)
        self.assertEqual(library, (self.archive.name, PACKAGE.digest(self.archive), self.archive.stat().st_size))
        self.assertEqual(starter, self.starter)
        self.assertEqual(set(files), {"start.py", "bootstrap_host.py", "prepare_host.py", "README.md",
                                      "current-distribution.json", "release-manifest.json"})
        self.assertEqual(record["starter"]["manifest_sha256"], self.manifest_sha)
        self.assertEqual(record["library"], {"path": str(self.archive), "sha256": PACKAGE.digest(self.archive),
                                            "bytes": self.archive.stat().st_size})
        for name, row in files.items():
            self.assertEqual(row, {"sha256": PACKAGE.digest(starter / name), "bytes": (starter / name).stat().st_size})

    def test_wrong_digest_size_partial_and_target_never_fall_back(self):
        for changed in ({"library_sha256": "0" * 64}, {"library_bytes": self.archive.stat().st_size + 1},
                        {"library_bytes": True}, {"release_manifest_sha256": "0" * 64}, {"starter": None}):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                self.selected(**changed)
        with self.assertRaisesRegex(ValueError, "target or starter closure"):
            PACKAGE.selected_resource_inputs("linux-x86_64", **self.kwargs())
        with mock.patch.object(PACKAGE, "pin") as pin, mock.patch.object(PACKAGE.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "complete explicit"):
                PACKAGE.build(self.root / "bad", self.root, Path("/unused/zstd"), library=self.archive)
            pin.assert_not_called(); run.assert_not_called()
        self.assertFalse((self.root / "bad").exists())

    def test_manifest_asset_and_pointer_are_independent_from_archive_pin(self):
        original = copy.deepcopy(self.manifest)
        for field, value in (("upload_name", "toolkit-library-other-macos-arm64.tar.gz"),
                             ("sha256", "0" * 64), ("bytes", self.archive.stat().st_size + 1)):
            self.manifest = copy.deepcopy(original)
            self.manifest["assets"][0][field] = value
            self.repin_manifest()
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "selected release asset mismatch"):
                self.selected()
        self.manifest = copy.deepcopy(original)
        self.manifest["assets"].append(copy.deepcopy(self.manifest["assets"][0]))
        self.repin_manifest()
        with self.assertRaisesRegex(ValueError, "selected release asset mismatch"):
            self.selected()
        self.manifest = original; self.repin_manifest()
        (self.starter / "current-distribution.json").write_text(json.dumps({"schema_version": 1,
            "manifest": "elsewhere.json", "manifest_sha256": self.manifest_sha}))
        with self.assertRaisesRegex(ValueError, "starter pointer mismatch"):
            self.selected()

    def test_manifest_and_pointer_validation_use_the_captured_selected_bytes(self):
        path = self.starter / "release-manifest.json"
        original = path.read_bytes()
        parse = json.loads
        calls = []
        def change_path_during_validation(raw, **kwargs):
            calls.append(raw)
            # The pointer validation runs first. Swap the manifest path only
            # while validation is entered, then restore it before returning.
            path.write_bytes(b'{"schema_version":1,"assets":[]}' if len(calls) == 1 else original)
            return parse(raw, **kwargs)
        with mock.patch.object(PACKAGE.json, "loads", side_effect=change_path_during_validation):
            selected = self.selected()
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[1], original)
        self.assertEqual(path.read_bytes(), original)
        self.assertEqual(selected[4]["starter"]["manifest_sha256"], hashlib.sha256(original).hexdigest())

    def test_metadata_changed_after_initial_hash_is_refused_before_validation(self):
        path = self.starter / "current-distribution.json"
        original = path.read_bytes()
        actual_digest = PACKAGE.digest
        changed = False
        def alter_after_hash(selected):
            nonlocal changed
            value = actual_digest(selected)
            if selected == path and not changed:
                changed = True
                # Keep size identical so the byte/hash join must reject it.
                path.write_bytes(original.replace(self.manifest_sha.encode(), b"0" * 64))
            return value
        with mock.patch.object(PACKAGE, "digest", side_effect=alter_after_hash), mock.patch.object(
                PACKAGE.json, "loads") as parse:
            with self.assertRaisesRegex(ValueError, "selected starter metadata changed"):
                self.selected()
        self.assertTrue(changed)
        parse.assert_not_called()

    def test_starter_cannot_replace_current_executable_sources_or_extend_closure(self):
        path = self.starter / "start.py"
        original = path.read_bytes()
        path.write_bytes(b"substituted setup owner")
        with self.assertRaisesRegex(ValueError, "does not match its pin"):
            self.selected()
        path.write_bytes(original)
        (self.starter / "unselected.py").write_bytes(b"unexpected")
        with self.assertRaisesRegex(ValueError, "closure mismatch"):
            self.selected()
        (self.starter / "unselected.py").unlink()
        path.unlink(); path.symlink_to(self.root / "sdk/consumer-setup/start.py")
        with self.assertRaisesRegex(ValueError, "invalid selected starter file"):
            self.selected()

    def test_actual_build_records_selection_and_hashes_copied_closure_without_runtime_execution(self):
        upstream = self.root / "upstream"; upstream.mkdir()
        selected = copy.deepcopy(PACKAGE.TARGETS)
        for kind in ("python", "node"):
            filename = selected["macos-arm64"][kind][0]
            path = upstream / filename; path.write_bytes(("inert " + kind + " archive").encode())
            selected["macos-arm64"][kind] = (filename, PACKAGE.digest(path), path.stat().st_size)
        pip = upstream / "pip.pyz"; pip.write_bytes(b"inert pip source")
        def unpack(archive, resources, mapping, **kwargs):
            if archive.name == "python.tar":
                RuntimeCompatibilityTests().resources(resources, "macos-arm64", node="24.19.0", npm="11.17.0")
                cli = resources / "runtime/node/lib/node_modules/npm/bin/npm-cli.js"
                cli.parent.mkdir(parents=True); cli.write_bytes(b"inert npm cli")
        with mock.patch.object(PACKAGE, "TARGETS", selected), mock.patch.object(
                PACKAGE, "PIP", ("pip.pyz", PACKAGE.digest(pip), pip.stat().st_size)), mock.patch.object(
                PACKAGE.subprocess, "run") as run, mock.patch.object(PACKAGE, "extract", side_effect=unpack):
            result = PACKAGE.build(self.root / "output", upstream, Path("/inert/zstd"), **self.kwargs())
        run.assert_called_once()
        self.assertEqual(run.call_args.args[0][:3], ["/inert/zstd", "-d", "-c"])
        self.assertEqual(result["schema_version"], 1)
        self.assertEqual(result["target"], "macos-arm64")
        self.assertEqual(result["packager_source_sha256"], PACKAGE.digest(Path(PACKAGE.__file__)))
        self.assertEqual(result["prepare_source_sha256"], PACKAGE.digest(PACKAGE.PREPARE))
        self.assertEqual(json.loads((self.root / "output/build.json").read_text()), result)
        resources = Path(result["resources"])
        manifest, manifest_sha = PREPARE.verify_resources(resources)
        self.assertEqual(manifest_sha, result["resources_sha256"])
        self.assertEqual(result["archive_sha256"], PACKAGE.digest(Path(result["archive"])))
        self.assertEqual(result["input_selection"]["starter"]["manifest_sha256"], self.manifest_sha)
        self.assertEqual({row["name"] for row in manifest["sources"]}, {
            "pip-26.2.1.pyz", selected["macos-arm64"]["python"][0], selected["macos-arm64"]["node"][0]})
        self.assertEqual((resources / manifest["archive"]).read_bytes(), self.archive.read_bytes())
        for name in PACKAGE.STARTER:
            self.assertEqual((resources / "starter" / name).read_bytes(), (self.starter / name).read_bytes())
        # A changed selected copy remains a hard closure failure.
        changed = resources / "starter/start.py"; changed.chmod(0o644); changed.write_bytes(b"changed copy")
        with self.assertRaises(PREPARE.ResourceError):
            PREPARE.verify_resources(resources)

    def npm_inputs(self, target="macos-arm64", *, version="24.19.0", npm_version="12.2.0"):
        suffix = {"macos-arm64": "darwin-arm64", "linux-x86_64": "linux-x64"}[target]
        node = self.root / ("node-v" + version + "-" + suffix + ".tar.gz")
        payload = self.root / ("acquisition-fixture-" + target)
        payload.mkdir(exist_ok=True)
        RuntimeCompatibilityTests().resources(payload, target, node=version, npm="11.17.0")
        cli = payload / "runtime/node/lib/node_modules/npm/bin/npm-cli.js"
        cli.parent.mkdir(parents=True)
        cli.write_bytes(b"// inert selected npm cli\n")
        with tarfile.open(node, "w:gz") as archive:
            for path in sorted((payload / "runtime/node").rglob("*")):
                if path.is_file():
                    raw = path.read_bytes()
                    member = tarfile.TarInfo(node.name.removesuffix(".tar.gz") + "/" + str(path.relative_to(payload / "runtime/node")))
                    member.mode = 0o755 if path.name in ("node", "npm-cli.js") else 0o644
                    member.size = len(raw); archive.addfile(member, io.BytesIO(raw))
        npm = self.root / ("npm-" + target + ".tgz")
        with tarfile.open(npm, "w:gz") as archive:
            for name, raw in {"package.json": json.dumps({"name": "npm", "version": npm_version}).encode(),
                              "bin/npm-cli.js": b"// inert trusted npm archive\n"}.items():
                member = tarfile.TarInfo("package/" + name)
                member.mode, member.size = 0o644, len(raw)
                archive.addfile(member, io.BytesIO(raw))
        return {"acquisition_node_archive": node, "acquisition_node_sha256": PACKAGE.digest(node),
                "acquisition_node_bytes": node.stat().st_size, "npm_archive": npm,
                "npm_archive_sha256": PACKAGE.digest(npm), "npm_archive_bytes": npm.stat().st_size}

    def build_with_npm(self, target="macos-arm64", *, resource_options=None, **overrides):
        npm = self.npm_inputs(target, **overrides)
        upstream = self.root / ("upstream-" + target); upstream.mkdir()
        selected = copy.deepcopy(PACKAGE.TARGETS)
        for kind in ("python", "node"):
            filename = selected[target][kind][0]
            path = upstream / filename; path.write_bytes(("inert " + kind + " archive").encode())
            selected[target][kind] = (filename, PACKAGE.digest(path), path.stat().st_size)
        pip = upstream / "pip.pyz"; pip.write_bytes(b"inert pip source")
        if target == "linux-x86_64":
            self.archive = self.root / "toolkit-library-controlled-linux-x86_64.tar.gz"
            RuntimeCompatibilityTests().library(self.archive, {"schema_version": 2,
                "runtime_target": RuntimeCompatibilityTests.LINUX})
            self.manifest["assets"] = [{"upload_name": self.archive.name, "sha256": PACKAGE.digest(self.archive),
                                      "bytes": self.archive.stat().st_size}]
            self.repin_manifest()
        extract = PACKAGE.extract
        def unpack(archive, resources, mapping, **kwargs):
            if archive.name == "python.tar":
                RuntimeCompatibilityTests().resources(resources, target,
                    node="22.23.2" if target == "linux-x86_64" else "24.19.0",
                    npm="10.9.8" if target == "linux-x86_64" else "11.17.0")
                cli = resources / "runtime/node/lib/node_modules/npm/bin/npm-cli.js"
                cli.parent.mkdir(parents=True)
                cli.write_bytes(b"// inert library npm cli")
            elif "acquisition-unpack-" in str(archive.parent):
                extract(archive, resources, mapping, **kwargs)
        with mock.patch.object(PACKAGE, "TARGETS", selected), mock.patch.object(
                PACKAGE, "PIP", ("pip.pyz", PACKAGE.digest(pip), pip.stat().st_size)), mock.patch.object(
                PACKAGE.subprocess, "run") as run, mock.patch.object(PACKAGE, "extract", side_effect=unpack), \
                mock.patch.object(PACKAGE, "require_case_sensitive") as case_check:
            # Inert headers have no case-colliding runtime tree. This fixture
            # substitutes the staging host prerequisite, not Linux execution.
            result = PACKAGE.build(self.root / ("output-" + target), upstream, Path("/inert/zstd"),
                                   target=target, **self.kwargs(), **npm, **(resource_options or {}))
        run.assert_called_once()  # Controlled zstd only; no target executable is invoked.
        if target == "linux-x86_64": case_check.assert_called_once_with(self.root)
        else: case_check.assert_not_called()
        return result, npm

    def test_npm_resources_package_both_targets_with_distinct_runtime_and_exact_capture(self):
        for target in ("macos-arm64", "linux-x86_64"):
            with self.subTest(target=target):
                result, inputs = self.build_with_npm(target)
                resources = Path(result["resources"])
                manifest, _ = PREPARE.verify_resources(resources)
                group = manifest["npm_acquisition"]
                self.assertEqual(group["node"], "runtime/acquisition-node/bin/node")
                self.assertNotEqual(group["node"], manifest["node"])
                self.assertEqual((resources / group["npm_archive"]).read_bytes(), inputs["npm_archive"].read_bytes())
                self.assertEqual(result["npm_acquisition_inputs"]["npm_archive"]["sha256"], inputs["npm_archive_sha256"])
                self.assertEqual(result["npm_acquisition_inputs"]["runtime_metadata"]["version"], "v24.19.0")
                library_node = result["runtime_compatibility"]["runtime_metadata"]["node"]["version"]
                self.assertEqual(library_node, "v22.23.2" if target == "linux-x86_64" else "v24.19.0")
                self.assertIn("not_runtime_qualification", result["npm_acquisition_inputs"]["scope"])
                changed = resources / group["npm_archive"]
                changed.chmod(0o644); changed.write_bytes(b"altered selected archive")
                with self.assertRaises(PREPARE.ResourceError): PREPARE.verify_resources(resources)

    def test_npm_group_requires_complete_pins_and_current_explicit_starter_before_output(self):
        values = self.npm_inputs()
        for changes in ({"npm_archive_sha256": None}, {"acquisition_node_bytes": True},
                        {"npm_archive_sha256": "0" * 64}, {"acquisition_node_archive": Path("relative.tar.gz")}):
            with self.subTest(changes=changes), mock.patch.object(PACKAGE.subprocess, "run") as run:
                with self.assertRaises((ValueError, OSError)):
                    PACKAGE.build(self.root / "unused", self.root, Path("/unused/zstd"),
                                  **self.kwargs(), **(values | changes))
                run.assert_not_called(); self.assertFalse((self.root / "unused").exists())
        with self.assertRaisesRegex(ValueError, "explicit current starter"):
            PACKAGE.build(self.root / "unused", self.root, Path("/unused/zstd"), **values)
        self.assertFalse((self.root / "unused").exists())

    def prebuilt_policy(self, target="macos-arm64"):
        observed = self.root / ("target-observation-" + target)
        RuntimeCompatibilityTests().resources(observed, target, node="24.19.0")
        policy = {"schema_version": 1, "scope": "trusted_public_node_prebuilt_resources",
            "target": {"node_sha256": PACKAGE.digest(observed / "runtime/node/bin/node"),
                "node_version": "v24.19.0", "node_abi": "137",
                "platform": "linux" if target == "linux-x86_64" else "darwin",
                "arch": "x64" if target == "linux-x86_64" else "arm64",
                "libc": {"family": "glibc", "version": "2.39"} if target == "linux-x86_64" else None},
            "allowed_origins": ["https://github.com"],
            "allowed_redirect_origins": ["https://release-assets.githubusercontent.com"]}
        path = self.root / ("prebuilt-" + target + ".json")
        path.write_text(json.dumps(policy, indent=2) + "\n")
        return path, policy

    def test_explicit_node_prebuilt_policy_is_captured_in_both_resource_archives(self):
        for target in ("macos-arm64", "linux-x86_64"):
            with self.subTest(target=target):
                path, _ = self.prebuilt_policy(target)
                original = path.read_bytes()
                result, _ = self.build_with_npm(target, resource_options={
                    "node_prebuilt_policy": path, "node_prebuilt_policy_sha256": PACKAGE.digest(path)})
                resources = Path(result["resources"])
                manifest, _ = PREPARE.verify_resources(resources)
                selected = PREPARE.node_prebuilt_resources(resources, manifest)
                self.assertEqual(selected["path"].read_bytes(), original)
                self.assertEqual(selected["sha256"], hashlib.sha256(original).hexdigest())
                self.assertEqual(result["node_prebuilt_policy_input"]["sha256"], selected["sha256"])
                with tarfile.open(result["archive"], "r:gz") as archive:
                    name = "meerkat-connect-" + target + "/libexec/meerkat-connect/node-prebuilt-policy.json"
                    self.assertEqual(archive.extractfile(name).read(), original)
                path.write_bytes(b"changed source after captured packaging")
                self.assertEqual(selected["path"].read_bytes(), original)

    def test_node_prebuilt_wrong_target_and_boolean_schema_never_emit_archive(self):
        for target, change in (("macos-arm64", "node"), ("linux-x86_64", "schema")):
            with self.subTest(change=change):
                path, policy = self.prebuilt_policy(target)
                if change == "node": policy["target"]["node_sha256"] = "0" * 64
                else: policy["schema_version"] = True
                path.write_text(json.dumps(policy))
                with self.assertRaisesRegex(ValueError, "does not bind the selected target"):
                    self.build_with_npm(target, resource_options={
                        "node_prebuilt_policy": path, "node_prebuilt_policy_sha256": PACKAGE.digest(path)})
                self.assertFalse(list((self.root / ("output-" + target)).glob("*.tar.gz")))
                self.assertFalse((self.root / ("output-" + target) / "build.json").exists())

    def test_node_prebuilt_requires_exact_hash_and_selected_npm_before_output(self):
        path, _ = self.prebuilt_policy()
        values = self.npm_inputs()
        for options in ({"node_prebuilt_policy": path},
                        {"node_prebuilt_policy": path, "node_prebuilt_policy_sha256": "0" * 64}):
            with self.subTest(options=options), mock.patch.object(PACKAGE.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    PACKAGE.build(self.root / "unused", self.root, Path("/unused/zstd"),
                                  **self.kwargs(), **values, **options)
                run.assert_not_called()
                self.assertFalse((self.root / "unused").exists())
        with self.assertRaises(ValueError):
            PACKAGE.build(self.root / "unused", self.root, Path("/unused/zstd"), **self.kwargs(),
                          node_prebuilt_policy=path, node_prebuilt_policy_sha256=PACKAGE.digest(path))

    def test_node_build_capture_requires_pair_bounds_regular_bytes_and_selected_node(self):
        capture = PACKAGE.captured_node_build_profile
        self.assertIsNone(capture(None, None, selected_node=False))
        profile = self.root / "build-profile.json"
        raw = b'{ "external_tool": "/host/selected/tool", "literal": "unchanged" }\n'
        profile.write_bytes(raw)
        expected = hashlib.sha256(raw).hexdigest()
        self.assertEqual(capture(profile, expected, selected_node=True), raw)
        for path, sha, selected in ((profile, None, True), (None, expected, True),
                (profile, expected, False), (profile, "0" * 64, True),
                (profile, expected.upper(), True), (Path("relative.json"), expected, True)):
            with self.subTest(path=path, sha=sha, selected=selected), self.assertRaises((ValueError, OSError)):
                capture(path, sha, selected_node=selected)
        link = self.root / "build-profile-link.json"
        link.symlink_to(profile)
        with self.assertRaises(OSError):
            capture(link, expected, selected_node=True)
        for raw in (b"", b"x" * 65537):
            profile.write_bytes(raw)
            with self.assertRaises(ValueError):
                capture(profile, hashlib.sha256(raw).hexdigest(), selected_node=True)

    def test_node_build_profile_is_exact_inventory_without_internal_path_relocation(self):
        # The packager transports bytes. The backend pins the profile; its
        # semantic tool validation belongs to the installer at build review.
        raw = b'{ "external_tool": "/host/selected/tool", "literal": "unchanged" }\n'
        profile = self.root / "build-profile.json"
        profile.write_bytes(raw)
        expected = hashlib.sha256(raw).hexdigest()
        for target in ("macos-arm64", "linux-x86_64"):
            with self.subTest(target=target):
                result, _ = self.build_with_npm(target, resource_options={
                    "node_build_profile": profile, "node_build_profile_sha256": expected})
                resources = Path(result["resources"])
                manifest, _ = PREPARE.verify_resources(resources)
                selected = PREPARE.node_build_resources(resources, manifest)
                self.assertEqual(selected, {"path": resources / "node-build-profile.json", "sha256": expected})
                self.assertEqual(selected["path"].read_bytes(), raw)
                self.assertNotIn("node_build", manifest)
                self.assertEqual(result["node_build_profile_input"], {"path": str(profile),
                    "sha256": expected, "bytes": len(raw),
                    "scope": "exact_profile_bytes_external_toolchain_paths_remain_host_owned"})
                with tarfile.open(result["archive"], "r:gz") as archive:
                    name = "meerkat-connect-" + target + "/libexec/meerkat-connect/node-build-profile.json"
                    self.assertEqual(archive.extractfile(name).read(), raw)
                for change in ("no_node", "no_entry", "duplicate", "changed"):
                    altered = copy.deepcopy(manifest)
                    rows = [row for row in altered["entries"] if row["path"] == "node-build-profile.json"]
                    if change == "no_node": altered.pop("npm_acquisition")
                    elif change == "no_entry": altered["entries"].remove(rows[0])
                    elif change == "duplicate": altered["entries"].append(rows[0])
                    else: rows[0]["sha256"] = "0" * 64
                    with self.subTest(change=change), self.assertRaises(PREPARE.ResourceError):
                        PREPARE.node_build_resources(resources, altered)
        profile.write_bytes(b"source changed after packaging")
        self.assertEqual(selected["path"].read_bytes(), raw)

    def test_node_build_cli_preserves_explicit_pair(self):
        values = self.kwargs() | self.npm_inputs() | {
            "node_build_profile": self.root / "build-profile.json", "node_build_profile_sha256": "a" * 64}
        argv = ["package", "--destination", "/unused/output", "--upstream", "/unused/upstream", "--zstd", "/unused/zstd"]
        for key, value in values.items(): argv.extend(["--" + key.replace("_", "-"), str(value)])
        with mock.patch("sys.argv", argv), mock.patch.object(PACKAGE, "build", return_value={}) as build:
            with contextlib.redirect_stdout(io.StringIO()): PACKAGE.main()
        self.assertEqual(build.call_args.kwargs, {"target": "macos-arm64", **values})

    def test_npm_owner_refuses_wrong_archive_version_and_unsupported_acquisition_node(self):
        with self.assertRaisesRegex(ValueError, "npm_tool_version"):
            self.build_with_npm(npm_version="11.17.0")
        self.assertFalse((self.root / "output-macos-arm64").exists())
        # Use a separate target so the earlier inert input directory is retained.
        with self.assertRaisesRegex(ValueError, "unsupported by the npm owner"):
            self.build_with_npm("linux-x86_64", version="22.23.2")
        self.assertFalse((self.root / "output-linux-x86_64/build.json").exists())

    def test_npm_group_explicit_cli_passes_archive_pins_without_runtime_execution(self):
        values = self.kwargs() | self.npm_inputs()
        argv = ["package", "--destination", "/unused/output", "--upstream", "/unused/upstream", "--zstd", "/unused/zstd"]
        for key, value in values.items(): argv.extend(["--" + key.replace("_", "-"), str(value)])
        with mock.patch("sys.argv", argv), mock.patch.object(PACKAGE, "build", return_value={}) as build:
            with contextlib.redirect_stdout(io.StringIO()): PACKAGE.main()
        self.assertEqual(build.call_args.kwargs, {"target": "macos-arm64", **values})

    def test_library_path_swap_cannot_change_compatibility_or_packaged_bytes(self):
        upstream = self.root / "upstream"; upstream.mkdir()
        selected = copy.deepcopy(PACKAGE.TARGETS)
        for kind in ("python", "node"):
            filename = selected["macos-arm64"][kind][0]
            path = upstream / filename; path.write_bytes(("inert " + kind + " archive").encode())
            selected["macos-arm64"][kind] = (filename, PACKAGE.digest(path), path.stat().st_size)
        pip = upstream / "pip.pyz"; pip.write_bytes(b"inert pip source")
        def unpack(archive, resources, mapping, **kwargs):
            if archive.name == "python.tar":
                RuntimeCompatibilityTests().resources(resources, "macos-arm64", node="24.19.0", npm="11.17.0")
                cli = resources / "runtime/node/lib/node_modules/npm/bin/npm-cli.js"
                cli.parent.mkdir(parents=True); cli.write_bytes(b"inert npm cli")
        inspect = PACKAGE.runtime_compatibility
        for selected_version, replacement_version in ((1, 3), (3, 1)):
            with self.subTest(selected_version=selected_version):
                # A valid replacement must not rescue an invalid selection;
                # an invalid replacement must not alter a valid selection.
                RuntimeCompatibilityTests().library(self.archive, {"schema_version": replacement_version})
                replacement = self.archive.read_bytes()
                RuntimeCompatibilityTests().library(self.archive, {"schema_version": selected_version})
                original = self.archive.read_bytes()
                self.manifest["assets"][0].update(sha256=PACKAGE.digest(self.archive), bytes=len(original))
                self.repin_manifest()
                calls = []
                def replace_path_during_inspection(*args, **kwargs):
                    self.archive.write_bytes(replacement)
                    calls.append(PACKAGE.digest(self.archive))
                    try:
                        return inspect(*args, **kwargs)
                    finally:
                        self.archive.write_bytes(original)
                destination = self.root / ("swap-output-" + str(selected_version))
                with mock.patch.object(PACKAGE, "TARGETS", selected), mock.patch.object(
                        PACKAGE, "PIP", ("pip.pyz", PACKAGE.digest(pip), pip.stat().st_size)), mock.patch.object(
                        PACKAGE.subprocess, "run") as run, mock.patch.object(PACKAGE, "extract", side_effect=unpack), \
                        mock.patch.object(PACKAGE, "runtime_compatibility", side_effect=replace_path_during_inspection):
                    if selected_version == 1:
                        result = PACKAGE.build(destination, upstream, Path("/inert/zstd"), **self.kwargs())
                    else:
                        with self.assertRaisesRegex(ValueError, "unsupported selected library runtime contract"):
                            PACKAGE.build(destination, upstream, Path("/inert/zstd"), **self.kwargs())
                run.assert_called_once()
                self.assertEqual(calls, [hashlib.sha256(replacement).hexdigest()])
                self.assertEqual(self.archive.read_bytes(), original)
                self.assertNotEqual(hashlib.sha256(original).hexdigest(), calls[0])
                if selected_version == 1:
                    resources = Path(result["resources"])
                    manifest, _ = PREPARE.verify_resources(resources)
                    self.assertEqual((resources / manifest["archive"]).read_bytes(), original)
                    observed = json.loads((resources / "notices/runtime-compatibility.json").read_bytes())
                    self.assertEqual(observed["library_schema_version"], 1)
                    self.assertEqual(observed["library_contents_sha256"],
                                     hashlib.sha256(json.dumps({"schema_version": 1}).encode()).hexdigest())
                else:
                    self.assertFalse((destination / "build.json").exists())
                    self.assertFalse(list(destination.glob("*.tar.gz")))

    def test_explicit_cli_passes_whole_selection_and_legacy_call_shape_stays_unchanged(self):
        expected = self.kwargs()
        argv = ["package", "--destination", "/unused/output", "--upstream", "/unused/upstream", "--zstd", "/unused/zstd"]
        for key, value in expected.items():
            argv.extend(["--" + key.replace("_", "-"), str(value)])
        with mock.patch("sys.argv", argv), mock.patch.object(PACKAGE, "build", return_value={}) as build:
            with contextlib.redirect_stdout(io.StringIO()): PACKAGE.main()
        self.assertEqual(build.call_args.kwargs, {"target": "macos-arm64", **expected})


class RuntimeCompatibilityTests(unittest.TestCase):
    LINUX = {
        "python": {"implementation": "cpython", "version": "3.11.16", "platform": "linux",
                   "machine": "x86_64", "pointer_bits": 64, "glibc": "glibc 2.39"},
        "node": {"version": "v22.23.2", "platform": "linux", "arch": "x64", "npm_version": "10.9.8"},
    }

    def resources(self, root, target, *, node="22.23.2", npm="10.9.8"):
        # These 64-byte headers are inert data, not executable runtime fixtures.
        header = bytearray(64)
        if target == "linux-x86_64":
            header[:7] = b"\x7fELF\x02\x01\x01"
            header[16:20] = bytes.fromhex("02003e00")
            platform, triple = "linux", "x86_64-unknown-linux-gnu"
        else:
            header[:8] = bytes.fromhex("cffaedfe0c000001")
            header[12:16] = bytes.fromhex("02000000")
            platform, triple = "darwin", "aarch64-apple-darwin"
        metadata = {"version": "8", "target_triple": triple, "python_implementation_name": "cpython",
                    "python_major_minor_version": "3.11", "python_exe": "install/bin/python3.11",
                    "python_version": "3.11.16", "python_config_vars": {
                        "SIZEOF_VOID_P": "8", "HOST_GNU_TYPE": triple, "MACHDEP": platform}}
        major, minor, patch = node.split(".")
        files = {
            "notices/python/PYTHON.json": json.dumps(metadata).encode(),
            "runtime/python/bin/python3.11": bytes(header),
            "runtime/node/bin/node": bytes(header),
            "runtime/node/include/node/node_version.h": (
                f"#define NODE_MAJOR_VERSION {major}\n#define NODE_MINOR_VERSION {minor}\n"
                f"#define NODE_PATCH_VERSION {patch}\n#define NODE_VERSION_IS_RELEASE 1\n").encode(),
            "runtime/node/lib/node_modules/npm/package.json": json.dumps({"name": "npm", "version": npm}).encode(),
        }
        for name, data in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)

    def library(self, path, contents, *, duplicate=False):
        raw = json.dumps(contents).encode() if isinstance(contents, dict) else contents
        with tarfile.open(path, "w:gz") as tar:
            member = tarfile.TarInfo(path.name.removesuffix(".tar.gz") + "/consumers/CONTENTS.json")
            member.size = len(raw)
            for _ in range(2 if duplicate else 1):
                tar.addfile(member, io.BytesIO(raw))

    def check(self, archive, resources, target, host=None):
        with mock.patch.object(PACKAGE.subprocess, "run") as run:
            try:
                return PACKAGE.runtime_compatibility(archive, resources, target,
                    PACKAGE.SUPPORTED_HOSTS[target] if host is None else host, PREPARE.decode)
            finally:
                run.assert_not_called()

    def test_linux_matches_complete_tuple_but_does_not_claim_host_execution(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            self.resources(root, "linux-x86_64")
            archive = root / "library.tar.gz"
            self.library(archive, {"schema_version": 2, "runtime_target": self.LINUX})
            result = self.check(archive, root, "linux-x86_64")
            expected_metadata = {**self.LINUX, "python": {
                key: value for key, value in self.LINUX["python"].items() if key != "glibc"}}
            self.assertEqual(result["runtime_metadata"], expected_metadata)
            self.assertEqual(result["supported_host"]["glibc"], "glibc 2.39")
            self.assertIn("not_runtime_qualification", result["scope"])

    def test_linux_refuses_node_and_npm_mismatch_independently(self):
        for node, npm in [("24.19.0", "10.9.8"), ("22.23.2", "11.17.0"), ("24.19.0", "11.17.0")]:
            with self.subTest(node=node, npm=npm), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch)
                self.resources(root, "linux-x86_64", node=node, npm=npm)
                archive = root / "library.tar.gz"
                self.library(archive, {"schema_version": 2, "runtime_target": self.LINUX})
                with self.assertRaisesRegex(ValueError, "Node/npm metadata differs"):
                    self.check(archive, root, "linux-x86_64")

    def test_linux_refuses_both_older_and_newer_declared_glibc_and_missing_host_field(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            self.resources(root, "linux-x86_64")
            archive = root / "library.tar.gz"
            self.library(archive, {"schema_version": 2, "runtime_target": self.LINUX})
            for version in ["glibc 2.36", "glibc 2.40"]:
                with self.subTest(glibc=version), self.assertRaisesRegex(ValueError, "declared host profile differs"):
                    self.check(archive, root, "linux-x86_64", {**PACKAGE.SUPPORTED_HOSTS["linux-x86_64"], "glibc": version})
            with self.assertRaisesRegex(ValueError, "complete declared Linux host"):
                self.check(archive, root, "linux-x86_64", {"platform": "linux", "machine": "x86_64", "pointer_bits": 64})

    def test_mac_preserves_exact_legacy_versions_without_inventing_an_npm_pin(self):
        for node in ["24.19.0", "22.19.0", "24.20.0"]:
            with self.subTest(node=node), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch)
                self.resources(root, "macos-arm64", node=node, npm="11.17.0")
                archive = root / "library.tar.gz"
                self.library(archive, {"schema_version": 1})
                if node == "24.20.0":
                    with self.assertRaisesRegex(ValueError, "legacy Mac"):
                        self.check(archive, root, "macos-arm64")
                else:
                    result = self.check(archive, root, "macos-arm64")
                    self.assertEqual(result["library_schema_version"], 1)
                    self.assertNotIn("glibc", result["runtime_metadata"]["python"])

    def test_metadata_and_binary_architecture_must_agree_for_each_target(self):
        changes = [
            ("notices/python/PYTHON.json", lambda b: b.replace(b'"SIZEOF_VOID_P": "8"', b'"SIZEOF_VOID_P": "4"')),
            ("notices/python/PYTHON.json", lambda b: b.replace(b'"python_version": "3.11.16"', b'"python_version": "3.12.0"')),
            ("runtime/python/bin/python3.11", lambda b: b"invalid" + b[7:]),
            ("runtime/node/bin/node", lambda b: b"invalid" + b[7:]),
            ("runtime/node/include/node/node_version.h", lambda b: b + b"#define NODE_MAJOR_VERSION 99\n"),
            ("runtime/node/lib/node_modules/npm/package.json", lambda b: b'{"name":"npm","version":"10.9.8","version":"11.17.0"}'),
        ]
        for target in PACKAGE.TARGETS:
            for name, alter in changes:
                with self.subTest(target=target, field=name), tempfile.TemporaryDirectory() as scratch:
                    root = Path(scratch)
                    self.resources(root, target)
                    path = root / name
                    path.write_bytes(alter(path.read_bytes()))
                    with self.assertRaises(ValueError):
                        PACKAGE.runtime_metadata(root, target, PREPARE.decode)

    def test_unknown_incomplete_ambiguous_or_legacy_override_contract_refuses(self):
        cases = [{"schema_version": 3}, {"schema_version": True}, {"schema_version": 2},
                 {"schema_version": 1, "runtime_target": self.LINUX},
                 {"schema_version": 2, "runtime_target": {**self.LINUX, "extra": "ignored?"}},
                 b'{"schema_version":1,"schema_version":2}',
                 {"schema_version": 2, "runtime_target": {**self.LINUX,
                    "python": {**self.LINUX["python"], "pointer_bits": 64.0}}}]
        for contents in cases:
            with self.subTest(contents=contents), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch)
                self.resources(root, "linux-x86_64")
                archive = root / "library.tar.gz"
                self.library(archive, contents)
                with self.assertRaises(ValueError):
                    self.check(archive, root, "linux-x86_64")
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            self.resources(root, "linux-x86_64")
            archive = root / "library.tar.gz"
            self.library(archive, {"schema_version": 2, "runtime_target": self.LINUX}, duplicate=True)
            with self.assertRaisesRegex(ValueError, "one bounded regular CONTENTS"):
                self.check(archive, root, "linux-x86_64")

    def test_build_refuses_retained_linux_mismatch_before_output_archive_or_success_receipt(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            selected = PACKAGE.TARGETS["linux-x86_64"]
            archive = root / "dist" / selected["library_dir"] / selected["library"][0]
            archive.parent.mkdir(parents=True)
            self.library(archive, {"schema_version": 2, "runtime_target": self.LINUX})
            starter = root / "dist/toolkit-setup-20261010-resource-selection-export"
            starter.mkdir()
            for name in PACKAGE.STARTER:
                (starter / name).write_bytes(b"inert input")
            destination = root / "output"
            with mock.patch.object(PACKAGE, "ROOT", root), mock.patch.object(PACKAGE, "pin"), \
                    mock.patch.object(PACKAGE, "require_case_sensitive"), \
                    mock.patch.object(PACKAGE.subprocess, "run") as decompress, \
                    mock.patch.object(PACKAGE, "extract", side_effect=lambda archive, output, mapping, **kw:
                        self.resources(output, "linux-x86_64", node="24.19.0", npm="11.17.0")):
                with self.assertRaisesRegex(ValueError, "Node/npm metadata differs"):
                    PACKAGE.build(destination, root / "upstream", root / "unused-zstd", target="linux-x86_64")
            decompress.assert_called_once()  # Mocked host zstd only; no runtime execution.
            self.assertTrue(destination.is_dir(), "failed staging is retained")
            self.assertFalse((destination / "build.json").exists())
            self.assertFalse(list(destination.glob("*.tar.gz")))


class PreparationTargetTests(unittest.TestCase):
    def resource_fixture(self, root, target):
        # Data-only files deliberately cannot be an executable Python or starter.
        for name in ("prepare.py", "python", "node", "npm.js", "start.py", "library.tar.gz"):
            (root / name).write_bytes(b"inert fixture, never executed")
        value = {"schema_version": 1, "kind": "meerkat_connect_resources", "target": target,
            "prepare": "prepare.py", "python": "python", "node": "node", "npm_cli": "npm.js",
            "starter": "start.py", "archive": "library.tar.gz", "entries": PREPARE.tree_entries(root)}
        (root / "resources.json").write_text(json.dumps(value))
        return value

    def test_both_manifest_targets_validate_without_executing_runtimes(self):
        for target in ("macos-arm64", "linux-x86_64"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch).resolve()
                expected = self.resource_fixture(root, target)
                self.assertEqual(PREPARE.verify_resources(root)[0], expected)

    def test_unknown_manifest_target_refuses(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            self.resource_fixture(root, "linux-arm64")
            with self.assertRaisesRegex(PREPARE.ResourceError, "unsupported_resource_manifest"):
                PREPARE.verify_resources(root)

    def test_manifest_must_match_actual_host_before_interpreter_or_starter_entry(self):
        cases = (("macos-arm64", "Linux", "x86_64"), ("linux-x86_64", "Darwin", "arm64"),
                 ("linux-x86_64", "Linux", "aarch64"), ("linux-x86_64", "Windows", "AMD64"))
        for target, system, machine in cases:
            with self.subTest(target=target, system=system, machine=machine), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch).resolve()
                self.resource_fixture(root, target)
                destination = root.parent / (root.name + "-unused")
                with mock.patch.object(PREPARE.platform, "system", return_value=system), mock.patch.object(
                        PREPARE.platform, "machine", return_value=machine):
                    with self.assertRaisesRegex(PREPARE.ResourceError, "unsupported_product_target"):
                        PREPARE.prepare(root, destination, threading.Event())
                self.assertFalse(destination.exists())

    def test_matching_hosts_still_require_the_supplied_interpreter(self):
        for target, system, machine in (("macos-arm64", "Darwin", "arm64"), ("linux-x86_64", "Linux", "x86_64")):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch).resolve()
                self.resource_fixture(root, target)
                with mock.patch.object(PREPARE.platform, "system", return_value=system), mock.patch.object(
                        PREPARE.platform, "machine", return_value=machine):
                    with self.assertRaisesRegex(PREPARE.ResourceError, "invoke_with_bundled_python"):
                        PREPARE.prepare(root, root / "unused", threading.Event())

    def test_linux_target_does_not_bypass_exact_closure_validation(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            self.resource_fixture(root, "linux-x86_64")
            (root / "start.py").write_bytes(b"changed")
            with self.assertRaisesRegex(PREPARE.ResourceError, "resource_closure_changed"):
                PREPARE.verify_resources(root)


if __name__ == "__main__":
    unittest.main(verbosity=2)
