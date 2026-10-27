"""Node acquisition custody tests with real archive owner and supervised I/O.

Public npm behavior is recorded separately in the verification guard fixtures.
These tests never install or execute selected packages.
"""
from __future__ import annotations

import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import resource
import socket
import sys
import tarfile
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[3]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


M = load("node_acquisition_test_module", ROOT / "sdk/consumer-setup/node_acquisition.py")
B = load("node_acquisition_test_backend", ROOT / "sdk/consumer-setup/backend.py")
W = load("node_acquisition_archive_fixtures", ROOT / "scripts/test_selected_node_install.py")


class NodeAcquisitionTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="toolkit-node-acquisition-test-")
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name).resolve()
        self.staging = self.root / "staging"
        self.staging.mkdir()
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        (self.workspace / "acquisitions").mkdir()
        (self.workspace / "tmp").mkdir()
        self.python = Path(sys.executable).resolve()
        self.node = self.root / "node"
        self.node.write_bytes(b"inert executable identity; fake runner never executes it\n")
        self.node.chmod(0o700)
        self.npm = self.root / "npm.tgz"
        self.npm.write_bytes(W.package("npm", M.NPM_VERSION,
            {"bin/npm-cli.js": b"// Trusted inert test tool.\n", "lib/owner.js": b"// Bound whole tree.\n"}))
        self.installer = ROOT / "scripts/install-library-consumer.py"
        self.profile_path = self.root / "profile.json"
        self.profile = {"schema_version": 1, "scope": M.SCOPE,
                        "python_sha256": M.sha(self.python.read_bytes()),
                        "node_sha256": M.sha(self.node.read_bytes()),
                        "npm": {"path": str(self.npm), "sha256": M.sha(self.npm.read_bytes())}}
        self.io = SimpleNamespace(Error=B.SetupError, read_file=B.read_file, write_new=B.write_new,
                                  canonical_directory=B.canonical_directory, decode=B.decode, keys=B.keys)
        self.request = {"request_id": "a" * 32, "package": "outside-server", "version": "1.0.0"}
        self.job = self.workspace / "acquisitions" / ("b" * 32)
        self.job.mkdir()
        (self.job / "request.json").write_bytes(M.wire(self.request))
        self.primary = W.package("outside-server", "1.0.0", dependencies={"dependency": "1.0.0"})
        self.dependency = W.package("dependency", "1.0.0")
        self.primary_url = "https://registry.npmjs.org/outside-server/-/outside-server-1.0.0.tgz"
        self.dependency_url = "https://registry.npmjs.org/dependency/-/dependency-1.0.0.tgz"
        self.metadata_url = "https://registry.npmjs.org/outside-server/1.0.0"
        self.calls = []
        self.stop_requested = False
        self.mutate_lock = lambda lock: None
        self.after_stage = lambda stage: None
        self.runtime = {"version": "v24.19.0", "platform": "darwin", "arch": "arm64", "abi": "137", "libc": None}

    def owner(self):
        self.profile_path.write_bytes(M.wire(self.profile))
        owner = M.NodeAcquisition(self.io, self.profile_path, M.sha(self.profile_path.read_bytes()),
                                  self.python, self.staging, self.installer,
                                  M.sha(self.installer.read_bytes()), self.node)
        owner.bind(self.workspace)
        return owner

    def runner(self, argv, **kwargs):
        self.calls.append((argv, kwargs))
        stage = argv[-3]
        raw = Path(argv[-2]).read_bytes()
        self.assertEqual(M.sha(raw), argv[-1])
        settings = json.loads(raw)
        if stage == "--probe":
            (self.job / "preflight.json").write_bytes(M.wire(self.runtime))
        elif stage == "--fetch":
            metadata = {"name": "outside-server", "version": "1.0.0",
                        "dist": {"tarball": self.primary_url, "integrity": M.integrity(self.primary)}}
            payloads = {self.metadata_url: M.wire(metadata), self.primary_url: self.primary,
                        self.dependency_url: self.dependency}
            Path(settings["destination"]).write_bytes(payloads[settings["url"]])
        else:
            self.assertEqual(stage, "--resolve")
            project = json.loads((self.job / "project/package.json").read_bytes())
            lock = {"name": project["name"], "version": project["version"], "lockfileVersion": 3,
                    "packages": {"": {"name": project["name"], "version": project["version"],
                                        "dependencies": project["dependencies"]},
                                 "node_modules/outside-server": {
                                     "version": "1.0.0", "resolved": "file:artifact.tgz",
                                     "integrity": M.integrity(self.primary),
                                     "dependencies": {"dependency": "1.0.0"}},
                                 "node_modules/dependency": {
                                     "version": "1.0.0", "resolved": self.dependency_url,
                                     "integrity": M.integrity(self.dependency)}}}
            self.mutate_lock(lock)
            (self.job / "project/package-lock.json").write_bytes(M.wire(lock))
            (self.job / "runtime.json").write_bytes(M.wire(self.runtime))
        self.after_stage(stage)
        return 0, b"private provider diagnostic must not leak"

    def check_stop(self):
        if self.stop_requested:
            raise B.SetupError("operation_stopped", 409)

    def execute(self, owner=None, runner=None):
        return (owner or self.owner()).run(self.request, self.job, runner or self.runner, self.check_stop)

    def assert_unpublished(self):
        self.assertFalse((self.staging / self.job.name / "acquisition.json").exists())

    def test_exact_retained_bytes_are_accepted_by_canonical_selected_owner(self):
        result = self.execute()
        destination = self.staging / self.job.name
        self.assertEqual(result["scope"], M.RESULT_SCOPE)
        self.assertEqual(result["request"], self.request)
        self.assertEqual([call[0][-3] for call in self.calls], ["--probe", "--fetch", "--fetch", "--resolve", "--fetch"])
        timeouts = [call[1]["inspection_timeout"] for call in self.calls]
        self.assertEqual(timeouts, sorted(timeouts, reverse=True))
        self.assertTrue(all(0 < value <= 120 for value in timeouts))
        self.assertTrue(all(call[0][1:4] == ["-I", "-S", "-B"] for call in self.calls))
        files = {name: B.read_file(destination, name, M.FILE_LIMIT) for name in result["files"]}
        self.assertEqual({name: M.sha(raw) for name, raw in files.items()}, result["files"])
        closure = W.MODULE.selected_npm_closure(files, {"entrypoint": {"executable": "index.js"}})
        self.assertEqual(set(closure["rows"]), {"node_modules/outside-server", "node_modules/dependency"})
        self.assertEqual(M.sha((destination / "acquisition.json").read_bytes()), result["receipt_sha256"])
        self.assertNotIn("private provider", json.dumps(result))
        self.assertFalse((destination / "package.json").exists())
        self.assertFalse((destination / "activation.json").exists())

    def test_invalid_host_scope_and_missing_profile_fail_before_bind(self):
        self.profile["scope"] = "arbitrary_registry"
        with self.assertRaisesRegex(B.SetupError, "invalid_node_acquisition_profile"):
            self.owner()
        self.assertFalse((self.workspace / "node-acquisition").exists())
        with self.assertRaisesRegex(B.SetupError, "invalid_node_acquisition_profile"):
            M.NodeAcquisition(self.io, None, "0" * 64, self.python, self.staging,
                              self.installer, M.sha(self.installer.read_bytes()), self.node)

    def test_named_primary_and_dependency_use_shared_archive_owner_without_rewriting(self):
        def rewrap(raw, wrapper):
            parsed = W.MODULE.npm_archive(raw)
            return W.tarball([(wrapper + "/" + path, data, tarfile.REGTYPE, "")
                              for path, data in parsed["files"].items()])
        self.primary = rewrap(self.primary, "primary-wrapper")
        self.dependency = rewrap(self.dependency, "dependency-wrapper")
        result = self.execute()
        destination = self.staging / self.job.name
        self.assertEqual((destination / "artifact.tgz").read_bytes(), self.primary)
        self.assertEqual((destination / "closure" / ("archive-" + M.sha(self.dependency))).read_bytes(),
                         self.dependency)
        self.assertEqual([(item["name"], item["version"]) for item in result["artifacts"]],
                         [("outside-server", "1.0.0"), ("dependency", "1.0.0")])
        files = {name: B.read_file(destination, name, M.FILE_LIMIT) for name in result["files"]}
        graph = W.MODULE.selected_npm_closure(files, {"entrypoint": {"executable": "index.js"}})
        self.assertEqual(set(graph["rows"]), {"node_modules/outside-server", "node_modules/dependency"})

    def test_mixed_archive_wrappers_fail_without_publishing_a_selection(self):
        self.dependency = W.tarball([
            ("first/package.json", M.wire({"name": "dependency", "version": "1.0.0"}), tarfile.REGTYPE, ""),
            ("second/index.js", b"// Inert test payload.\n", tarfile.REGTYPE, "")])
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assert_unpublished()

    def test_only_exact_safe_package_and_version_are_accepted(self):
        owner = self.owner()
        for mutation in ({"package": "npm:outside-server"}, {"package": "../elsewhere"},
                         {"package": "https://registry.npmjs.org/a.tgz"}, {"package": "@bad"},
                         {"version": "latest"}, {"version": "^1.0.0"}, {"version": "v1.0.0"},
                         {"version": "1.0.0\n--registry=x"}, {"version": "01.0.0"},
                         {"version": "1.0.0-01"}, {"version": "1.0.0-a.01"},
                         {"request_id": "../host"}, {"extra": True}):
            with self.subTest(mutation=mutation), self.assertRaises(B.SetupError):
                owner.arguments({**self.request, **mutation})
        for version in ("1.0.0", "1.0.0-rc.1", "1.0.0+001.build", "1.0.0-0"):
            self.assertEqual(owner.arguments({**self.request, "version": version})["version"], version)
        duplicate = owner.arguments(self.request)
        duplicate["version"] = "2.0.0"
        self.assertEqual(self.request["version"], "1.0.0")

    def test_old_npm_distribution_rejected_even_with_a_valid_host_hash(self):
        self.npm.write_bytes(W.package("npm", "11.12.1", {"bin/npm-cli.js": b"// old tool\n"}))
        self.profile["npm"]["sha256"] = M.sha(self.npm.read_bytes())
        with self.assertRaisesRegex(B.SetupError, "invalid_node_acquisition_profile"):
            self.owner()
        self.assertFalse((self.workspace / "node-acquisition").exists())

    def test_tool_archive_rejects_escape_links_and_collisions(self):
        bad = [("package/../escape", b"x", tarfile.REGTYPE, ""),
               ("package/link", b"", tarfile.SYMTYPE, "/tmp"),
               ("package/link", b"", tarfile.LNKTYPE, "package/package.json")]
        for entry in bad:
            with self.subTest(entry=entry), self.assertRaises(ValueError):
                M.tool_payloads(W.tarball([entry]))

    def test_changed_original_tool_stops_before_any_subprocess(self):
        owner = self.owner()
        self.npm.write_bytes(b"replaced")
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_host_changed"):
            self.execute(owner)
        self.assertEqual(self.calls, [])
        self.assert_unpublished()

    def test_frozen_tree_rejects_added_directory_symlink(self):
        owner = self.owner()
        elsewhere = self.root / "new-module"
        elsewhere.mkdir()
        (owner.assets / "npm/node_modules").symlink_to(elsewhere, target_is_directory=True)
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_host_changed"):
            self.execute(owner)
        self.assertEqual(self.calls, [])

    def test_frozen_tree_rejects_added_empty_directory(self):
        owner = self.owner()
        (owner.assets / "npm/new-empty-directory").mkdir()
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_host_changed"):
            self.execute(owner)
        self.assertEqual(self.calls, [])

    def test_frozen_module_edit_stops_before_any_subprocess(self):
        owner = self.owner()
        path = owner.assets / "npm/lib/owner.js"
        path.chmod(0o600)
        path.write_bytes(b"// replaced trusted module\n")
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_host_changed"):
            self.execute(owner)
        self.assertEqual(self.calls, [])

    def test_existing_staging_child_is_not_reused(self):
        destination = self.staging / self.job.name
        destination.mkdir()
        (destination / "evidence").write_text("retain")
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assertEqual((destination / "evidence").read_text(), "retain")
        self.assertEqual(self.calls, [])

    def test_forbidden_lock_origin_rejected_before_dependency_fetch(self):
        self.mutate_lock = lambda lock: lock["packages"]["node_modules/dependency"].update(resolved="https://outside.invalid/package.tgz")
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assertEqual(len(self.calls), 4)
        self.assert_unpublished()

    def test_alias_archive_is_rejected_by_the_canonical_owner(self):
        self.dependency = W.package("different-name", "1.0.0")
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assertEqual(len(self.calls), 5)
        self.assert_unpublished()

    def test_workspace_and_shrinkwrap_archives_remain_unsupported(self):
        owner = self.owner()
        package = {"name": "outside-server", "version": "1.0.0", "workspaces": ["x"]}
        raw = W.package("outside-server", "1.0.0", {"package.json": M.wire(package)})
        with self.assertRaises(ValueError):
            owner.archive(vars(W.MODULE), raw)
        raw = W.package("outside-server", "1.0.0", {"npm-shrinkwrap.json": b"{}"})
        with self.assertRaises(ValueError):
            owner.archive(vars(W.MODULE), raw)

    def test_optional_lock_dependency_without_integrity_cannot_be_silently_omitted(self):
        def optional_lock(lock):
            packages = lock["packages"]
            root = packages.pop("")
            lock["packages"] = {
                "": root,
                "node_modules/@gar/promisify": {"version": "1.1.3", "optional": True},
                **packages,
            }
        self.mutate_lock = optional_lock
        with self.assertRaisesRegex(B.SetupError, "^node_acquisition_unconfirmed$"):
            self.execute()
        self.assertEqual([call[0][-3] for call in self.calls],
                         ["--probe", "--fetch", "--fetch", "--resolve"])
        self.assert_unpublished()

    def test_matching_platform_archive_retains_exact_owned_target(self):
        self.primary = W.package("outside-server", "1.0.0", {"package.json": M.wire({
            "name": "outside-server", "version": "1.0.0", "os": ["darwin"],
            "dependencies": {"dependency": "1.0.0"}}), "index.js": b"// inert\n"})
        self.mutate_lock = lambda lock: lock["packages"]["node_modules/outside-server"].update(os=["darwin"])
        result = self.execute()
        target = self.staging / self.job.name / "closure/npm-target"
        self.assertEqual(json.loads(target.read_bytes()), M._npm_target(self.runtime, M.sha(self.node.read_bytes())))
        self.assertEqual(result["files"]["closure/npm-target"], M.sha(target.read_bytes()))

    def test_optional_platform_closure_is_retained_and_readback_rejects_target_removal(self):
        self.primary = W.package("outside-server", "1.0.0", {"package.json": M.wire({
            "name": "outside-server", "version": "1.0.0", "optionalDependencies": {"dependency": "1.0.0"}}),
            "index.js": b"// inert\n"})
        self.dependency = W.package("dependency", "1.0.0", {"package.json": M.wire({
            "name": "dependency", "version": "1.0.0", "os": ["linux"]})})
        def optional(lock):
            row = lock["packages"]["node_modules/outside-server"]
            row["optionalDependencies"] = row.pop("dependencies")
            lock["packages"]["node_modules/dependency"].update(optional=True, os=["linux"])
        self.mutate_lock = optional
        owner = self.owner()
        result = owner.run(self.request, self.job, self.runner, lambda: None)
        destination = self.staging / self.job.name
        files = {key: (destination / key).read_bytes() for key in result["files"]}
        graph = W.MODULE.npm_archive_closure(files)
        self.assertEqual(set(graph["rows"]), {"node_modules/outside-server"})
        self.assertEqual(graph["excluded_optional_packages"], {"node_modules/dependency": "platform_mismatch"})
        self.assertEqual(len(result["artifacts"]), 2)
        retained = {"arguments": self.request, "family": "selected-node", "job_id": self.job.name,
                    "workspace": str(self.workspace), "staging_root": str(self.staging),
                    "host": {"profile_sha256": owner.profile_sha,
                             "assets": {name: M.sha(owner.frozen[name]) for name in
                                        ("profile.json", "npm.tgz", "installer.py", "worker.py")}}}
        self.assertEqual(M.readback(self.io, retained, self.job)["receipt_sha256"], result["receipt_sha256"])
        altered = dict(result); altered.pop("receipt_path"); altered.pop("receipt_sha256")
        altered["files"] = {key: value for key, value in altered["files"].items() if key != "closure/npm-target"}
        (destination / "closure/npm-target").unlink()
        (destination / "acquisition.json").write_bytes(M.wire(altered))
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_readback_unconfirmed"):
            M.readback(self.io, retained, self.job)

    def test_malformed_lock_row_remains_unconfirmed(self):
        self.mutate_lock = lambda lock: lock["packages"].update({"node_modules/dependency": None})
        with self.assertRaisesRegex(B.SetupError, "^node_acquisition_unconfirmed$"):
            self.execute()
        self.assert_unpublished()

    def test_unexpected_lock_failure_remains_unconfirmed_without_private_details(self):
        owner = self.owner()
        with patch.object(owner, "lock_rows", side_effect=RuntimeError("private provider diagnostic")):
            with self.assertRaisesRegex(B.SetupError, "^node_acquisition_unconfirmed$"):
                self.execute(owner=owner)
        self.assert_unpublished()

    def test_integrity_mismatch_is_rejected_after_supervised_fetch(self):
        def corrupting_runner(argv, **kwargs):
            result = self.runner(argv, **kwargs)
            settings = json.loads(Path(argv[-2]).read_bytes())
            if settings.get("url") == self.dependency_url:
                Path(settings["destination"]).write_bytes(b"substituted package")
            return result
        with self.assertRaises(B.SetupError):
            self.execute(runner=corrupting_runner)
        self.assert_unpublished()

    def test_stop_before_first_probe_never_publishes(self):
        self.stop_requested = True
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute()
        self.assertEqual(self.calls, [])
        self.assert_unpublished()

    def test_stop_after_active_probe_prevents_the_first_fetch(self):
        self.after_stage = lambda stage: setattr(self, "stop_requested", True)
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute()
        self.assertEqual(len(self.calls), 1)
        self.assert_unpublished()

    def test_host_runtime_changed_during_resolution_never_publishes(self):
        def after(stage):
            if stage == "--resolve":
                self.node.write_bytes(b"changed during npm resolution")
        self.after_stage = after
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_host_changed"):
            self.execute()
        self.assert_unpublished()

    def test_node_24_before_npm_supported_minor_is_rejected_before_dependency_fetch(self):
        def after(stage):
            if stage == "--resolve":
                (self.job / "runtime.json").write_bytes(M.wire({"version": "v24.14.0", "platform": "darwin", "arch": "arm64"}))
        self.after_stage = after
        with self.assertRaises(B.SetupError):
            self.execute()
        self.assertEqual(len(self.calls), 4)
        self.assert_unpublished()

    def test_stop_during_staging_leaves_no_success_receipt(self):
        owner = self.owner()
        original = self.io.write_new
        def write(path, raw):
            original(path, raw)
            if path == self.staging / self.job.name / "artifact.tgz":
                self.stop_requested = True
        self.io.write_new = write
        with self.assertRaisesRegex(B.SetupError, "operation_stopped"):
            self.execute(owner)
        self.assertTrue((self.staging / self.job.name / "artifact.tgz").exists())
        self.assert_unpublished()

    def test_provider_error_is_fixed_and_keeps_staging_unconfirmed(self):
        def failing(argv, **kwargs):
            if argv[-3] == "--probe":
                return self.runner(argv, **kwargs)
            return 74, b"credential-like private provider output"
        with self.assertRaisesRegex(B.SetupError, "node_acquisition_download_failed") as caught:
            self.execute(runner=failing)
        self.assertNotIn("credential-like", str(caught.exception))
        self.assert_unpublished()

    def test_registry_urls_require_the_exact_public_origin(self):
        for url in ("http://registry.npmjs.org/a", "https://registry.npmjs.org.evil.invalid/a",
                    "https://user@registry.npmjs.org/a", "https://registry.npmjs.org/a?token=x",
                    "https://registry.npmjs.org/a#fragment", "https://registry.npmjs.org/a/%2e%2e/b",
                    "https://registry.npmjs.org/a%5cb", "file:///tmp/a"):
            with self.subTest(url=url), self.assertRaises(ValueError):
                M.registry_url(url)

    def test_fetch_worker_checks_size_encoding_integrity_and_redirect(self):
        class Response(io.BytesIO):
            status = 200
            headers = {}
            def geturl(self):
                return self.primary_url
        for headers, contents in (({"Content-Length": "3"}, b"four"),
                                  ({"Content-Encoding": "gzip"}, b"one"),
                                  ({}, b"oversize")):
            response = Response(contents)
            response.primary_url = self.primary_url
            response.headers = headers
            opener = SimpleNamespace(open=lambda *a, **k: response)
            value = {"url": self.primary_url, "limit": 3, "destination": str(self.root / "download"), "integrity": None}
            with self.subTest(headers=headers), patch.object(M.urllib.request, "build_opener", return_value=opener):
                with self.assertRaises(ValueError):
                    M.fetch_worker(value)
            self.assertFalse((self.root / "download").exists())
        with self.assertRaises(ValueError):
            M.NoRedirect().redirect_request(None, None, 302, "redirect", {}, "https://registry.npmjs.org/b")

    def test_fetch_worker_checks_integrity_before_writing_payload(self):
        class Response(io.BytesIO):
            status = 200
            headers = {}
            def geturl(response):
                return self.primary_url
        response = Response(b"unmatched archive")
        opener = SimpleNamespace(open=lambda *a, **k: response)
        value = {"url": self.primary_url, "limit": 100, "destination": str(self.root / "download"),
                 "integrity": M.integrity(b"expected archive")}
        with patch.object(M.urllib.request, "build_opener", return_value=opener), self.assertRaises(ValueError):
            M.fetch_worker(value)
        self.assertFalse((self.root / "download").exists())

    @unittest.skipUnless(sys.platform == "darwin" and platform.machine() == "arm64", "macOS arm64 worker")
    def test_worker_exec_is_the_existing_process_with_minimal_environment(self):
        owner = self.owner()
        for name in ("home", "tmp", "cache", "project"):
            (self.job / name).mkdir()
        settings = {"node": str(self.node), "node_sha256": M.sha(self.node.read_bytes()),
                    "npm": str(owner.assets / "npm"), "directory": str(self.job)}
        with patch.object(os, "chdir"), patch.object(os, "execve") as execution, patch.object(resource, "setrlimit"):
            with patch.dict(os.environ, {"NODE_OPTIONS": "--require=/private/untrusted", "npm_config_registry": "https://private.invalid"}):
                M.resolve_worker(settings)
        node, args, environment = execution.call_args.args
        self.assertEqual(Path(node), self.node)
        for flag in ("--ignore-scripts", "--package-lock-only", "--allow-git=none", "--allow-remote=none",
                     "--allow-directory=none", "--allow-file=root", "--registry=https://registry.npmjs.org"):
            self.assertIn(flag, args)
        self.assertNotIn("NODE_OPTIONS", environment)
        self.assertNotIn("npm_config_registry", environment)
        self.assertNotEqual(environment["npm_config_userconfig"], environment["npm_config_globalconfig"])
        self.assertEqual(Path(environment["HOME"]), self.job / "home")


class SupervisedFetchTests(unittest.TestCase):
    def run_case(self, body, stop):
        with tempfile.TemporaryDirectory(prefix="toolkit-node-network-test-") as scratch:
            root = Path(scratch).resolve()
            (root / "tmp").mkdir()
            accepted = threading.Event()
            closed = threading.Event()
            server = socket.socket()
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            server.settimeout(5)
            port = server.getsockname()[1]
            def serve():
                try:
                    connection, _ = server.accept()
                    with connection:
                        connection.recv(65536)
                        accepted.set()
                        if body:
                            connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n")
                        while not closed.wait(0.025):
                            connection.sendall(b"x")
                except (OSError, TimeoutError):
                    pass
                finally:
                    server.close()
            thread = threading.Thread(target=serve, daemon=True)
            thread.start()
            runner = B.Runner(root)
            event = threading.Event()
            runner._job_context.control = {"requested": event, "spawn_lock": threading.Lock()}
            value = {"url": f"http://127.0.0.1:{port}/inert", "limit": 100000,
                     "destination": str(root / "payload"), "integrity": None}
            # Only the test child permits loopback. Production URL validation is
            # tested separately and remains unchanged in the imported module.
            code = ("import importlib.util,json,sys;"
                    "s=importlib.util.spec_from_file_location('worker',sys.argv[1]);"
                    "m=importlib.util.module_from_spec(s);s.loader.exec_module(m);"
                    "m.registry_url=lambda u:u;m.fetch_worker(json.loads(sys.argv[2]))")
            timer = None
            if stop:
                def request_stop():
                    if accepted.wait(5):
                        event.set()
                timer = threading.Thread(target=request_stop, daemon=True)
                timer.start()
            started = time.monotonic()
            try:
                with self.assertRaisesRegex(B.SetupError, "operation_stopped" if stop else "inspection_unavailable"):
                    runner.run([str(Path(sys.executable).resolve()), "-I", "-S", "-B", "-c", code,
                                str(ROOT / "sdk/consumer-setup/node_acquisition.py"), json.dumps(value)],
                               capture=True, inspection_timeout=5 if stop else 0.6)
                self.assertTrue(accepted.is_set())
                self.assertLess(time.monotonic() - started, 3)
                self.assertFalse((root / "payload").exists())
            finally:
                closed.set()
                server.close()
                thread.join(2)
                if timer:
                    timer.join(2)
            self.assertFalse(thread.is_alive())

    def test_slow_header_is_cut_off_by_the_existing_runner_deadline(self):
        self.run_case(body=False, stop=False)

    def test_slow_body_is_cut_off_by_the_existing_runner_deadline(self):
        self.run_case(body=True, stop=False)

    def test_explicit_stop_settles_the_slow_header_child(self):
        self.run_case(body=False, stop=True)

    def test_explicit_stop_settles_the_slow_body_child(self):
        self.run_case(body=True, stop=True)


if __name__ == "__main__":
    unittest.main()
