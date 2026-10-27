"""Focused selected-Node boundary tests; fixtures do not prove npm or native readiness."""
from __future__ import annotations

import argparse
import base64
import copy
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("install-library-consumer.py").resolve()
SPEC = importlib.util.spec_from_file_location("selected_node_installer", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
ENV = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}


def tarball(entries):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name, data, kind, link in entries:
            member = tarfile.TarInfo(name)
            member.type = kind
            member.linkname = link
            member.size = len(data) if kind == tarfile.REGTYPE else 0
            archive.addfile(member, io.BytesIO(data))
    return gzip.compress(raw.getvalue(), mtime=0)


def package(name, version, payload=None, dependencies=None):
    metadata = {"name": name, "version": version}
    if dependencies:
        metadata["dependencies"] = dependencies
    files = {"package.json": MODULE.encoded(metadata),
             **(payload or {"index.js": b"// Inert test archive.\n"})}
    return tarball([("package/" + path, data, tarfile.REGTYPE, "") for path, data in files.items()])


def integrity(raw):
    return "sha512-" + base64.b64encode(hashlib.sha512(raw).digest()).decode()


class SelectedNodeTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="toolkit-selected-node-test-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.bundle = self.root / "bundle"
        self.bundle.mkdir()
        self.destination = self.root / "consumer"
        component = package("@outside/server", "1.0.0", {"dist/main.js": b"// Must never be executed by installation.\n"},
                            {"outer": "1.0.0", "nested": "2.0.0"})
        self.files = {
            "package.json": b'{"fixture":"the Rust owner is independently tested"}\n',
            "activation.json": b'{"fixture":"explicit operator-selected input"}\n',
            "artifact.tgz": component,
            "closure/outer": package("outer", "1.0.0", dependencies={"nested": "1.0.0"}),
            "closure/nested-one": package("nested", "1.0.0"),
            "closure/nested-two": package("nested", "2.0.0"),
        }
        self.project = {"name": "consumer-fixture", "version": "1.0.0", "private": True,
                        "dependencies": {"@outside/server": "file:./artifact.tgz"}}
        self.rows = {"": {"name": "consumer-fixture", "version": "1.0.0", "dependencies": dict(self.project["dependencies"])}}
        for path, blob in (("node_modules/@outside/server", "artifact.tgz"),
                           ("node_modules/outer", "closure/outer"),
                           ("node_modules/nested", "closure/nested-one"),
                           ("node_modules/@outside/server/node_modules/nested", "closure/nested-two")):
            data = MODULE.npm_archive(self.files[blob])
            self.rows[path] = {"version": data["version"], "integrity": integrity(self.files[blob]),
                               "resolved": "file:artifact.tgz" if blob == "artifact.tgz" else "https://example.invalid/" + blob.split("/")[-1] + ".tgz"}
        self.rows["node_modules/@outside/server"]["dependencies"] = {"outer": "1.0.0", "nested": "2.0.0"}
        self.rows["node_modules/outer"]["dependencies"] = {"nested": "1.0.0"}
        self.lock = {"name": "consumer-fixture", "version": "1.0.0", "lockfileVersion": 3, "packages": self.rows}
        self.plan = {"schema_version": 1, "component": "server", "manifest_digest": "d" * 64,
                     "entrypoint": {"executable": "dist/main.js", "arguments": ["literal ${HOME}; $(not-a-shell)"]},
                     "mcp_name": "outside/server"}
        self.bind()
        self.calls = []

    def bind(self):
        self.files["closure/package-json"] = MODULE.encoded(self.project)
        self.files["closure/package-lock"] = MODULE.encoded(self.lock)
        self.plan["artifact_digest"] = MODULE.sha(self.files["artifact.tgz"])
        self.plan["closure"] = {path.removeprefix("closure/"): MODULE.sha(raw)
                                for path, raw in self.files.items() if path.startswith("closure/")}
        for path, raw in self.files.items():
            target = self.bundle / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(raw)
        self.args = argparse.Namespace(selected_node=str(self.bundle), destination=str(self.destination),
                                       selection_sha256=MODULE.sha(self.files["declaration.json" if "document_kind" in self.plan else "activation.json"]),
                                       component="server", validator=sys.executable, node=sys.executable,
                                       npm_cli=sys.executable)
        if "document_kind" in self.plan:
            self.args.document_kind = "local_software_declaration"

    def software(self):
        # Metadata validation is controlled; preserve the real artifact closure.
        self.plan["document_kind"] = "local_software_declaration"
        self.plan["declaration_digest"] = self.plan.pop("manifest_digest")
        self.files["declaration.json"] = self.files.pop("activation.json")
        (self.bundle / "activation.json").unlink()
        self.bind()

    def materialize(self, target, closure):
        for path, (_, archive) in closure["rows"].items():
            for filename, raw in archive["files"].items():
                output = target / path / filename
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(raw)

    def fake_run(self, argv, cwd, env, label, timeout=900):
        self.calls.append(label)
        self.assertNotIn("HOME", env)
        self.assertNotIn("CODEX_HOME", env)
        if label == "validate-selection":
            software = "document_kind" in self.plan
            filename = "declaration.json" if software else "activation.json"
            flag = "--declaration" if software else "--selection"
            self.assertEqual(argv[argv.index(flag) + 1], str(cwd / "inputs" / filename))
            self.assertNotIn("--selection" if software else "--declaration", argv)
            self.assertEqual((cwd / "inputs" / filename).read_bytes(), self.files[filename])
            self.assertEqual((cwd / "inputs/package.json").read_bytes(), self.files["package.json"])
            self.assertIn("--component", argv)
            return MODULE.encoded(self.plan)
        if label == "node-runtime":
            return MODULE.encoded(["v22.19.0", "darwin", "arm64"])
        self.assertEqual(argv[2:4], ["--prefix", str(cwd)])
        self.assertEqual(env["npm_config_ignore_scripts"], "true")
        self.assertEqual(env["npm_config_offline"], "true")
        self.assertEqual(env["npm_config_bin_links"], "false")
        if label.startswith("npm-cache-"):
            archive = Path(argv[6])
            raw = archive.read_bytes()
            self.assertEqual(archive.parent, cwd / "npm-tarballs")
            self.assertEqual(archive.name, MODULE.sha(raw) + ".tgz")
            self.assertIn(raw, [value for key, value in self.files.items()
                                if key == "artifact.tgz" or (key.startswith("closure/")
                                   and key not in ("closure/package-json", "closure/package-lock"))])
        if label == "npm-install":
            self.assertIn("--bin-links=false", argv)
            self.materialize(cwd, MODULE.selected_npm_closure(self.files, self.plan))
        return b""

    def test_software_declaration_uses_same_installer_and_exact_receipt(self):
        self.software()
        with patch.object(MODULE, "run", side_effect=self.fake_run):
            result = MODULE.install_selected_node(self.args)
        self.assertEqual(result["document_kind"], "local_software_declaration")
        self.assertEqual(result["declaration_digest"], "d" * 64)
        self.assertNotIn("manifest_digest", result)
        self.assertIn("declaration.json", result["inputs"])
        self.assertNotIn("activation.json", result["inputs"])
        self.assertEqual(json.loads((self.destination / "ready/installed.json").read_bytes()), result)
        self.assertEqual(result["installed_graph"]["node_modules/@outside/server/node_modules/nested"]["version"], "2.0.0")
        self.assertEqual(self.calls[-1], "npm-install")

    def test_software_dual_identity_refuses_before_runtime_or_npm(self):
        self.software()
        self.plan["manifest_digest"] = "d" * 64
        with patch.object(MODULE, "run", side_effect=self.fake_run), self.assertRaises(MODULE.InstallError):
            MODULE.install_selected_node(self.args)
        self.assertEqual(self.calls, ["validate-selection"])
        self.assertFalse((self.destination / "ready").exists())

    def test_nested_versions_remain_distinct_and_launch_is_literal(self):
        with patch.object(MODULE, "run", side_effect=self.fake_run):
            result = MODULE.install_selected_node(self.args)
        graph = result["installed_graph"]
        self.assertEqual(graph["node_modules/nested"]["version"], "1.0.0")
        self.assertEqual(graph["node_modules/@outside/server/node_modules/nested"]["version"], "2.0.0")
        self.assertEqual(result["command"], [str(Path(sys.executable).resolve()),
                         str(self.destination / "node_modules/@outside/server/dist/main.js"),
                         "literal ${HOME}; $(not-a-shell)"])
        ready = self.destination / "ready"
        self.assertEqual(json.loads((ready / "mcp.json").read_bytes()), result["mcp_configuration"])
        self.assertEqual(json.loads((ready / "installed.json").read_bytes()), result)
        self.assertNotIn("env", result["mcp_configuration"]["mcpServers"]["outside/server"])
        self.assertIn("does not clear ambient environment", result["launch_environment"])
        self.assertEqual(self.calls[-1], "npm-install")

    def test_raw_selection_change_is_refused_before_any_child_or_destination(self):
        (self.bundle / "activation.json").write_bytes(b"replacement")
        with patch.object(MODULE, "run") as run, self.assertRaises(ValueError):
            MODULE.install_selected_node(self.args)
        run.assert_not_called()
        self.assertFalse(self.destination.exists())

    def test_archive_lock_and_nested_blob_changes_are_refused_before_npm(self):
        for index, path in enumerate(("artifact.tgz", "closure/package-lock", "closure/nested-two")):
            original = (self.bundle / path).read_bytes()
            (self.bundle / path).write_bytes(original + b"changed")
            self.args.destination = str(self.root / ("mutation-" + str(index)))
            self.calls.clear()
            with self.subTest(path=path), patch.object(MODULE, "run", side_effect=self.fake_run), self.assertRaises(ValueError):
                MODULE.install_selected_node(self.args)
            self.assertEqual(self.calls, ["validate-selection"])
            self.assertFalse((Path(self.args.destination) / "ready").exists())
            (self.bundle / path).write_bytes(original)

    def test_existing_destination_and_nonabsolute_runtime_are_refused(self):
        self.destination.mkdir()
        (self.destination / "owned").write_bytes(b"untouched")
        with patch.object(MODULE, "run") as run, self.assertRaises(ValueError):
            MODULE.install_selected_node(self.args)
        run.assert_not_called()
        self.assertEqual((self.destination / "owned").read_bytes(), b"untouched")
        self.args.destination = str(self.root / "another")
        self.args.node = "node"
        with patch.object(MODULE, "run") as run, self.assertRaises(ValueError):
            MODULE.install_selected_node(self.args)
        run.assert_not_called()
        self.assertFalse(Path(self.args.destination).exists())

    def test_missing_escaping_and_nonjavascript_entrypoints_are_refused(self):
        for value in ("dist/missing.js", "../outside.js", "/outside.js", "package.json"):
            plan = copy.deepcopy(self.plan)
            plan["entrypoint"]["executable"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                MODULE.selected_npm_closure(self.files, plan)

    def test_dependency_integrity_and_nested_version_cannot_be_substituted(self):
        original = copy.deepcopy(self.rows["node_modules/@outside/server/node_modules/nested"])
        for mutation in ({"version": "1.0.0"}, {"integrity": integrity(self.files["closure/nested-one"])}):
            self.rows["node_modules/@outside/server/node_modules/nested"].update(mutation)
            self.bind()
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                MODULE.selected_npm_closure(self.files, self.plan)
            self.rows["node_modules/@outside/server/node_modules/nested"] = dict(original)

    def test_npm_external_links_workspaces_and_inline_credentials_are_refused(self):
        row = self.rows["node_modules/nested"]
        original = dict(row)
        for mutation in ({"resolved": "file:../../outside"}, {"resolved": "git+https://example.invalid/repo"},
                         {"resolved": "https://user:secret@example.invalid/archive"},
                         {"resolved": "https://example.invalid/archive?token=secret"}, {"link": True},
                         {"inBundle": True}, {"dev": True}):
            row.clear()
            row.update({**original, **mutation})
            self.bind()
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                MODULE.selected_npm_closure(self.files, self.plan)
        row.clear()
        row.update(original)
        self.project["workspaces"] = ["../../outside"]
        self.bind()
        with self.assertRaises(ValueError):
            MODULE.selected_npm_closure(self.files, self.plan)

    def test_archive_links_escape_duplicates_and_bundled_payload_are_refused(self):
        baseline = [("package/package.json", b'{"name":"fixture","version":"1"}', tarfile.REGTYPE, "")]
        cases = [
            [("package/link", b"", tarfile.SYMTYPE, "/outside")],
            [("package/link", b"", tarfile.LNKTYPE, "package/package.json")],
            [("../outside", b"x", tarfile.REGTYPE, "")],
            [("package/Foo.js", b"1", tarfile.REGTYPE, ""), ("package/foo.js", b"2", tarfile.REGTYPE, "")],
            [("package/dupe", b"1", tarfile.REGTYPE, ""), ("package/dupe", b"1", tarfile.REGTYPE, "")],
            [("package/node_modules/hidden/index.js", b"x", tarfile.REGTYPE, "")],
        ]
        for index, additions in enumerate(cases):
            with self.subTest(index=index), self.assertRaises(ValueError):
                MODULE.npm_archive(tarball(baseline + additions))

    def test_archive_parent_case_alias_and_file_directory_conflict_are_refused(self):
        baseline = [("package/package.json", b'{"name":"@outside/server","version":"1.0.0"}', tarfile.REGTYPE, ""),
                    ("package/dist/main.js", b"// Inert fixture.\n", tarfile.REGTYPE, "")]
        for additions in (
            [("package/Foo/a.js", b"a", tarfile.REGTYPE, ""), ("package/foo/b.js", b"b", tarfile.REGTYPE, "")],
            [("package/dir", b"file", tarfile.REGTYPE, ""), ("package/dir/child.js", b"child", tarfile.REGTYPE, "")],
        ):
            self.files["artifact.tgz"] = tarball(baseline + additions)
            self.rows["node_modules/@outside/server"]["integrity"] = integrity(self.files["artifact.tgz"])
            self.bind()
            with self.subTest(paths=[entry[0] for entry in additions]), self.assertRaises(ValueError):
                MODULE.selected_npm_closure(self.files, self.plan)

    def test_invalid_npm_root_and_lock_row_shapes_are_controlled_refusals(self):
        for mutation in ("project-name", "project-version", "root-row", "dependency-row"):
            project, lock = copy.deepcopy(self.project), copy.deepcopy(self.lock)
            if mutation == "project-name":
                project.pop("name")
                lock.pop("name")
            elif mutation == "project-version":
                project.pop("version")
                lock.pop("version")
            elif mutation == "root-row":
                lock["packages"][""] = []
            else:
                lock["packages"]["node_modules/nested"] = []
            files = {**self.files, "closure/package-json": MODULE.encoded(project), "closure/package-lock": MODULE.encoded(lock)}
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                MODULE.selected_npm_closure(files, self.plan)

    def test_resolved_location_requires_text(self):
        for value in (["https://example.invalid/archive"], {"location": "https://example.invalid/archive"}, 7, None):
            self.rows["node_modules/nested"]["resolved"] = value
            self.bind()
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "resolved location must be text"):
                MODULE.selected_npm_closure(self.files, self.plan)

    def test_archive_member_inventory_is_bounded_across_unique_archives(self):
        # Four separate archives each contain two files. All individually fit.
        with patch.object(MODULE, "MAX_NPM_MEMBERS", 8):
            MODULE.selected_npm_closure(self.files, self.plan)
        with patch.object(MODULE, "MAX_NPM_MEMBERS", 7), self.assertRaises(ValueError):
            MODULE.selected_npm_closure(self.files, self.plan)

    def test_realized_inventory_count_includes_each_nested_instance(self):
        self.rows["node_modules/outer/node_modules/nested"] = dict(self.rows["node_modules/nested"])
        self.bind()
        # Still only eight unique archive files, now ten installed files.
        with patch.object(MODULE, "MAX_NPM_MEMBERS", 10):
            MODULE.selected_npm_closure(self.files, self.plan)
        with patch.object(MODULE, "MAX_NPM_MEMBERS", 8), self.assertRaises(ValueError):
            MODULE.selected_npm_closure(self.files, self.plan)

    def test_realized_byte_budget_includes_each_nested_instance(self):
        closure = MODULE.selected_npm_closure(self.files, self.plan)
        unique_bytes = sum(archive["expanded_bytes"] for _, archive in closure["archives"].values())
        repeated_bytes = MODULE.npm_archive(self.files["closure/nested-one"])["expanded_bytes"]
        with patch.object(MODULE, "MAX_TOTAL", unique_bytes):
            MODULE.selected_npm_closure(self.files, self.plan)
        self.rows["node_modules/outer/node_modules/nested"] = dict(self.rows["node_modules/nested"])
        self.bind()
        with patch.object(MODULE, "MAX_TOTAL", unique_bytes + repeated_bytes):
            MODULE.selected_npm_closure(self.files, self.plan)
        with patch.object(MODULE, "MAX_TOTAL", unique_bytes), self.assertRaises(ValueError):
            MODULE.selected_npm_closure(self.files, self.plan)

    def test_compressed_bytes_member_counts_and_file_size_are_bounded(self):
        raw = self.files["artifact.tgz"]
        for key, limit in (("MAX_NPM_EXPANDED", 1024), ("MAX_NPM_MEMBERS", 1), ("MAX_FILE", 1)):
            with self.subTest(limit=key), patch.object(MODULE, key, limit), self.assertRaises(ValueError):
                MODULE.npm_archive(raw)
        bad_json = tarball([("package/package.json", b'{"name":"one","name":"two","version":"1"}', tarfile.REGTYPE, "")])
        with self.assertRaises(ValueError):
            MODULE.npm_archive(bad_json)

    def test_installed_payload_extra_file_symlink_and_missing_optional_are_refused(self):
        closure = MODULE.selected_npm_closure(self.files, self.plan)
        installed = self.root / "installed"
        self.materialize(installed, closure)
        MODULE.verify_selected_npm(installed, closure)
        target = installed / "node_modules/nested/index.js"
        original = target.read_bytes()
        for mutation in ("change", "delete", "symlink", "extra"):
            if mutation == "change":
                target.write_bytes(b"substituted")
            elif mutation == "delete":
                target.unlink()
            elif mutation == "symlink":
                target.unlink()
                target.symlink_to(installed / "node_modules/outer/index.js")
            else:
                (installed / "node_modules/nested/untracked.js").write_bytes(b"extra")
            with self.subTest(mutation=mutation), self.assertRaises((ValueError, OSError)):
                MODULE.verify_selected_npm(installed, closure)
            if target.is_symlink():
                target.unlink()
            target.write_bytes(original)
            (installed / "node_modules/nested/untracked.js").unlink(missing_ok=True)

    def test_cancelled_install_or_postinstall_mutation_has_no_published_config(self):
        for index, failure in enumerate(("cancel", "mutate")):
            self.args.destination = str(self.root / ("incomplete-" + str(index)))
            def interrupted(argv, cwd, env, label, timeout=900):
                value = self.fake_run(argv, cwd, env, label, timeout)
                if label == "npm-install":
                    if failure == "cancel":
                        raise KeyboardInterrupt
                    (cwd / "node_modules/@outside/server/dist/main.js").write_bytes(b"different")
                return value
            with self.subTest(failure=failure), patch.object(MODULE, "run", side_effect=interrupted), self.assertRaises((KeyboardInterrupt, ValueError)):
                MODULE.install_selected_node(self.args)
            destination = Path(self.args.destination)
            self.assertFalse((destination / "ready").exists())
            self.assertFalse((destination / "mcp.json").exists())

    def test_public_cli_rejects_conflicting_modes_and_missing_paths_without_traceback(self):
        base = [sys.executable, "-B", "-I", str(SCRIPT), "--selected-node", str(self.bundle),
                "--selection-sha256", self.args.selection_sha256, "--component", "server",
                "--validator", sys.executable, "--node", sys.executable, "--npm-cli", sys.executable,
                "--destination", str(self.destination)]
        for extra in (["--family", "agno"], ["--node", str(self.root / "missing-runtime")]):
            result = subprocess.run(base + extra, env=ENV, capture_output=True, timeout=5)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn(b"Traceback", result.stderr)
            self.assertFalse(self.destination.exists())

    def test_public_cli_truncated_gzip_is_redacted_and_unpublished(self):
        self.files["artifact.tgz"] = self.files["artifact.tgz"][:6]
        self.bind()
        validator = self.root / "validator-fixture"
        validator.write_text("#!" + sys.executable + " -I\nimport sys\nsys.stdout.buffer.write(" + repr(MODULE.encoded(self.plan)) + ")\n")
        validator.chmod(0o700)
        argv = [sys.executable, "-B", "-I", str(SCRIPT), "--selected-node", str(self.bundle),
                "--selection-sha256", self.args.selection_sha256, "--component", "server",
                "--validator", str(validator), "--node", sys.executable, "--npm-cli", sys.executable,
                "--destination", str(self.destination)]
        result = subprocess.run(argv, env=ENV, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 2)
        self.assertIn(b"installation status unconfirmed", result.stderr)
        self.assertNotIn(b"Traceback", result.stderr)
        self.assertFalse((self.destination / "ready").exists())

    def test_public_cli_cancellation_joins_validator_and_keeps_output_unpublished(self):
        validator = self.root / "waiting-validator"
        validator.write_text("#!" + sys.executable + " -I\nimport os,signal,time\n"
                             "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
                             "print(os.getpid(),flush=True)\ntime.sleep(30)\n")
        validator.chmod(0o700)
        argv = [sys.executable, "-B", "-I", str(SCRIPT), "--selected-node", str(self.bundle),
                "--selection-sha256", self.args.selection_sha256, "--component", "server",
                "--validator", str(validator), "--node", sys.executable, "--npm-cli", sys.executable,
                "--destination", str(self.destination)]
        child = subprocess.Popen(argv, env=ENV, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            log = self.destination / "validate-selection.stdout"
            deadline = time.monotonic() + 5
            while not log.exists() or not log.read_bytes():
                if child.poll() is not None or time.monotonic() > deadline:
                    self.fail("selected-node validator did not start")
                time.sleep(0.02)
            owned_pid = int(log.read_text())
            child.terminate()
            time.sleep(0.1)
            child.send_signal(signal.SIGINT)
            stdout, stderr = child.communicate(timeout=5)
            self.assertEqual(child.returncode, 2)
            self.assertFalse(stdout)
            self.assertNotIn(b"Traceback", stderr)
            self.assertFalse((self.destination / "ready").exists())
            with self.assertRaises(ProcessLookupError):
                os.kill(owned_pid, 0)
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate()


if __name__ == "__main__":
    unittest.main(verbosity=2)
