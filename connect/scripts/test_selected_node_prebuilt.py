"""Explicit native payload composition with inert archives; no binary is loaded."""
from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import sys
import tarfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("node_prebuilt_fixtures", Path(__file__).with_name("test_selected_node_install.py"))
N = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(N)
M = N.MODULE


def snapshot(root):
    return {str(p.relative_to(root)): p.read_bytes() for p in root.rglob("*") if p.is_file()}


class SelectedNodePrebuiltTests(unittest.TestCase):
    def setUp(self):
        self.f = N.SelectedNodeTests()
        self.f.setUp()
        self.addCleanup(self.f.doCleanups)
        self.f.software()
        component = M.npm_archive(self.f.files["artifact.tgz"])
        source = {"schema_version": 1, "scope": "retained_node_package_inputs",
                  "primary": {"name": component["name"], "version": component["version"],
                              "sha256": M.sha(self.f.files["artifact.tgz"])},
                  "files": {key: M.sha(raw) for key, raw in self.f.files.items()
                            if key == "artifact.tgz" or key.startswith("closure/")}}
        self.f.files[M.NODE_NATIVE_SOURCE] = M.encoded(source)
        self.binary = b"\x7fELF\x00inert native addon fixture, never loaded"
        self.target = {"node_sha256": M.node_runtime_hash(str(Path(sys.executable).resolve())),
                       "node_version": "v24.19.0", "node_abi": "137", "platform": "linux", "arch": "x64",
                       "libc": {"family": "glibc", "version": "2.39"}}
        asset = N.tarball([("build/Release/example.node", self.binary, tarfile.REGTYPE, "")])
        self.manifest = {"schema_version": 1, "scope": "selected_node_native_payloads",
                         "source_receipt_sha256": M.sha(self.f.files[M.NODE_NATIVE_SOURCE]),
                         "target": self.target,
                         "payloads": [{"package_location": "node_modules/outer",
                                       "package_archive_sha256": M.sha(self.f.files["closure/outer"]),
                                       "asset_id": "native-asset-" + M.sha(asset),
                                       "url": "https://releases.example.invalid/addon.tar.gz",
                                       "files": [{"member": "build/Release/example.node",
                                                  "destination": "build/Release/example.node",
                                                  "sha256": M.sha(self.binary)}]}]}
        self.f.files["closure/" + self.manifest["payloads"][0]["asset_id"]] = asset
        self.bind()

    def bind(self):
        self.f.files[M.NODE_NATIVE_MANIFEST] = M.encoded(self.manifest)
        self.f.bind()

    def closure(self):
        return M.selected_npm_closure(self.f.files, self.f.plan)

    def fake_run(self, argv, cwd, env, label, timeout=900):
        if label == "node-runtime":
            self.f.calls.append(label)
            return M.encoded([self.target["node_version"], self.target["platform"], self.target["arch"]])
        if label == "node-native-target":
            self.f.calls.append(label)
            self.assertEqual(argv, [str(Path(sys.executable).resolve()), "-p", M.NODE_NATIVE_TARGET_EXPRESSION])
            self.assertEqual(timeout, 30)
            self.assertEqual(set(env), {"PATH", "LANG", "TMPDIR"})
            return M.encoded({key: value for key, value in self.target.items() if key != "node_sha256"})
        return self.f.fake_run(argv, cwd, env, label, timeout)

    def install(self):
        with patch.object(M, "run", side_effect=self.fake_run):
            return M.install_selected_node(self.f.args)

    def check(self):
        before = snapshot(self.f.destination)
        with patch.object(M, "readback_validator", return_value=M.encoded(self.f.plan)), patch.object(M, "run") as no_runtime:
            result = M.verify_existing_selected(self.f.args)
        no_runtime.assert_not_called()
        self.assertEqual(snapshot(self.f.destination), before)
        return result

    def test_install_preserves_original_bytes_adds_exact_payload_and_reconciles_inertly(self):
        before = snapshot(self.f.bundle)
        receipt = self.install()
        self.assertEqual(snapshot(self.f.bundle), before)
        self.assertEqual(receipt["native_payloads"]["target"], self.target)
        self.assertEqual(receipt["native_payloads"]["source_receipt_sha256"], self.manifest["source_receipt_sha256"])
        added = self.f.destination / "node_modules/outer/build/Release/example.node"
        self.assertEqual(added.read_bytes(), self.binary)
        self.assertEqual(receipt["installed_graph"]["node_modules/outer"]["native_payloads"],
                         {"build/Release/example.node": M.sha(self.binary)})
        self.assertEqual(self.f.calls.count("npm-install"), 1)
        self.assertLess(self.f.calls.index("node-native-target"), self.f.calls.index("npm-install"))
        self.assertEqual(self.check()["retained_payloads"]["native_payloads"], receipt["native_payloads"])

    def test_missing_binary_is_not_silently_repaired_or_claimed_ready(self):
        self.install()
        (self.f.destination / "node_modules/outer/build/Release/example.node").unlink()
        with self.assertRaises(FileNotFoundError):
            self.check()

    def test_each_supplemental_input_and_installed_binary_is_bound(self):
        self.install()
        keys = [key for key in self.f.files if key.startswith("closure/native-")]
        for root in (self.f.bundle, self.f.destination / "inputs"):
            for key in keys:
                p = root / key
                raw = p.read_bytes()
                with self.subTest(root=root.name, key=key):
                    p.write_bytes(raw + b"changed")
                    with self.assertRaises(M.InstallError):
                        self.check()
                    p.write_bytes(raw)
        p = self.f.destination / "node_modules/outer/build/Release/example.node"
        p.write_bytes(self.binary + b"changed")
        with self.assertRaises(M.InstallError):
            self.check()

    def test_historical_receipt_cannot_rewrite_native_graph_or_target(self):
        receipt = self.install()
        path = self.f.destination / "ready/installed.json"
        mutations = [lambda r: r.pop("native_payloads"),
                     lambda r: r["native_payloads"]["target"].update(node_abi="127"),
                     lambda r: r["native_payloads"]["files"].clear(),
                     lambda r: r["installed_graph"]["node_modules/outer"]["native_payloads"].clear()]
        for change in mutations:
            value = copy.deepcopy(receipt)
            change(value)
            path.write_bytes(M.encoded(value))
            with self.assertRaises(M.InstallError):
                self.check()
        path.write_bytes(M.encoded(receipt))
        self.check()

    def test_current_runtime_hash_is_required_for_readback_without_execution(self):
        self.install()
        with patch.object(M, "node_runtime_hash", return_value="0" * 64), self.assertRaises(M.InstallError):
            self.check()

    def test_wrong_runtime_target_refuses_before_npm(self):
        observed = copy.deepcopy(self.target)
        observed["node_abi"] = "127"
        with patch.object(M, "observe_node_native_target", return_value=observed), patch.object(M, "run", side_effect=self.fake_run):
            with self.assertRaises(M.InstallError):
                M.install_selected_node(self.f.args)
        self.assertNotIn("npm-install", self.f.calls)
        self.assertFalse((self.f.destination / "ready").exists())

    def test_runtime_change_during_probe_and_after_install_refuses(self):
        with patch.object(M, "node_runtime_hash", side_effect=["a" * 64, "b" * 64]), patch.object(M, "run", return_value=M.encoded({k: v for k, v in self.target.items() if k != "node_sha256"})):
            with self.assertRaises(M.InstallError):
                M.observe_node_native_target("/unused/node", self.f.root, {})
        with patch.object(M, "observe_node_native_target", return_value=self.target), patch.object(M, "node_runtime_hash", return_value="0" * 64), patch.object(M, "run", side_effect=self.fake_run):
            with self.assertRaises(M.InstallError):
                M.install_selected_node(self.f.args)
        self.assertFalse((self.f.destination / "ready").exists())

    def test_target_schema_refuses_unsupported_or_ambiguous_fields(self):
        cases = [{**self.target, "platform": "win32"}, {**self.target, "arch": "arm64"},
                 {**self.target, "node_abi": 137}, {**self.target, "node_sha256": "not-a-hash"},
                 {**self.target, "libc": {"family": "musl", "version": "1.2"}},
                 {**self.target, "libc": None}, {**self.target, "extra": True}]
        for value in cases:
            with self.subTest(value=value), self.assertRaises(M.InstallError):
                M.checked_node_native_target(value)
        self.assertEqual(M.checked_node_native_target({**self.target, "platform": "darwin", "arch": "arm64", "libc": None})["platform"], "darwin")

    def test_source_receipt_and_original_archive_binding_cannot_be_rewritten(self):
        original = self.f.files[M.NODE_NATIVE_SOURCE]
        for change in (lambda s: s["files"].pop("closure/outer"),
                       lambda s: s.update(scope="installed"),
                       lambda s: s["primary"].update(sha256="0" * 64)):
            source = json.loads(original)
            change(source)
            self.f.files[M.NODE_NATIVE_SOURCE] = M.encoded(source)
            self.manifest["source_receipt_sha256"] = M.sha(self.f.files[M.NODE_NATIVE_SOURCE])
            self.bind()
            with self.assertRaises(M.InstallError):
                self.closure()
        self.f.files[M.NODE_NATIVE_SOURCE] = original
        self.manifest["source_receipt_sha256"] = M.sha(original)
        self.manifest["payloads"][0]["package_archive_sha256"] = "0" * 64
        self.bind()
        with self.assertRaises(M.InstallError):
            self.closure()

    def test_unknown_reserved_input_and_duplicate_or_missing_assets_refuse(self):
        original = copy.deepcopy(self.f.files)
        for key in (M.NODE_NATIVE_MANIFEST, M.NODE_NATIVE_SOURCE, "closure/" + self.manifest["payloads"][0]["asset_id"]):
            self.f.files = {k: v for k, v in original.items() if k != key}
            with self.assertRaises(M.InstallError):
                self.closure()
        self.f.files = {**original, "closure/native-asset-" + "0" * 64: b"unselected"}
        with self.assertRaises(M.InstallError):
            self.closure()
        self.f.files = original
        self.manifest["payloads"].append(copy.deepcopy(self.manifest["payloads"][0]))
        self.bind()
        with self.assertRaises(M.InstallError):
            self.closure()

    def test_overwrite_escape_and_case_alias_mappings_refuse(self):
        for path in ("package.json", "index.js/child.node", "../escape.node", "/absolute.node",
                     "node_modules/injected.node", "NODE_MODULES/injected.node",
                     "Node_Modules/injected.node", "PACKAGE.JSON", "Index.js/child.node"):
            with self.subTest(path=path):
                self.manifest["payloads"][0]["files"][0]["destination"] = path
                self.bind()
                with self.assertRaises(M.InstallError):
                    self.closure()

    def test_unmapped_members_digest_changes_and_nonaddon_rows_refuse(self):
        row = self.manifest["payloads"][0]
        for change in (lambda r: r["files"][0].update(member="absent.node"),
                       lambda r: r["files"][0].update(sha256="0" * 64),
                       lambda r: r["files"][0].update(destination="not-an-addon.txt"),
                       lambda r: r.update(package_location="node_modules/absent"),
                       lambda r: r.update(url="https://name:secret@example.invalid/file.tgz"),
                       lambda r: r.update(unrecognized=True)):
            changed = copy.deepcopy(row)
            change(changed)
            self.manifest["payloads"][0] = changed
            self.bind()
            with self.assertRaises(M.InstallError):
                self.closure()
        self.manifest["payloads"][0] = row
        raw = N.tarball([("build/Release/example.node", self.binary, tarfile.REGTYPE, ""),
                         ("unmapped.txt", b"unmapped", tarfile.REGTYPE, "")])
        self.f.files.pop("closure/" + row["asset_id"])
        row["asset_id"] = "native-asset-" + M.sha(raw)
        self.f.files["closure/" + row["asset_id"]] = raw
        self.bind()
        with self.assertRaises(M.InstallError):
            self.closure()

    def test_archive_rejects_traversal_links_special_files_and_aliases(self):
        cases = [[("../escape", b"x", tarfile.REGTYPE, "")],
                 [("link", b"", tarfile.SYMTYPE, "/tmp/elsewhere")],
                 [("link", b"", tarfile.LNKTYPE, "elsewhere")],
                 [("device", b"", tarfile.CHRTYPE, "")],
                 [("A/file.node", b"x", tarfile.REGTYPE, ""), ("a/other.node", b"y", tarfile.REGTYPE, "")],
                 [("file", b"x", tarfile.REGTYPE, ""), ("file/child", b"y", tarfile.REGTYPE, "")]]
        for entries in cases:
            with self.subTest(entries=entries), self.assertRaises(M.InstallError):
                M.node_native_archive(N.tarball(entries))

    def test_empty_directory_and_file_ancestry_collision_refuses_in_both_orders(self):
        entries = [("a/empty-child", b"", tarfile.DIRTYPE, ""), ("a", b"x", tarfile.REGTYPE, "")]
        for ordered in (entries, list(reversed(entries))):
            with self.subTest(ordered=ordered), self.assertRaises(M.InstallError):
                M.node_native_archive(N.tarball(ordered))

    def test_combined_size_and_count_refuse_during_accumulation(self):
        base_files, native = M.node_native_inputs(self.f.files)
        base = M.npm_archive_closure(base_files)
        size = sum(len(raw) for _, archive in base["rows"].values() for raw in archive["files"].values())
        selected = []
        class ObservedFiles(dict):
            def __getitem__(self, key):
                selected.append(key)
                return super().__getitem__(key)
        blobs = ObservedFiles({"one.node": b"one", "two.node": b"two", "three.node": b"three"})
        native["manifest"]["payloads"][0]["files"] = [
            {"member": key, "destination": key, "sha256": M.sha(value)} for key, value in blobs.items()]
        for constant, limit in (("MAX_TOTAL", size + 3), ("MAX_NPM_MEMBERS", len(base["paths"]) + 1)):
            selected.clear()
            with self.subTest(constant=constant), patch.object(M, constant, limit), patch.object(M, "node_native_archive", return_value=blobs):
                with self.assertRaises(M.InstallError):
                    M.compose_node_native_payloads(base, base_files, native)
                self.assertEqual(selected, ["one.node", "two.node"])

    def test_archive_limits_and_document_count_are_enforced(self):
        raw = N.tarball([("addon.node", self.binary, tarfile.REGTYPE, "")])
        for constant, value in (("MAX_FILE", 1), ("MAX_NPM_EXPANDED", 1), ("MAX_NPM_MEMBERS", 0)):
            with self.subTest(constant=constant), patch.object(M, constant, value), self.assertRaises(M.InstallError):
                M.node_native_archive(raw)
        self.manifest["payloads"] *= 17
        self.bind()
        with self.assertRaises(M.InstallError):
            self.closure()

    def test_exclusive_creation_never_overwrites_existing_file_or_follows_link(self):
        native = self.closure()["native"]
        path = self.f.destination / "node_modules/outer/build/Release/example.node"
        path.parent.mkdir(parents=True)
        path.write_bytes(b"owner-existing")
        with self.assertRaises(FileExistsError):
            M.install_node_native_payloads(self.f.destination, native)
        self.assertEqual(path.read_bytes(), b"owner-existing")
        path.unlink()
        outside = self.f.root / "outside"
        outside.write_bytes(b"untouched")
        path.symlink_to(outside)
        with self.assertRaises(FileExistsError):
            M.install_node_native_payloads(self.f.destination, native)
        self.assertEqual(outside.read_bytes(), b"untouched")


if __name__ == "__main__":
    unittest.main()
