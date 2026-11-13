#!/usr/bin/env python3
"""Focused archive construction and input-boundary tests, with small fixtures."""

import copy
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import signal
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("package_library_release", Path(__file__).with_name("package-library-release.py"))
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The macOS run route packages what the kit packager wrote: its tests' inert run fixture is shared.
kit_fixture = load("agent_install_kit_fixture", "test_agent_install_kit.py")
REAL_ROUTE_BINDINGS = copy.deepcopy(release.OFFICIAL_ROUTE_BINDINGS)
RUN_TEXTS = "release/library-macos-arm64"
PLAYWRIGHT = {"@playwright/mcp": ("0.0.83", "mcp-0.0.83.tgz"),
              "playwright": ("1.64.0-alpha-1790635538000", "playwright-1.64.0-alpha-1790635538000.tgz"),
              "playwright-core": ("1.64.0-alpha-1790635538000", "playwright-core-1.64.0-alpha-1790635538000.tgz")}
GUIDE_TEMPLATE = """# Fixture guide for {{EDITION}}
--contents-sha256 {{CONSUMERS_CONTENTS_SHA256}}
"--contents-sha256", "{{CONSUMERS_CONTENTS_SHA256}}",
"--official-contents-sha256", "{{OFFICIAL_CONTENTS_SHA256}}",
"--official-installer-sha256", "{{OFFICIAL_INSTALLER_SHA256}}",
"--verifier-sha256", "{{VERIFIER_SHA256}}",
--config-sha256 {{HACKERNEWS_CONFIG_SHA256}}
GitHub MCP server v{{GITHUB_VERSION}} at {{GITHUB_SOURCE_REVISION}}, executable {{GITHUB_EXECUTABLE_SHA256}}.
"""


def gzip_tar(members, *, prefix=""):
    raw = io.BytesIO()
    with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as archive:
            for name, (data, mode) in sorted(members.items()):
                item = tarfile.TarInfo(prefix + name)
                item.mode, item.size = mode, len(data)
                archive.addfile(item, io.BytesIO(data))
    return raw.getvalue()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write(path, data, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(mode)


def manifest(directory):
    rows = {}
    for path in sorted(directory.rglob("*")):
        if path.is_file() and path.name not in {"CONTENTS.json", "SHA256SUMS"}:
            data = path.read_bytes()
            rows[str(path.relative_to(directory))] = {"sha256": sha(data), "bytes": len(data)}
    contents = (json.dumps({"files": rows}, sort_keys=True) + "\n").encode()
    write(directory / "CONTENTS.json", contents)
    sums = {name: row["sha256"] for name, row in rows.items()}
    sums["CONTENTS.json"] = sha(contents)
    raw = "".join(f"{value}  {name}\n" for name, value in sorted(sums.items())).encode()
    write(directory / "SHA256SUMS", raw)
    return {"contents": sha(contents), "sums": sha(raw)}


def donor_archive(path, root, files):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(path, "w:gz") as archive:
        for name, (data, mode) in sorted(files.items()):
            item = tarfile.TarInfo(root + "/" + name)
            item.mode, item.size = mode, len(data)
            archive.addfile(item, io.BytesIO(data))
    return sha(path.read_bytes())


class LinuxMcpbSelectionTests(unittest.TestCase):
    def test_actual_linux_selection_binds_two_feature_enabled_tools_and_notices(self):
        linux = release.PYTHON_KITS["linux-x86_64"]
        self.assertIn("mcpb-archive-inspection", linux["capabilities"])
        for name, key in (("toolkit-mcpb-install", "tool_additions"), ("toolkit-package-inspect", "tool_updates")):
            selected = linux[key]["bin/" + name]
            record = json.loads((release.ROOT / selected["record"]).read_text())
            self.assertEqual(record["binary"], selected["source"])
            self.assertEqual(record["sha256"], selected["sha256"])
            self.assertEqual(record["target"], "x86_64-unknown-linux-gnu")
            self.assertEqual(record["features"], ["local-artifacts"])
        self.assertIn("selected/contracts/MCPB-INSTALL.md", linux["sources"])
        dep = linux["sources"]["selected/provenance/linux-mcpb/dependencies.json"]
        raw = (release.ROOT / dep[0]).read_bytes()
        self.assertEqual(sha(raw), dep[1])
        facts = json.loads(raw)
        self.assertTrue({"zip", "flate2", "rustix"} <= {x["name"] for x in facts["registry_crates"]})
        self.assertTrue(all(x["notices"] for x in facts["registry_crates"]))
        self.assertEqual(release.TARGETS["linux-x86_64"]["edition"], "20261009-library-support-selection")
        self.assertEqual(release.EDITION, "20261009-library-support-selection")


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="toolkit-release-tests-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.root = self.base / "checkout"
        self.root.mkdir()
        self.targets = copy.deepcopy(release.TARGETS)
        self.python_kits = copy.deepcopy(release.PYTHON_KITS)
        # Tests opt in to additions and overrides from this fixed Linux donor.
        for key in ("tool_updates", "tool_additions", "capabilities", "sources"):
            self.python_kits["linux-x86_64"].pop(key, None)
        self.python_sources = {}
        self.donor_files = {}
        self.official = self.targets["macos-arm64"]["official"]
        self.sources = {}
        for destination, (path, _) in release.SOURCES.items():
            data = ("selected source: " + path + "\n").encode()
            write(self.root / path, data)
            self.sources[destination] = (path, sha(data))
        for name, selection in self.targets.items():
            for destination, (path, _) in selection.get("setup_assets", {}).items():
                data = ("Selected setup asset: " + path + "\n").encode()
                write(self.root / path, data)
                selection["setup_assets"][destination] = (path, sha(data))
            backend_path, _ = selection["setup_backend"]
            backend = ("Selected setup backend: " + backend_path + "\n").encode()
            write(self.root / backend_path, backend)
            selection["setup_backend"] = (backend_path, sha(backend))
            if "setup_adapter" in selection:
                adapter_path, _ = selection["setup_adapter"]
                adapter = ("Selected setup adapter: " + adapter_path + "\n").encode()
                write(self.root / adapter_path, adapter)
                selection["setup_adapter"] = (adapter_path, sha(adapter))
            if "setup_installer" in selection:
                installer_path, _ = selection["setup_installer"]
                installer = ("Selected setup installer: " + installer_path + "\n").encode()
                write(self.root / installer_path, installer)
                selection["setup_installer"] = (installer_path, sha(installer))
            directory = self.root / selection["bundle"]
            write(directory / "README.md", b"Frozen inner guide.\n")
            write(directory / "package/payload.whl", (name + " exact wheel bytes").encode())
            write(directory / ("notices/" + "a" * 110 + ".txt"), b"Retained license\n")
            selection.update(manifest(directory))
            guide = ("Selected release guide: " + name + "\n").encode()
            write(self.root / selection["guide"], guide)
            selection["guide_sha256"] = sha(guide)
            donor = self.python_kits[name]
            files = {path: ((name + " selected " + path).encode(), 0o755) for path in donor["tools"]}
            donor["tools"] = {path: sha(data) for path, (data, _) in files.items()}
            for prefix in ("licenses/crates/", "licenses/rust/", "licenses/toolkit/", "provenance/crates/", *donor["provenance_trees"]):
                files[prefix + "retained.txt"] = (b"retained original evidence", 0o644)
            for path in (*donor["provenance_files"], "provenance/kit.json", "provenance/dependencies.json"):
                files[path] = (b"{}\n", 0o644)
            for path in release.PYTHON_DONOR_CONTRACTS | donor.get("contracts", {}):
                files[path.removeprefix("selected/")] = (b"Retained contract.\n", 0o644)
            for destination, (path, _) in donor.get("sources", {}).items():
                data = ("Selected target source: " + path + "\n").encode()
                write(self.root / path, data)
                donor["sources"][destination] = (path, sha(data))
            for path in ("setup/backend.py", "setup/python_acquisition.py", "verification-environment/old.whl", "bin/unselected-tool"):
                files[path] = (b"not selected from donor", 0o755 if path.startswith("bin/") else 0o644)
            donor["sha256"] = donor_archive(self.root / donor["archive"], donor["root"], files)
            for tool, update in donor.get("tool_updates", {}).items():
                raw = ("updated tool: " + tool + "\n").encode()
                write(self.root / update["source"], raw, 0o755)
                update["sha256"] = sha(raw)
                record = json.dumps({"sha256": sha(raw)}).encode()
                write(self.root / update["record"], record)
                for destination, (path, _) in donor.get("sources", {}).items():
                    if path == update["record"]:
                        donor["sources"][destination] = (path, sha(record))
            self.donor_files[name] = files
        for destination, (path, _) in release.PYTHON_SOURCES.items():
            data = ("Selected Python source: " + path + "\n").encode()
            write(self.root / path, data)
            self.python_sources[destination] = (path, sha(data))
        for name, selection in self.targets.items():
            official = selection["official"]
            directory = self.root / official["bundle"]
            write(directory / "README.md", b"Frozen official guide.\n")
            write(directory / release.OFFICIAL_EXECUTABLE, (name + " selected original binary\n").encode(), 0o755)
            if "playwright" in official["components"]:
                write(directory / "playwright/archive.tgz", b"selected original npm archive\n")
            rows = [{"catalog_id": component, **({"target": name} if name != "macos-arm64" else {})}
                    for component in official["components"]]
            delivery = {"format": "meerkat-toolkit.official-consumers.v1",
                        "catalog_ids": list(official["components"]), "components": rows}
            write(directory / "delivery.json", json.dumps(delivery).encode())
            official.update(manifest(directory))
        # Unrelated checkout state must not become distribution input.
        write(self.root / "sdk/consumer-setup/tests/private-fixture.json", b"not a release input")
        write(self.root / "sdk/consumer-setup/workspace/start.html", b"not a release input")
        write(self.root / "unrelated.txt", b"not a release input")
        for name, value in [("TARGETS", self.targets), ("SOURCES", self.sources),
                            ("OFFICIAL_ROUTE_BINDINGS", {target: [] for target in self.targets}),
                            ("PYTHON_KITS", self.python_kits), ("PYTHON_SOURCES", self.python_sources)]:
            manager = patch.object(release, name, value)
            manager.start()
            self.addCleanup(manager.stop)

    def bundle(self, target="macos-arm64"):
        return self.root / self.targets[target]["bundle"]

    def package(self, name="output", target="macos-arm64", root=None):
        return release.package(root or self.root, target, self.base / name)

    def repin_bundle(self, target="macos-arm64"):
        self.targets[target].update(manifest(self.bundle(target)))

    def assert_refused(self, code, *, output="refused", target="macos-arm64"):
        with self.assertRaisesRegex(release.PackageError, "^" + code + "$"):
            self.package(output, target)
        self.assertFalse((self.base / output).exists())

    def repin_donor(self, target="macos-arm64"):
        donor = self.python_kits[target]
        donor["sha256"] = donor_archive(self.root / donor["archive"], donor["root"], self.donor_files[target])

    def explicit_inputs(self, target="macos-arm64"):
        source = self.targets[target]
        directory = self.bundle(target)
        for name in ("agno/runtime-constraints.txt", "services/runtime-constraints.txt",
                     "activepieces/package.json", "activepieces/package-lock.json",
                     "agno/meerkat_toolkit_agno-0.1.4-py3-none-any.whl",
                     "services/meerkat_toolkit_services-0.1.23-py3-none-any.whl",
                     "activepieces/rkat-toolkit-activepieces-0.1.0.tgz"):
            write(directory / name, (target + " inert " + name).encode())
        manifest(directory)
        contents = json.loads((directory / "CONTENTS.json").read_bytes())
        closure_path = "docs/controlled-current/" + target + "-dependency-closure.json"
        closure = {"schema_version": 1, "scope": "dependency_bytes_for_existing_consumer_selection",
            "python_platform": {"implementation": "cpython", "version": "3.11",
                "system": "Darwin" if target == "macos-arm64" else "Linux",
                "machine": "arm64" if target == "macos-arm64" else "x86_64"},
            "accepted_inputs": {name: row["sha256"] for name, row in contents["files"].items()
                if name.startswith(("agno/", "services/", "activepieces/"))}}
        closure_raw = json.dumps(closure).encode()
        write(self.root / closure_path, closure_raw)
        contents.update(schema_version=1, scope="consumer_export_only_no_installation_or_activation",
                        accepted_inputs={closure_path: sha(closure_raw)})
        if target == "linux-x86_64":
            contents.update(schema_version=2, runtime_target={
                "python": {"platform": "linux", "machine": "x86_64", "pointer_bits": 64},
                "node": {"platform": "linux", "arch": "x64"}})
        raw = (json.dumps(contents, sort_keys=True) + "\n").encode()
        write(directory / "CONTENTS.json", raw)
        hashes = {name: row["sha256"] for name, row in contents["files"].items()}
        hashes["CONTENTS.json"] = sha(raw)
        sums = "".join(value + "  " + name + "\n" for name, value in sorted(hashes.items())).encode()
        write(directory / "SHA256SUMS", sums)
        # Fixture legacy pins follow this inert producer metadata; tests of
        # explicit selection still independently mutate only the supplied row.
        source.update(contents=sha(raw), sums=sha(sums))
        return {"schema_version": 1, "target": target, "edition": "controlled-selected-inputs",
                "consumer": {**{key: source[key] for key in ("bundle", "contents", "sums")},
                             "dependency_closure": closure_path, "dependency_closure_sha256": sha(closure_raw)},
                "sources": {}, "tools": {}}

    def select_recipe_tool(self, value):
        name = "bin/toolkit-local-recipes"
        source = "dist/controlled-current/toolkit-local-recipes"
        record = "docs/controlled-current/recipes-build.json"
        data = b"inert selected recipe tool; never executed"
        write(self.root / source, data, 0o755)
        body = {"binary": source, "sha256": sha(data), "bytes": len(data), "mode": "0755",
                "target": {"macos-arm64": "aarch64-apple-darwin",
                           "linux-x86_64": "x86_64-unknown-linux-gnu"}[value["target"]],
                "command": ["cargo", "build", "--bin", "toolkit-local-recipes"], "features": ["local-recipes"]}
        write(self.root / record, json.dumps(body).encode())
        value["tools"][name] = {"source": source, "sha256": sha(data), "record": record,
                                "record_sha256": sha((self.root / record).read_bytes())}
        return body

    def explicit_package(self, value, *, name="selected", target=None):
        path = self.base / (name + "-inputs.json")
        raw = (json.dumps(value, sort_keys=True) + "\n").encode()
        write(path, raw)
        return release.package(self.root, target or value["target"], self.base / name,
            input_selection=path, input_selection_sha256=sha(raw))

    def enterprise_inputs(self):
        value = self.explicit_inputs("linux-x86_64")
        directory = self.root / "dist/selected-enterprise"
        binary = b"inert enterprise input, never executed"
        delivery = {"format":"meerkat-toolkit.official-consumers.v1", "components":[{
            "catalog_id":"github", "profile":"enterprise-selected-reads", "target":"linux-x86_64",
            "artifact":"github/github-mcp-server", "artifact_sha256":sha(binary)}]}
        write(directory / "github/github-mcp-server", binary, 0o600)
        write(directory / "delivery.json", json.dumps(delivery).encode(), 0o600)
        # Match the offline helper's CONTENTS-only, non-executable input layout.
        manifest(directory)
        (directory / "SHA256SUMS").unlink()
        value['official'] = {'bundle':'dist/selected-enterprise',
            'contents':sha((directory / 'CONTENTS.json').read_bytes()), 'profile':'enterprise-selected-reads'}
        source, pin = self.targets['linux-x86_64']['setup_installer']
        value['sources']['setup/install-library-consumer.py'] = {'path':source,'sha256':pin}
        source, pin = self.targets['linux-x86_64']['setup_backend']
        value['sources']['setup/backend.py'] = {'path':source,'sha256':pin}
        return value, directory

    def test_explicit_enterprise_bundle_preserves_bytes_and_does_not_retarget_catalog(self):
        value, directory = self.enterprise_inputs()
        before = copy.deepcopy((release.TARGETS, release.OFFICIAL_ROUTE_BINDINGS))
        original = {str(path.relative_to(directory)):path.read_bytes() for path in directory.rglob('*') if path.is_file()}
        # Default GitHub.com associations must not run for this explicit family.
        with patch.object(release, 'compose_official_routes', side_effect=AssertionError('implicit route')):
            result = self.explicit_package(value, name='enterprise')
        prefix = 'toolkit-library-controlled-selected-inputs-linux-x86_64/'
        with tarfile.open(result['archive']) as archive:
            for name, raw in original.items():
                self.assertEqual(archive.extractfile(prefix+'official/'+name).read(), raw)
            self.assertEqual(archive.getmember(prefix+'official/github/github-mcp-server').mode,0o755)
            self.assertNotIn(prefix+'official/SHA256SUMS', archive.getnames())
            selected = json.load(archive.extractfile(prefix+'selected/provenance/current-input-selection.json'))
            self.assertEqual(selected['official'],value['official'])
        self.assertEqual((release.TARGETS,release.OFFICIAL_ROUTE_BINDINGS),before)
        self.assertEqual((directory/'github/github-mcp-server').stat().st_mode & 0o777,0o600)

    def test_enterprise_selection_requires_profile_target_installer_and_exact_inventory(self):
        value, directory = self.enterprise_inputs()
        cases = []
        for key,new in [('profile','selected-reads'),('contents','bad'),('bundle','../escape')]:
            bad=copy.deepcopy(value);bad['official'][key]=new;cases.append(bad)
        bad=copy.deepcopy(value);bad['sources'].clear();cases.append(bad)
        bad=copy.deepcopy(value);del bad['sources']['setup/backend.py'];cases.append(bad)
        bad=copy.deepcopy(value);del bad['sources']['setup/install-library-consumer.py'];cases.append(bad)
        bad=copy.deepcopy(value);bad['official']['sums']='a'*64;cases.append(bad)
        bad=copy.deepcopy(value);bad['target']='macos-arm64';cases.append(bad)
        for i,bad in enumerate(cases):
            with self.subTest(case=i),self.assertRaises(release.PackageError):
                self.explicit_package(bad,name='enterprise-invalid-'+str(i))
            self.assertFalse((self.base/('enterprise-invalid-'+str(i))).exists())
        bad=copy.deepcopy(value);bad['official']['contents']='a'*64
        with self.assertRaisesRegex(release.PackageError,'selected_manifest_digest_mismatch'):
            self.explicit_package(bad,name='enterprise-digest')
        binary=directory/'github/github-mcp-server'
        binary.write_bytes(binary.read_bytes()+b'changed')
        with self.assertRaisesRegex(release.PackageError,'selected_payload_digest_mismatch'):
            self.explicit_package(value,name='enterprise-drift')

    def test_enterprise_delivery_cannot_masquerade_as_selected_profile(self):
        value,directory=self.enterprise_inputs()
        original=(directory/'delivery.json').read_bytes()
        for i,(key,new) in enumerate((('profile','selected-reads'),('target','macos-arm64'),
                ('artifact','github/other'),('artifact_sha256','a'*64))):
            delivery=json.loads(original);delivery['components'][0][key]=new
            (directory/'delivery.json').write_bytes(json.dumps(delivery).encode())
            manifest(directory);(directory/'SHA256SUMS').unlink()
            value['official']['contents']=sha((directory/'CONTENTS.json').read_bytes())
            with self.subTest(key=key),self.assertRaises(release.PackageError):
                self.explicit_package(value,name='enterprise-delivery-'+str(i))

    def test_enterprise_output_cannot_overlap_selected_input_and_legacy_routes_are_refused(self):
        value,directory=self.enterprise_inputs()
        path=self.base/'enterprise-inputs.json';raw=json.dumps(value).encode();path.write_bytes(raw)
        with self.assertRaisesRegex(release.PackageError,'output_inside_input'):
            release.package(self.root,'linux-x86_64',directory/'output',
                input_selection=path,input_selection_sha256=sha(raw))
        valid={'components':{'services':{}},'routes':[{'family':'services'}]}
        release.require_explicit_official_family({'consumers/routes.json':(json.dumps(valid).encode(),0o644)})
        for index in ({**valid,'components':{'official-github':{}}},
                      {**valid,'routes':[{'family':'official-github','profile':'selected-reads'}]}):
            with self.assertRaisesRegex(release.PackageError,'requires_uncomposed_routes'):
                release.require_explicit_official_family({'consumers/routes.json':(json.dumps(index).encode(),0o644)})

    def test_explicit_inputs_retain_exact_current_source_tool_and_frozen_donor(self):
        value = self.explicit_inputs()
        self.select_recipe_tool(value)
        path = self.targets["macos-arm64"]["setup_backend"][0]
        source = b"inert changed current backend, no execution"
        write(self.root / path, source)
        value["sources"]["setup/backend.py"] = {"path": path, "sha256": sha(source)}
        guide_name = "selected/contracts/PYTHON-INSTALL.md"
        guide_path = "crates/toolkit-package/PYTHON-INSTALL.md"
        guide = b"Current console entry point contract.\n[Selected](../../sdk/consumer-setup/SELECTED.md)\n"
        guide_output = b"Current console entry point contract.\n[Selected](SETUP-SELECTED.md)\n"
        write(self.root / guide_path, guide, 0o600)
        value["sources"][guide_name] = {"path": guide_path, "sha256": sha(guide)}
        before = copy.deepcopy((release.TARGETS, release.PYTHON_KITS, release.PYTHON_SOURCES))
        result = self.explicit_package(value)
        self.assertEqual((release.TARGETS, release.PYTHON_KITS, release.PYTHON_SOURCES), before)
        self.assertEqual(json.loads((self.base / "selected/build.json").read_text()), result)
        self.assertEqual(result["packager_source_sha256"], sha(Path(release.__file__).read_bytes()))
        self.assertIn("not_execution_qualification", result["scope"])
        with tarfile.open(result["archive"]) as archive:
            prefix = "toolkit-library-controlled-selected-inputs-macos-arm64/"
            read = lambda name: archive.extractfile(prefix + name).read()
            self.assertEqual(read("setup/backend.py"), source)
            tool = value["tools"]["bin/toolkit-local-recipes"]
            self.assertEqual(read("selected/bin/toolkit-local-recipes"), (self.root / tool["source"]).read_bytes())
            self.assertEqual(read("selected/provenance/updates/toolkit-local-recipes.json"),
                             (self.root / tool["record"]).read_bytes())
            self.assertEqual(read("selected/provenance/current-input-selection.json"),
                             (self.base / "selected-inputs.json").read_bytes())
            history = json.loads(read("selected/provenance/document-sources.json"))
            self.assertEqual(history["base_source_revision"], release.SOURCE_REVISION)
            self.assertEqual(read(guide_name), guide_output)
            self.assertEqual(archive.getmember(prefix + guide_name).mode, 0o644)
            self.assertEqual(history["files"][guide_name], {
                "source": guide_path, "source_sha256": sha(guide), "output_sha256": sha(guide_output)})
            donor_guide = self.donor_files["macos-arm64"]["contracts/PYTHON-INSTALL.md"][0]
            retained = json.loads(read("selected/provenance/selection.json"))
            self.assertEqual(retained["files"]["contracts/PYTHON-INSTALL.md"], sha(donor_guide))
            self.assertEqual(read("selected/provenance/donor-kit.json"),
                             self.donor_files["macos-arm64"]["provenance/kit.json"][0])
        bad = copy.deepcopy(value); bad["sources"][guide_name]["sha256"] = "0" * 64
        with self.assertRaisesRegex(release.PackageError, "selected_source_digest_mismatch"):
            self.explicit_package(bad, name="changed-contract")
        (self.root / guide_path).chmod(0o755)
        with self.assertRaisesRegex(release.PackageError, "unexpected_input_mode"):
            self.explicit_package(value, name="executable-contract")
        (self.root / guide_path).chmod(0o600)
        omitted = copy.deepcopy(value); del omitted["sources"][guide_name]
        legacy = self.explicit_package(omitted, name="omitted-contract")
        with tarfile.open(legacy["archive"]) as archive:
            self.assertEqual(archive.extractfile(prefix + guide_name).read(), donor_guide)
            history = json.load(archive.extractfile(prefix + "selected/provenance/document-sources.json"))
            self.assertEqual(history["files"][guide_name], {
                "donor_member": "contracts/PYTHON-INSTALL.md",
                "source_sha256": sha(donor_guide), "output_sha256": sha(donor_guide)})
        # This input group is scoped to one target, not a mutation of legacy pins.
        with self.assertRaisesRegex(release.PackageError, "selected_source_digest_mismatch"):
            self.package("legacy-still-pinned")

    def test_explicit_node_contract_preserves_donor_default_and_source_provenance(self):
        name = "selected/contracts/NODE-INSTALL.md"
        path = "crates/toolkit-package/NODE-INSTALL.md"
        data = b"Selected Node guide.\n[Selected](../../sdk/consumer-setup/SELECTED.md)\n"
        output = b"Selected Node guide.\n[Selected](SETUP-SELECTED.md)\n"
        write(self.root / path, data, 0o600)
        for target in self.targets:
            with self.subTest(target=target):
                value = self.explicit_inputs(target)
                value["sources"][name] = {"path": path, "sha256": sha(data)}
                before = copy.deepcopy((release.TARGETS, release.PYTHON_KITS, release.PYTHON_SOURCES))
                result = self.explicit_package(value, name="node-" + target)
                self.assertEqual((release.TARGETS, release.PYTHON_KITS, release.PYTHON_SOURCES), before)
                prefix = "toolkit-library-controlled-selected-inputs-" + target + "/"
                donor = self.donor_files[target]["contracts/NODE-INSTALL.md"][0]
                with tarfile.open(result["archive"]) as archive:
                    read = lambda member: archive.extractfile(prefix + member).read()
                    self.assertEqual(read(name), output)
                    self.assertEqual(archive.getmember(prefix + name).mode, 0o644)
                    history = json.loads(read("selected/provenance/document-sources.json"))
                    self.assertEqual(history["files"][name], {
                        "source": path, "source_sha256": sha(data), "output_sha256": sha(output)})
                    retained = json.loads(read("selected/provenance/selection.json"))
                    self.assertEqual(retained["files"]["contracts/NODE-INSTALL.md"], sha(donor))
                bad = copy.deepcopy(value)
                bad["sources"][name]["sha256"] = "0" * 64
                with self.assertRaisesRegex(release.PackageError, "selected_source_digest_mismatch"):
                    self.explicit_package(bad, name="node-bad-digest-" + target)
                omitted = copy.deepcopy(value)
                del omitted["sources"][name]
                legacy = self.explicit_package(omitted, name="node-omitted-" + target)
                with tarfile.open(legacy["archive"]) as archive:
                    self.assertEqual(archive.extractfile(prefix + name).read(), donor)
                    history = json.load(archive.extractfile(prefix + "selected/provenance/document-sources.json"))
                    self.assertEqual(history["files"][name], {
                        "donor_member": "contracts/NODE-INSTALL.md",
                        "source_sha256": sha(donor), "output_sha256": sha(donor)})

    def test_explicit_selected_installer_source_preserves_mode_history_and_refusals(self):
        value = self.explicit_inputs()
        name, source = "selected/bin/install-library-consumer.py", "scripts/current-installer.py"
        data = b"# exact inert selected installer source; never executed\n"
        write(self.root / source, data, 0o600)
        value["sources"][name] = {"path": source, "sha256": sha(data)}
        result = self.explicit_package(value)
        with tarfile.open(result["archive"]) as archive:
            prefix = "toolkit-library-controlled-selected-inputs-macos-arm64/"
            self.assertEqual(archive.extractfile(prefix + name).read(), data)
            self.assertEqual(archive.getmember(prefix + name).mode, 0o755)
            record = json.load(archive.extractfile(prefix + "selected/provenance/selection.json"))
            old = self.python_kits["macos-arm64"]["tools"]["bin/install-library-consumer.py"]
            self.assertEqual(record["files"]["bin/install-library-consumer.py"], sha(data))
            self.assertEqual(record["source_updates"]["bin/install-library-consumer.py"],
                {"path": source, "sha256": sha(data), "previous_sha256": old})
            self.assertNotIn("bin/install-library-consumer.py", record.get("tool_updates", {}))
            self.assertEqual(archive.extractfile(prefix + "selected/provenance/donor-kit.json").read(),
                             self.donor_files["macos-arm64"]["provenance/kit.json"][0])
        # A source override cannot masquerade as a compiled-tool build receipt.
        bad = copy.deepcopy(value); bad["tools"]["bin/install-library-consumer.py"] = {}
        with self.assertRaisesRegex(release.PackageError, "duplicate_selected_installer_source"):
            self.explicit_package(bad, name="dual-installer")
        bad = copy.deepcopy(value); bad["sources"][name]["sha256"] = "0" * 64
        with self.assertRaisesRegex(release.PackageError, "selected_source_digest_mismatch"):
            self.explicit_package(bad, name="changed-installer")
        (self.root / source).chmod(0o755)
        with self.assertRaisesRegex(release.PackageError, "unexpected_input_mode"):
            self.explicit_package(value, name="executable-source")
        # Omitting the new source entry still selects the exact frozen donor.
        legacy = self.explicit_package(self.explicit_inputs(), name="omitted-installer")
        with tarfile.open(legacy["archive"]) as archive:
            self.assertEqual(sha(archive.extractfile(prefix + name).read()), old)

    def test_explicit_tools_require_bound_build_record_and_declared_target(self):
        cases = ("record_hash", "binary", "bytes", "mode", "target", "command", "features")
        for field in cases:
            with self.subTest(field=field):
                value = self.explicit_inputs()
                body = self.select_recipe_tool(value)
                row = value["tools"]["bin/toolkit-local-recipes"]
                code = "selected_tool_build_record_mismatch"
                if field == "record_hash":
                    row["record_sha256"] = "0" * 64
                    code = "selected_tool_build_record_digest_mismatch"
                else:
                    body[field] = {"binary": "dist/other/toolkit-local-recipes", "bytes": body["bytes"] + 1,
                        "mode": "0644", "target": "x86_64-unknown-linux-gnu",
                        "command": ["cargo", "build", "--bin", "toolkit_gateway"], "features": []}[field]
                    raw = json.dumps(body).encode()
                    write(self.root / row["record"], raw)
                    row["record_sha256"] = sha(raw)
                    if field == "command": code = "selected_tool_build_command_mismatch"
                    if field == "features": code = "selected_tool_build_features_mismatch"
                with self.assertRaisesRegex(release.PackageError, "^" + code + "$"):
                    self.explicit_package(value, name="bad-" + field)
                self.assertFalse((self.base / ("bad-" + field)).exists())

    def test_explicit_selection_rejects_unknown_partial_or_cross_target_inputs(self):
        base = self.explicit_inputs()
        cases = []
        value = copy.deepcopy(base); value["sources"]["setup/new-owner.py"] = {"path": "unrelated.txt", "sha256": "0" * 64}
        cases.append(("source-name", value, "unknown_selected_source", None))
        value = copy.deepcopy(base); value["tools"]["bin/new-owner"] = {}
        cases.append(("tool-name", value, "unknown_selected_tool", None))
        value = copy.deepcopy(base); self.select_recipe_tool(value); value["tools"]["bin/toolkit-local-recipes"].pop("record_sha256")
        cases.append(("partial-tool", value, "invalid_selected_tool", None))
        value = copy.deepcopy(base); value["consumer"].pop("sums")
        cases.append(("partial-consumer", value, "invalid_consumer_selection", None))
        value = copy.deepcopy(base); value["future_default"] = True
        cases.append(("unknown-field", value, "invalid_input_selection", None))
        cases.append(("target", base, "invalid_input_selection", "linux-x86_64"))
        for name, value, code, target in cases:
            with self.subTest(name=name), self.assertRaisesRegex(release.PackageError, "^" + code + "$"):
                self.explicit_package(value, name=name, target=target)
            self.assertFalse((self.base / name).exists())

    def test_explicit_source_and_consumer_closure_mismatches_do_not_fall_back(self):
        value = self.explicit_inputs()
        path = self.targets["macos-arm64"]["setup_backend"][0]
        value["sources"]["setup/backend.py"] = {"path": path, "sha256": "0" * 64}
        with self.assertRaisesRegex(release.PackageError, "selected_source_digest_mismatch"):
            self.explicit_package(value, name="source-mismatch")
        value = self.explicit_inputs()
        write(self.bundle() / "package/unlisted.whl", b"unlisted")
        with self.assertRaises(release.PackageError):
            self.explicit_package(value, name="closure-mismatch")
        self.assertFalse((self.base / "source-mismatch").exists())
        self.assertFalse((self.base / "closure-mismatch").exists())

    def test_explicit_consumer_target_cannot_be_changed_by_outer_target_label(self):
        value = self.explicit_inputs("macos-arm64")
        other = self.explicit_inputs("linux-x86_64")
        value["consumer"] = other["consumer"]
        with self.assertRaisesRegex(release.PackageError, "selected_consumer_target_mismatch"):
            self.explicit_package(value, name="wrong-consumer-platform")
        self.assertFalse((self.base / "wrong-consumer-platform").exists())

    def test_explicit_consumer_closure_is_bound_to_manifest_and_packaged_components(self):
        for kind in ("digest", "manifest-binding", "component", "platform"):
            with self.subTest(kind=kind):
                value = self.explicit_inputs()
                row = value["consumer"]
                path = self.root / row["dependency_closure"]
                record = json.loads(path.read_bytes())
                code = "selected_consumer_closure_digest_mismatch"
                if kind == "digest":
                    row["dependency_closure_sha256"] = "0" * 64
                else:
                    if kind == "component": record["accepted_inputs"]["agno/runtime-constraints.txt"] = "0" * 64
                    elif kind == "platform": record["python_platform"]["system"] = "Linux"
                    else: record["extra_retained_fact"] = True
                    raw = json.dumps(record).encode(); write(path, raw)
                    row["dependency_closure_sha256"] = sha(raw)
                    code = "selected_consumer_closure_binding_mismatch"
                    if kind != "manifest-binding":
                        directory = self.bundle()
                        contents = json.loads((directory / "CONTENTS.json").read_bytes())
                        contents["accepted_inputs"][row["dependency_closure"]] = sha(raw)
                        data = (json.dumps(contents) + "\n").encode(); write(directory / "CONTENTS.json", data)
                        hashes = {name: item["sha256"] for name, item in contents["files"].items()}
                        hashes["CONTENTS.json"] = sha(data)
                        sums = "".join(pin + "  " + name + "\n" for name, pin in sorted(hashes.items())).encode()
                        write(directory / "SHA256SUMS", sums)
                        row.update(contents=sha(data), sums=sha(sums))
                        code = "selected_consumer_component_mismatch" if kind == "component" else "selected_consumer_target_mismatch"
                with self.assertRaisesRegex(release.PackageError, "^" + code + "$"):
                    self.explicit_package(value, name="bad-closure-" + kind)
                self.assertFalse((self.base / ("bad-closure-" + kind)).exists())

    def test_explicit_guide_still_binds_packaged_consumer_contents(self):
        value = self.explicit_inputs()
        path = "docs/current-guide.md"
        raw = ("Use --contents-sha256 " + "0" * 64 + "\n").encode()
        write(self.root / path, raw)
        value["sources"]["README.md"] = {"path": path, "sha256": sha(raw)}
        with self.assertRaisesRegex(release.PackageError, "guide_command_digest_mismatch"):
            self.explicit_package(value, name="stale-guide")
        self.assertFalse((self.base / "stale-guide").exists())

    def test_selection_pin_pair_duplicate_keys_and_symlink_are_refused(self):
        path = self.base / "inputs.json"
        raw = json.dumps(self.explicit_inputs()).encode()
        write(path, raw)
        for name, selected, pin in (("path-only", path, None), ("pin-only", None, sha(raw)),
                                     ("wrong-pin", path, "0" * 64)):
            with self.subTest(name=name), self.assertRaises(release.PackageError):
                release.package(self.root, "macos-arm64", self.base / name,
                                input_selection=selected, input_selection_sha256=pin)
            self.assertFalse((self.base / name).exists())
        duplicate = raw.replace(b'"schema_version": 1', b'"schema_version": 1, "schema_version": 1')
        write(path, duplicate)
        with self.assertRaisesRegex(release.PackageError, "duplicate_metadata_key"):
            release.package(self.root, "macos-arm64", self.base / "duplicate",
                            input_selection=path, input_selection_sha256=sha(duplicate))
        linked = self.base / "linked.json"; linked.symlink_to(path)
        with self.assertRaises((OSError, release.PackageError)):
            release.package(self.root, "macos-arm64", self.base / "linked",
                            input_selection=linked, input_selection_sha256=sha(duplicate))

    def test_selected_inspector_update_is_exact_and_target_specific(self):
        name = "bin/toolkit-package-inspect"
        update = self.python_kits["macos-arm64"]["tool_updates"][name]
        files = release.collect(self.root, "macos-arm64")
        self.assertEqual(files["selected/" + name][0], (self.root / update["source"]).read_bytes())
        record = json.loads(files["selected/provenance/selection.json"][0])
        self.assertEqual(record["capabilities"], ["agent-plugin-files", "mcpb-archive-inspection", "compose-local-execution"])
        self.assertEqual(record["tool_updates"][name]["previous_sha256"], self.python_kits["macos-arm64"]["tools"][name])
        self.assertEqual(record["files"][name], update["sha256"])
        linux = release.collect(self.root, "linux-x86_64")
        self.assertEqual(linux["selected/" + name], self.donor_files["linux-x86_64"][name])
        self.assertNotIn("capabilities", json.loads(linux["selected/provenance/selection.json"][0]))
        (self.root / update["source"]).write_bytes(b"tampered")
        self.assert_refused("selected_tool_update_digest_mismatch")

    def test_selected_inspector_update_requires_matching_build_record(self):
        update = self.python_kits["macos-arm64"]["tool_updates"]["bin/toolkit-package-inspect"]
        (self.root / update["record"]).write_text(json.dumps({"sha256": "0" * 64}))
        self.assert_refused("selected_tool_update_record_mismatch")

    def test_compose_payload_preserves_platform_and_source_owners(self):
        for target in self.targets:
            files = release.collect(self.root, target)
            record = json.loads(files["selected/provenance/selection.json"][0])
            for name in ("compose_runtime", "compose_acquisition", "compose_bridge"):
                destination = "setup/" + name + ".py"
                self.assertEqual(files[destination][0], (self.root / self.python_sources[destination][0]).read_bytes())
            if target == "macos-arm64":
                update = self.python_kits[target]["tool_updates"]["bin/toolkit-compose-plan"]
                self.assertEqual(files["selected/bin/toolkit-compose-plan"],
                                 ((self.root / update["source"]).read_bytes(), 0o755))
                self.assertEqual(record["files"]["bin/toolkit-compose-plan"], update["sha256"])
                self.assertEqual(record["tool_updates"]["bin/toolkit-compose-plan"]["previous_sha256"],
                                 self.python_kits[target]["tools"]["bin/toolkit-compose-plan"])
                self.assertIn("compose-local-execution", record["capabilities"])
                self.assertTrue(any(name.startswith("selected/provenance/compose-acquisition/") for name in files))
            else:
                self.assertNotIn("selected/bin/toolkit-compose-plan", files)
                self.assertNotIn("compose-local-execution", record.get("capabilities", []))

    def test_selected_mcpb_reuses_macos_donor_and_base_linux_fixture_omits_it(self):
        name = "bin/toolkit-mcpb-install"
        donor = self.python_kits["macos-arm64"]
        self.assertIn(name, donor["tools"])
        self.assertNotIn(name, donor.get("tool_additions", {}))
        files = release.collect(self.root, "macos-arm64")
        self.assertEqual(files["selected/" + name], self.donor_files["macos-arm64"][name])
        record = json.loads(files["selected/provenance/selection.json"][0])
        self.assertEqual(record["files"][name], donor["tools"][name])
        self.assertEqual(files["selected/provenance/mcpb-setup/retained.txt"],
                         self.donor_files["macos-arm64"]["provenance/mcpb-setup/retained.txt"])
        self.assertIn("selected/contracts/MCPB-INSTALL.md", files)
        linux = release.collect(self.root, "linux-x86_64")
        self.assertNotIn("selected/" + name, linux)
        self.assertNotIn("selected/provenance/mcpb-setup/retained.txt", linux)
        self.donor_files["macos-arm64"][name] = (b"tampered installer", 0o755)
        self.repin_donor("macos-arm64")
        self.assert_refused("python_tool_digest_mismatch")

    def declare_tool_addition(self, name="bin/additional-tool"):
        raw = b"explicitly added executable\n"
        addition = {"source": "new-build/additional-tool", "sha256": sha(raw), "record": "new-build/build.json"}
        write(self.root / addition["source"], raw, 0o755)
        write(self.root / addition["record"], json.dumps({"binary": addition["source"], "sha256": sha(raw)}).encode())
        self.python_kits["linux-x86_64"]["tool_additions"] = {name: addition}
        return addition

    def test_tool_addition_preserves_existing_files_and_binds_exact_new_bytes(self):
        previous = release.collect(self.root, "linux-x86_64")
        macos = release.collect(self.root, "macos-arm64")
        name = "bin/additional-tool"
        addition = self.declare_tool_addition(name)
        self.python_kits["linux-x86_64"]["capabilities"] = ["explicit-capability"]
        files = release.collect(self.root, "linux-x86_64")
        provenance = "selected/provenance/additions/additional-tool.json"
        raw_record = (self.root / addition["record"]).read_bytes()
        self.assertEqual(files["selected/" + name], ((self.root / addition["source"]).read_bytes(), 0o755))
        self.assertEqual(files[provenance], (raw_record, 0o644))
        self.assertEqual(set(files) - set(previous), {"selected/" + name, provenance})
        selection = "selected/provenance/selection.json"
        self.assertEqual({path: value for path, value in files.items() if path in previous and path != selection},
                         {path: value for path, value in previous.items() if path != selection})
        expected = json.loads(previous[selection][0])
        expected["files"][name] = addition["sha256"]
        expected["capabilities"] = ["explicit-capability"]
        expected["tool_additions"] = {name: {"source": addition["source"], "sha256": addition["sha256"],
                                               "record_sha256": sha(raw_record)}}
        self.assertEqual(json.loads(files[selection][0]), expected)
        self.assertNotIn("previous_sha256", expected["tool_additions"][name])
        self.assertEqual(release.collect(self.root, "macos-arm64"), macos)
        archive_path = self.package("with-addition", "linux-x86_64")["archive"]
        with tarfile.open(archive_path) as archive:
            root = f"toolkit-library-{self.targets['linux-x86_64']['edition']}-linux-x86_64/"
            for path in ("selected/" + name, provenance, selection):
                member = archive.getmember(root + path)
                self.assertEqual((archive.extractfile(member).read(), member.mode), files[path])

    def test_empty_tool_additions_preserve_archive_bytes_and_omit_metadata(self):
        first = self.package("no-additions", "linux-x86_64")
        self.python_kits["linux-x86_64"]["tool_additions"] = {}
        second = self.package("empty-additions", "linux-x86_64")
        self.assertEqual(Path(first["archive"]).read_bytes(), Path(second["archive"]).read_bytes())
        record = json.loads(release.collect(self.root, "linux-x86_64")["selected/provenance/selection.json"][0])
        self.assertNotIn("tool_additions", record)
        self.assertNotIn("capabilities", record)

    def test_selected_tool_bytes_have_separate_input_and_output_budgets(self):
        target = "linux-x86_64"
        addition = self.declare_tool_addition()
        raw = b"selected executable bytes\n" * 1024
        write(self.root / addition["source"], raw, 0o755)
        addition["sha256"] = sha(raw)
        write(self.root / addition["record"], json.dumps({"binary": addition["source"], "sha256": sha(raw)}).encode())
        sizes = []
        original = release.Inputs.read
        def counted(reader, path, **kwargs):
            data = original(reader, path, **kwargs)
            sizes.append(len(data))
            return data
        with patch.object(release.Inputs, "read", counted):
            files = release.collect(self.root, target)
        expanded = sum(len(data) for data, _ in self.donor_files[target].values())
        limit = max(sum(sizes), expanded, sum(len(data) for data, _ in files.values()))
        # Each real budget fits; adding selected output to reads would not.
        self.assertGreater(sum(sizes) + len(raw), limit)
        first = self.package("normal-budget", target)
        with patch.object(release, "MAX_TOTAL_BYTES", limit):
            second = self.package("separate-budgets", target)
        self.assertEqual(Path(first["archive"]).read_bytes(), Path(second["archive"]).read_bytes())

    def test_raw_read_total_refuses_before_individually_bounded_files_accumulate(self):
        write(self.root / "first.dat", b"a" * 128)
        write(self.root / "second.dat", b"b" * 128)
        reader = release.Inputs(self.root)
        with patch.object(release, "MAX_TOTAL_BYTES", 200):
            self.assertEqual(reader.read("first.dat"), b"a" * 128)
            with self.assertRaisesRegex(release.PackageError, "^input_too_large$"):
                reader.read("second.dat")

    def test_final_output_total_refuses_even_when_inputs_and_donor_fit(self):
        target = "linux-x86_64"
        self.donor_files[target]["licenses/crates/large.txt"] = (b"retained notice\n" * 1024, 0o644)
        self.repin_donor(target)
        sizes = []
        original = release.Inputs.read
        def counted(reader, path, **kwargs):
            data = original(reader, path, **kwargs)
            sizes.append(len(data))
            return data
        with patch.object(release.Inputs, "read", counted):
            files = release.collect(self.root, target)
        expanded = sum(len(data) for data, _ in self.donor_files[target].values())
        limit = max(sum(sizes), expanded)
        self.assertGreater(sum(len(data) for data, _ in files.values()), limit)
        with patch.object(release, "MAX_TOTAL_BYTES", limit):
            self.assert_refused("output_too_large", target=target)

    def test_tool_additions_refuse_existing_and_casefolded_paths(self):
        for index, name in enumerate(("bin/toolkit-python-plan", "bin/TOOLKIT-PYTHON-PLAN", "bin/unselected-tool")):
            with self.subTest(name=name):
                self.declare_tool_addition(name)
                self.assert_refused("selected_tool_addition_collision", target="linux-x86_64", output="collision-" + str(index))
        addition = self.declare_tool_addition()
        self.python_kits["linux-x86_64"]["tool_additions"]["bin/ADDITIONAL-TOOL"] = addition
        self.assert_refused("selected_tool_addition_collision", target="linux-x86_64", output="addition-casefold")
        self.declare_tool_addition()
        self.python_sources["selected/bin/additional-tool"] = (addition["source"], addition["sha256"])
        self.assert_refused("selected_tool_addition_collision", target="linux-x86_64", output="later-source-collision")

    def test_tool_addition_requires_safe_bin_name_and_closed_declaration(self):
        for index, name in enumerate(("other/tool", "bin/nested/tool", "bin/../tool")):
            with self.subTest(name=name):
                self.declare_tool_addition(name)
                self.assert_refused("invalid_selected_tool_addition", target="linux-x86_64", output="unsafe-addition-" + str(index))
        addition = self.declare_tool_addition()
        addition["unrecognized"] = True
        self.assert_refused("invalid_selected_tool_addition", target="linux-x86_64", output="extra-addition-field")

    def test_tool_addition_requires_unchanged_executable_and_bound_build_record(self):
        addition = self.declare_tool_addition()
        (self.root / addition["source"]).write_bytes(b"replacement")
        self.assert_refused("selected_tool_addition_digest_mismatch", target="linux-x86_64", output="changed-addition")
        addition = self.declare_tool_addition()
        (self.root / addition["source"]).chmod(0o644)
        self.assert_refused("unexpected_input_mode", target="linux-x86_64", output="nonexecutable-addition")
        records = ({"sha256": addition["sha256"]},
                   {"binary": "unrelated/tool", "sha256": addition["sha256"]},
                   {"binary": addition["source"], "sha256": "0" * 64})
        for index, record in enumerate(records):
            with self.subTest(record=record):
                self.declare_tool_addition()
                (self.root / addition["record"]).write_text(json.dumps(record))
                self.assert_refused("selected_tool_addition_record_mismatch", target="linux-x86_64", output="unbound-addition-" + str(index))
        self.declare_tool_addition()
        (self.root / addition["record"]).write_text('{"binary":"wrong","binary":"new-build/additional-tool","sha256":"' + addition["sha256"] + '"}')
        self.assert_refused("duplicate_metadata_key", target="linux-x86_64", output="duplicate-addition-record")

    def test_guide_bootstrap_pins_match_final_payload_for_both_command_forms(self):
        for target in self.targets:
            selection = self.targets[target]
            guide = self.root / selection["guide"]
            for flag, expected in (("--contents-sha256", selection["contents"]),
                                   ("--official-contents-sha256", selection["official"]["contents"])):
                for expression in (flag + " " + expected, '"' + flag + '", "' + expected + '"'):
                    with self.subTest(target=target, expression=expression):
                        raw = (expression + "\n").encode()
                        guide.write_bytes(raw)
                        selection["guide_sha256"] = sha(raw)
                        release.collect(self.root, target)
                        wrong = raw.replace(expected.encode(), b"0" * 64)
                        guide.write_bytes(wrong)
                        selection["guide_sha256"] = sha(wrong)
                        self.assert_refused("guide_command_digest_mismatch", target=target)
            guide.write_bytes(b"No literal command pins in fixture.\n")
            selection["guide_sha256"] = sha(guide.read_bytes())

    def test_combined_selection_preserves_one_current_controller_and_distinct_installers(self):
        for target in self.targets:
            files = release.collect(self.root, target)
            self.assertEqual(files["setup/backend.py"][0], (self.root / self.targets[target]["setup_backend"][0]).read_bytes())
            self.assertEqual(files["setup/python_acquisition.py"][0], (self.root / self.python_sources["setup/python_acquisition.py"][0]).read_bytes())
            self.assertEqual({name for name in files if name.startswith("selected/bin/")},
                             {"selected/" + name for name in self.python_kits[target]["tools"]})
            for name in self.python_kits[target]["tools"]:
                self.assertEqual(files["selected/" + name][1], 0o755)
            self.assertIn("setup/install-library-consumer.py", files)
            self.assertIn("selected/provenance/donor-kit.json", files)
            self.assertFalse(any("verification-environment" in name or name.endswith("unselected-tool") for name in files))
            self.assertNotIn("selected/setup/backend.py", files)

    def test_python_donor_wrong_hash_and_cross_platform_are_refused_before_output(self):
        self.python_kits["macos-arm64"]["sha256"] = "0" * 64
        self.assert_refused("python_donor_digest_mismatch")
        self.python_kits["macos-arm64"] = self.python_kits["linux-x86_64"]
        self.assert_refused("python_donor_target_mismatch")

    def test_macos_recipe_tools_and_provenance_are_selected_without_linux_support(self):
        macos = release.collect(self.root, "macos-arm64")
        linux = release.collect(self.root, "linux-x86_64")
        donor = self.python_kits["macos-arm64"]
        names = ("bin/toolkit-local-recipes", "bin/toolkit-mcp-check.py",
                 "contracts/LOCAL-RECIPES.md", "contracts/MCP-EXECUTION-CHECK.md")
        for name in names:
            self.assertNotIn("selected/" + name, linux)
        for name in ("bin/toolkit-mcp-check.py", "contracts/MCP-EXECUTION-CHECK.md"):
            self.assertEqual(macos["selected/" + name], self.donor_files["macos-arm64"][name])
        recipe = "bin/toolkit-local-recipes"
        update = donor["tool_updates"][recipe]
        self.assertEqual(macos["selected/" + recipe], ((self.root / update["source"]).read_bytes(), 0o755))
        build_record = (self.root / update["record"]).read_bytes()
        self.assertEqual(macos["selected/provenance/updates/toolkit-local-recipes.json"], (build_record, 0o644))
        contract = "selected/contracts/LOCAL-RECIPES.md"
        contract_source, contract_sha = donor["sources"][contract]
        self.assertEqual(macos[contract], ((self.root / contract_source).read_bytes(), 0o644))
        documents = json.loads(macos["selected/provenance/document-sources.json"][0])
        self.assertEqual(documents["files"][contract], {
            "source": contract_source, "source_sha256": contract_sha, "output_sha256": contract_sha})
        self.assertIn("selected/provenance/current-setup/retained.txt", macos)
        self.assertNotIn("selected/provenance/current-setup/retained.txt", linux)
        record = json.loads(macos["selected/provenance/selection.json"][0])
        self.assertEqual(record["scope"], "retained_selected_tools")
        self.assertEqual(record["tool_updates"][recipe], {
            "source": update["source"], "sha256": update["sha256"],
            "record_sha256": sha(build_record), "previous_sha256": donor["tools"][recipe]})
        for name in names:
            expected = update["sha256"] if name == recipe else sha(self.donor_files["macos-arm64"][name][0])
            self.assertEqual(record["files"][name], expected)
        for path, _ in self.python_kits["macos-arm64"]["sources"].values():
            (self.root / path).unlink()
        self.assertEqual(linux, release.collect(self.root, "linux-x86_64"))

    def test_recipe_tools_contracts_and_provenance_must_all_be_present(self):
        original = copy.deepcopy(self.donor_files["macos-arm64"])
        for name in ("bin/toolkit-local-recipes", "bin/toolkit-mcp-check.py",
                     "contracts/LOCAL-RECIPES.md", "contracts/MCP-EXECUTION-CHECK.md",
                     "provenance/current-setup/retained.txt"):
            with self.subTest(missing=name):
                self.donor_files["macos-arm64"] = {key: value for key, value in original.items() if key != name}
                self.repin_donor()
                self.assert_refused("incomplete_python_donor")

    def test_recipe_tool_pin_and_executable_mode_are_required(self):
        for name in ("bin/toolkit-local-recipes", "bin/toolkit-mcp-check.py"):
            original = self.donor_files["macos-arm64"][name]
            for value in ((b"changed tool", 0o755), (original[0], 0o644)):
                with self.subTest(tool=name, value=value):
                    self.donor_files["macos-arm64"][name] = value
                    self.repin_donor()
                    self.assert_refused("python_tool_digest_mismatch")
            self.donor_files["macos-arm64"][name] = original
            self.repin_donor()

    def test_recipe_source_pin_and_target_specific_links(self):
        raw = b"[Profile](RECIPES.md) [Store](../../crates/toolkit-package/LOCAL-RECIPES.md#save)"
        arguments = ("sdk/consumer-setup/HOSTED-RECIPES.md", "selected/contracts/HOSTED-RECIPES.md", raw)
        macos = release.python_document(*arguments, target="macos-arm64")
        self.assertEqual(macos, b"[Profile](SETUP-RECIPES.md) [Store](LOCAL-RECIPES.md#save)")
        linux = release.python_document(*arguments, target="linux-x86_64")
        self.assertNotIn(b"](SETUP-RECIPES.md)", linux)
        self.assertNotIn(b"](LOCAL-RECIPES.md#save)", linux)
        self.assertEqual(linux.count(b"not included"), 2)
        path, _ = self.python_kits["macos-arm64"]["sources"]["selected/contracts/HOSTED-RECIPES.md"]
        (self.root / path).write_bytes(b"changed source contract")
        self.assert_refused("selected_source_digest_mismatch")

    def test_python_tool_change_and_incomplete_notices_are_refused(self):
        files = self.donor_files["macos-arm64"]
        original = copy.deepcopy(files)
        files["bin/toolkit-python-plan"] = (b"other tool", 0o755)
        self.repin_donor()
        self.assert_refused("python_tool_digest_mismatch")
        self.donor_files["macos-arm64"] = {name: value for name, value in original.items() if not name.startswith("licenses/crates/")}
        self.repin_donor()
        self.assert_refused("incomplete_python_donor")

    def test_python_donor_bad_paths_modes_and_count_are_refused(self):
        files = self.donor_files["macos-arm64"]
        files["../escape"] = (b"bad", 0o644)
        self.repin_donor()
        self.assert_refused("unsafe_input_path")
        del files["../escape"]
        files["licenses/crates/retained.txt"] = (b"executable notice", 0o755)
        self.repin_donor()
        self.assert_refused("invalid_python_donor_member")
        files["licenses/crates/retained.txt"] = (b"notice", 0o644)
        self.repin_donor()
        with patch.object(release, "MAX_DONOR_ENTRIES", 1):
            self.assert_refused("python_donor_too_large")

    def test_license_markdown_keeps_exact_bytes_and_links(self):
        target = "linux-x86_64"
        previous = release.collect(self.root, target)
        destination = "selected/licenses/new-dependency/LICENSE.md"
        source = "legal/new-dependency/LICENSE.md"
        raw = b"Original notice.\r\nSee [terms](COPYING) and [author](../AUTHORS.md).\r\n"
        write(self.root / source, raw)
        self.python_kits[target]["sources"] = {destination: (source, sha(raw))}
        self.assertNotEqual(release.python_document(source, destination, raw, target=target), raw)
        files = release.collect(self.root, target)
        self.assertEqual(files[destination], (raw, 0o644))
        self.assertEqual({name: value for name, value in files.items() if name != destination}, previous)
        archive_path = self.package("license-markdown", target)["archive"]
        with tarfile.open(archive_path) as archive:
            root = f"toolkit-library-{self.targets[target]['edition']}-{target}/"
            member = archive.getmember(root + destination)
            self.assertEqual((archive.extractfile(member).read(), member.mode), (raw, 0o644))

    def test_python_owner_pin_and_document_relocation_are_independent_of_donor(self):
        path = self.python_sources["setup/python_acquisition.py"][0]
        (self.root / path).write_bytes(b"changed owner")
        self.assert_refused("selected_source_digest_mismatch")
        actual = release.python_document("sdk/consumer-setup/PYTHON-ACQUISITION.md",
            "selected/contracts/PYTHON-ACQUISITION.md", b"[Selected](SELECTED.md) [Compose](COMPOSE-ACQUISITION.md) [Part](#part)")
        self.assertIn(b"[Selected](SETUP-SELECTED.md)", actual)
        self.assertIn(b"[Compose](COMPOSE-ACQUISITION.md)", actual)
        self.assertIn(b"[Part](#part)", actual)

    def test_exact_archive_contents_modes_metadata_and_relocated_readback(self):
        selected = release.collect(self.root, "macos-arm64")
        result = self.package()
        path = Path(result["archive"])
        self.assertEqual(sha(path.read_bytes()), result["sha256"])
        self.assertEqual((path.parent / "SHA256SUMS").read_text(), f"{result['sha256']}  {path.name}\n")
        archive_root = f"toolkit-library-{release.EDITION}-macos-arm64"
        extracted = self.base / "relocated" / "different parent"
        extracted.mkdir(parents=True)
        with tarfile.open(path) as archive:
            members = archive.getmembers()
            self.assertEqual([member.name for member in members], sorted(member.name for member in members))
            files = {member.name: member for member in members if member.isfile()}
            self.assertEqual(set(files), {archive_root + "/" + name for name in selected})
            for member in members:
                self.assertEqual((member.uid, member.gid, member.mtime, member.uname, member.gname), (0, 0, 0, "", ""))
                self.assertTrue(member.isfile() or member.isdir())
                if member.isfile():
                    relative = member.name[len(archive_root) + 1:]
                    data, mode = selected[relative]
                    self.assertEqual(archive.extractfile(member).read(), data)
                    self.assertEqual(member.mode, mode)
                else:
                    self.assertEqual(member.mode, 0o755)
            archive.extractall(extracted, filter="data")
        root = extracted / archive_root
        for name, (data, mode) in selected.items():
            self.assertEqual((root / name).read_bytes(), data)
            self.assertEqual(stat.S_IMODE((root / name).stat().st_mode), mode)
        self.assertFalse(any("private-fixture" in name or "workspace" in name or "unrelated" in name for name in files))
        self.assertEqual((root / "consumers/README.md").read_bytes(), b"Frozen inner guide.\n")

    def test_deterministic_after_checkout_relocation_mtime_and_data_permission_changes(self):
        first = self.package("first")
        relocated = self.base / "relocated-checkout"
        shutil.copytree(self.root, relocated)
        for path in relocated.rglob("*"):
            os.utime(path, (1730000000, 1730000000))
            if path.is_file() and path.name != "github-mcp-server":
                path.chmod(0o755 if path.stat().st_mode & 0o111 else 0o600)
        second = self.package("second", root=relocated)
        self.assertEqual(Path(first["archive"]).read_bytes(), Path(second["archive"]).read_bytes())
        self.assertEqual(Path(first["archive"]).read_bytes()[4:8], b"\x00" * 4)

    def test_setup_installer_is_pinned_independently_per_target(self):
        for target, selection in self.targets.items():
            path, expected = selection["setup_installer"]
            selected = release.collect(self.root, target)
            self.assertEqual(sha(selected["setup/install-library-consumer.py"][0]), expected)
            (self.root / path).write_bytes(b"changed installer")
            self.assert_refused("selected_source_digest_mismatch", target=target, output=target)

    def test_linux_includes_github_installer_and_omits_unselected_macos_inputs(self):
        macos = self.targets["macos-arm64"]
        shutil.rmtree(self.root / macos["bundle"])
        shutil.rmtree(self.root / macos["official"]["bundle"])
        for field in ("setup_installer", "setup_backend"):
            if macos[field][0] != self.targets["linux-x86_64"][field][0]:
                (self.root / macos[field][0]).unlink()
        result = self.package(target="linux-x86_64")
        with tarfile.open(result["archive"]) as archive:
            names = archive.getnames()
            self.assertTrue(any(name.endswith("/official/github/github-mcp-server") for name in names))
            self.assertFalse(any("/playwright/" in name for name in names))
            self.assertTrue(any(name.endswith("/setup/install-library-consumer.py") for name in names))
            self.assertTrue(any(name.endswith("/mcp_adapter.py") for name in names))

    def test_macos_does_not_require_linux_inputs(self):
        linux = self.targets["linux-x86_64"]
        shutil.rmtree(self.root / linux["bundle"])
        shutil.rmtree(self.root / linux["official"]["bundle"])
        for field in ("setup_installer", "setup_backend"):
            if linux[field][0] != self.targets["macos-arm64"][field][0]:
                (self.root / linux[field][0]).unlink()
        result = self.package()
        with tarfile.open(result["archive"]) as archive:
            self.assertTrue(any("/official/playwright/" in name for name in archive.getnames()))

    def test_linux_official_metadata_must_match_selected_target_and_components(self):
        selection = self.targets["linux-x86_64"]["official"]
        directory = self.root / selection["bundle"]
        path = directory / "delivery.json"
        original = path.read_bytes()
        for index, target in enumerate((None, "macos-arm64", "linux-arm64", True)):
            delivery = json.loads(original)
            if target is None:
                delivery["components"][0].pop("target")
            else:
                delivery["components"][0]["target"] = target
            write(path, json.dumps(delivery).encode())
            selection.update(manifest(directory))
            self.assert_refused("official_target_mismatch", target="linux-x86_64", output="wrong-target-" + str(index))
        write(path, original)
        write(directory / "playwright/unselected.tgz", b"unselected Playwright")
        selection.update(manifest(directory))
        self.assert_refused("official_components_mismatch", target="linux-x86_64", output="wrong-namespace")
        delivery = json.loads(original)
        delivery["catalog_ids"].append("playwright")
        delivery["components"].append({"catalog_id": "playwright", "target": "linux-x86_64"})
        write(path, json.dumps(delivery).encode())
        selection.update(manifest(directory))
        self.assert_refused("official_components_mismatch", target="linux-x86_64", output="wrong-components")

    def test_cross_target_official_selection_is_refused(self):
        self.targets["linux-x86_64"]["official"] = self.targets["macos-arm64"]["official"]
        self.assert_refused("official_target_mismatch", target="linux-x86_64")

    def test_linux_manifest_or_executable_mismatch_is_refused(self):
        directory = self.root / self.targets["linux-x86_64"]["official"]["bundle"]
        binary = directory / release.OFFICIAL_EXECUTABLE
        binary.write_bytes(b"unaccepted replacement")
        self.assert_refused("selected_payload_digest_mismatch", target="linux-x86_64", output="binary-change")
        (directory / "CONTENTS.json").write_bytes(b"{}")
        self.assert_refused("selected_manifest_digest_mismatch", target="linux-x86_64", output="manifest-change")

    def test_linux_archive_is_deterministic_and_retains_exact_selected_entries(self):
        selected = release.collect(self.root, "linux-x86_64")
        first = self.package("linux-first", "linux-x86_64")
        relocated = self.base / "relocated-linux-checkout"
        shutil.copytree(self.root, relocated)
        for path in relocated.rglob("*"):
            os.utime(path, (1730000000, 1730000000))
            if path.is_file() and path.name != "github-mcp-server":
                path.chmod(0o600)
        second = self.package("linux-second", "linux-x86_64", root=relocated)
        self.assertEqual(Path(first["archive"]).read_bytes(), Path(second["archive"]).read_bytes())
        edition = self.targets["linux-x86_64"].get("edition", release.EDITION)
        root = f"toolkit-library-{edition}-linux-x86_64"
        with tarfile.open(first["archive"]) as archive:
            actual = {item.name: item for item in archive.getmembers() if item.isfile()}
            self.assertEqual(set(actual), {root + "/" + name for name in selected})
            for name, (data, mode) in selected.items():
                self.assertEqual(archive.extractfile(actual[root + "/" + name]).read(), data)
                self.assertEqual(actual[root + "/" + name].mode, mode)

    def test_shared_setup_asset_change_refused_for_both_targets(self):
        before = self.package("linux-before", "linux-x86_64")
        frozen = Path(before["archive"]).read_bytes()
        path = self.root / self.targets["macos-arm64"]["setup_assets"]["setup/static/app.js"][0]
        self.assertEqual(path, self.root / self.targets["linux-x86_64"]["setup_assets"]["setup/static/app.js"][0])
        path.write_bytes(b"unaccepted shared asset change\n")
        self.assert_refused("selected_source_digest_mismatch")
        self.assert_refused("selected_source_digest_mismatch", target="linux-x86_64", output="linux-refused")
        self.assertEqual(Path(before["archive"]).read_bytes(), frozen)

    def test_platform_setup_and_archive_edition_do_not_cross(self):
        for target, selection in self.targets.items():
            result = self.package(name=target, target=target)
            edition = selection.get("edition", release.EDITION)
            archive_root = f"toolkit-library-{edition}-{target}"
            self.assertEqual(Path(result["archive"]).name, archive_root + ".tar.gz")
            with tarfile.open(result["archive"]) as archive:
                data = archive.extractfile(archive_root + "/setup/backend.py").read()
                self.assertEqual(data, (self.root / selection["setup_backend"][0]).read_bytes())
                adapter_name = archive_root + "/setup/mcp_adapter.py"
                if "setup_adapter" in selection:
                    data = archive.extractfile(adapter_name).read()
                    self.assertEqual(data, (self.root / selection["setup_adapter"][0]).read_bytes())
                else:
                    self.assertNotIn(adapter_name, archive.getnames())
        source = self.root / self.targets["macos-arm64"]["setup_backend"][0]
        source.write_bytes(b"unaccepted replacement")
        self.assert_refused("selected_source_digest_mismatch", output="backend-change")

    def test_changed_adapter_is_refused_before_output(self):
        source = self.root / self.targets["macos-arm64"]["setup_adapter"][0]
        source.write_bytes(b"unaccepted replacement")
        self.assert_refused("selected_source_digest_mismatch", output="adapter-change")

    def test_missing_adapter_is_refused_before_output(self):
        source = self.root / self.targets["macos-arm64"]["setup_adapter"][0]
        source.unlink()
        with self.assertRaises(FileNotFoundError):
            self.package("adapter-missing")
        self.assertFalse((self.base / "adapter-missing").exists())

    def test_changed_payload_is_refused(self):
        (self.bundle() / "package/payload.whl").write_bytes(b"changed")
        self.assert_refused("selected_payload_digest_mismatch")

    def test_changed_manifest_is_refused_before_payload(self):
        (self.bundle() / "CONTENTS.json").write_bytes(b"{}")
        self.assert_refused("selected_manifest_digest_mismatch")

    def test_missing_extra_file_and_empty_directory_are_refused(self):
        path = self.bundle() / "README.md"
        old = path.read_bytes()
        path.unlink()
        self.assert_refused("bundle_file_set_mismatch", output="missing")
        write(path, old)
        extra = self.bundle() / "unexpected"
        write(extra, b"extra")
        self.assert_refused("bundle_file_set_mismatch", output="extra-file")
        extra.unlink()
        extra.mkdir()
        self.assert_refused("bundle_file_set_mismatch", output="extra-directory")

    def test_symlink_file_directory_and_hard_link_are_refused(self):
        path = self.bundle() / "README.md"
        old = path.read_bytes()
        path.unlink()
        path.symlink_to(self.root / "unrelated.txt")
        self.assert_refused("input_not_regular_unlinked_file", output="file-link")
        path.unlink()
        write(path, old)
        directory = self.bundle() / "package"
        outside = self.base / "linked-package"
        directory.rename(outside)
        directory.symlink_to(outside, target_is_directory=True)
        self.assert_refused("input_not_regular_unlinked_file", output="directory-link")
        directory.unlink()
        outside.rename(directory)
        os.link(path, self.base / "hard-link-input")
        self.assert_refused("input_not_regular_unlinked_file", output="hard-link")

    def test_fifo_is_refused_without_opening(self):
        os.mkfifo(self.bundle() / "unexpected-pipe")
        self.assert_refused("input_not_regular_unlinked_file")

    def test_executable_mode_and_special_bits_are_checked(self):
        path = self.bundle() / "README.md"
        path.chmod(0o755)
        self.assert_refused("unexpected_input_mode", output="data-executable")
        path.chmod(0o644)
        identity = path.stat()
        original_fstat = os.fstat
        injected = []

        def special_mode(fd):
            info = original_fstat(fd)
            if (info.st_dev, info.st_ino) != (identity.st_dev, identity.st_ino):
                return info
            injected.append((info.st_dev, info.st_ino))
            fields = list(info)
            fields[stat.ST_MODE] |= stat.S_ISUID
            return os.stat_result(fields)

        # The host filesystem may strip special bits during chmod. Supply the
        # selected file's mode through fstat; keep the actual package guard.
        with patch.object(release.os, "fstat", side_effect=special_mode):
            self.assert_refused("unexpected_input_mode", output="data-special")
        self.assertEqual(injected, [(identity.st_dev, identity.st_ino)])
        binary = self.root / self.official["bundle"] / release.OFFICIAL_EXECUTABLE
        binary.chmod(0o644)
        self.assert_refused("unexpected_input_mode", output="binary-mode")

    def test_source_change_and_unindexed_static_assets_are_refused(self):
        path = self.root / self.targets["macos-arm64"]["setup_backend"][0]
        old = path.read_bytes()
        path.write_bytes(b"changed")
        self.assert_refused("selected_source_digest_mismatch", output="source-change")
        path.write_bytes(old)
        write(self.root / "sdk/consumer-setup/static/private-state.json", b"unexpected")
        self.assert_refused("setup_file_set_mismatch", output="static-extra")

    def test_existing_output_relative_output_and_input_destination_are_refused(self):
        existing = self.base / "exists"
        existing.mkdir()
        write(existing / "keep", b"untouched")
        with self.assertRaisesRegex(release.PackageError, "output_exists"):
            release.package(self.root, "macos-arm64", existing)
        self.assertEqual((existing / "keep").read_bytes(), b"untouched")
        with self.assertRaisesRegex(release.PackageError, "invalid_output_or_target"):
            release.package(self.root, "macos-arm64", Path("relative"))
        # Even a non-selected predecessor bundle must not be mutated.
        inside = self.bundle("linux-x86_64") / "new-output"
        with self.assertRaisesRegex(release.PackageError, "output_inside_input"):
            release.package(self.root, "macos-arm64", inside)
        self.assertFalse(inside.exists())
        for target, selection in self.targets.items():
            inside = self.root / selection["official"]["bundle"] / "new-output"
            with self.assertRaisesRegex(release.PackageError, "output_inside_input"):
                release.package(self.root, "macos-arm64", inside)
            self.assertFalse(inside.exists())

    def test_checksum_rows_must_agree_and_cannot_duplicate(self):
        path = self.bundle() / "SHA256SUMS"
        old = path.read_bytes()
        path.write_bytes(old.replace(b"README.md", b"renamed.md"))
        self.targets["macos-arm64"]["sums"] = sha(path.read_bytes())
        self.assert_refused("checksums_disagree", output="disagree")
        path.write_bytes(old + old.splitlines(keepends=True)[0])
        self.targets["macos-arm64"]["sums"] = sha(path.read_bytes())
        self.assert_refused("invalid_checksums", output="duplicates")

    def test_unsafe_manifest_path_and_duplicate_json_keys_are_refused(self):
        path = self.bundle() / "CONTENTS.json"
        path.write_bytes(json.dumps({"files": {"../escape": {"sha256": "0" * 64}}}).encode())
        self.targets["macos-arm64"]["contents"] = sha(path.read_bytes())
        self.assert_refused("unsafe_input_path", output="unsafe")
        path.write_bytes(b'{"files":{},"files":{}}')
        self.targets["macos-arm64"]["contents"] = sha(path.read_bytes())
        self.assert_refused("duplicate_metadata_key", output="json-duplicate")

    def test_declared_size_and_byte_limits_are_enforced(self):
        path = self.bundle() / "CONTENTS.json"
        document = json.loads(path.read_bytes())
        document["files"]["README.md"]["bytes"] += 1
        raw = json.dumps(document).encode()
        path.write_bytes(raw)
        sums_path = self.bundle() / "SHA256SUMS"
        sums = sums_path.read_text()
        sums = sums.replace(self.targets["macos-arm64"]["contents"], sha(raw))
        sums_path.write_text(sums)
        self.targets["macos-arm64"].update(contents=sha(raw), sums=sha(sums.encode()))
        self.assert_refused("selected_payload_digest_mismatch", output="declared-size")
        self.repin_bundle()
        with patch.object(release, "MAX_TOTAL_BYTES", 1):
            self.assert_refused("input_too_large", output="bounded")

    def test_source_change_after_collection_does_not_change_snapshotted_archive(self):
        original = release.collect
        before = (self.bundle() / "README.md").read_bytes()

        def collect_then_change(root, target, *, settings=None):
            files = original(root, target, settings=settings)
            (self.bundle() / "README.md").write_bytes(b"changed after collection")
            return files

        with patch.object(release, "collect", side_effect=collect_then_change):
            result = self.package()
        with tarfile.open(result["archive"]) as archive:
            path = f"toolkit-library-{release.EDITION}-macos-arm64/consumers/README.md"
            self.assertEqual(archive.extractfile(path).read(), before)

    def test_archive_write_failure_never_publishes_checksum_or_replaces_output(self):
        with patch.object(tarfile.TarFile, "addfile", side_effect=OSError("synthetic failure")):
            with self.assertRaises(OSError):
                self.package()
        output = self.base / "output"
        self.assertEqual([path.suffix for path in output.iterdir()], [".part"])
        self.assertFalse((output / "SHA256SUMS").exists())
        with self.assertRaisesRegex(release.PackageError, "output_exists"):
            self.package()

    # The macOS run route: every input is this run's or the checkout's; no retained file, no donor.

    COMPONENTS = {
        "schema_version": 1,
        "github": {"catalog_id": "github", "version": "2.0.3", "asset": "github-mcp-server_Darwin_arm64.tar.gz",
                   "source_revision": "b98d115bbb080e9555466a96656ec4df605aea21", "profile": "selected-reads",
                   "selected_operations": ["get_me", "list_commits", "get_file_contents"],
                   "required_secret_environment": ["GITHUB_PERSONAL_ACCESS_TOKEN"]},
        "playwright": {"catalog_id": "playwright", "profile": "isolated-chrome",
                       "source_revision": "f183dad4a52965583e3cc1d59b88cdc279e2e57d",
                       "selected_operations": ["browser_navigate", "browser_snapshot", "browser_fill_form",
                                               "browser_click", "browser_close"],
                       "packages": {name: version for name, (version, _) in PLAYWRIGHT.items()}},
    }

    def write_bundle_manifest(self, directory, closure_raw, **changes):
        rows = {}
        for path in sorted(directory.rglob("*")):
            if path.is_file() and path.name not in {"CONTENTS.json", "SHA256SUMS"}:
                data = path.read_bytes()
                rows[path.relative_to(directory).as_posix()] = {"sha256": sha(data), "bytes": len(data)}
        contents = {"schema_version": 1, "scope": "consumer_export_only_no_installation_or_activation",
                    "files": rows, "accepted_inputs": {"dependency-closure": sha(closure_raw)}}
        contents.update(changes)
        raw = (json.dumps(contents, indent=2, sort_keys=True) + "\n").encode()
        write(directory / "CONTENTS.json", raw)
        sums = {name: row["sha256"] for name, row in rows.items()}
        sums["CONTENTS.json"] = sha(raw)
        write(directory / "SHA256SUMS", "".join(f"{value}  {name}\n" for name, value in sorted(sums.items())).encode())
        return sha(raw)

    def macos_run(self, *, texts=None):
        """Lay out an inert run: kit (built by the kit packager), consumer bundle, pinned official files."""
        work = self.base / "work"
        run = kit_fixture.run_fixture(self.base, checkout=self.root)
        # Official inputs as their publishers package them: one release archive, three registry tarballs.
        binary = kit_fixture.MACHO_ARM64 + b"inert upstream server; never executed\n"
        asset = gzip_tar({"LICENSE": (b"Upstream MIT licence\n", 0o644), "README.md": (b"upstream readme\n", 0o644),
                          "github-mcp-server": (binary, 0o755)})
        write(run["pinned"] / "release-download/github-mcp-server_Darwin_arm64.tar.gz", asset)
        rows = [{"kind": "release-download", "name": "github-mcp-server", "version": "2.0.3",
                 "file": "github-mcp-server_Darwin_arm64.tar.gz",
                 "url": "https://github.com/github/github-mcp-server/releases/download/v2.0.3/github-mcp-server_Darwin_arm64.tar.gz",
                 "sha256": sha(asset), "sha512": None, "digest_source": "publisher checksum file", "use": ["official github"]}]
        versions = {name: version for name, (version, _) in PLAYWRIGHT.items()}
        for name, (version, file) in PLAYWRIGHT.items():
            dependencies = {"@playwright/mcp": {"playwright": versions["playwright"], "playwright-core": versions["playwright-core"]},
                            "playwright": {"playwright-core": versions["playwright-core"]}, "playwright-core": {}}[name]
            members = {"package.json": (json.dumps({"name": name, "version": version, "license": "Apache-2.0",
                                                    **({"dependencies": dependencies} if dependencies else {})}).encode(), 0o644),
                       "cli.js": (b"// inert; never executed\n", 0o644), "LICENSE": ((name + " licence\n").encode(), 0o644)}
            if name != "@playwright/mcp":
                members["NOTICE"] = ((name + " notice\n").encode(), 0o644)
                members["ThirdPartyNotices.txt"] = ((name + " third party\n").encode(), 0o644)
            raw = gzip_tar(members, prefix="package/")
            write(run["pinned"] / "npm-tarball" / file, raw)
            rows.append({"kind": "npm-tarball", "name": name, "version": version, "file": file,
                         "url": "https://registry.npmjs.org/" + name + "/-/" + file, "sha256": None,
                         "sha512": hashlib.sha512(raw).hexdigest(), "digest_source": "dist.integrity",
                         "use": ["official playwright"]})
        run["extra_pins"] = rows
        kit_fixture.write_pins(run)
        kit = kit_fixture.package_run(run, work / "kit")
        # The texts the checkout carries for this route.
        if texts is None:
            for name, data in {"README.md": GUIDE_TEMPLATE.encode(), "official/README.md": b"Official bundle guide.\n",
                               "official/github/UPSTREAM-PROFILE.md": b"GitHub selected reads.\n",
                               "official/playwright/UPSTREAM-PROFILE.md": b"Playwright isolated Chrome.\n",
                               "official/components.json": json.dumps(self.COMPONENTS).encode(),
                               "selected/contracts/COMPOSE-CONTRACT.md": b"Compose contract.\n",
                               "selected/contracts/COMPOSE-request.example.json": b"{}\n"}.items():
                write(self.root / RUN_TEXTS / name, data)
        else:
            shutil.copytree(texts, self.root / RUN_TEXTS)
        # The consumer bundle as the exporter leaves it: uncomposed routes, schema 1, no runtime target.
        bundle = work / "consumers"
        components = ("agno/runtime-constraints.txt", "services/runtime-constraints.txt",
                      "activepieces/package.json", "activepieces/package-lock.json",
                      "agno/meerkat_toolkit_agno-0.1.5-py3-none-any.whl",
                      "services/meerkat_toolkit_services-0.1.23-py3-none-any.whl",
                      "activepieces/rkat-toolkit-activepieces-0.1.0.tgz")
        for name in components:
            write(bundle / name, ("inert " + name).encode())
        write(bundle / "README.md", b"Consumer guide.\n")
        write(bundle / "agno/hackernews.json", b'{"profile": "hackernews"}\n')
        catalog = (json.dumps([{"identity": row["identity"], "strategies": [{"id": row["strategy_id"], "kind": "local_mcp"}]}
                               for row in REAL_ROUTE_BINDINGS["macos-arm64"]]) + "\n").encode()
        write(bundle / "catalog.json", catalog)
        write(bundle / "routes.json", (json.dumps({"schema_version": 1, "catalog_sha256": sha(catalog),
                                                   "components": {"agno": {"path": "agno"}}, "routes": []}) + "\n").encode())
        closure = work / "consumers-closure.json"
        closure_raw = json.dumps({"schema_version": 1, "scope": "dependency_bytes_for_existing_consumer_selection",
                                  "python_platform": {"implementation": "cpython", "version": "3.11", "system": "Darwin", "machine": "arm64"},
                                  "accepted_inputs": {name: sha((bundle / name).read_bytes()) for name in components}}).encode()
        write(closure, closure_raw)
        contents = self.write_bundle_manifest(bundle, closure_raw)
        # Nothing retained may be needed: remove every legacy input location of the fixture checkout.
        shutil.rmtree(self.root / "dist")
        shutil.rmtree(self.root / "docs")
        manager = patch.object(release, "OFFICIAL_ROUTE_BINDINGS", copy.deepcopy(REAL_ROUTE_BINDINGS))
        manager.start(); self.addCleanup(manager.stop)
        run.update(bundle=bundle, closure=closure, closure_raw=closure_raw, kit_result=kit, asset=asset, binary=binary,
                   arguments={"edition": "20261112-connect-0a1b2c3d", "pins": run["pins"], "pinned_inputs": run["pinned"],
                              "consumers": bundle, "consumers_contents_sha256": contents, "consumer_closure": closure,
                              "kit": work / "kit" / kit["archive"], "kit_sha256": kit["sha256"]})
        return run

    def run_package(self, run, name="library", **changes):
        return release.package_run(self.root, "macos-arm64", self.base / name, **{**run["arguments"], **changes})

    def run_refused(self, run, code, name="run-refused", **changes):
        with self.assertRaisesRegex(release.PackageError, "^" + code + "$"):
            self.run_package(run, name, **changes)
        self.assertFalse((self.base / name).exists())

    def run_members(self, result):
        prefix = Path(result["archive"]).name.removesuffix(".tar.gz") + "/"
        with tarfile.open(result["archive"]) as archive:
            return {member.name.removeprefix(prefix): (archive.extractfile(member).read(), member.mode)
                    for member in archive.getmembers() if member.isfile()}

    def test_macos_run_library_holds_only_run_and_checkout_inputs(self):
        run = self.macos_run()
        with patch("subprocess.Popen", side_effect=AssertionError("must not execute")):
            result = self.run_package(run)
            again = self.run_package(run, "library-again")
        archive = Path(result["archive"])
        self.assertEqual(archive.name, "toolkit-library-20261112-connect-0a1b2c3d-macos-arm64.tar.gz")
        self.assertEqual(archive.read_bytes(), Path(again["archive"]).read_bytes())
        self.assertEqual(result["sha256"], sha(archive.read_bytes()))
        self.assertEqual((archive.parent / "SHA256SUMS").read_text(), f"{result['sha256']}  {archive.name}\n")
        self.assertEqual(json.loads((archive.parent / "build.json").read_text()), result)
        self.assertEqual((result["schema_version"], result["target"], result["edition"], result["scope"]),
                         (2, "macos-arm64", "20261112-connect-0a1b2c3d", "run_built_library_packaging_not_execution_qualification"))
        self.assertEqual(result["packager_source_sha256"], sha(Path(release.__file__).read_bytes()))
        self.assertEqual(result["inputs"]["kit"], {"sha256": run["kit_result"]["sha256"], "bytes": run["kit_result"]["bytes"]})
        self.assertEqual(result["inputs"]["pins"]["sha256"], sha(run["pins"].read_bytes()))
        self.assertEqual(result["inputs"]["consumer_closure"], {"sha256": sha(run["closure_raw"])})
        files = self.run_members(result)
        self.assertEqual(result["files"], len(files))
        tools = set(kit_fixture.RUN_TOOL_FEATURES)
        self.assertEqual({name for name in files if name.startswith("selected/bin/")},
                         {"selected/bin/" + tool for tool in tools} | {"selected/bin/install-library-consumer.py"})
        self.assertEqual({name for name in files if name.startswith("selected/provenance/")}, {
            "selected/provenance/crates/alpha-1.0.0.crate", "selected/provenance/crates/beta-macro-0.2.1.crate",
            "selected/provenance/dependencies.json", "selected/provenance/kit.json", "selected/provenance/selection.json",
            "selected/provenance/document-sources.json", "selected/provenance/consumer-dependency-closure.json",
            "selected/provenance/run-inputs.json", *("selected/provenance/tools/" + tool + ".build.json" for tool in tools)})
        self.assertEqual({name for name in files if name.startswith("selected/licenses/")}, {
            "selected/licenses/crates/alpha-1.0.0/LICENSE-APACHE", "selected/licenses/crates/alpha-1.0.0/LICENSE-MIT",
            "selected/licenses/crates/beta-macro-0.2.1/NOTICE", "selected/licenses/rust/COPYRIGHT-library.html",
            "selected/licenses/toolkit/LICENSE-APACHE", "selected/licenses/toolkit/LICENSE-MIT"})
        contracts = {name for name in files if name.startswith("selected/contracts/")}
        self.assertEqual(contracts, {"selected/contracts/" + leaf for leaf in (
            "INSPECT.md", "PYTHON-INSTALL.md", "NODE-INSTALL.md", "MCPB-INSTALL.md", "LOCAL-RECIPES.md",
            "HOSTED-METADATA.md", "SETUP-RECIPES.md", "HOSTED-RECIPES.md")}
            | {name for name in self.python_sources if name.startswith("selected/contracts/")})
        self.assertIn("selected/contracts/COMPOSE-CONTRACT.md", contracts)
        self.assertEqual(files["selected/contracts/COMPOSE-CONTRACT.md"][0], b"Compose contract.\n")
        self.assertEqual({name for name in files if name.startswith("setup/")},
                         {"setup/backend.py", "setup/mcp_adapter.py", "setup/install-library-consumer.py",
                          "setup/static/index.html", "setup/static/app.js", "setup/static/form.js", "setup/static/style.css"}
                         | {name for name in self.python_sources if name.startswith("setup/")})
        installer = (self.root / "scripts/install-library-consumer.py").read_bytes()
        self.assertEqual(files["setup/install-library-consumer.py"], (installer, 0o644))
        self.assertEqual(files["selected/bin/install-library-consumer.py"], (installer, 0o755))
        for name in ("LICENSE-MIT", "LICENSE-APACHE", "README.md", "selected/agent/instructions.md",
                     "consumers/CONTENTS.json", "consumers/README.md", "official/CONTENTS.json"):
            self.assertIn(name, files)
        self.assertEqual(files["selected/provenance/consumer-dependency-closure.json"][0], run["closure_raw"])
        # M1 and M4: no history row, no donor record, no MCP check tool.
        for forbidden in ("selected/bin/toolkit-mcp-check.py", "selected/contracts/MCP-EXECUTION-CHECK.md",
                          "selected/provenance/donor-kit.json", "selected/provenance/current-input-selection.json"):
            self.assertNotIn(forbidden, files)
        for name, (_, mode) in files.items():
            executable = name.startswith("selected/bin/") or name == "official/github/github-mcp-server"
            self.assertEqual(mode, 0o755 if executable else 0o644, name)
        selection = json.loads(files["selected/provenance/selection.json"][0])
        self.assertEqual((selection["scope"], selection["platform"]), ("retained_selected_tools", "macos-arm64"))
        self.assertEqual(selection["capabilities"], ["agent-plugin-files", "mcpb-archive-inspection", "compose-local-execution"])
        self.assertEqual(selection["kit"], {"sha256": run["kit_result"]["sha256"], "scope": "run-built-selected-tools-kit"})
        self.assertNotIn("donor", selection)
        for tool in tools:
            self.assertEqual(selection["files"]["bin/" + tool], sha((run["tools"] / tool).read_bytes()))
        inputs = json.loads(files["selected/provenance/run-inputs.json"][0])
        self.assertEqual(inputs["sources"]["setup/backend.py"],
                         {"path": "sdk/consumer-setup/backend.py", "sha256": sha(files["setup/backend.py"][0])})
        self.assertEqual(inputs["sources"]["README.md"]["path"], RUN_TEXTS + "/README.md")
        self.assertEqual(inputs["inputs"], result["inputs"])
        # Records written here name no path of this machine.
        for name in ("selected/provenance/selection.json", "selected/provenance/run-inputs.json",
                     "selected/provenance/document-sources.json", "official/delivery.json", "README.md"):
            self.assertNotIn(str(self.base).encode(), files[name][0], name)
        self.assertNotIn(str(self.base), json.dumps({key: value for key, value in result.items() if key != "archive"}))

    def test_macos_run_checkout_rows_follow_the_release_commit_not_a_stale_pin(self):
        run = self.macos_run()
        path = self.targets["macos-arm64"]["setup_backend"][0]
        changed = b"backend at the release commit; its table pin is stale\n"
        write(self.root / path, changed)
        files = self.run_members(self.run_package(run))
        self.assertEqual(files["setup/backend.py"][0], changed)
        inputs = json.loads(files["selected/provenance/run-inputs.json"][0])
        self.assertEqual(inputs["sources"]["setup/backend.py"], {"path": path, "sha256": sha(changed)})
        (self.root / path).unlink()
        with self.assertRaises((OSError, release.PackageError)):
            self.run_package(run, "missing-row")
        self.assertFalse((self.base / "missing-row").exists())
        write(self.root / path, changed)
        write(self.root / "sdk/consumer-setup/static/new.js", b"unindexed asset")
        self.run_refused(run, "setup_file_set_mismatch")

    def test_macos_run_official_bundle_is_the_upstream_asset_with_generated_records(self):
        run = self.macos_run()
        files = self.run_members(self.run_package(run))
        official = {name.removeprefix("official/"): value for name, value in files.items() if name.startswith("official/")}
        archives = {"playwright/archives/" + file for _, file in PLAYWRIGHT.values()}
        notices = {"playwright/notices/playwright-mcp/LICENSE",
                   *("playwright/notices/" + package + "/" + leaf for package in ("playwright", "playwright-core")
                     for leaf in ("LICENSE", "NOTICE", "ThirdPartyNotices.txt"))}
        self.assertEqual(set(official), {
            "README.md", "delivery.json", "CONTENTS.json", "SHA256SUMS", "github/github-mcp-server",
            "github/notices/upstream-LICENSE", "github/UPSTREAM-PROFILE.md", "github/mcp.template.json",
            "playwright/package.json", "playwright/package-lock.json", "playwright/closure.json",
            "playwright/mcp.template.json", "playwright/UPSTREAM-PROFILE.md", *archives, *notices})
        self.assertEqual(official["github/github-mcp-server"], (run["binary"], 0o755))
        self.assertEqual(official["github/notices/upstream-LICENSE"][0], b"Upstream MIT licence\n")
        self.assertEqual(official["github/UPSTREAM-PROFILE.md"][0], b"GitHub selected reads.\n")
        for _, file in PLAYWRIGHT.values():
            self.assertEqual(official["playwright/archives/" + file][0], (run["pinned"] / "npm-tarball" / file).read_bytes())
        contents = json.loads(official["CONTENTS.json"][0])
        self.assertEqual(set(contents), {"format", "files"})
        self.assertEqual(contents["format"], 1)
        self.assertEqual(contents["files"], {name: {"sha256": sha(data), "bytes": len(data)}
                                             for name, (data, _) in official.items() if name not in ("CONTENTS.json", "SHA256SUMS")})
        sums = dict(reversed(line.split("  ")) for line in official["SHA256SUMS"][0].decode().splitlines())
        self.assertEqual(sums, {**{name: row["sha256"] for name, row in contents["files"].items()},
                                "CONTENTS.json": sha(official["CONTENTS.json"][0])})
        delivery = json.loads(official["delivery.json"][0])
        self.assertEqual((delivery["format"], delivery["role"], delivery["catalog_ids"]),
                         ("meerkat-toolkit.official-consumers.v1", "service_connector", ["github", "playwright"]))
        github, playwright = delivery["components"]
        self.assertEqual(github, {
            "artifact": "github/github-mcp-server", "artifact_sha256": sha(run["binary"]), "catalog_id": "github",
            "configuration_template": "github/mcp.template.json", "notices": "github/notices", "profile": "selected-reads",
            "required_secret_environment": ["GITHUB_PERSONAL_ACCESS_TOKEN"],
            "runtime_profile": "GitHub MCP server v2.0.3 macOS arm64; upstream release asset, unexecuted input",
            "selected_operations": ["get_me", "list_commits", "get_file_contents"],
            "source_archive": {"bytes": len(run["asset"]), "sha256": sha(run["asset"])},
            "source_revision": "b98d115bbb080e9555466a96656ec4df605aea21", "target": "macos-arm64"})
        self.assertEqual({key: playwright[key] for key in ("catalog_id", "profile", "target", "source_revision", "whole_endpoint_exposed",
                                                           "browser_distributed", "package_json", "package_lock", "closure",
                                                           "configuration_template", "notices")},
                         {"catalog_id": "playwright", "profile": "isolated-chrome", "target": "macos-arm64",
                          "source_revision": "f183dad4a52965583e3cc1d59b88cdc279e2e57d", "whole_endpoint_exposed": True,
                          "browser_distributed": False, "package_json": "playwright/package.json",
                          "package_lock": "playwright/package-lock.json", "closure": "playwright/closure.json",
                          "configuration_template": "playwright/mcp.template.json", "notices": "playwright/notices"})
        # The release composes both official families into the bundle's routes with this run's digests.
        routes = json.loads(files["consumers/routes.json"][0])
        self.assertEqual(routes["schema_version"], 2)
        installer_sha = sha(files["setup/install-library-consumer.py"][0])
        self.assertEqual(routes["components"]["official-github"], {
            "path": "github/github-mcp-server", "sha256": sha(run["binary"]), "profiles": ["selected-reads"],
            "contents_sha256": sha(official["CONTENTS.json"][0]), "installer_sha256": installer_sha})
        self.assertEqual(routes["components"]["official-playwright"]["sha256"], sha(official["playwright/package-lock.json"][0]))
        self.assertEqual([row["family"] for row in routes["routes"]], ["official-github", "official-playwright"])
        consumers = json.loads(files["consumers/CONTENTS.json"][0])
        self.assertEqual((consumers["schema_version"], "runtime_target" in consumers), (1, False))
        self.assertEqual(consumers["files"]["routes.json"]["sha256"], sha(files["consumers/routes.json"][0]))

    def test_macos_run_official_bundle_meets_the_shipped_installer(self):
        """The installer that ships in this library must admit what the packager composed."""
        installer = load("library_consumer_installer", "install-library-consumer.py")
        run = self.macos_run()
        files = self.run_members(self.run_package(run))
        bundle = self.base / "extracted-official"
        for name, (data, mode) in files.items():
            if name.startswith("official/"):
                write(bundle / name.removeprefix("official/"), data, mode)
        contents = sha(files["official/CONTENTS.json"][0])
        payloads, closure, component = installer.playwright_inputs(bundle, contents)
        self.assertEqual(component["catalog_id"], "playwright")
        self.assertEqual(len(closure["rows"]), 3)
        delivery = json.loads(files["official/delivery.json"][0])
        github = delivery["components"][0]
        self.assertEqual(installer.official_profile(github), installer.OFFICIAL_PROFILE)
        self.assertEqual(installer.official_target(github), "macos-arm64")
        self.assertEqual(json.loads(files["official/github/mcp.template.json"][0]),
                         installer.official_template(installer.OFFICIAL_PROFILE))
        installer.check_official_executable(files["official/github/github-mcp-server"][0], "macos-arm64")
        self.assertEqual(github["selected_operations"], installer.OFFICIAL_TOOLS)
        self.assertEqual(github["required_secret_environment"], [installer.OFFICIAL_ENV])
        # One rule is the installer's to change (assigned outside this file set): a selected-reads component
        # taken from an upstream release archive carries that archive's licence only. Until the installer
        # accepts that shape it refuses here with exactly this reason and no other.
        try:
            admitted, selected = installer.official_inputs(bundle, contents, "github")
        except installer.InstallError as error:
            self.assertEqual(str(error), "incomplete official notices")
        else:
            self.assertEqual(selected, github)
            self.assertEqual([name for name in admitted if name.startswith("github/notices/")],
                             ["github/notices/upstream-LICENSE"])

    def test_macos_run_guide_pins_are_rendered_from_the_payload_never_kept(self):
        run = self.macos_run()
        files = self.run_members(self.run_package(run))
        guide = files["README.md"][0].decode()
        installer_sha = sha(files["setup/install-library-consumer.py"][0])
        expected = GUIDE_TEMPLATE
        for name, value in {"EDITION": "20261112-connect-0a1b2c3d",
                            "CONSUMERS_CONTENTS_SHA256": sha(files["consumers/CONTENTS.json"][0]),
                            "OFFICIAL_CONTENTS_SHA256": sha(files["official/CONTENTS.json"][0]),
                            "OFFICIAL_INSTALLER_SHA256": installer_sha, "VERIFIER_SHA256": installer_sha,
                            "HACKERNEWS_CONFIG_SHA256": sha(files["consumers/agno/hackernews.json"][0]),
                            "GITHUB_VERSION": "2.0.3", "GITHUB_SOURCE_REVISION": "b98d115bbb080e9555466a96656ec4df605aea21",
                            "GITHUB_EXECUTABLE_SHA256": sha(run["binary"])}.items():
            expected = expected.replace("{{" + name + "}}", value)
        self.assertEqual(guide, expected)
        self.assertNotIn("{{", guide)
        template = self.root / RUN_TEXTS / "README.md"
        for index, (text, code) in enumerate((
                (GUIDE_TEMPLATE + "{{UNKNOWN_PIN}}\n", "guide_placeholder_unknown"),
                (GUIDE_TEMPLATE + "--contents-sha256 " + "0" * 64 + "\n", "guide_command_digest_mismatch"),
                (GUIDE_TEMPLATE + '"--verifier-sha256", "' + "0" * 64 + '"\n', "guide_command_digest_mismatch"),
                (GUIDE_TEMPLATE + "--official-installer-sha256 " + "0" * 64 + "\n", "guide_command_digest_mismatch"),
                (GUIDE_TEMPLATE.replace('"--official-contents-sha256", "{{OFFICIAL_CONTENTS_SHA256}}",\n', ""), "guide_pin_missing"))):
            with self.subTest(code=code, index=index):
                write(template, text.encode())
                self.run_refused(run, code, "guide-" + str(index))
        write(template, GUIDE_TEMPLATE.encode())
        (run["bundle"] / "agno/hackernews.json").unlink()
        contents = self.write_bundle_manifest(run["bundle"], run["closure_raw"])
        self.run_refused(run, "guide_input_missing", "guide-no-example", consumers_contents_sha256=contents)

    def test_macos_run_refuses_changed_foreign_or_retained_shaped_inputs(self):
        run = self.macos_run()
        self.run_refused(run, "kit_digest_mismatch", "kit-digest", kit_sha256="0" * 64)
        self.run_refused(run, "selected_manifest_digest_mismatch", "contents-digest", consumers_contents_sha256="0" * 64)
        self.run_refused(run, "invalid_edition", "edition", edition="../x")
        self.run_refused(run, "output_inside_input", name="work/consumers/out")
        with self.assertRaisesRegex(release.PackageError, "^run_route_is_macos_only$"):
            release.package_run(self.root, "linux-x86_64", self.base / "linux-run", **run["arguments"])
        # A bundle for another runtime contract, a closure the manifest does not bind, composed routes.
        for index, (changes, code) in enumerate((
                ({"schema_version": 2, "runtime_target": {"python": {}, "node": {}}}, "selected_consumer_target_mismatch"),
                ({"accepted_inputs": {"dependency-closure": "0" * 64}}, "selected_consumer_closure_binding_mismatch"))):
            contents = self.write_bundle_manifest(run["bundle"], run["closure_raw"], **changes)
            self.run_refused(run, code, "bundle-" + str(index), consumers_contents_sha256=contents)
        routes = json.loads((run["bundle"] / "routes.json").read_bytes())
        routes["components"]["official-github"] = {"path": "github/github-mcp-server"}
        write(run["bundle"] / "routes.json", json.dumps(routes).encode())
        contents = self.write_bundle_manifest(run["bundle"], run["closure_raw"])
        self.run_refused(run, "invalid_official_route_binding", "composed-routes", consumers_contents_sha256=contents)

    def test_macos_run_official_pins_must_be_the_declared_upstream_files(self):
        run = self.macos_run()
        document = json.loads(run["pins"].read_text())
        def repin(change):
            pins = copy.deepcopy(document)
            change(pins["pins"])
            run["pins"].write_text(json.dumps(pins))
        cases = (
            (lambda pins: [row.update(version="2.0.4") for row in pins if "official github" in row["use"]], "official_pin_mismatch"),
            (lambda pins: pins.remove(next(row for row in pins if row["name"] == "playwright-core")), "official_pin_mismatch"),
            (lambda pins: [row.update(version="0.0.84") for row in pins if row["name"] == "@playwright/mcp"], "official_pin_mismatch"),
            (lambda pins: [row.update(sha256="0" * 64) for row in pins if "official github" in row["use"]], "pinned_input_digest_mismatch"),
            (lambda pins: [row.update(sha512="0" * 128) for row in pins if row["name"] == "playwright"], "pinned_input_digest_mismatch"),
            (lambda pins: [row.update(sha512=None) for row in pins if row["name"] == "playwright"], "invalid_pin_file"),
            # `use` is a non-empty list of labels (the pin schema), never one string.
            (lambda pins: [row.update(use="official github") for row in pins if "official github" in row["use"]], "invalid_pin_file"),
            (lambda pins: [row.update(use=[]) for row in pins if row["name"] == "playwright"], "invalid_pin_file"),
            (lambda pins: [row.update(use=["official playwright", 7]) for row in pins if row["name"] == "playwright"], "invalid_pin_file"),
            (lambda pins: [row.update(use=["agno closure"]) for row in pins if row["name"] == "playwright"], "official_pin_mismatch"),
        )
        for index, (change, code) in enumerate(cases):
            with self.subTest(index=index):
                repin(change)
                self.run_refused(run, code, "pins-" + str(index))
        run["pins"].write_text(json.dumps(document))
        # The release may be named by its tag: the same version with a leading "v".
        repin(lambda pins: [row.update(version="v2.0.3") for row in pins if "official github" in row["use"]])
        self.assertEqual(self.run_package(run, "tag-version")["inputs"]["official"]["github"]["sha256"], sha(run["asset"]))
        asset = gzip_tar({"LICENSE": (b"Upstream MIT licence\n", 0o644), "github-mcp-server": (b"\x7fELF not a Mach-O file", 0o755)})
        write(run["pinned"] / "release-download/github-mcp-server_Darwin_arm64.tar.gz", asset)
        repin(lambda pins: [row.update(sha256=sha(asset)) for row in pins if "official github" in row["use"]])
        self.run_refused(run, "official_executable_mismatch", "elf-asset")

    def test_macos_run_kit_must_be_the_run_built_scope_with_bound_records(self):
        run = self.macos_run()
        # The kit of another scope (for example a donor-shaped archive) is not this route's input.
        raw = gzip_tar({"provenance/kit.json": (json.dumps({"schema_version": 1, "platform": "macos-arm64",
                        "scope": "selected-python-node-mcpb-installation-kit", "files": {}}).encode(), 0o644)},
                       prefix="agent-install-kit-macos-arm64/")
        other = self.base / "work/other-kit.tar.gz"
        write(other, raw)
        self.run_refused(run, "kit_scope_mismatch", "other-kit", kit=other, kit_sha256=sha(raw))
        linked = self.base / "work/linked-kit.tar.gz"; linked.symlink_to(run["arguments"]["kit"])
        with self.assertRaises((OSError, release.PackageError)):
            self.run_package(run, "linked-kit", kit=linked)
        self.assertFalse((self.base / "linked-kit").exists())

    def test_macos_run_cli_never_mixes_the_run_route_with_a_selection(self):
        script = Path(release.__file__)
        base = [sys.executable, "-I", "-B", str(script), "--target", "macos-arm64"]
        for index, arguments in enumerate((
                ["--pins", str(self.base / "pins.json")],
                ["--pins", str(self.base / "pins.json"), "--input-selection", str(self.base / "selection.json"),
                 "--input-selection-sha256", "0" * 64],
                ["--edition", "20261112-connect-0a1b2c3d"])):
            output = self.base / ("cli-" + str(index))
            result = subprocess.run(base + arguments + ["--output", str(output)],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual((result.returncode, result.stdout), (1, ""), arguments)
            self.assertEqual(result.stderr, "Packaging refused: incomplete_run_selection. Inputs and earlier outputs were not replaced.\n")
            self.assertFalse(output.exists())

    def test_macos_run_committed_texts_render_bind_and_carry_no_kept_digest(self):
        texts = release.ROOT / RUN_TEXTS
        self.assertEqual(sorted(path.relative_to(texts).as_posix() for path in texts.rglob("*") if path.is_file()), [
            "README.md", "official/README.md", "official/components.json", "official/github/UPSTREAM-PROFILE.md",
            "official/playwright/UPSTREAM-PROFILE.md", "selected/contracts/COMPOSE-CONTRACT.md",
            "selected/contracts/COMPOSE-request.example.json"])
        self.assertEqual(json.loads((texts / "official/components.json").read_bytes()), self.COMPONENTS)
        installer = load("library_consumer_installer_constants", "install-library-consumer.py")
        self.assertEqual(self.COMPONENTS["github"]["selected_operations"], installer.OFFICIAL_TOOLS)
        self.assertEqual(self.COMPONENTS["playwright"]["selected_operations"], installer.PLAYWRIGHT_OPERATIONS)
        # The two contract files are the public bytes of the Linux library.
        self.assertEqual(sha((texts / "selected/contracts/COMPOSE-CONTRACT.md").read_bytes()),
                         "04f9e674dfe8180e3c7f53d9686a3d6e17cb58d894a2b6bec5098965e201e00d")
        self.assertEqual(sha((texts / "selected/contracts/COMPOSE-request.example.json").read_bytes()),
                         "616d22d7318f903643d09e3ec58868b57cc48638ab812b139007432a4afbe6b3")
        template = (texts / "README.md").read_text(encoding="utf-8")
        for flag, name in (("--contents-sha256", "CONSUMERS_CONTENTS_SHA256"), ("--official-contents-sha256", "OFFICIAL_CONTENTS_SHA256"),
                           ("--official-installer-sha256", "OFFICIAL_INSTALLER_SHA256"), ("--verifier-sha256", "VERIFIER_SHA256")):
            self.assertRegex(template, re.escape(flag) + r"(?:\"\s*,\s*\"|\s+)\{\{" + name + r"\}\}")
        for path in (texts / "README.md", texts / "official/README.md", texts / "official/github/UPSTREAM-PROFILE.md",
                     texts / "official/playwright/UPSTREAM-PROFILE.md"):
            text = path.read_text(encoding="utf-8")
            self.assertIsNone(re.search(r"[0-9a-f]{64}", text), path.name)   # no hand-kept digest
            self.assertIsNone(re.search("[" + chr(0x2013) + chr(0x2014) + "]", text), path.name)
        run = self.macos_run(texts=texts)
        files = self.run_members(self.run_package(run))
        guide = files["README.md"][0].decode()
        self.assertNotIn("{{", guide)
        self.assertEqual(guide.count(sha(files["consumers/CONTENTS.json"][0])), template.count("{{CONSUMERS_CONTENTS_SHA256}}"))
        self.assertIn(sha(files["official/CONTENTS.json"][0]), guide)
        self.assertEqual(files["official/github/UPSTREAM-PROFILE.md"][0], (texts / "official/github/UPSTREAM-PROFILE.md").read_bytes())


class PackagedSelectedContractTests(unittest.TestCase):
    """Opt-in package boundary check, never package installation or runtime readiness.

    The native package owner supplies an exact archive and SHA-256. Only bundled
    metadata validators execute. The installer stops after argument parsing and
    repeated validation, before payload interpretation or runtime execution.
    """

    def test_packaged_declaration_backend_planners_and_installer_agree(self):
        selected = os.environ.get("TOOLKIT_LIBRARY_CONTRACT_ARCHIVE")
        expected = os.environ.get("TOOLKIT_LIBRARY_CONTRACT_SHA256")
        if selected is None and expected is None:
            self.skipTest("requires an explicitly pinned built library package")
        self.assertTrue(selected and expected)
        archive_path = Path(selected)
        self.assertTrue(archive_path.is_absolute())
        self.assertRegex(expected, r"\A[0-9a-f]{64}\Z")
        with archive_path.open("rb") as stream:
            self.assertEqual(hashlib.file_digest(stream, "sha256").hexdigest(), expected)
        scratch = tempfile.TemporaryDirectory(prefix="toolkit-packaged-declaration-")
        self.addCleanup(scratch.cleanup)
        root = Path(scratch.name).resolve()
        names = {"setup/backend.py": 0o644, "selected/bin/install-library-consumer.py": 0o755,
                 "selected/bin/toolkit-python-plan": 0o755, "selected/bin/toolkit-node-plan": 0o755}
        prefix = archive_path.name.removesuffix(".tar.gz") + "/"
        pins = {}
        with tarfile.open(archive_path) as archive:
            members = archive.getmembers()
            for name, mode in names.items():
                rows = [row for row in members if row.name == prefix + name]
                self.assertEqual(len(rows), 1)
                row = rows[0]
                self.assertTrue(row.isfile() and row.mode == mode and 0 < row.size <= 64 * 1024 * 1024)
                raw = archive.extractfile(row).read()
                self.assertEqual(len(raw), row.size)
                write(root / name, raw, mode)
                pins[name] = sha(raw)

        def load(name, path):
            spec = importlib.util.spec_from_file_location(name, path)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            return module

        backend = load("packaged_selected_backend", root / "setup/backend.py")
        installer = load("packaged_selected_installer", root / "selected/bin/install-library-consumer.py")
        tools = {profile: {"path": str(root / ("selected/bin/toolkit-" + profile + "-plan")),
                          "sha256": pins["selected/bin/toolkit-" + profile + "-plan"]}
                 for profile in ("python", "node")}
        tools["installer"] = {"path": str(root / "selected/bin/install-library-consumer.py"),
                              "sha256": pins["selected/bin/install-library-consumer.py"]}
        staging = root / "staging"; staging.mkdir()
        host = {"schema_version": 1, "scope": backend.SELECTED_SCOPE, "staging_root": str(staging),
                "installer": tools["installer"], "validators": {k: tools[k] for k in ("python", "node")}}
        host_path = root / "host.json"; host_path.write_bytes(backend.encoded(host))
        runtime = str(Path(sys.executable).resolve())
        controller = backend.Controller(bundle=None, contents_sha256=None, families=(),
            workspace=root / "workspace", python=runtime, node=runtime, npm_cli=runtime,
            selected_install_profile=host_path, selected_install_profile_sha256=sha(host_path.read_bytes()))
        self.addCleanup(controller.join)
        fixtures = Path(__file__).resolve().parents[1] / "crates/toolkit-package/tests/fixtures"

        class MetadataBoundary(Exception):
            pass

        wheel_fixture = load("packaged_contract_wheel_fixture",
                             Path(__file__).with_name("test_selected_python_install.py"))
        for profile, console_script in (("python", None), ("python", "fixture-server"), ("node", None)):
            case = profile + ("-console" if console_script else "")
            with self.subTest(profile=profile, console_script=console_script):
                fixture = fixtures / (profile + "-install")
                package = (fixture / "package.json").read_bytes()
                activation = json.loads((fixture / "activation.json").read_bytes())
                # Reuse only declared software metadata from the inert Rust
                # fixture; host policy/lifetime/readiness never become authority.
                declaration = {key: activation[key] for key in
                    ("schema_version", "package", "package_content_digest", "strategy", "portable_mcp_entries")}
                declaration["document_kind"] = "local_software_declaration"
                component = {key: activation["components"][0][key] for key in ("id", "version", "source", "runtime")}
                declaration["components"] = [component]
                declaration["exports"] = [{key: activation["exports"][0][key] for key in
                    ("role", "id", "components", "authentication", "operations")}]
                source = component["source"]
                payloads = {"artifact.whl" if profile == "python" else "artifact.tgz": b"inert payload, never interpreted"}
                if console_script:
                    filename, wheel, _ = wheel_fixture.wheel(package="fixture_server", extra={
                        "fixture_server-1.0.dist-info/entry_points.txt":
                            b"[console_scripts]\nfixture-server = fixture_server:main\n",
                        "fixture_server/__init__.py": b"def main():\n    raise RuntimeError('must not execute')\n"})
                    source["location"] = "https://example.invalid/" + filename
                    source["entrypoint"].update(executable="fixture_server/__init__.py",
                                                python_console_script=console_script)
                    payloads["artifact.whl"] = wheel
                source["content_digest"] = sha(next(iter(payloads.values())))
                for name in source["dependencies"]["artifacts"]:
                    raw = ("inert dependency " + name).encode()
                    payloads["closure/" + name] = raw
                    source["dependencies"]["artifacts"][name] = sha(raw)
                raw = backend.encoded(declaration)
                for name, data in {"package.json": package, "declaration.json": raw, **payloads}.items():
                    write(staging / case / name, data, 0o600)
                request = {"profile": profile, "staged_id": case, "component": component["id"],
                           "document_kind": "local_software_declaration", "package_sha256": sha(package),
                           "selection_sha256": sha(raw)}
                checked = controller.selected_check(request)
                intent = {"root": controller.workspace, **{key: checked[key] for key in ("validation_id", "effect_sha256")}}
                argv, effect, plan = controller.selected_command(intent, verify=False)
                self.assertEqual(plan["document_kind"], "local_software_declaration")
                self.assertNotIn("manifest_digest", plan)
                self.assertEqual(plan["entrypoint"].get("python_console_script"), console_script)
                self.assertEqual(sha(Path(argv[3]).read_bytes()), tools["installer"]["sha256"])
                self.assertEqual(argv[argv.index("--document-kind") + 1], "local_software_declaration")
                calls = []
                original_run = installer.run

                def metadata_only(command, cwd, env, label, timeout=900):
                    calls.append(label)
                    if label == "validate-selection":
                        self.assertIn("--declaration", command)
                        observed = original_run(command, cwd, env, label, timeout=30)
                        self.assertEqual(json.loads(observed), plan)
                        return observed
                    if profile == "python" and label == "python-runtime":
                        raise MetadataBoundary()
                    self.fail("unexpected runtime or payload execution")

                previous_signal = signal.getsignal(signal.SIGTERM)
                try:
                    with patch.object(sys, "argv", argv[3:]), patch.object(installer, "run", side_effect=metadata_only), \
                            patch.object(installer, "selected_npm_closure", side_effect=MetadataBoundary), \
                            self.assertRaises(MetadataBoundary):
                        installer.main()
                finally:
                    signal.signal(signal.SIGTERM, previous_signal)
                self.assertEqual(calls, ["validate-selection"] + (["python-runtime"] if profile == "python" else []))
                self.assertFalse((Path(effect["destination"]) / "ready").exists())
                # The real packaged owner also refuses crossed document forms.
                invalid = copy.deepcopy(declaration); invalid["document_kind"] = "activation_manifest"
                (staging / case / "declaration.json").write_bytes(backend.encoded(invalid))
                with self.assertRaisesRegex(backend.SetupError, "selected_invalid_selection"):
                    controller.selected_check({**request, "selection_sha256": sha(backend.encoded(invalid))})
        self.assertEqual({name: sha((root / name).read_bytes()) for name in names}, pins)


if __name__ == "__main__":
    unittest.main()
