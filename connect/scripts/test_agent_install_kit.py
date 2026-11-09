"""Packaging tests use inert bytes; no bundled tool or server is executed."""
from __future__ import annotations

import copy
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("package-agent-install-kit.py")
SPEC = importlib.util.spec_from_file_location("kit_packager", SCRIPT)
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


class AgentKitTests(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="kit packaging tests ")
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.inputs = self.root / "inputs"
        self.inputs.mkdir()
        self.entries = []
        for i, name in enumerate(sorted(M.REQUIRED)):
            source = "retained/" + str(i)
            file = self.inputs / source
            file.parent.mkdir(exist_ok=True)
            # A delivered Python program that would fail if the packager ran it.
            data = b'raise RuntimeError("packaging must never execute this")\n'
            file.write_bytes(data)
            self.entries.append({"source": source, "path": name, "sha256": hashlib.sha256(data).hexdigest(), "mode": 0o755 if name.startswith("bin/") else 0o644})
        self.write_manifest()

    def write_manifest(self):
        self.spec = {"schema_version": 1, "scope": "selected-python-node-installation-kit", "files": self.entries}
        self.manifest = self.inputs / "kit-inputs.json"
        self.manifest.write_text(json.dumps(self.spec))

    def rejected(self):
        output = self.root / "output"
        with self.assertRaises((M.Rejected, OSError, ValueError, TypeError)):
            M.package(self.inputs, output)
        self.assertFalse(output.exists())

    def test_deterministic_archive_preserves_only_checked_bytes_and_modes(self):
        original = {str(p.relative_to(self.inputs)): p.read_bytes() for p in self.inputs.rglob("*") if p.is_file()}
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            a = M.package(self.inputs, self.root / "first")
            b = M.package(self.inputs, self.root / "second")
        self.assertEqual(a, b)
        archive = self.root / "first" / a["archive"]
        with tarfile.open(archive) as tar:
            members = tar.getmembers()
            self.assertEqual(len(members), len(self.entries) + 2)
            self.assertTrue(all(m.isfile() and m.uid == m.gid == m.mtime == 0 for m in members))
            self.assertTrue(all(m.name.startswith(M.ROOT + "/") and ".." not in Path(m.name).parts for m in members))
            for entry in self.entries:
                member = tar.getmember(M.ROOT + "/" + entry["path"])
                self.assertEqual(member.mode, entry["mode"])
                self.assertEqual(hashlib.sha256(tar.extractfile(member).read()).hexdigest(), entry["sha256"])
            metadata = json.load(tar.extractfile(M.ROOT + "/provenance/kit.json"))
            self.assertEqual(set(metadata["files"]), M.REQUIRED)
            sums = tar.extractfile(M.ROOT + "/SHA256SUMS").read().decode().splitlines()
            for line in sums:
                digest, name = line.split("  ")
                self.assertEqual(hashlib.sha256(tar.extractfile(M.ROOT + "/" + name).read()).hexdigest(), digest)
        self.assertEqual(original, {str(p.relative_to(self.inputs)): p.read_bytes() for p in self.inputs.rglob("*") if p.is_file()})

    def test_malformed_scope_is_explicitly_rejected(self):
        for scope in ([], {}, None, 7):
            with self.subTest(scope=scope):
                self.spec["scope"] = scope
                self.manifest.write_text(json.dumps(self.spec))
                with self.assertRaises(M.Rejected):
                    M.load_inputs(self.inputs)

    def python_profile(self):
        self.entries = [entry for entry in self.entries if entry["path"] in M.PYTHON_REQUIRED]
        for index, name in enumerate(sorted(M.PYTHON_REQUIRED - {entry["path"] for entry in self.entries})):
            source = "retained/python-" + str(index)
            data = b"Inert selected Python delivery fixture\n"
            (self.inputs / source).write_bytes(data)
            self.entries.append({"source": source, "path": name, "sha256": hashlib.sha256(data).hexdigest(),
                                 "mode": 0o755 if name.startswith("bin/") else 0o644})
        self.write_manifest()
        self.spec.update(scope="selected-python-installation-kit", platform="linux-x86_64")
        self.manifest.write_text(json.dumps(self.spec))

    def test_linux_python_scope_preserves_pinned_files_and_declares_exact_platform(self):
        self.python_profile()
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            result = M.package(self.inputs, self.root / "linux")
        self.assertEqual(result["archive"], "agent-install-kit-linux-x86_64.tar.gz")
        with tarfile.open(self.root / "linux" / result["archive"]) as archive:
            self.assertTrue(all(member.name.startswith(M.LINUX_ROOT + "/") for member in archive.getmembers()))
            meta = json.load(archive.extractfile(M.LINUX_ROOT + "/provenance/kit.json"))
            self.assertEqual(meta["platform"], "linux-x86_64")
            self.assertEqual(meta["scope"], "selected-python-installation-kit")
            self.assertEqual(set(meta["files"]), M.PYTHON_REQUIRED)
            for entry in self.entries:
                member = archive.getmember(M.LINUX_ROOT + "/" + entry["path"])
                self.assertEqual(member.mode, entry["mode"])
                self.assertEqual(archive.extractfile(member).read(), (self.inputs / entry["source"]).read_bytes())
        guide = (self.root / "linux/README.md").read_text()
        self.assertIn("Native 64-bit CPython 3.11", guide)
        self.assertNotIn("Python/Node", guide)
        self.assertNotIn("macOS", guide)

    def test_linux_requires_the_complete_python_owner_and_contracts(self):
        self.python_profile()
        original = copy.deepcopy(self.spec)
        for name in sorted(M.PYTHON_REQUIRED):
            with self.subTest(missing=name):
                self.spec = copy.deepcopy(original)
                self.spec["files"] = [entry for entry in self.spec["files"] if entry["path"] != name]
                self.manifest.write_text(json.dumps(self.spec))
                self.rejected()

    def test_platform_and_scope_cannot_disagree_or_infer_linux_from_payloads(self):
        self.python_profile()
        for platform in (None, True, {}, [], "linux-arm64", "macos-arm64"):
            with self.subTest(platform=platform):
                self.spec["platform"] = platform
                self.manifest.write_text(json.dumps(self.spec))
                self.rejected()
        self.spec.pop("platform")
        self.manifest.write_text(json.dumps(self.spec))
        self.rejected()
        for scope in ("selected-python-node-installation-kit", "selected-python-node-mcpb-installation-kit"):
            self.spec.update(scope=scope, platform="linux-x86_64")
            self.manifest.write_text(json.dumps(self.spec))
            self.rejected()

    def test_linux_refuses_non_python_binary_tools_but_preserves_shared_contracts(self):
        self.python_profile()
        original = copy.deepcopy(self.spec)
        for path in ("bin/toolkit-node-plan", "bin/toolkit-mcpb-install", "bin/toolkit-compose-plan", "bin/other-tool"):
            self.spec = copy.deepcopy(original)
            extra = {**self.entries[0], "path": path, "mode": 0o755}
            self.spec["files"].append(extra)
            self.manifest.write_text(json.dumps(self.spec))
            self.rejected()
        self.spec = copy.deepcopy(original)
        for path in ("contracts/NODE-INSTALL.md", "contracts/MCPB-INSTALL.md", "contracts/COMPOSE.md"):
            self.spec["files"].append({**self.entries[0], "path": path, "mode": 0o644})
        self.manifest.write_text(json.dumps(self.spec))
        self.assertIn("contracts/COMPOSE.md", M.load_inputs(self.inputs))

    def linux_node_profile(self):
        for index, name in enumerate(sorted(M.LINUX_NODE_REQUIRED - {entry["path"] for entry in self.entries})):
            source = "retained/node-" + str(index)
            data = b"Inert Linux Python/Node delivery fixture\n"
            (self.inputs / source).write_bytes(data)
            self.entries.append({"source": source, "path": name, "sha256": hashlib.sha256(data).hexdigest(),
                                 "mode": 0o755 if name.startswith("bin/") else 0o644})
        self.write_manifest()
        self.spec["platform"] = "linux-x86_64"
        self.manifest.write_text(json.dumps(self.spec))

    def test_linux_node_scope_preserves_all_pinned_members_without_execution(self):
        self.linux_node_profile()
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            result = M.package(self.inputs, self.root / "linux-node")
        with tarfile.open(self.root / "linux-node" / result["archive"]) as archive:
            meta = json.load(archive.extractfile(M.LINUX_ROOT + "/provenance/kit.json"))
            self.assertEqual(meta["scope"], "selected-python-node-installation-kit")
            self.assertEqual(meta["platform"], "linux-x86_64")
            self.assertEqual(set(meta["files"]), M.LINUX_NODE_REQUIRED)
            for entry in self.entries:
                member = archive.getmember(M.LINUX_ROOT + "/" + entry["path"])
                self.assertEqual(member.mode, entry["mode"])
                self.assertEqual(hashlib.sha256(archive.extractfile(member).read()).hexdigest(), entry["sha256"])
        guide = (self.root / "linux-node/README.md").read_text()
        self.assertIn("Python/Node", guide)
        self.assertIn("Node 24", guide)
        self.assertNotIn("macOS", guide)

    def test_linux_node_requires_complete_owner_and_contracts(self):
        self.linux_node_profile()
        original = copy.deepcopy(self.spec)
        for name in sorted(M.LINUX_NODE_REQUIRED):
            with self.subTest(missing=name):
                self.spec = copy.deepcopy(original)
                self.spec["files"] = [entry for entry in self.spec["files"] if entry["path"] != name]
                self.manifest.write_text(json.dumps(self.spec))
                self.rejected()

    def test_linux_node_refuses_other_executables_and_mcpb_scope(self):
        self.linux_node_profile()
        original = copy.deepcopy(self.spec)
        for name in ("bin/toolkit-compose-plan", "bin/toolkit-mcpb-install", "bin/other-tool"):
            with self.subTest(extra=name):
                self.spec = copy.deepcopy(original)
                self.spec["files"].append({**self.entries[0], "path": name, "mode": 0o755})
                self.manifest.write_text(json.dumps(self.spec))
                self.rejected()
        self.spec = copy.deepcopy(original)
        self.spec["scope"] = "selected-python-node-mcpb-installation-kit"
        self.manifest.write_text(json.dumps(self.spec))
        self.rejected()

    def test_linux_node_allows_inherited_reference_contracts_without_their_tools(self):
        self.linux_node_profile()
        for path in ("contracts/MCPB-INSTALL.md", "contracts/COMPOSE.md"):
            self.spec["files"].append({**self.entries[0], "path": path, "mode": 0o644})
        self.manifest.write_text(json.dumps(self.spec))
        files = M.load_inputs(self.inputs)
        self.assertIn("contracts/MCPB-INSTALL.md", files)
        self.assertNotIn("bin/toolkit-mcpb-install", files)

    def test_legacy_mac_tar_and_readme_bytes_match_previous_packager(self):
        # Derived from retained pre-Linux packager SHA256 65d97cb1b3b234a2635509dc3949476e716aaed7ba159dcf4bffdd34e494016b.
        # Hash the tar stream independently of a host zlib version's compression.
        for profile, tar_sha, readme_sha in (
            ("basic", "9c98071336751053ad817ff4c5a525cb25648ac7d9afbaa3b9450b4c54a6e511",
             "603557c6a137f514bb3387d49dcfd89ab63b59aa358fba3b410ae4cbef62edce"),
            ("mcpb", "2c4b2a3292cef809c912d36970445c4877f9b68a802dd67aed01ac6c32120071",
             "0c0b39adfd7b790aaef7e13d25edab1544ac4fedca192d17a085b32a56eae71b"),
        ):
            with self.subTest(profile=profile):
                if profile == "mcpb":
                    self.mcpb_profile()
                output = self.root / profile
                result = M.package(self.inputs, output)
                archive = (output / result["archive"]).read_bytes()
                self.assertEqual(hashlib.sha256(gzip.decompress(archive)).hexdigest(), tar_sha)
                self.assertEqual(hashlib.sha256((output / "README.md").read_bytes()).hexdigest(), readme_sha)

    def mcpb_profile(self):
        for i, name in enumerate(sorted(M.MCPB_REQUIRED)):
            source = "retained/mcpb-" + str(i)
            data = b"MCPB selected input; not executable test code\n"
            (self.inputs / source).write_bytes(data)
            self.entries.append({"source": source, "path": name,
                "sha256": hashlib.sha256(data).hexdigest(),
                "mode": 0o755 if name.startswith("bin/") else 0o644})
        self.write_manifest()
        self.spec["scope"] = "selected-python-node-mcpb-installation-kit"
        self.manifest.write_text(json.dumps(self.spec))

    def test_mcpb_profile_preserves_exact_installer_contract_and_explicit_scope(self):
        self.mcpb_profile()
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            result = M.package(self.inputs, self.root / "mcpb-output")
        with tarfile.open(self.root / "mcpb-output" / result["archive"]) as archive:
            meta = json.load(archive.extractfile(M.ROOT + "/provenance/kit.json"))
            self.assertEqual(meta["scope"], "selected-python-node-mcpb-installation-kit")
            self.assertTrue(M.MCPB_REQUIRED.issubset(meta["files"]))
            for entry in self.entries:
                member = archive.getmember(M.ROOT + "/" + entry["path"])
                self.assertEqual(member.mode, entry["mode"])
                self.assertEqual(archive.extractfile(member).read(), (self.inputs / entry["source"]).read_bytes())
        self.assertIn("Python/Node/MCPB", (self.root / "mcpb-output/README.md").read_text())

    def test_mcpb_profile_requires_its_tool_contract_and_provenance(self):
        self.mcpb_profile()
        original = copy.deepcopy(self.spec)
        for name in sorted(M.MCPB_REQUIRED):
            with self.subTest(missing=name):
                self.spec = copy.deepcopy(original)
                self.spec["files"] = [e for e in self.spec["files"] if e["path"] != name]
                self.manifest.write_text(json.dumps(self.spec))
                self.rejected()

    def test_old_profile_cannot_silently_claim_mcpb_tool_or_contract(self):
        self.mcpb_profile()
        self.spec["scope"] = "selected-python-node-installation-kit"
        self.manifest.write_text(json.dumps(self.spec))
        self.rejected()

    def test_changed_input_refused_before_output(self):
        (self.inputs / self.entries[0]["source"]).write_bytes(b"changed")
        self.rejected()

    def test_symlink_file_and_symlink_parent_are_refused(self):
        original = self.inputs / self.entries[0]["source"]
        outside = self.root / "outside"
        outside.write_bytes(original.read_bytes())
        original.unlink()
        original.symlink_to(outside)
        self.rejected()
        original.unlink()
        original.write_bytes(outside.read_bytes())
        retained = self.inputs / "retained"
        retained.rename(self.root / "moved")
        retained.symlink_to(self.root / "moved", target_is_directory=True)
        self.rejected()

    def test_missing_and_special_file_are_refused(self):
        original = self.inputs / self.entries[0]["source"]
        original.unlink()
        self.rejected()
        original.mkdir()
        self.rejected()

    def test_unsafe_or_unportable_paths_are_refused(self):
        original = copy.deepcopy(self.entries)
        for value in ("/absolute", "../escape", "ok/../escape", "ok//file", "./file", "a\\b", "line\nname", "a" * 101, "a/" * 111):
            with self.subTest(value=value):
                self.entries = copy.deepcopy(original)
                self.entries[0]["path"] = value
                self.write_manifest()
                self.rejected()

    def test_source_escape_refused(self):
        self.entries[0]["source"] = "../outside"
        self.write_manifest()
        self.rejected()

    def test_duplicate_case_collision_reserved_and_file_directory_collision(self):
        original = copy.deepcopy(self.entries)
        for name in (original[0]["path"], original[0]["path"].upper(), "SHA256SUMS", "sha256sums", "provenance/kit.json", "provenance/kit.json/child", "SHA256SUMS/child", "sha256sums/child", original[0]["path"] + "/child", original[0]["path"].upper() + "/child"):
            with self.subTest(name=name):
                self.entries = copy.deepcopy(original)
                extra = copy.deepcopy(original[0])
                extra["path"] = name
                self.entries.append(extra)
                self.write_manifest()
                self.rejected()

    def test_size_and_count_budgets_refuse_before_output(self):
        for constant, value in (("MAX_FILE", 1), ("MAX_TOTAL", 1), ("MAX_FILES", 1), ("MAX_MANIFEST", 1)):
            with self.subTest(constant=constant), patch.object(M, constant, value):
                self.rejected()

    def test_required_tools_modes_version_and_scope_are_checked(self):
        original = copy.deepcopy(self.entries)
        self.entries.pop()
        self.write_manifest()
        self.rejected()
        self.entries = original
        self.entries[0]["mode"] = True
        self.write_manifest()
        self.rejected()
        self.entries[0]["mode"] = 0o644
        self.write_manifest()
        for key, value in (("schema_version", True), ("scope", "another"), ("files", {})):
            self.spec[key] = value
            self.manifest.write_text(json.dumps(self.spec))
            self.rejected()
            self.write_manifest()

    def test_executable_modes_must_match_the_delivered_role(self):
        for entry in self.entries:
            if entry["path"].startswith("bin/"):
                entry["mode"] = 0o644
                break
        self.write_manifest()
        self.rejected()

    def test_existing_output_is_never_overwritten(self):
        output = self.root / "output"
        output.mkdir()
        sentinel = output / "keep"
        sentinel.write_bytes(b"keep")
        with self.assertRaises(FileExistsError):
            M.package(self.inputs, output)
        self.assertEqual(list(output.iterdir()), [sentinel])
        self.assertEqual(sentinel.read_bytes(), b"keep")

    def test_malformed_manifest_cli_has_fixed_refusal(self):
        self.manifest.write_text('{"secret":"do not echo"')
        result = subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), "--inputs", str(self.inputs), "--output", str(self.root / "output")], capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr, "installation kit packaging refused; preserve any partial output\n")
        self.assertFalse((self.root / "output").exists())


if __name__ == "__main__":
    unittest.main()
