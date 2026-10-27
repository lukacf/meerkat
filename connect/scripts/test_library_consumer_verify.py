"""Inert reconciliation against independent original library selections."""
from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

SCRIPT = Path(__file__).with_name("install-library-consumer.py")
SPEC = importlib.util.spec_from_file_location("consumer_verify", SCRIPT)
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
ENV = {"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"}


def put(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def archive(payloads, wheel=False):
    output = io.BytesIO()
    if wheel:
        with zipfile.ZipFile(output, "w") as stream:
            for name, raw in payloads.items():
                stream.writestr(name, raw)
    else:
        with tarfile.open(fileobj=output, mode="w:gz") as stream:
            for name, raw in payloads.items():
                row = tarfile.TarInfo("package/" + name)
                row.size = len(raw)
                stream.addfile(row, io.BytesIO(raw))
    return output.getvalue()


def snapshot(root):
    result = {}
    for parent, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            path = Path(parent) / name
            if path.is_symlink():
                result[str(path.relative_to(root))] = ("link", os.readlink(path))
            elif path.is_file():
                result[str(path.relative_to(root))] = ("file", path.read_bytes())
            elif path.is_dir():
                result[str(path.relative_to(root))] = ("directory",)
    return result


class ReconciliationTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="toolkit reconcile ")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.bundle = self.root / "bundle"
        self.destination = self.root / "installed"
        self.runtime = self.root / "selected-runtime"
        self.runtime.write_text("#!/bin/sh\ntouch '" + str(self.root / "EXECUTED") + "'\nexit 1\n")
        self.runtime.chmod(0o700)

    def build(self, family="services", configured=True, native=None, profile="fixture"):
        self.family = family
        self.files = {}
        if family in M.FAMILIES:
            namespace, module = M.FAMILIES[family]
            dist = namespace + "-0.1.0.dist-info"
            self.payload = module.replace(".", "/") + ".py"
            self.members = {self.payload: b"raise RuntimeError('must not execute')\n",
                            dist + "/METADATA": ("Metadata-Version: 2.1\nName: " + namespace + "\nVersion: 0.1.0\n").encode(),
                            dist + "/RECORD": b"original-record",
                            dist + "/entry_points.txt": b"[console_scripts]\nfixture = selected.module:main\n",
                            namespace + "-0.1.0.data/scripts/retained.py": b"#!python\nprint('retained')\n"}
            if native is not None:
                # An inert publisher fixture, not proof that its code implements
                # the mode. Publisher and native-consumer tests cover that source.
                declaration = {"schema_version": 1, "module": module, "mode": "serve-native",
                    "configuration_argument": "--config", "member": self.payload,
                    "member_sha256": M.sha(self.members[self.payload])}
                if family == "agno": declaration["profiles"] = ["salesforce"]
                source = {"canonical_source": "sdk/python/src/meerkat_toolkit/adapters/" + family + "_mcp.py",
                          "sha256": declaration["member_sha256"]}
                if native == "wrong-member": declaration["member"] = namespace + "/absent.py"
                if native == "wrong-hash": declaration["member_sha256"] = "0" * 64
                if native == "missing-hash": del declaration["member_sha256"]
                if native == "wrong-mode": declaration["mode"] = "serve"
                if native == "mapping-mismatch": source["sha256"] = "0" * 64
                manifest = {"distribution": "meerkat-toolkit-" + family, "namespace": namespace,
                    "source_mapping": {"src/" + self.payload: source}, "native_connector": declaration}
                self.members[namespace + "/PACKAGE-SOURCES.json"] = M.encoded(manifest)
            filename = namespace + "-0.1.0-py3-none-any.whl"
            raw = archive(self.members, wheel=True)
            self.files = {filename: raw, "install-offline.txt": (M.name(namespace) + "==0.1.0 --hash=sha256:" + M.sha(raw) + "\n").encode()}
            venv = self.destination / "runtime"
            self.site = venv / "lib/python3.11/site-packages"
            for path, content in self.members.items():
                if ".data/scripts/" in path:
                    content = b"#!" + str(venv / "bin/python").encode() + b"\n" + content.split(b"\n", 1)[1]
                    put(venv / "bin/retained.py", content)
                else:
                    put(self.site / path, content)
            for name in ("python", "python3", "python3.11"):
                (venv / "bin" / name).symlink_to(self.runtime)
            (self.root / "python3.11").symlink_to(self.runtime)
            put(venv / "pyvenv.cfg", ("home = " + str(self.root) + "\ninclude-system-site-packages = false\nversion = 3.11.15\nexecutable = " + str(self.runtime) + "\n").encode())
            put(venv / "bin/fixture", b"unverified generated wrapper")
            put(self.site / "pip/__init__.py", b"unverified bootstrap bytes")
            put(self.site / "distutils-precedence.pth", b"unverified known bootstrap startup file")
            put(self.site / dist / "INSTALLER", b"pip\n")
            parent, leaf = self.payload.rsplit("/", 1)
            put(self.site / parent / "__pycache__" / (leaf[:-3] + ".cpython-311.pyc"), b"unverified generated cache")
            command = [str(venv / "bin/python"), "-I", "-m", module]
            versions = {M.name(namespace): "0.1.0"}
            payloads = len(self.members) - 1
        else:
            component = "@rkat/toolkit-activepieces"
            self.members = {"package.json": M.encoded({"name": component, "version": "0.1.0", "bin": {"toolkit-fixture": "./dist/mcp-cli.js"}}),
                            "dist/mcp-cli.js": b"throw Error('must not execute');\n"}
            dependency = {"package.json": M.encoded({"name": "fixture-dependency", "version": "1.0.0"}), "index.js": b"module.exports = 1;\n"}
            component_raw, dependency_raw = archive(self.members), archive(dependency)
            integrity = "sha512-" + base64.b64encode(hashlib.sha512(dependency_raw).digest()).decode()
            deps = {component: "file:./rkat-toolkit-activepieces-0.1.0.tgz"}
            lock = {"lockfileVersion": 3, "packages": {"": {"dependencies": deps},
                "node_modules/" + component: {"version": "0.1.0", "resolved": "file:rkat-toolkit-activepieces-0.1.0.tgz"},
                "node_modules/fixture-dependency": {"version": "1.0.0", "resolved": "https://example.invalid/fixed.tgz", "integrity": integrity}}}
            self.files = {"package.json": M.encoded({"dependencies": deps}), "package-lock.json": M.encoded(lock),
                "rkat-toolkit-activepieces-0.1.0.tgz": component_raw, "fixture-dependency.tgz": dependency_raw,
                "offline-cache.json": M.encoded({"schema_version": 1, "tarballs": [{"name": "fixture-dependency", "version": "1.0.0",
                    "path": "fixture-dependency.tgz", "sha256": M.sha(dependency_raw), "integrity": integrity}]})}
            for name, members in ((component, self.members), ("fixture-dependency", dependency)):
                for path, raw in members.items():
                    put(self.destination / "node_modules" / name / path, raw)
            bins = self.destination / "node_modules/.bin"
            bins.mkdir()
            (bins / "toolkit-fixture").symlink_to("../" + component + "/dist/mcp-cli.js")
            put(self.destination / "node_modules/.package-lock.json", b"unverified generated lock metadata")
            for path in ("package.json", "package-lock.json", "rkat-toolkit-activepieces-0.1.0.tgz"):
                put(self.destination / path, self.files[path])
            command = [str(self.runtime), str(self.destination / "node_modules/@rkat/toolkit-activepieces/dist/mcp-cli.js")]
            versions = {component: "0.1.0", "fixture-dependency": "1.0.0"}
            payloads = len(self.members)
        for path, raw in self.files.items():
            put(self.bundle / family / path, raw)
            put(self.destination / "inputs" / path, raw)
        contents = M.encoded({"schema_version": 1, "files": {family + "/" + path: {"sha256": M.sha(raw)} for path, raw in self.files.items()}})
        put(self.bundle / "CONTENTS.json", contents)
        self.args = argparse.Namespace(bundle=str(self.bundle), contents_sha256=M.sha(contents), family=family,
            destination=str(self.destination), python=str(self.runtime) if family in M.FAMILIES else None,
            node=str(self.runtime) if family == "activepieces" else None, npm_cli=None,
            config=None, config_sha256=None, server_name=None)
        self.receipt = {"schema_version": 1, "scope": "observed_software_installation_only", "family": family,
            "contents_sha256": self.args.contents_sha256, "inputs": {path: M.sha(raw) for path, raw in self.files.items()},
            "versions": versions, "matching_component_payloads": payloads, "command": command,
            "destination": str(self.destination), "runtime": "HISTORICAL-NOT-REVERIFIED"}
        if configured:
            config = M.encoded({"profile": profile, "operations": ["read"], "config": {}})
            self.args.config = str(self.root / "original.json")
            self.args.config_sha256 = M.sha(config)
            self.args.server_name = "selected-fixture"
            put(Path(self.args.config), config)
            config_path = self.destination / "configuration/selected.json"
            put(config_path, config)
            command = command + ["serve", "--config", str(config_path)]
            self.mcp = {"mcpServers": {self.args.server_name: {"command": command[0], "args": command[1:]}}}
            self.receipt.update({"command": command, "mcp_configuration": self.mcp,
                "configuration": {"sha256": self.args.config_sha256, "path": str(config_path), "server_name": self.args.server_name,
                    "validation": "configuration_only"}})
            put(self.destination / "ready/mcp.json", M.encoded(self.mcp))
            self.receipt_path = self.destination / "ready/installed.json"
        else:
            self.receipt_path = self.destination / "installed.json"
        self.save_receipt()

    def save_receipt(self):
        put(self.receipt_path, M.encoded(self.receipt))

    def verify(self):
        before = snapshot(self.destination)
        with patch.object(M, "run", side_effect=AssertionError("no child execution")), patch.object(M.subprocess, "Popen", side_effect=AssertionError("no subprocess")), patch.object(Path, "write_bytes", side_effect=AssertionError("no writes")), patch.object(Path, "write_text", side_effect=AssertionError("no writes")):
            result = M.verify_existing(self.args)
        self.assertEqual(snapshot(self.destination), before)
        self.assertFalse((self.root / "EXECUTED").exists())
        self.assertNotIn("HISTORICAL-NOT-REVERIFIED", json.dumps(result))
        return result

    def refused(self):
        with self.assertRaises((ValueError, OSError, KeyError, StopIteration)):
            self.verify()

    def test_configured_python_payloads_and_request_binding_are_inert(self):
        self.build()
        result = self.verify()
        self.assertTrue(result["verified"])
        self.assertEqual(result["scope"], "retained_payload_and_launch_binding_only")
        self.assertEqual(result["historical_receipt"]["sha256"], M.sha(self.receipt_path.read_bytes()))
        self.assertEqual(result["configuration_sha256"], self.args.config_sha256)
        self.assertEqual(result["retained_payloads"]["excluded_file_counts"], {"bootstrap": 2, "generated_metadata": 2, "source_caches": 1})
        self.assertIn("not verified", " ".join(result["excluded"]))

    def test_native_connector_observation_is_bound_to_selected_wheel_without_receipt_rewrite(self):
        self.build(native="valid")
        original = self.receipt_path.read_bytes()
        result = self.verify()
        native = result["native_connector"]
        wheel = next(name for name in self.files if name.endswith(".whl"))
        self.assertEqual(native["wheel"], {"filename": wheel, "sha256": M.sha(self.files[wheel])})
        self.assertEqual(native["member"], self.payload)
        self.assertEqual(native["member_sha256"], M.sha(self.members[self.payload]))
        self.assertEqual(native["package_sources_sha256"], M.sha(self.members[
            "meerkat_toolkit_services/PACKAGE-SOURCES.json"]))
        self.assertEqual(native["mode"], "serve-native")
        self.assertEqual(self.receipt_path.read_bytes(), original)
        self.assertNotIn("native_connector", self.receipt)
        # The same declaration cannot bless a changed installed entry member.
        (self.site / self.payload).write_bytes(b"changed installed entry")
        self.refused()

    def unsupported_native_declaration(self, mode):
        self.build(native=mode)
        self.assertNotIn("native_connector", self.verify())
        self.assertNotIn("native_connector", self.receipt)

    def test_old_wheel_without_native_declaration_keeps_ordinary_installation(self):
        self.unsupported_native_declaration(None)

    def test_agno_native_entry_requires_the_exact_original_selected_profile(self):
        self.build(family="agno", native="valid", profile="salesforce")
        original = self.receipt_path.read_bytes()
        native = self.verify()["native_connector"]
        wheel = next(name for name in self.files if name.endswith(".whl"))
        self.assertEqual(native["profiles"], ["salesforce"])
        self.assertEqual(native["module"], "meerkat_toolkit_agno.agno_mcp")
        self.assertEqual(native["wheel"], {"filename": wheel, "sha256": M.sha(self.files[wheel])})
        self.assertEqual(native["member_sha256"], M.sha(self.members[self.payload]))
        self.assertEqual(native["package_sources_sha256"], M.sha(self.members[
            "meerkat_toolkit_agno/PACKAGE-SOURCES.json"]))
        self.assertEqual(self.receipt_path.read_bytes(), original)
        self.assertNotIn("native_connector", self.receipt)
        # Replacing only the original input cannot change the selected profile:
        # verification still compares it with the actual retained configuration.
        changed = M.encoded({"profile": "tavily", "operations": ["read"], "config": {}})
        put(Path(self.args.config), changed)
        self.args.config_sha256 = M.sha(changed)
        self.refused()

    def test_other_agno_profile_keeps_ordinary_installation_without_native_entry(self):
        self.build(family="agno", native="valid", profile="tavily")
        self.assertNotIn("native_connector", self.verify())

    def test_unconfigured_agno_cannot_select_native_entry_from_family_alone(self):
        self.build(family="agno", native="valid", configured=False)
        self.assertNotIn("native_connector", self.verify())

    def test_old_agno_wheel_without_declaration_keeps_ordinary_installation(self):
        self.build(family="agno", profile="salesforce")
        self.assertNotIn("native_connector", self.verify())

    def test_agno_native_declaration_rejects_profile_and_source_substitution(self):
        self.build(family="agno", native="valid", profile="salesforce")
        wheel = next(name for name in self.files if name.endswith(".whl"))
        manifest_name = "meerkat_toolkit_agno/PACKAGE-SOURCES.json"
        for mode in ("missing-profile", "null-profile", "wrong-profile", "extra-profile",
                     "duplicate-profile", "wrong-mode", "wrong-module", "wrong-member",
                     "wrong-member-hash", "wrong-source-hash", "wrong-source", "wrong-distribution",
                     "wrong-namespace", "unknown-field", "boolean-schema", "wrong-wheel"):
            with self.subTest(mode=mode):
                members = dict(self.members)
                manifest = M.unique_json(members[manifest_name])
                declared = manifest["native_connector"]
                source = manifest["source_mapping"]["src/" + self.payload]
                if mode == "missing-profile": del declared["profiles"]
                elif mode == "null-profile": declared["profiles"] = None
                elif mode == "wrong-profile": declared["profiles"] = ["tavily"]
                elif mode == "extra-profile": declared["profiles"].append("tavily")
                elif mode == "duplicate-profile": declared["profiles"].append("salesforce")
                elif mode == "wrong-mode": declared["mode"] = "serve"
                elif mode == "wrong-module": declared["module"] = "another.module"
                elif mode == "wrong-member": declared["member"] = "another/member.py"
                elif mode == "wrong-member-hash": declared["member_sha256"] = "0" * 64
                elif mode == "wrong-source-hash": source["sha256"] = "0" * 64
                elif mode == "wrong-source": source["canonical_source"] = "unrelated.py"
                elif mode == "wrong-distribution": manifest["distribution"] = "meerkat-toolkit-services"
                elif mode == "wrong-namespace": manifest["namespace"] = "meerkat_toolkit_services"
                elif mode == "unknown-field": declared["allow_all_profiles"] = True
                elif mode == "boolean-schema": declared["schema_version"] = True
                members[manifest_name] = M.encoded(manifest)
                selected = {**self.files, wheel: archive(members, wheel=True)}
                if mode == "wrong-wheel": selected["other-0.1.0-py3-none-any.whl"] = selected.pop(wheel)
                self.assertIsNone(M.native_connector_observation(selected, "agno", "salesforce"))
        for profile in (None, "tavily", "Salesforce", [], True):
            with self.subTest(profile=profile):
                self.assertIsNone(M.native_connector_observation(self.files, "agno", profile))

    def test_changed_installed_agno_entry_cannot_inherit_verified_native_capability(self):
        self.build(family="agno", native="valid", profile="salesforce")
        self.assertIn("native_connector", self.verify())
        (self.site / self.payload).write_bytes(b"changed installed native entry")
        self.refused()

    def test_changed_frozen_agno_wheel_cannot_inherit_verified_native_capability(self):
        self.build(family="agno", native="valid", profile="salesforce")
        self.assertIn("native_connector", self.verify())
        wheel = next(name for name in self.files if name.endswith(".whl"))
        (self.destination / "inputs" / wheel).write_bytes(b"changed frozen wheel")
        self.refused()

    def test_native_declaration_wrong_member_is_not_observed(self):
        self.unsupported_native_declaration("wrong-member")

    def test_native_declaration_wrong_member_hash_is_not_observed(self):
        self.unsupported_native_declaration("wrong-hash")

    def test_native_declaration_missing_member_hash_is_not_observed(self):
        self.unsupported_native_declaration("missing-hash")

    def test_native_declaration_missing_native_mode_is_not_observed(self):
        self.unsupported_native_declaration("wrong-mode")

    def test_native_declaration_source_mapping_must_agree(self):
        self.unsupported_native_declaration("mapping-mismatch")

    def test_changed_installed_declaration_does_not_replace_selected_wheel(self):
        self.build(native="valid")
        self.assertIn("native_connector", self.verify())
        (self.site / "meerkat_toolkit_services/PACKAGE-SOURCES.json").write_bytes(b"{}")
        self.refused()

    def test_unconfigured_agno_cli_returns_current_observation_without_runtime_execution(self):
        self.build("agno", configured=False)
        argv = [sys.executable, "-I", "-B", str(SCRIPT), "--verify-existing"]
        for key in ("bundle", "contents_sha256", "family", "destination", "python"):
            argv += ["--" + key.replace("_", "-"), getattr(self.args, key)]
        before = snapshot(self.destination)
        result = subprocess.run(argv, env=ENV, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(json.loads(result.stdout)["verified"])
        self.assertEqual(snapshot(self.destination), before)
        self.assertFalse((self.root / "EXECUTED").exists())

    def test_activepieces_retained_dependency_and_bin_link_are_checked(self):
        self.build("activepieces")
        self.assertEqual(self.verify()["retained_payloads"]["payloads"]["fixture-dependency"], 2)
        (self.destination / "node_modules/fixture-dependency/index.js").write_bytes(b"modified")
        self.receipt["installed_identity"] = {"resealed": True}
        self.save_receipt()
        self.refused()

    def test_agreeing_receipt_frozen_input_and_payload_cannot_replace_original_bundle(self):
        self.build()
        (self.site / self.payload).write_bytes(b"replacement")
        self.receipt["matching_component_payloads"] = 100
        self.receipt["installed_identity"] = {"wheels": "replacement"}
        self.save_receipt()
        self.refused()
        (self.site / self.payload).write_bytes(self.members[self.payload])
        filename = next(path for path in self.files if path.endswith(".whl"))
        (self.destination / "inputs" / filename).write_bytes(b"resealed")
        self.receipt["inputs"][filename] = M.sha(b"resealed")
        self.save_receipt()
        self.refused()

    def test_original_selection_changes_are_refused(self):
        self.build()
        for key, value in (("contents_sha256", "0" * 64), ("family", "agno"), ("config_sha256", "0" * 64), ("server_name", "another")):
            old = getattr(self.args, key)
            with self.subTest(key=key):
                setattr(self.args, key, value)
                self.refused()
                setattr(self.args, key, old)
        other = self.root / "other-runtime"
        other.write_bytes(self.runtime.read_bytes())
        other.chmod(0o700)
        self.args.python = str(other)
        self.refused()

    def test_changed_config_cannot_be_authorized_by_agreeing_receipt_and_mcp(self):
        self.build()
        raw = b"{\"different\":true}\n"
        (self.destination / "configuration/selected.json").write_bytes(raw)
        self.receipt["configuration"]["sha256"] = M.sha(raw)
        self.save_receipt()
        self.refused()

    def test_receipt_selected_command_is_never_executed_or_accepted(self):
        self.build()
        self.receipt["command"][0] = str(self.root / "hostile-runtime")
        self.mcp["mcpServers"][self.args.server_name]["command"] = self.receipt["command"][0]
        self.receipt["mcp_configuration"] = self.mcp
        put(self.destination / "ready/mcp.json", M.encoded(self.mcp))
        self.save_receipt()
        self.refused()

    def test_missing_pending_or_crossed_publication_is_refused(self):
        self.build()
        original = self.receipt_path.read_bytes()
        self.receipt_path.unlink()
        self.refused()
        self.receipt_path.write_bytes(original)
        (self.destination / "pending-ready").mkdir()
        self.refused()
        (self.destination / "pending-ready").rmdir()
        (self.destination / "installed.json").write_bytes(original)
        self.refused()

    def test_configured_publication_requires_original_config_arguments(self):
        self.build()
        self.args.config = self.args.config_sha256 = self.args.server_name = None
        self.refused()

    def test_duplicate_key_and_oversized_receipt_are_refused(self):
        self.build()
        for raw in (b'{"schema_version":1,"schema_version":1}', b" " * (1024 * 1024 + 1)):
            with self.subTest(size=len(raw)):
                self.receipt_path.write_bytes(raw)
                self.refused()

    def test_symlink_fifo_and_alias_destination_are_refused(self):
        self.build()
        selected = self.site / self.payload
        raw = selected.read_bytes()
        selected.unlink()
        selected.symlink_to(self.runtime)
        self.refused()
        selected.unlink()
        os.mkfifo(selected)
        self.refused()
        selected.unlink()
        selected.write_bytes(raw)
        alias = self.root / "alias"
        alias.symlink_to(self.destination, target_is_directory=True)
        self.args.destination = str(alias)
        self.refused()

    def test_extra_startup_and_sourceless_cache_files_are_refused(self):
        self.build()
        for path in ("sitecustomize.py", "custom.pth", "surprise.pyc", "__pycache__/unknown.cpython-311.pyc", "new_dist-1.dist-info/METADATA"):
            with self.subTest(path=path):
                target = self.site / path
                put(target, b"must not be admitted")
                self.refused()
                target.unlink()

    def test_excluded_bootstrap_and_generated_bytes_are_never_relabelled_verified(self):
        self.build()
        (self.site / "pip/__init__.py").write_bytes(b"raise RuntimeError('unsafe bootstrap')")
        (self.destination / "runtime/bin/fixture").write_bytes(b"unsafe generated wrapper")
        result = self.verify()
        self.assertIn("not a filesystem sandbox or a claim the environment is safe to execute", result["custody"])
        self.assertNotIn("installed_graph", result)

    def test_relocated_script_body_and_interpreter_are_checked(self):
        self.build()
        script = self.destination / "runtime/bin/retained.py"
        good = script.read_bytes()
        for raw in (good.replace(b"retained", b"altered"), good.replace(b"/bin/python", b"/bin/other")):
            script.write_bytes(raw)
            self.refused()

    def test_retained_relocated_script_cache_is_excluded_without_execution(self):
        self.build()
        put(self.destination / "runtime/bin/__pycache__/retained.cpython-311.pyc", b"unverified relocated-script cache")
        result = self.verify()
        self.assertEqual(result["retained_payloads"]["excluded_file_counts"]["source_caches"], 2)
        (self.destination / "runtime/bin/retained.py").write_bytes(b"changed retained script")
        self.refused()

    def test_sourceless_or_generated_wrapper_bin_cache_is_refused(self):
        self.build()
        for name in ("absent.cpython-311.pyc", "fixture.cpython-311.pyc", "retained.cpython-310.pyc"):
            with self.subTest(name=name):
                path = self.destination / "runtime/bin/__pycache__" / name
                put(path, b"unselected cache")
                self.refused()
                path.unlink()

    def test_changed_venv_home_cannot_redirect_runtime_search(self):
        self.build()
        cfg = self.destination / "runtime/pyvenv.cfg"
        cfg.write_bytes(cfg.read_bytes().replace(("home = " + str(self.root)).encode(), b"home = /unselected/runtime"))
        self.refused()

    def test_public_cli_symlink_loop_is_a_fixed_refusal_without_traceback(self):
        self.build(configured=False)
        python = self.destination / "runtime/bin/python"
        python.unlink()
        python.symlink_to("python")
        argv = [sys.executable, "-I", "-B", str(SCRIPT), "--verify-existing"]
        for key in ("bundle", "contents_sha256", "family", "destination", "python"):
            argv += ["--" + key.replace("_", "-"), getattr(self.args, key)]
        result = subprocess.run(argv, env=ENV, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, b"")
        self.assertEqual(result.stderr, b"existing installation verification failed; no installation or repair was performed\n")
        self.assertFalse((self.root / "EXECUTED").exists())

    def test_extra_npm_code_or_forged_bin_link_is_refused(self):
        self.build("activepieces")
        extra = self.destination / "node_modules/fixture-dependency/extra.js"
        extra.write_bytes(b"unselected")
        self.refused()
        extra.unlink()
        link = self.destination / "node_modules/.bin/toolkit-fixture"
        link.unlink()
        link.symlink_to(self.runtime)
        self.refused()

    def test_selected_readback_requires_complete_original_intent_before_effect(self):
        result = subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), "--verify-existing", "--selected-node", str(self.root),
                                 "--destination", str(self.destination)], env=ENV, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 2)
        self.assertIn(b"selected Node mode requires selection digest, component, validator and Node", result.stderr)
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()
