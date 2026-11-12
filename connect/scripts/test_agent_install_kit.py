"""Packaging tests use inert bytes; no bundled tool or server is executed."""
from __future__ import annotations

import copy
import gzip
import hashlib
import importlib.util
import io
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


# A thin arm64 Mach-O executable header; the rest of each fixture tool is inert text.
MACHO_ARM64 = bytes.fromhex("cffaedfe0c0000010000000002000000")
RUN_TOOL_FEATURES = {
    "toolkit-local-recipes": ["local-recipes"], "toolkit-package-inspect": ["local-artifacts"],
    "toolkit-python-plan": [], "toolkit-node-plan": [], "toolkit-compose-plan": [],
    "toolkit-mcpb-install": ["local-artifacts"],
}
RUN_CONTRACTS = ("INSPECT", "PYTHON-INSTALL", "NODE-INSTALL", "MCPB-INSTALL", "LOCAL-RECIPES")
RUN_CRATES = {
    ("alpha", "1.0.0"): {"LICENSE-MIT": b"alpha MIT\n", "LICENSE-APACHE": b"alpha Apache\n",
                         "README.md": b"not a notice\n", "src/LICENSE-nested": b"not at the root\n",
                         "Cargo.toml": b"[package]\n"},
    ("beta-macro", "0.2.1"): {"NOTICE": b"beta notice\n", "src/lib.rs": b"// inert\n"},
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def put(path, data, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(mode)


def crate_archive(name, version, members):
    raw = io.BytesIO()
    with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as tar:
            for member, data in sorted(members.items()):
                info = tarfile.TarInfo(f"{name}-{version}/{member}")
                info.size, info.mode = len(data), 0o644
                tar.addfile(info, io.BytesIO(data))
    return raw.getvalue()


def write_tool_record(tools, tool, **changes):
    data = (tools / tool).read_bytes()
    features = RUN_TOOL_FEATURES[tool]
    command = ["repo-cargo", "build", "--locked", "--release", "-p", "toolkit-package",
               *(["--features", ",".join(features)] if features else []), "--bin", tool]
    record = {"schema_version": 1, "binary": "tools/" + tool, "sha256": sha256(data), "bytes": len(data),
              "mode": "0755", "target": "aarch64-apple-darwin", "command": command, "features": features,
              "cargo_profile": "release"}
    record.update(changes)
    put(tools / (tool + ".build.json"), json.dumps(record, sort_keys=True).encode())
    return record


def write_pins(run, rows=None):
    """Write the pin file from the crate files on disk plus any extra rows (connect-library-pins/1)."""
    pins = []
    for (name, version) in sorted(run["crates"]):
        file = f"{name}-{version}.crate"
        pins.append({"kind": "crate", "name": name, "version": version, "file": file,
                     "url": f"https://static.crates.io/crates/{name}/{file}", "sha256": run["crates"][(name, version)],
                     "sha512": None, "digest_source": "crates.io index cksum", "use": "tool build provenance"})
    pins.extend(rows or run.get("extra_pins", []))
    pins.sort(key=lambda row: (row["kind"], row["name"], row["version"], row["file"]))
    put(run["pins"], (json.dumps({"schema": "connect-library-pins/1", "target": "macos-arm64", "pins": pins},
                                 indent=2) + "\n").encode())


def write_lock(run):
    text = "version = 4\n\n"
    for name in ("toolkit-package", "toolkit-store"):
        text += f'[[package]]\nname = "{name}"\nversion = "0.9.14"\n\n'
    for (name, version), checksum in sorted(run["crates"].items()):
        text += (f'[[package]]\nname = "{name}"\nversion = "{version}"\n'
                 'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
                 f'checksum = "{checksum}"\n\n')
    put(run["lock"], text.encode())


def run_fixture(base, checkout=None):
    """An inert macOS run: checkout rows, lock, six tools with records, a pin file and its files."""
    checkout = checkout or base / "repo" / "connect"
    run = {"checkout": checkout, "lock": checkout.parent / "Cargo.lock", "tools": base / "work" / "tools",
           "pins": base / "work" / "library-pins-macos-arm64.json", "pinned": base / "work" / "pins",
           "rust_license": base / "work" / "sysroot" / "COPYRIGHT-library.html", "crates": {}}
    run["dependencies"] = run["tools"] / "dependencies.txt"
    # A shared checkout keeps the rows it already has: only missing ones are seeded.
    seeds = {"scripts/install-library-consumer.py": b"# inert installer source; never executed\n",
             "LICENSE-MIT": b"Toolkit LICENSE-MIT\n", "LICENSE-APACHE": b"Toolkit LICENSE-APACHE\n"}
    seeds.update({"crates/toolkit-package/" + name + ".md": ("Contract " + name + ".\n").encode() for name in RUN_CONTRACTS})
    for name, data in seeds.items():
        if not (checkout / name).exists():
            put(checkout / name, data)
    for tool in RUN_TOOL_FEATURES:
        put(run["tools"] / tool, MACHO_ARM64 + ("inert " + tool + "; never executed\n").encode(), 0o755)
        write_tool_record(run["tools"], tool)
    for (name, version), members in RUN_CRATES.items():
        raw = crate_archive(name, version, members)
        put(run["pinned"] / "crate" / f"{name}-{version}.crate", raw)
        run["crates"][(name, version)] = sha256(raw)
    put(run["dependencies"], (
        "toolkit-package v0.9.14 (" + str(checkout / "crates/toolkit-package") + ")\n"
        "alpha v1.0.0\n"
        "beta-macro v0.2.1 (proc-macro)\n"
        "toolkit-store v0.9.14 (" + str(checkout / "crates/toolkit-store") + ")\n"
        "alpha v1.0.0 (*)\n").encode())
    put(run["rust_license"], b"<html>Rust library licences</html>\n")
    write_lock(run)
    write_pins(run)
    return run


def package_run(run, output):
    return M.package_run_built(checkout=run["checkout"], lock=run["lock"], pins=run["pins"],
                               pinned_inputs=run["pinned"], tools=run["tools"],
                               tool_dependencies=run["dependencies"], rust_license=run["rust_license"],
                               output=output)


class RunBuiltKitTests(unittest.TestCase):
    """The kit built in the macOS run: no donor archive, no history tree, nothing executed."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="run built kit tests ")
        self.addCleanup(tmp.cleanup)
        self.base = Path(tmp.name)
        self.run = run_fixture(self.base)

    def refused(self, code, names=None, output="refused"):
        with self.assertRaises(M.Rejected) as caught:
            package_run(self.run, self.base / output)
        self.assertEqual(str(caught.exception), code)
        if names is not None:
            self.assertEqual(caught.exception.names, names)
        self.assertFalse((self.base / output).exists())

    def members(self, result, output):
        with tarfile.open(self.base / output / result["archive"]) as tar:
            return {member.name: (tar.extractfile(member).read(), member.mode) for member in tar.getmembers()}

    def test_run_built_kit_holds_exactly_the_run_inputs_and_is_deterministic(self):
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            first = package_run(self.run, self.base / "first")
            second = package_run(self.run, self.base / "second")
        self.assertEqual(first, second)
        archive = self.base / "first" / first["archive"]
        self.assertEqual(first["archive"], "agent-install-kit-macos-arm64.tar.gz")
        self.assertEqual(archive.read_bytes(), (self.base / "second" / second["archive"]).read_bytes())
        self.assertEqual(first["sha256"], sha256(archive.read_bytes()))
        self.assertEqual(first["bytes"], archive.stat().st_size)
        self.assertEqual((first["platform"], first["scope"]), ("macos-arm64", "run-built-selected-tools-kit"))
        self.assertEqual(json.loads((self.base / "first" / "build.json").read_text()), first)
        self.assertEqual((self.base / "first" / "SHA256SUMS").read_text(),
                         first["sha256"] + "  agent-install-kit-macos-arm64.tar.gz\n")
        self.assertEqual(sorted(path.name for path in (self.base / "first").iterdir()),
                         ["README.md", "SHA256SUMS", "agent-install-kit-macos-arm64.tar.gz", "build.json"])
        files = self.members(first, "first")
        root = "agent-install-kit-macos-arm64/"
        expected = {"SHA256SUMS", "bin/install-library-consumer.py", "licenses/rust/COPYRIGHT-library.html",
                    "licenses/toolkit/LICENSE-MIT", "licenses/toolkit/LICENSE-APACHE",
                    "licenses/crates/alpha-1.0.0/LICENSE-MIT", "licenses/crates/alpha-1.0.0/LICENSE-APACHE",
                    "licenses/crates/beta-macro-0.2.1/NOTICE", "provenance/crates/alpha-1.0.0.crate",
                    "provenance/crates/beta-macro-0.2.1.crate", "provenance/dependencies.json",
                    "provenance/kit.json"}
        expected |= {"bin/" + tool for tool in RUN_TOOL_FEATURES}
        expected |= {"provenance/tools/" + tool + ".build.json" for tool in RUN_TOOL_FEATURES}
        expected |= {"contracts/" + name + ".md" for name in RUN_CONTRACTS}
        self.assertEqual(set(files), {root + name for name in expected})
        self.assertEqual(first["files"], len(expected))
        for name, (data, mode) in files.items():
            self.assertEqual(mode, 0o755 if name.startswith(root + "bin/") else 0o644, name)
        for tool in RUN_TOOL_FEATURES:
            self.assertEqual(files[root + "bin/" + tool][0], (self.run["tools"] / tool).read_bytes())
            self.assertEqual(files[root + "provenance/tools/" + tool + ".build.json"][0],
                             (self.run["tools"] / (tool + ".build.json")).read_bytes())
        self.assertEqual(files[root + "bin/install-library-consumer.py"][0],
                         (self.run["checkout"] / "scripts/install-library-consumer.py").read_bytes())
        self.assertEqual(files[root + "licenses/crates/alpha-1.0.0/LICENSE-MIT"][0], b"alpha MIT\n")
        self.assertEqual(files[root + "provenance/crates/alpha-1.0.0.crate"][0],
                         (self.run["pinned"] / "crate/alpha-1.0.0.crate").read_bytes())
        kit = json.loads(files[root + "provenance/kit.json"][0])
        self.assertEqual((kit["schema_version"], kit["scope"], kit["platform"]),
                         (1, "run-built-selected-tools-kit", "macos-arm64"))
        self.assertEqual(kit["inputs"], {"pins_sha256": sha256(self.run["pins"].read_bytes()),
                                         "lock_sha256": sha256(self.run["lock"].read_bytes()),
                                         "tool_dependencies_sha256": sha256(self.run["dependencies"].read_bytes())})
        self.assertEqual(first["inputs"], kit["inputs"])
        self.assertEqual(set(kit["files"]), expected - {"SHA256SUMS", "provenance/kit.json"})
        facts = json.loads(files[root + "provenance/dependencies.json"][0])
        self.assertEqual(facts["target"], "aarch64-apple-darwin")
        self.assertEqual(facts["workspace_crates"], [{"name": "toolkit-package", "version": "0.9.14"},
                                                    {"name": "toolkit-store", "version": "0.9.14"}])
        self.assertEqual(facts["registry_crates"], [
            {"name": "alpha", "version": "1.0.0", "checksum": self.run["crates"][("alpha", "1.0.0")],
             "archive": "provenance/crates/alpha-1.0.0.crate",
             "notices": ["licenses/crates/alpha-1.0.0/LICENSE-APACHE", "licenses/crates/alpha-1.0.0/LICENSE-MIT"]},
            {"name": "beta-macro", "version": "0.2.1", "checksum": self.run["crates"][("beta-macro", "0.2.1")],
             "archive": "provenance/crates/beta-macro-0.2.1.crate",
             "notices": ["licenses/crates/beta-macro-0.2.1/NOTICE"]}])
        # No host path of the run reaches the kit.
        for name, (data, _) in files.items():
            self.assertNotIn(str(self.base).encode(), data, name)

    def test_run_built_kit_carries_no_history_tree_and_no_mcp_check_tool(self):
        files = self.members(package_run(self.run, self.base / "out"), "out")
        names = {name.split("/", 1)[1] for name in files}
        self.assertNotIn("bin/toolkit-mcp-check.py", names)
        self.assertNotIn("contracts/MCP-EXECUTION-CHECK.md", names)
        for tree in ("provenance/python/", "provenance/node/", "provenance/inspector/", "provenance/rust/",
                     "provenance/current-setup/", "provenance/mcpb-setup/", "provenance/compose-acquisition/",
                     "provenance/mcpb/", "verification-environment/", "setup/"):
            self.assertFalse([name for name in names if name.startswith(tree)], tree)

    def test_every_closure_crate_needs_a_pin_equal_to_the_lock_and_to_the_file(self):
        del self.run["crates"][("beta-macro", "0.2.1")]
        write_pins(self.run)
        self.refused("tool_crate_not_pinned", ["beta-macro-0.2.1.crate"], "no-pin")
        self.run = run_fixture(self.base / "b")
        self.run["crates"][("alpha", "1.0.0")] = "0" * 64
        write_pins(self.run)
        self.refused("tool_crate_pin_differs_from_lock", ["alpha-1.0.0.crate"], "pin-differs")
        self.run = run_fixture(self.base / "c")
        write_lock({**self.run, "crates": {("alpha", "1.0.0"): self.run["crates"][("alpha", "1.0.0")]}})
        self.refused("tool_crate_not_locked", ["beta-macro-0.2.1.crate"], "no-lock")
        self.run = run_fixture(self.base / "d")
        (self.run["pinned"] / "crate/alpha-1.0.0.crate").write_bytes(b"other bytes")
        self.refused("pinned_input_digest_mismatch", ["alpha-1.0.0.crate"], "other-bytes")
        self.run = run_fixture(self.base / "e")
        (self.run["pinned"] / "crate/alpha-1.0.0.crate").unlink()
        self.refused("pinned_input_unavailable", ["alpha-1.0.0.crate"], "missing-file")
        self.run = run_fixture(self.base / "f")
        target = self.run["pinned"] / "crate/alpha-1.0.0.crate"
        moved = target.with_name("moved"); target.rename(moved); target.symlink_to(moved)
        self.refused("pinned_input_unavailable", ["alpha-1.0.0.crate"], "linked-file")

    def test_extra_crate_pins_are_ignored_and_a_crate_without_a_notice_is_refused(self):
        raw = crate_archive("unused", "9.9.9", {"LICENSE": b"unused\n"})
        put(self.run["pinned"] / "crate/unused-9.9.9.crate", raw)
        self.run["crates"][("unused", "9.9.9")] = sha256(raw)
        write_pins(self.run)
        del self.run["crates"][("unused", "9.9.9")]
        files = self.members(package_run(self.run, self.base / "extra"), "extra")
        self.assertFalse([name for name in files if "unused" in name])
        raw = crate_archive("alpha", "1.0.0", {"README.md": b"no licence file\n", "docs/LICENSE": b"nested\n"})
        put(self.run["pinned"] / "crate/alpha-1.0.0.crate", raw)
        self.run["crates"][("alpha", "1.0.0")] = sha256(raw)
        write_pins(self.run); write_lock(self.run)
        self.refused("tool_crate_without_notice", ["alpha-1.0.0.crate"])

    def test_dependency_listing_shapes_and_sources_are_closed(self):
        good = self.run["dependencies"].read_bytes()
        for index, line in enumerate((b"gamma v1.0.0 (https://example.invalid/gamma#abc)\n", b"alpha 1.0.0\n",
                                      b"alpha v1.0.0 extra\n", b"../alpha v1.0.0\n",
                                      b"outside v1.0.0 (/somewhere/else)\n")):
            with self.subTest(line=line):
                self.run["dependencies"].write_bytes(good + line)
                self.refused("invalid_tool_dependency_listing", output="listing-" + str(index))
        self.run["dependencies"].write_bytes(b"")
        self.refused("invalid_tool_dependency_listing", output="listing-empty")
        self.run["dependencies"].write_bytes(b"alpha v1.0.0\nbeta-macro v0.2.1\n")
        self.refused("invalid_tool_dependency_listing", output="listing-no-tool-crate")

    def test_a_dependency_listing_with_a_terminal_escape_sequence_is_refused(self):
        # Cargo colours the repeat mark when its environment asks for colour. The listing is refused
        # whole; nothing here strips or skips terminal control text.
        good = self.run["dependencies"].read_bytes()
        for index, line in enumerate((b"alpha v1.0.0 \x1b[33m\x1b[2m(*)\x1b[39m\x1b[22m\n",
                                      b"\x1b[1malpha\x1b[0m v1.0.0\n", b"alpha v1.0.0\x1b[0m\n",
                                      b"alpha v1.0.0 (\x1b[2m*\x1b[22m)\n", b"\x1b\n")):
            with self.subTest(line=line):
                self.run["dependencies"].write_bytes(good + line)
                self.refused("invalid_tool_dependency_listing", output="escape-" + str(index))

    def test_tool_records_bind_signed_bytes_target_command_and_features(self):
        cases = [("binary", "tools/other"), ("sha256", "0" * 64), ("bytes", 1), ("mode", "0644"),
                 ("target", "x86_64-unknown-linux-gnu"), ("features", "local-recipes"),
                 ("command", ["repo-cargo", "build", "--locked", "--release", "--bin", "toolkit_gateway"]),
                 ("command", ["repo-cargo", "build", "--release", "--bin", "toolkit-local-recipes"]),
                 ("command", ["repo-cargo", "build", "--locked", "--bin", "toolkit-local-recipes"]),
                 ("command", ["repo-cargo", "build", "--locked", "--release", "--bin", "toolkit-local-recipes",
                              "--bin", "toolkit-local-recipes"]),
                 ("features", [])]
        for index, (field, value) in enumerate(cases):
            with self.subTest(field=field, value=value):
                write_tool_record(self.run["tools"], "toolkit-local-recipes", **{field: value})
                self.refused("tool_build_record_mismatch", ["toolkit-local-recipes"], "record-" + str(index))
        write_tool_record(self.run["tools"], "toolkit-local-recipes")
        # The inspector advertises MCPB archive inspection only when built with local-artifacts.
        for tool in ("toolkit-package-inspect", "toolkit-mcpb-install"):
            write_tool_record(self.run["tools"], tool, features=["local-recipes"])
            self.refused("tool_build_record_mismatch", [tool], "features-" + tool)
            write_tool_record(self.run["tools"], tool)
        record = self.run["tools"] / "toolkit-python-plan.build.json"
        record.write_bytes(record.read_bytes().replace(b'"mode": "0755"', b'"mode": "0644", "mode": "0755"'))
        self.refused("tool_build_record_mismatch", ["toolkit-python-plan"], "duplicate-key")

    def test_tools_must_be_complete_executable_and_macos_arm64(self):
        tool = self.run["tools"] / "toolkit-node-plan"
        data = tool.read_bytes()
        tool.write_bytes(b"\x7fELF" + data[4:])
        write_tool_record(self.run["tools"], "toolkit-node-plan")
        self.refused("tool_not_macos_arm64_executable", ["toolkit-node-plan"], "elf")
        tool.write_bytes(data); write_tool_record(self.run["tools"], "toolkit-node-plan")
        tool.chmod(0o644)
        self.refused("tool_unavailable", ["toolkit-node-plan"], "mode")
        tool.unlink()
        self.refused("tool_unavailable", ["toolkit-node-plan"], "missing")
        put(tool, data, 0o755)
        (self.run["tools"] / "toolkit-node-plan.build.json").unlink()
        self.refused("tool_unavailable", ["toolkit-node-plan"], "no-record")

    def test_pin_file_shape_checkout_rows_and_output_are_checked(self):
        document = json.loads(self.run["pins"].read_text())
        for index, change in enumerate(({"schema": "connect-library-pins/2"}, {"target": "linux-x86_64"},
                                        {"pins": {}})):
            self.run["pins"].write_text(json.dumps({**document, **change}))
            self.refused("invalid_pin_file", output="pins-" + str(index))
        duplicate = dict(document, pins=document["pins"] + [document["pins"][0]])
        self.run["pins"].write_text(json.dumps(duplicate))
        self.refused("invalid_pin_file", output="pins-duplicate")
        self.run["pins"].write_text(json.dumps(document))
        (self.run["checkout"] / "crates/toolkit-package/MCPB-INSTALL.md").unlink()
        self.refused("checkout_input_unavailable", ["crates/toolkit-package/MCPB-INSTALL.md"], "no-contract")
        self.run = run_fixture(self.base / "g")
        self.run["rust_license"].write_bytes(b"")
        self.refused("rust_license_unavailable", output="empty-licence")
        self.run = run_fixture(self.base / "h")
        output = self.base / "exists"; output.mkdir(); (output / "keep").write_bytes(b"keep")
        with self.assertRaises((M.Rejected, FileExistsError)):
            package_run(self.run, output)
        self.assertEqual([path.name for path in output.iterdir()], ["keep"])

    def test_cli_selects_the_run_route_only_with_pins_and_never_mixes_routes(self):
        base = [sys.executable, "-I", "-B", str(SCRIPT)]
        run = ["--target", "macos-arm64", "--pins", str(self.run["pins"]), "--pinned-inputs", str(self.run["pinned"]),
               "--tools", str(self.run["tools"]), "--tool-dependencies", str(self.run["dependencies"]),
               "--rust-license", str(self.run["rust_license"]), "--checkout", str(self.run["checkout"]),
               "--cargo-lock", str(self.run["lock"])]
        result = subprocess.run(base + run + ["--output", str(self.base / "cli")], capture_output=True, text=True, timeout=30)
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        printed = json.loads(result.stdout)
        self.assertEqual(result.stdout.count("\n"), 1)
        self.assertEqual(printed["sha256"], sha256((self.base / "cli" / printed["archive"]).read_bytes()))
        (self.run["pinned"] / "crate/beta-macro-0.2.1.crate").write_bytes(b"other")
        result = subprocess.run(base + run + ["--output", str(self.base / "cli-refused")], capture_output=True, text=True, timeout=30)
        self.assertEqual((result.returncode, result.stdout), (2, ""))
        self.assertEqual(result.stderr, "installation kit packaging refused: pinned_input_digest_mismatch: beta-macro-0.2.1.crate\n")
        self.assertFalse((self.base / "cli-refused").exists())
        for index, arguments in enumerate((run + ["--inputs", str(self.base)], run[2:], run[:-8],
                                           ["--target", "linux-x86_64"] + run[2:])):
            result = subprocess.run(base + arguments + ["--output", str(self.base / ("mixed-" + str(index)))],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual((result.returncode, result.stdout), (2, ""), arguments)
            self.assertFalse((self.base / ("mixed-" + str(index))).exists())


if __name__ == "__main__":
    unittest.main()
