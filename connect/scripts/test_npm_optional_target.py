"""Conditional npm graphs use exact target facts and retain every archive.

All payloads and npm effects below are inert fixtures, not package execution.
"""
import copy
import json
from pathlib import Path
import sys
import tarfile
import unittest
from unittest.mock import patch

import test_selected_node_install as F

M = F.MODULE
ROOT = "node_modules/@outside/server"
TARGET = {"node_sha256": "a" * 64, "node_version": "v24.19.0", "node_abi": "137",
          "platform": "linux", "arch": "x64", "libc": {"family": "glibc", "version": "2.39"}}


def inputs(packages, target=TARGET):
    project = {"name": "consumer-fixture", "version": "1.0.0", "private": True,
               "dependencies": {"@outside/server": "file:./artifact.tgz"}}
    rows = {"": dict(project)}
    files = {"closure/package-json": M.encoded(project), M.NPM_TARGET_INPUT: M.encoded(target)}
    for index, (path, (metadata, optional)) in enumerate(packages.items()):
        name = M.npm_location(path)
        metadata = {"name": name, "version": "1.0.0", **metadata}
        raw = F.package(name, "1.0.0", {"package.json": M.encoded(metadata), "dist/main.js": b"// inert\n"})
        key = "artifact.tgz" if path == ROOT else "closure/archive-" + str(index)
        files[key] = raw
        rows[path] = {"version": "1.0.0", "integrity": F.integrity(raw),
                      "resolved": "file:artifact.tgz" if path == ROOT else "https://registry.npmjs.org/" + str(index) + ".tgz",
                      **{key: value for key, value in metadata.items() if key not in ("name", "version")},
                      **({"optional": True} if optional else {})}
    lock = {"name": project["name"], "version": project["version"], "lockfileVersion": 3, "packages": rows}
    files["closure/package-lock"] = M.encoded(lock)
    return files


def sample():
    return {
        ROOT: ({"dependencies": {"shared": "1.0.0"}, "optionalDependencies": {"linux-addon": "1.0.0", "mac-addon": "1.0.0"}}, False),
        "node_modules/shared": ({}, False),
        "node_modules/linux-addon": ({"os": ["linux"], "cpu": ["x64"], "libc": ["glibc"]}, True),
        "node_modules/mac-addon": ({"os": ["darwin"], "dependencies": {"child": "1.0.0", "shared": "1.0.0"}}, True),
        "node_modules/mac-addon/node_modules/child": ({}, True),
    }


class OptionalTargetTests(unittest.TestCase):
    def test_target_filters_optional_subtree_but_preserves_shared_required_and_all_archives(self):
        files = inputs(sample())
        original = copy.deepcopy(files)
        graph = M.npm_archive_closure(files)
        self.assertEqual(set(graph["rows"]), {ROOT, "node_modules/shared", "node_modules/linux-addon"})
        self.assertEqual(graph["excluded_optional_packages"], {
            "node_modules/mac-addon": "platform_mismatch",
            "node_modules/mac-addon/node_modules/child": "only_reachable_from_excluded_optional_branch"})
        self.assertEqual(len(graph["archives"]), 5)
        self.assertEqual(files, original)

    def test_incompatible_required_child_excludes_parent_to_optional_boundary(self):
        packages = {ROOT: ({"optionalDependencies": {"outer": "1.0.0"}}, False),
                    "node_modules/outer": ({"dependencies": {"inner": "1.0.0"}}, True),
                    "node_modules/outer/node_modules/inner": ({"os": ["darwin"]}, True)}
        graph = M.npm_archive_closure(inputs(packages))
        self.assertEqual(set(graph["rows"]), {ROOT})
        self.assertEqual(graph["excluded_optional_packages"]["node_modules/outer"], "required_dependency_platform_mismatch")

    def test_required_edge_wins_over_optional_flag_or_other_optional_path(self):
        packages = sample()
        packages[ROOT][0]["dependencies"]["mac-addon"] = "1.0.0"
        # Same-name optional overrides are tested separately. This required
        # route is through another required parent, not the optional root edge.
        packages[ROOT][0]["dependencies"]["required-parent"] = "1.0.0"
        packages["node_modules/required-parent"] = ({"dependencies": {"mac-addon": "1.0.0"}}, False)
        for optional in (True, False):
            packages["node_modules/mac-addon"] = (packages["node_modules/mac-addon"][0], optional)
            with self.subTest(optional=optional), self.assertRaises(ValueError):
                M.npm_archive_closure(inputs(packages))

    def test_optional_overrides_same_name_and_nested_version_uses_actual_location(self):
        packages = {ROOT: ({"dependencies": {"outer": "1.0.0", "shared": "1.0.0"},
                           "optionalDependencies": {"outer": "1.0.0"}}, False),
                    "node_modules/shared": ({}, False),
                    "node_modules/outer": ({"dependencies": {"shared": "1.0.0"}}, True),
                    "node_modules/outer/node_modules/shared": ({"cpu": ["arm64"]}, True)}
        graph = M.npm_archive_closure(inputs(packages))
        self.assertEqual(set(graph["rows"]), {ROOT, "node_modules/shared"})
        self.assertIn("node_modules/outer", graph["excluded_optional_packages"])

    def test_bound_target_is_required_and_lock_cannot_forge_platform_or_required_edges(self):
        files = inputs(sample())
        with self.assertRaisesRegex(ValueError, "bound target"):
            M.npm_archive_closure({k: v for k, v in files.items() if k != M.NPM_TARGET_INPUT})
        for field, value in (("os", ["linux"]), ("dependencies", {}), ("optional", False)):
            lock = M.unique_json(files["closure/package-lock"])
            lock["packages"]["node_modules/mac-addon"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                M.npm_archive_closure({**files, "closure/package-lock": M.encoded(lock)})

    def test_conditional_metadata_cannot_be_hidden_by_omitting_lock_fields(self):
        for field, value in (("os", ["linux"]), ("libc", [])):
            files = inputs({ROOT: ({field: value}, False)})
            lock = M.unique_json(files["closure/package-lock"])
            del lock["packages"][ROOT][field]
            files["closure/package-lock"] = M.encoded(lock)
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "bound target"):
                M.npm_archive_closure({k: v for k, v in files.items() if k != M.NPM_TARGET_INPUT})
            with self.subTest(field=field), self.assertRaises(ValueError):
                M.npm_archive_closure(files)
        for field in ("os", "cpu", "libc"):
            with self.subTest(empty=field), self.assertRaisesRegex(ValueError, "platform restriction"):
                M.npm_archive_closure(inputs({ROOT: ({field: []}, False)}))

    def test_required_platform_mismatch_and_unknown_libc_refuse(self):
        packages = {ROOT: ({"os": ["darwin"]}, False)}
        with self.assertRaisesRegex(ValueError, "required npm dependency"):
            M.npm_archive_closure(inputs(packages))
        for target in ({**TARGET, "libc": None}, {**TARGET, "libc": {"family": "musl", "version": "1.2"}}):
            with self.subTest(target=target), self.assertRaises(ValueError):
                M.npm_archive_closure(inputs(sample(), target))

    def test_allow_deny_and_any_follow_pinned_npm_platform_semantics(self):
        for rule, selected in ((["linux"], True), (["!darwin"], True), (["linux", "!linux"], False),
                               (["darwin"], False), ("linux", True), (["any"], True)):
            packages = {ROOT: ({"optionalDependencies": {"child": "1.0.0"}}, False),
                        "node_modules/child": ({"os": rule}, True)}
            with self.subTest(rule=rule):
                self.assertEqual("node_modules/child" in M.npm_archive_closure(inputs(packages))["rows"], selected)

    def test_libc_restriction_with_no_libc_is_not_satisfied_even_by_any(self):
        target = {**TARGET, "platform": "darwin", "arch": "arm64", "libc": None}
        packages = {ROOT: ({"optionalDependencies": {"child": "1.0.0"}}, False),
                    "node_modules/child": ({"libc": ["any"]}, True)}
        graph = M.npm_archive_closure(inputs(packages, target))
        self.assertEqual(set(graph["rows"]), {ROOT})
        self.assertEqual(graph["excluded_optional_packages"], {"node_modules/child": "platform_mismatch"})

    def test_excluded_archive_is_still_integrity_checked_and_bounded(self):
        files = inputs(sample())
        files["closure/archive-3"] = F.package("mac-addon", "9.0.0")
        with self.assertRaises(ValueError):
            M.npm_archive_closure(files)
        with patch.object(M, "MAX_NPM_MEMBERS", 9), self.assertRaises(ValueError):
            M.npm_archive_closure(inputs(sample()))

    def test_peer_missing_dependency_and_unreachable_rows_are_explicit_refusals(self):
        for change in ("peer", "missing", "unreachable"):
            packages = sample()
            if change == "peer":
                packages[ROOT][0]["peerDependencies"] = {"shared": "*"}
            elif change == "missing":
                del packages["node_modules/shared"]
            else:
                packages["node_modules/unrelated"] = ({}, True)
            with self.subTest(change=change), self.assertRaises(ValueError):
                M.npm_archive_closure(inputs(packages))

    def test_missing_compatible_optional_or_present_excluded_payload_never_verifies(self):
        f = F.SelectedNodeTests(); f.setUp(); self.addCleanup(f.doCleanups)
        graph = M.npm_archive_closure(inputs(sample()))
        dest = f.root / "installed"
        f.materialize(dest, graph)
        M.verify_selected_npm(dest, graph)
        path = dest / "node_modules/linux-addon/dist/main.js"
        original = path.read_bytes(); path.unlink()
        with self.assertRaises((OSError, ValueError)):
            M.verify_selected_npm(dest, graph)
        path.write_bytes(original)
        extra = dest / "node_modules/mac-addon/dist/main.js"
        extra.parent.mkdir(parents=True); extra.write_bytes(b"// inert\n")
        with self.assertRaisesRegex(ValueError, "file graph differs"):
            M.verify_selected_npm(dest, graph)

    def test_native_additions_must_share_target_and_name_a_selected_package(self):
        files = inputs(sample())
        source = {"schema_version": 1, "scope": "retained_node_package_inputs",
                  "primary": {"name": "@outside/server", "version": "1.0.0", "sha256": M.sha(files["artifact.tgz"])},
                  "files": {name: M.sha(raw) for name, raw in files.items()}}
        files[M.NODE_NATIVE_SOURCE] = M.encoded(source)
        binary = b"inert addon, never loaded"
        asset = F.tarball([("build/addon.node", binary, tarfile.REGTYPE, "")])
        key = "native-asset-" + M.sha(asset)
        manifest = {"schema_version": 1, "scope": "selected_node_native_payloads",
                    "source_receipt_sha256": M.sha(files[M.NODE_NATIVE_SOURCE]), "target": TARGET,
                    "payloads": [{"package_location": "node_modules/linux-addon",
                                  "package_archive_sha256": M.sha(files["closure/archive-2"]),
                                  "asset_id": key, "url": "https://releases.example.invalid/addon.tgz",
                                  "files": [{"member": "build/addon.node", "destination": "build/addon.node", "sha256": M.sha(binary)}]}]}
        files["closure/" + key] = asset
        files[M.NODE_NATIVE_MANIFEST] = M.encoded(manifest)
        graph = M.npm_archive_closure(files)
        self.assertEqual(graph["native"]["files"], {"node_modules/linux-addon/build/addon.node": binary})
        altered = copy.deepcopy(manifest); altered["target"]["node_abi"] = "999"
        with self.assertRaisesRegex(ValueError, "targets differ"):
            M.npm_archive_closure({**files, M.NODE_NATIVE_MANIFEST: M.encoded(altered)})
        altered = copy.deepcopy(manifest)
        altered["payloads"][0].update(package_location="node_modules/mac-addon",
                                      package_archive_sha256=M.sha(files["closure/archive-3"]))
        with self.assertRaisesRegex(ValueError, "package differs"):
            M.npm_archive_closure({**files, M.NODE_NATIVE_MANIFEST: M.encoded(altered)})

    def test_install_uses_target_before_npm_and_retains_exact_exclusions(self):
        f = F.SelectedNodeTests(); f.setUp(); self.addCleanup(f.doCleanups)
        target = {**TARGET, "node_sha256": M.node_runtime_hash(str(Path(sys.executable).resolve()))}
        f.files = {"package.json": f.files["package.json"], "activation.json": f.files["activation.json"],
                   **inputs(sample(), target)}
        f.project = M.unique_json(f.files["closure/package-json"])
        f.lock = M.unique_json(f.files["closure/package-lock"])
        f.bind()
        graph = M.selected_npm_closure(f.files, f.plan)
        def run(argv, cwd, env, label, timeout=900):
            if label == "node-runtime":
                return M.encoded([target["node_version"], target["platform"], target["arch"]])
            if label == "npm-install":
                f.calls.append(label); f.materialize(cwd, graph); return b""
            return f.fake_run(argv, cwd, env, label, timeout)
        f.args.destination = str(f.root / "mismatched-target")
        with patch.object(M, "observe_node_native_target", return_value={**target, "node_abi": "999"}), \
                patch.object(M, "run", side_effect=run), self.assertRaisesRegex(ValueError, "target differs"):
            M.install_selected_node(f.args)
        self.assertFalse(any(label.startswith("npm-") for label in f.calls))
        f.args.destination = str(f.destination)
        with patch.object(M, "observe_node_native_target", return_value=target), patch.object(M, "run", side_effect=run):
            result = M.install_selected_node(f.args)
        self.assertEqual(result["npm_target"], target)
        self.assertEqual(result["excluded_optional_packages"], graph["excluded_optional_packages"])
        self.assertEqual(f.calls.count("npm-install"), 1)
        self.assertEqual((f.destination / "package-lock.json").read_bytes(), f.files["closure/package-lock"])
        with patch.object(M, "readback_validator", return_value=M.encoded(f.plan)), patch.object(M, "run", side_effect=AssertionError("no donor/runtime execution")):
            proof = M.verify_existing_selected(f.args)
        self.assertEqual(proof["retained_payloads"]["excluded_optional_packages"], graph["excluded_optional_packages"])


if __name__ == "__main__":
    unittest.main()
