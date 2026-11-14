#!/usr/bin/env python3
"""Focused archive construction and input-boundary tests, with small fixtures."""

import contextlib
import copy
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
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


# The Linux library guide must hold this sentence, byte for byte and on one line. It names no file:
# the shipped record states what ships and lists no member.
DISCLOSURE = "The bound source is the exact build source except for members this library does not publish."
RECORD = "selected/provenance/redactions.json"
NOTE = "Build provenance records and some members of the source snapshots are not published with this library."
SOURCE_TREE = "selected/provenance/source/"
PUBLISHED_LIST = "release/library-published-source-members.json"
LIST_SCHEMA = "package-library-release/source-members/1"
PRIVATE_SCHEMA = "package-library-release/non-published-source-members/1"
# The Linux fixture's source snapshots -> the prefix their members carry. Any other snapshot has none.
SNAPSHOTS = {
    "selected/provenance/canonical-mcpb/source.tar.gz": "",
    "selected/provenance/compose-migration/source.tar.gz": "source/",
    "selected/provenance/compose-retention/source.tar.gz": "source/",
    "selected/provenance/linux-mcpb/source.tar.gz": "",
}
# Synthetic member names. The fixture checkout's committed list publishes PUBLISHED; WITHHELD are named
# only by the private list a test passes; no list names UNLISTED.
PUBLISHED = ("crates/sample/Cargo.toml", "crates/sample/src/empty.rs", "crates/sample/src/lib.rs",
             "crates/sample/src/wide.rs")
WITHHELD = ("crates/sample/NOTES.md", "crates/sample/data/listing.json", "crates/sample/tests/listed.rs")
UNLISTED = "crates/sample/src/added.rs"


def source_snapshot(rows, *, format=tarfile.USTAR_FORMAT, extra=None):
    """A small source snapshot: (gzip bytes, the tar bytes inside)."""
    plain = io.BytesIO()
    with tarfile.open(fileobj=plain, mode="w:", format=format) as archive:
        for index, (name, data) in enumerate(rows):
            item = tarfile.TarInfo(name)
            item.size, item.mode = len(data), 0o755 if index % 2 else 0o644
            # Distinct header fields: a rewritten header would not equal the original block.
            item.mtime, item.uid, item.gid = 1700000000 + index, 501 + index, 20
            item.uname, item.gname = "builder", "staff"
            archive.addfile(item, io.BytesIO(data))
        if extra is not None:
            extra(archive)
    raw = plain.getvalue()
    return gzip.compress(raw, mtime=1700000000), raw


def member_blocks(raw):
    """Each tar member's whole raw block (header, data and padding), in archive order."""
    rows = []
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as archive:
        for item in archive:
            end = item.offset_data + (item.size + 511) // 512 * 512
            rows.append((item.name, raw[item.offset:end]))
    return rows


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
        # The packager's own pins, read before any fixture replaces them.
        self.pinned_sources = {target: set(release.PYTHON_SOURCES) | set(kit.get("sources", {}))
                               for target, kit in release.PYTHON_KITS.items()}
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
        self.provenance_fixture()

    def provenance_fixture(self):
        """Give every target source snapshots, and Linux a donor source tree, that hold only members the
        fixture checkout's committed list publishes. The Linux guide holds the disclosure sentence."""
        target = "linux-x86_64"
        self.published_data = dict(zip(PUBLISHED, (b"[package]\n", b"", b"r" * 512, b"w" * 513)))
        self.snapshot_rows, self.snapshot_tars, self.private_trees = {}, {}, {}
        self.non_published = None
        self.donor_files[target]["provenance/source/" + PUBLISHED[2]] = (b"retained tree source\n", 0o644)
        self.repin_donor(target)
        self.published = {SOURCE_TREE: sorted([PUBLISHED[2], "retained.txt"])}
        for member in SNAPSHOTS:
            self.python_kits[target].setdefault("sources", {})[member] = (
                "retained-inputs/" + member.removeprefix("selected/"), None)
        for name, donor in self.python_kits.items():
            for member in donor.get("sources", {}):
                if member.endswith("/source.tar.gz"):
                    prefix = SNAPSHOTS.get(member, "")
                    # The first member differs per snapshot, so no two snapshots are the same bytes.
                    self.snapshot_rows[member] = [
                        (prefix + path, self.published_data[path] + (b"# " + member.encode() + b"\n" if not index else b""))
                        for index, path in enumerate(PUBLISHED)]
                    self.published[member] = [prefix + path for path in PUBLISHED]
                    self.repin_snapshot(member, target=name)
        self.write_published()
        selection = self.targets[target]
        guide = ("Selected release guide: " + target + "\n\n" + DISCLOSURE + "\n").encode()
        write(self.root / selection["guide"], guide)
        selection["guide_sha256"] = sha(guide)

    def write_published(self):
        write(self.root / PUBLISHED_LIST, (json.dumps(
            {"schema": LIST_SCHEMA, "trees": {tree: sorted(names) for tree, names in sorted(self.published.items())}},
            indent=1) + "\n").encode())

    def private_document(self):
        """The private list: per tree the members that are not published with their digests, and the
        digests of the original source snapshots that hold one."""
        originals = {}
        for donor in self.python_kits.values():
            for member, (_, pin) in donor.get("sources", {}).items():
                if member in self.private_trees and self.private_trees[member]:
                    originals.setdefault(member, set()).add(pin)
        return {"schema": PRIVATE_SCHEMA,
                "trees": {tree: [{"name": name, "sha256": names[name]} for name in sorted(names)]
                          for tree, names in sorted(self.private_trees.items())},
                "original_snapshots": {member: sorted(pins) for member, pins in sorted(originals.items())}}

    def write_non_published(self, document=None):
        self.non_published = self.base / "private" / "non-published-source-members.json"
        write(self.non_published, (json.dumps(document or self.private_document(), indent=1) + "\n").encode(), 0o600)

    def private_list(self):
        return release.read_non_published_list(self.non_published.read_bytes())

    def withhold(self):
        """Add members that only a private list names: to the Linux donor source tree and to every source
        snapshot of every target, between the published ones. Later builds of this test pass that list."""
        target = "linux-x86_64"
        self.withheld_tree = {name: ("withheld tree member: " + name + "\n").encode() for name in WITHHELD}
        # One member differs between the donor tree and the snapshots.
        self.withheld_snapshot = {**self.withheld_tree, WITHHELD[0]: b"withheld snapshot member: other bytes\n"}
        for name, data in self.withheld_tree.items():
            self.donor_files[target]["provenance/source/" + name] = (data, 0o644)
        self.repin_donor(target)
        self.private_trees[SOURCE_TREE] = {name: sha(data) for name, data in self.withheld_tree.items()}
        order = (WITHHELD[0], PUBLISHED[0], PUBLISHED[1], WITHHELD[1], PUBLISHED[2], PUBLISHED[3], WITHHELD[2])
        data = {**self.published_data, **self.withheld_snapshot}
        for name, donor in self.python_kits.items():
            for member in donor.get("sources", {}):
                if member.endswith("/source.tar.gz"):
                    prefix = SNAPSHOTS.get(member, "")
                    self.snapshot_rows[member] = [
                        (prefix + path, data[path] + (b"# " + member.encode() + b"\n" if path == PUBLISHED[0] else b""))
                        for path in order]
                    self.private_trees[member] = {prefix + path: sha(self.withheld_snapshot[path]) for path in WITHHELD}
                    self.repin_snapshot(member, target=name)
        self.write_non_published()

    def collect(self, target):
        return release.collect(self.root, target, non_published=None if self.non_published is None else self.private_list())

    def repin_snapshot(self, member, rows=None, *, raw=None, target="linux-x86_64", **form):
        """Write one source snapshot input (its fixture rows by default) and pin it."""
        path = self.python_kits[target]["sources"][member][0]
        plain = None
        if raw is None:
            raw, plain = source_snapshot(self.snapshot_rows[member] if rows is None else rows, **form)
        write(self.root / path, raw)
        self.python_kits[target]["sources"][member] = (path, sha(raw))
        if rows is None and not form and plain is not None:
            self.snapshot_tars[member] = plain
        return raw

    def shipped(self, result):
        """Every regular file of a produced library archive, by its name inside the library."""
        prefix = Path(result["archive"]).name.removesuffix(".tar.gz") + "/"
        with tarfile.open(result["archive"]) as archive:
            return {item.name.removeprefix(prefix): archive.extractfile(item).read()
                    for item in archive.getmembers() if item.isfile()}

    def bundle(self, target="macos-arm64"):
        return self.root / self.targets[target]["bundle"]

    def package(self, name="output", target="macos-arm64", root=None):
        return release.package(root or self.root, target, self.base / name, non_published_list=self.non_published,
                               proof_only=self.non_published is None)

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
            input_selection=path, input_selection_sha256=sha(raw), non_published_list=self.non_published,
            proof_only=self.non_published is None)

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
                        if target == "linux-x86_64":
                            raw += (DISCLOSURE + "\n").encode()
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
        self.python_kits[target]["sources"][destination] = (source, sha(raw))
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

        def collect_then_change(root, target, **options):
            files = original(root, target, **options)
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

    def expected_record(self, files):
        """The shipped record: one sentence and each shipped source snapshot, nothing about what was left out."""
        return {"schema_version": 2, "note": NOTE,
                "snapshots": [{"member": member, "sha256": sha(files[member]), "bytes": len(files[member])}
                              for member in sorted(files) if member.endswith("/source.tar.gz")]}

    def refused_with(self, target="linux-x86_64", output="refused-unlisted", **options):
        with self.assertRaises(release.PackageError) as refused:
            release.package(self.root, target, self.base / output, **options)
        self.assertFalse((self.base / output).exists())
        return refused.exception

    def command(self, *options):
        """The packager's command line on the fixture checkout: (exit code, standard output, standard error)."""
        out, err = io.StringIO(), io.StringIO()
        argv = ["package-library-release.py", *map(str, options)]
        with patch.object(release, "ROOT", self.root), patch.object(sys, "argv", argv), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = release.main()
            except SystemExit as stop:
                code = stop.code
        return code, out.getvalue(), err.getvalue()

    def test_the_packager_names_no_member_it_leaves_out(self):
        # The packager holds no table of members: what is left out comes from the two lists alone. Its
        # provenance constants are this closed set, so another one does not arrive unseen.
        self.assertEqual(sorted(name for name in vars(release) if name.startswith("PROVENANCE_")),
                         ["PROVENANCE_DISCLOSURE", "PROVENANCE_DISCLOSURE_TARGETS", "PROVENANCE_NOTE",
                          "PROVENANCE_RECORD"])
        # The committed list of this checkout is in the stated form.
        self.assertEqual(release.PUBLISHED_LIST, PUBLISHED_LIST)
        committed = (release.ROOT / PUBLISHED_LIST).read_bytes()
        self.assertEqual(set(json.loads(committed)), {"schema", "trees"})
        self.assertEqual(json.loads(committed)["schema"], LIST_SCHEMA)
        self.assertIsInstance(release.read_source_member_lists(committed), dict)
        # Every source snapshot either target ships is one the rule covers, and so is the donor source tree.
        archives = sorted(name for names in self.pinned_sources.values() for name in names if "source.tar" in name)
        self.assertTrue(set(SNAPSHOTS) <= set(archives))
        for name in archives:
            self.assertIsNotNone(release.SOURCE_SNAPSHOT.fullmatch(name), name)
        self.assertEqual(release.SOURCE_TREE, SOURCE_TREE)
        self.assertIn("provenance/source/", self.python_kits["linux-x86_64"]["provenance_trees"])

    def test_every_target_publishes_only_the_listed_source_members_and_ships_the_record(self):
        self.withhold()
        originals = [sha((self.root / donor["sources"][member][0]).read_bytes())
                     for donor in self.python_kits.values() for member in donor["sources"] if member.endswith("/source.tar.gz")]
        for target in self.targets:
            builds = (("pinned", lambda: self.package("only-pinned-" + target, target)),
                      ("explicit", lambda: self.explicit_package(self.explicit_inputs(target), name="only-explicit-" + target)))
            for label, build in builds:
                with self.subTest(target=target, inputs=label):
                    files = self.shipped(build())
                    snapshots = [member for member in files if member.endswith("/source.tar.gz")]
                    self.assertTrue(snapshots)
                    for member in snapshots:
                        names = [name for name, _ in member_blocks(gzip.decompress(files[member]))]
                        self.assertEqual(names, [SNAPSHOTS.get(member, "") + name for name in PUBLISHED])
                    if target == "linux-x86_64":
                        self.assertEqual([name for name in WITHHELD if SOURCE_TREE + name in files], [])
                        self.assertEqual(files[SOURCE_TREE + PUBLISHED[2]], b"retained tree source\n")
                        self.assertEqual(files[SOURCE_TREE + "retained.txt"], b"retained original evidence")
                        selection = json.loads(files["selected/provenance/selection.json"])
                        self.assertEqual([name for name in selection["files"]
                                          if name.removeprefix("provenance/source/") in WITHHELD], [])
                        self.assertIn("provenance/source/" + PUBLISHED[2], selection["files"])
                    # No withheld member's bytes remain anywhere, inside a snapshot or outside one.
                    payload = [gzip.decompress(data) if name in snapshots else data for name, data in files.items()]
                    for data in (*self.withheld_tree.values(), *self.withheld_snapshot.values()):
                        self.assertFalse(any(data in item for item in payload))
                    record = files[RECORD].decode()
                    self.assertEqual(json.loads(record), self.expected_record(files))
                    # The record names no member that was left out, no digest of one, and no original input.
                    for text in (*WITHHELD, *originals, self.python_kits[target]["sha256"],
                                 *(sha(data) for data in (*self.withheld_tree.values(), *self.withheld_snapshot.values()))):
                        self.assertNotIn(text, record)
                    self.assertFalse(any(key in record for key in ("omitted", "original", "donor", "reason")))

    def test_a_snapshot_with_nothing_left_out_ships_as_it_is(self):
        for target, donor in self.python_kits.items():
            files = release.collect(self.root, target)
            snapshots = [member for member in donor["sources"] if member.endswith("/source.tar.gz")]
            self.assertTrue(snapshots)
            for member in snapshots:
                self.assertEqual(files[member], ((self.root / donor["sources"][member][0]).read_bytes(), 0o644))
            self.assertEqual(json.loads(files[RECORD][0]), self.expected_record({name: data for name, (data, _) in files.items()}))

    def test_snapshot_projection_keeps_every_published_member_block(self):
        self.withhold()
        for target in self.targets:
            files = self.collect(target)
            for member in (name for name in files if name.endswith("/source.tar.gz")):
                with self.subTest(target=target, snapshot=member):
                    original = self.snapshot_tars[member]
                    data, mode = files[member]
                    self.assertEqual(mode, 0o644)
                    self.assertEqual(data[4:8], b"\x00" * 4)  # No build time in the gzip header.
                    derived = gzip.decompress(data)
                    withheld = {SNAPSHOTS.get(member, "") + name for name in WITHHELD}
                    before = member_blocks(original)
                    kept = [(name, block) for name, block in before if name not in withheld]
                    self.assertEqual(len(before) - len(kept), len(WITHHELD))
                    # Header, data and padding of every published member, in the original order.
                    self.assertEqual(member_blocks(derived), kept)
                    end = sum(len(block) for _, block in before)
                    self.assertEqual(derived, b"".join(block for _, block in kept) + original[end:])
            self.assertEqual(self.collect(target), files)

    def test_a_source_member_in_neither_list_refuses_the_build(self):
        target = "linux-x86_64"
        self.withhold()
        # Without the private list, the members only it names are in neither list.
        error = self.refused_with(target, proof_only=True)
        self.assertEqual(str(error), "unlisted_provenance_member")
        self.assertEqual(error.unlisted, {tree: sorted(names) for tree, names in self.private_trees.items()
                                          if tree == SOURCE_TREE or tree in SNAPSHOTS})
        # A member neither list names, in the donor tree and in one snapshot.
        self.donor_files[target]["provenance/source/" + UNLISTED] = (b"added source\n", 0o644)
        self.repin_donor(target)
        member = "selected/provenance/compose-migration/source.tar.gz"
        self.repin_snapshot(member, [*self.snapshot_rows[member], ("source/" + UNLISTED, b"added source\n")])
        error = self.refused_with(target, non_published_list=self.non_published)
        self.assertEqual(str(error), "unlisted_provenance_member")
        self.assertEqual(error.unlisted, {SOURCE_TREE: [UNLISTED], member: ["source/" + UNLISTED]})
        # The command line names the tree and the count; the member's name only on request.
        arguments = ("--target", target, "--output", self.base / "refused-cli", "--non-published-list", self.non_published)
        code, out, err = self.command(*arguments)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("unlisted_provenance_member", err)
        self.assertIn(SOURCE_TREE + ": 1 ", err)
        self.assertIn(member + ": 1 ", err)
        self.assertNotIn(UNLISTED, err)
        code, out, err = self.command(*arguments, "--show-names")
        self.assertEqual((code, out), (1, ""))
        self.assertEqual(err.count(UNLISTED), 2)
        self.assertFalse((self.base / "refused-cli").exists())
        # Named in the private list, both are left out and the build succeeds.
        self.private_trees[SOURCE_TREE][UNLISTED] = sha(b"added source\n")
        self.private_trees[member]["source/" + UNLISTED] = sha(b"added source\n")
        self.write_non_published()
        files = self.shipped(self.package("unlisted-named", target))
        self.assertNotIn(SOURCE_TREE + UNLISTED, files)
        self.assertNotIn(b"added source\n", gzip.decompress(files[member]))

    def test_source_member_lists_must_be_whole_sorted_and_disjoint(self):
        target = "linux-x86_64"
        path = self.root / PUBLISHED_LIST
        good = json.loads(path.read_bytes())
        tree = "selected/provenance/linux-mcpb/source.tar.gz"

        def trees(**changes):
            return {**good, "trees": {**good["trees"], **changes}}

        cases = {"not-json": b"{", "other-schema": {**good, "schema": "another/1"}, "extra-key": {**good, "note": "x"},
                 "no-trees": {"schema": good["schema"]}, "unknown-tree": trees(**{"selected/other/": []}),
                 "nested-snapshot": trees(**{"selected/provenance/a/b/source.tar.gz": []}),
                 "unsorted": trees(**{tree: sorted(good["trees"][tree], reverse=True)}),
                 "repeated": trees(**{tree: [good["trees"][tree][0]] * 2}),
                 "unsafe-name": trees(**{tree: ["../outside"]}), "not-a-list": trees(**{tree: "crates"}),
                 "not-a-name": trees(**{tree: [7]})}
        for label, value in cases.items():
            with self.subTest(committed=label):
                path.write_bytes(value if isinstance(value, bytes) else json.dumps(value).encode())
                self.assert_refused("invalid_source_member_list", target=target, output="list-" + label)
        path.unlink()
        with self.assertRaises(OSError):
            self.package("list-absent", target)
        self.assertFalse((self.base / "list-absent").exists())
        # A tree the committed list has no section for refuses; an empty section publishes nothing of it.
        del good["trees"][tree]
        path.write_bytes(json.dumps(good).encode())
        self.assert_refused("provenance_tree_not_listed", target=target, output="list-no-section")
        good["trees"][tree] = []
        path.write_bytes(json.dumps(good).encode())
        self.assertEqual(self.refused_with(target, output="list-empty-section", proof_only=True).unlisted,
                         {tree: list(PUBLISHED)})
        self.write_published()
        self.package("list-restored", target)
        # The private list has the same form, and a name in both lists refuses.
        self.withhold()
        private = self.non_published
        good = self.private_document()
        entry = good["trees"][SOURCE_TREE][0]

        def private_trees(rows):
            return {**good, "trees": {**good["trees"], SOURCE_TREE: rows}}

        cases = {"not-json": b"{", "published-form": {"schema": LIST_SCHEMA, "trees": {SOURCE_TREE: list(WITHHELD)}},
                 "no-snapshots": {key: value for key, value in good.items() if key != "original_snapshots"},
                 "names-only": private_trees(list(WITHHELD)), "no-digest": private_trees([{"name": entry["name"]}]),
                 "short-digest": private_trees([{**entry, "sha256": "ab"}]),
                 "upper-digest": private_trees([{**entry, "sha256": entry["sha256"].upper()}]),
                 "extra-key": private_trees([{**entry, "bytes": 3}]),
                 "unsorted": private_trees(list(reversed(good["trees"][SOURCE_TREE]))),
                 "repeated": private_trees([entry, entry]),
                 "snapshot-not-a-list": {**good, "original_snapshots": {tree: "0" * 64}},
                 "snapshot-of-no-tree": {**good, "original_snapshots": {"selected/other.tar.gz": ["0" * 64]}}}
        for label, value in cases.items():
            with self.subTest(private=label):
                private.write_bytes(value if isinstance(value, bytes) else json.dumps(value).encode())
                self.assert_refused("invalid_source_member_list", target=target, output="private-" + label)
        # A snapshot that loses members must be one whose original digest the list holds.
        self.write_non_published({**good, "original_snapshots": {
            member: pins for member, pins in good["original_snapshots"].items() if member != tree}})
        self.assert_refused("original_snapshot_not_listed", target=target, output="private-no-original")
        self.private_trees[SOURCE_TREE][PUBLISHED[2]] = "0" * 64
        self.write_non_published()
        self.assert_refused("provenance_lists_conflict", target=target, output="private-conflict")
        self.non_published = self.base / "absent-private-list.json"
        with self.assertRaises(OSError):
            self.package("private-absent", target)

    def add_source(self, name, data, target="linux-x86_64"):
        """One more pinned source of the target, shipped under name."""
        path = "retained-inputs/" + name.removeprefix("selected/")
        write(self.root / path, data)
        self.python_kits[target].setdefault("sources", {})[name] = (path, sha(data))

    def reference_fixture(self):
        """Linux records that name withheld members by path or by digest, and published members that state
        the digests of those records and the digest and size of a source snapshot."""
        target = "linux-x86_64"
        self.withhold()
        withheld = {name: sha(data) for name, data in self.withheld_tree.items()}
        kept = sha(b"retained tree source\n")
        # By path and by digest: rows of a list, a list of names, a map keyed by path.
        listing = (json.dumps({"schema": 1, "files": [
            {"path": PUBLISHED[2], "sha256": kept},
            {"path": WITHHELD[1], "sha256": withheld[WITHHELD[1]], "bytes": 3}],
            "names": [PUBLISHED[0], WITHHELD[0], PUBLISHED[1]],
            "by_path": {PUBLISHED[0]: {"bytes": 10}, WITHHELD[2]: {"bytes": 7}}}, indent=2) + "\n").encode()
        self.donor_files[target]["provenance/dependencies.json"] = (listing, 0o644)
        self.repin_donor(target)
        # By digest only, on one line, in upper case.
        manifest = json.dumps({"members": [{"id": 1, "sha256": withheld[WITHHELD[0]].upper()},
                                           {"id": 2, "sha256": kept}]}).encode()
        # A record that states the digests of both records and the digest and size of a source snapshot, and
        # a text file that states the digest of that record and of the snapshot.
        self.snapshot = "selected/provenance/linux-mcpb/source.tar.gz"
        path, self.snapshot_input_sha = self.python_kits[target]["sources"][self.snapshot]
        self.snapshot_input_bytes = len((self.root / path).read_bytes())
        index = json.dumps({"records": {"dependencies": sha(listing), "manifest": sha(manifest)},
                            "snapshot": {"sha256": self.snapshot_input_sha, "bytes": self.snapshot_input_bytes,
                                         "members": 7}, "bytes": self.snapshot_input_bytes}, indent=1).encode()
        summary = ("index " + sha(index) + "\nsnapshot " + self.snapshot_input_sha.upper() + "\n").encode()
        self.listing_name = "selected/provenance/dependencies.json"
        self.manifest_name = "selected/provenance/linux-mcpb/source-manifest.json"
        self.index_name = "selected/provenance/linux-mcpb/index.json"
        self.summary_name = "selected/provenance/linux-mcpb/summary.txt"
        self.record_inputs = {self.listing_name: listing, self.manifest_name: manifest, self.index_name: index,
                              self.summary_name: summary}
        for name in (self.manifest_name, self.index_name, self.summary_name):
            self.add_source(name, self.record_inputs[name])
        self.kept_digest = kept

    def needles(self):
        names = {name for names in self.private_trees.values() for name in names}
        digests = {value for names in self.private_trees.values() for value in names.values()}
        return [text.encode() for text in (*names, *digests, *(value.upper() for value in digests))]

    def test_published_records_that_name_a_member_left_out_are_projected_and_restated(self):
        target = "linux-x86_64"
        self.reference_fixture()
        report = {}
        files = {name: data for name, (data, _) in release.collect(
            self.root, target, non_published=self.private_list(), report=report).items()}
        # No published member names a member that was left out, by path or by digest.
        for name, data in files.items():
            for needle in self.needles():
                self.assertNotIn(needle, data, name)
                if name.endswith("/source.tar.gz"):
                    self.assertNotIn(needle, gzip.decompress(data), name)
        # The rows that named one are gone, and each projected record says that it was projected.
        self.assertEqual(json.loads(files[self.listing_name]), {
            "schema": 1, "files": [{"path": PUBLISHED[2], "sha256": self.kept_digest}],
            "names": [PUBLISHED[0], PUBLISHED[1]], "by_path": {PUBLISHED[0]: {"bytes": 10}},
            "projected_for_publication": True})
        self.assertEqual(json.loads(files[self.manifest_name]), {
            "members": [{"id": 2, "sha256": self.kept_digest}], "projected_for_publication": True})
        # The rows were cut out of the original text: every other line is an original line.
        original = {line.rstrip(",") for line in self.record_inputs[self.listing_name].decode().splitlines()}
        for line in files[self.listing_name].decode().splitlines():
            self.assertTrue(line.rstrip(",") in original or "projected_for_publication" in line, line)
        # Every digest a published member states about a projected member is the shipped one, and so is the
        # snapshot's size where the same map states its digest. Any other number stays.
        shipped = files[self.snapshot]
        self.assertNotEqual(len(shipped), self.snapshot_input_bytes)
        self.assertEqual(json.loads(files[self.index_name]), {
            "records": {"dependencies": sha(files[self.listing_name]), "manifest": sha(files[self.manifest_name])},
            "snapshot": {"sha256": sha(shipped), "bytes": len(shipped), "members": 7},
            "bytes": self.snapshot_input_bytes})
        self.assertEqual(files[self.summary_name], ("index " + sha(files[self.index_name]) + "\nsnapshot "
                                                    + sha(shipped).upper() + "\n").encode())
        selection = json.loads(files["selected/provenance/selection.json"])
        self.assertEqual(selection["files"]["provenance/dependencies.json"], sha(files[self.listing_name]))
        self.assertIn({"member": self.snapshot, "sha256": sha(shipped), "bytes": len(shipped)},
                      json.loads(files[RECORD])["snapshots"])
        # No digest of an input that was projected stays anywhere.
        stale = [sha(data) for data in self.record_inputs.values()] + [self.snapshot_input_sha]
        for name, data in files.items():
            for value in stale:
                self.assertNotIn(value.encode(), data.lower(), name)
        # The build result says what was projected, which member was restated for which, and where to look.
        self.assertEqual(report["projected_records"], sorted([self.listing_name, self.manifest_name]))
        self.assertEqual(report["restated"][self.index_name], sorted([self.listing_name, self.manifest_name, self.snapshot]))
        self.assertEqual(report["restated"][self.summary_name], sorted([self.index_name, self.snapshot]))
        self.assertEqual(report["restated"]["selected/provenance/selection.json"], [self.listing_name])
        self.assertEqual(report["sizes_restated"], {self.index_name: ["bytes"]})
        self.assertEqual(report["integer_fields"], {self.listing_name: ["schema"], self.manifest_name: [],
                                                    self.index_name: ["bytes", "members"]})
        self.assertNotIn(str(self.snapshot_input_bytes), json.dumps(report))
        # A second build of the same inputs is the same library, and its result carries the same report.
        first = self.package("references-1", target)
        self.assertEqual({key: first[key] for key in report}, report)
        self.assertEqual((first["proof_only"], first["reference_check"]), (False, "passed"))
        second = self.package("references-2", target)
        self.assertEqual(Path(second["archive"]).read_bytes(), Path(first["archive"]).read_bytes())

    def test_a_published_member_that_names_a_member_left_out_refuses_the_build(self):
        target = "linux-x86_64"
        self.reference_fixture()
        lists = self.private_list()
        # Without the projection the records name withheld members: the check refuses with paths and counts.
        with patch.object(release, "project_record", side_effect=lambda raw, names, digests: raw), \
                self.assertRaises(release.PackageError) as refused:
            release.collect(self.root, target, non_published=lists)
        self.assertEqual(str(refused.exception), "published_member_names_non_published")
        self.assertEqual(refused.exception.members, {self.listing_name: 4, self.manifest_name: 1})
        # A text file cannot be projected: it refuses, and the command line shows its path and count only.
        log = "selected/provenance/linux-mcpb/build.log"
        self.add_source(log, ("compiled " + WITHHELD[2] + "\nread " + WITHHELD[2] + " again\n").encode())
        with self.assertRaises(release.PackageError) as refused:
            release.collect(self.root, target, non_published=lists)
        self.assertEqual((str(refused.exception), refused.exception.members),
                         ("published_member_names_non_published", {log: 2}))
        code, out, err = self.command("--target", target, "--output", self.base / "refused-reference",
                                      "--non-published-list", self.non_published, "--show-names")
        self.assertEqual((code, out), (1, ""))
        self.assertIn("published_member_names_non_published", err)
        self.assertIn(log + ": 2 ", err)
        for name in WITHHELD:
            self.assertNotIn(name, err)
        self.assertFalse((self.base / "refused-reference").exists())
        del self.python_kits[target]["sources"][log]
        # An original snapshot digest that restating cannot reach (mixed case here) refuses as well.
        mixed = "".join(c.upper() if n % 2 else c for n, c in enumerate(self.snapshot_input_sha))
        self.add_source(log, ("snapshot " + mixed + "\n").encode())
        with self.assertRaises(release.PackageError) as refused:
            release.collect(self.root, target, non_published=lists)
        self.assertEqual((str(refused.exception), refused.exception.members),
                         ("published_member_names_non_published", {log: 1}))
        del self.python_kits[target]["sources"][log]
        # A record whose reference is a field of the record itself, or that is no JSON object, refuses by name.
        cases = {"field-of-the-record": json.dumps({"kind": "entry", "entry_point": WITHHELD[0]}).encode(),
                 "not-an-object": json.dumps(["kept", WITHHELD[0]]).encode(),
                 "not-json": b"{ " + WITHHELD[0].encode(),
                 "only-rows": json.dumps({WITHHELD[0]: 1}).encode(),
                 "field-taken": json.dumps({"projected_for_publication": False, "names": [WITHHELD[0]]}).encode()}
        entry = "selected/provenance/linux-mcpb/entry.json"
        for label, raw in cases.items():
            with self.subTest(record=label):
                self.add_source(entry, raw)
                with self.assertRaises(release.PackageError) as refused:
                    release.collect(self.root, target, non_published=lists)
                self.assertEqual((str(refused.exception), refused.exception.members),
                                 ("provenance_record_not_projectable", {entry: 1}))
        del self.python_kits[target]["sources"][entry]
        # Restating stops at its bound instead of running on.
        with patch.object(release, "RESTATE_LIMIT", 1):
            self.assert_refused("restatement_not_settled", target=target, output="references-bound")
        self.package("references-restored", target)

    def test_restating_refuses_a_chain_that_returns_to_an_earlier_state(self):
        first, second, third = "a" * 64, "b" * 64, "f" * 64
        files = {"one.txt": (b"states " + first.encode(), 0o644)}
        # The member's digests as the two restatements see them: c -> d, then d -> c again.
        states = iter(["c" * 64, "d" * 64, "d" * 64, "c" * 64])
        with patch.object(release, "digest", side_effect=lambda data: next(states)), \
                self.assertRaisesRegex(release.PackageError, "^restatement_not_settled$"):
            release.restate_digests(files, [("x", first, second, None), ("y", second, third, None)])

    def test_each_branch_of_the_row_rule_cuts_the_row_from_the_original_text(self):
        names, digests = frozenset({"a/withheld.txt"}), frozenset({"1" * 64})
        field = '"projected_for_publication": true'
        cases = {
            # a string that is a list item: that item
            "list-string": ('{\n  "files": [\n    "a/kept.txt",\n    "a/withheld.txt"\n  ]\n}\n',
                            '{\n  "files": [\n    "a/kept.txt"\n  ],\n  ' + field + '\n}\n'),
            "list-string-first": ('{"files": ["a/withheld.txt", "a/kept.txt"], "n": 1}',
                                  '{"files": ["a/kept.txt"], "n": 1, ' + field + '}'),
            "list-string-middle": ('{"files": ["a/kept.txt", "a/withheld.txt", "b"], "n": 1}',
                                   '{"files": ["a/kept.txt", "b"], "n": 1, ' + field + '}'),
            "list-string-only": ('{"files": ["a/withheld.txt"], "n": 1}', '{"files": [], "n": 1, ' + field + '}'),
            # a map key: that entry
            "map-key": ('{"by": {"a/withheld.txt": 1, "a/kept.txt": 2}}', '{"by": {"a/kept.txt": 2}, ' + field + '}'),
            # a string value of a map that is itself a list item: the list item, by path or by digest
            "map-in-list-row": ('{"rows":[{"p":"k","d":"%s"},{"p":"k2","d":"%s"}]}' % ("1" * 64, "2" * 64),
                                '{"rows":[{"p":"k2","d":"%s"}],%s}' % ("2" * 64, field.replace(": ", ":"))),
            "map-in-list-row-path": ('{"rows": [{"p": "k"}, {"p": "a/withheld.txt", "n": 2}]}',
                                     '{"rows": [{"p": "k"}], ' + field + '}'),
            # a string value of any other nested map: that map entry
            "nested-map-entry": ('{"a": {"b": {"path": "x/a/withheld.txt", "n": 1}}}', '{"a": {"b": {"n": 1}}, ' + field + '}'),
            # everything else stays byte for byte: escapes, number forms, white space
            "other-bytes-stay": ('{"files": ["a\\/withheld.txt", "caf\\u00e9", 1.50, null]}',
                                 '{"files": ["caf\\u00e9", 1.50, null], ' + field + '}'),
            "tabs": ('{\n\t"keep": {"x": [1, 2]},\n\t"files": [\n\t\t"a/withheld.txt"\n\t]\n}',
                     '{\n\t"keep": {"x": [1, 2]},\n\t"files": [],\n\t' + field + '\n}'),
        }
        for label, (original, expected) in cases.items():
            with self.subTest(branch=label):
                projected = release.project_record(original.encode(), names, digests).decode()
                self.assertEqual(projected, expected)
                json.loads(projected)
        untouched = b'{"files": ["a/kept.txt"], "sha256": "' + b"2" * 64 + b'"}'
        self.assertEqual(release.project_record(untouched, names, digests), untouched)
        refusals = {
            # a string value of the document's own top-level map
            "top-level-field": '{"entry": "a/withheld.txt"}',
            "top-level-digest": '{"sha256": "%s", "n": [1]}' % ("1" * 64),
            # a document that is not an object
            "not-an-object": '["a/withheld.txt", "k"]',
            "a-string": '"a/withheld.txt"',
            "not-json": 'not json a/withheld.txt',
            "nothing-remains": '{"a/withheld.txt": 1}',
            "field-taken": '{"projected_for_publication": 1, "files": ["a/withheld.txt"]}',
        }
        for label, refused in refusals.items():
            with self.subTest(refused=label), \
                    self.assertRaisesRegex(release.PackageError, "^provenance_record_not_projectable$"):
                release.project_record(refused.encode(), names, digests)

    def test_a_path_matches_at_its_boundaries_and_a_digest_anywhere(self):
        digest = "1" * 63 + "a"
        count = lambda data: release.count_references(data, frozenset({"a/withheld.txt", "TOPLEVEL.md"}), frozenset({digest}))
        for data, expected in ((b"see a/withheld.txt.", 1), (b"dir/a/withheld.txt and a/withheld.txt", 2),
                               (b'"a\\/withheld.txt"', 1), (b"A/Withheld.TXT", 1),
                               # another path that only holds the listed one
                               (b"a/withheld.txt.bak", 0), (b"xa/withheld.txt", 0), (b"a/withheld.txt/child", 0),
                               (b"a/withheld.txtx", 0), (b"a/withheld-txt", 0),
                               # a name without a separator: a whole JSON string or a whole line only
                               (b'{"file": "TOPLEVEL.md"}', 1), (b"first\n  TOPLEVEL.md\nlast\n", 1),
                               (b"see TOPLEVEL.md here", 0), (b"x/TOPLEVEL.md", 0), (b'"TOPLEVEL.md.bak"', 0),
                               # a digest: anywhere, in either case
                               (b"xx" + digest.encode() + b"yy", 1), (digest.upper().encode(), 1),
                               (digest[:-1].encode(), 0)):
            with self.subTest(data=data):
                self.assertEqual(count(data), expected)
        self.assertEqual(release.count_references(b"anything", frozenset(), frozenset()), 0)

    def git(self, *arguments, directory=None):
        """git in the directory around the fixture checkout (or another one), with a made-up author."""
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        done = subprocess.run(["git", "-C", str(directory or self.base), "-c", "user.name=test",
                               "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
                               "-c", "core.hooksPath=/dev/null", *arguments],
                              capture_output=True, text=True, env=environment)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout

    def test_a_path_the_checkouts_commit_tracks_is_no_reference_to_a_member_left_out(self):
        target = "linux-x86_64"
        self.withhold()
        lists = self.private_list()
        public, other = WITHHELD[1], WITHHELD[2]
        digest = self.private_trees[SOURCE_TREE][public]
        # A published member names, as a map key and as a string value, a path that a member left out has too.
        member = "selected/provenance/linux-mcpb/contents.txt"
        text = json.dumps({"files": {public: {"kind": "catalog"}}, "catalog": public}, indent=1) + "\n"
        self.add_source(member, text.encode())

        def refused(expected):
            with self.assertRaises(release.PackageError) as error:
                release.collect(self.root, target, non_published=lists)
            self.assertEqual((str(error.exception), error.exception.members),
                             ("published_member_names_non_published", expected))

        # The fixture checkout is in no repository: nothing is exempt.
        self.assertEqual(release.tracked_paths(self.root), frozenset())
        refused({member: 2})
        # The file is in the checkout, and staged, but no commit holds it. The checkout is a directory of
        # the repository, as a product root is.
        inside = self.root.name + "/" + public
        write(self.root / public, b"the checkout's own public file\n")
        self.git("init", "-q")
        self.git("add", "--", inside)
        self.assertEqual(release.tracked_paths(self.root), frozenset())
        refused({member: 2})
        # The checkout's commit tracks exactly that path: both references are exempt, and counted.
        self.git("commit", "-q", "-m", "a public file")
        self.assertEqual(release.tracked_paths(self.root), frozenset({public}))
        report = {}
        files = release.collect(self.root, target, non_published=lists, report=report)
        self.assertEqual(files[member][0], text.encode())
        self.assertEqual(report["exempt_references"], {"public_repository_path": 2})
        self.assertEqual(report["exempt_reference_members"], {member: 2})
        result = self.package("exempt", target)
        self.assertEqual((result["reference_check"], result["exempt_references"]),
                         ("passed", {"public_repository_path": 2}))
        self.assertIn(text.encode(), self.shipped(result)[member])
        # The member's digest is never exempt: in the same file it refuses, and it alone is counted.
        for stated in (digest, digest.upper()):
            self.add_source(member, (text + "sha256 " + stated + "\n").encode())
            refused({member: 1})
        # Another member left out, whose path no commit tracks, refuses; so do the exempt path in another
        # case and the name the same member has in a snapshot.
        for line in (other, public.upper(), "source/" + public):
            self.add_source(member, (text + line + "\n").encode())
            refused({member: 1})
        # Matching itself: a path is exempt only as the exact tracked path, a digest never.
        references = release.References(frozenset({public, "TOPLEVEL.md"}), frozenset({digest}),
                                        frozenset({public, "TOPLEVEL.md", digest, "crates/unlisted.rs"}))
        data = ('"TOPLEVEL.md" ' + public + " " + digest + " crates/unlisted.rs " + public.title()).encode()
        self.assertEqual(references.split(data), (2, 2))
        self.assertEqual(references.count(data), 2)
        self.assertEqual(release.References(frozenset({public, "TOPLEVEL.md"}), frozenset({digest})).split(data), (4, 0))
        # A JSON provenance record is projected as before: its row for that path is cut, exempt or not.
        record = "selected/provenance/linux-mcpb/files.json"
        self.add_source(member, text.encode())
        self.add_source(record, json.dumps({"files": [{"path": public}, {"path": PUBLISHED[0]}]}).encode())
        report = {}
        files = release.collect(self.root, target, non_published=lists, report=report)
        self.assertEqual(json.loads(files[record][0]), {"files": [{"path": PUBLISHED[0]}], "projected_for_publication": True})
        self.assertEqual((report["projected_records"], report["exempt_references"]),
                         ([record], {"public_repository_path": 2}))
        del self.python_kits[target]["sources"][record]
        # Another repository that the environment names is not the checkout's: one that tracks the path
        # makes nothing exempt once the checkout's own commit does not.
        second = self.base / "second-repository"
        write(second / public, b"another repository's file\n")
        self.git("init", "-q", directory=second)
        self.git("add", "-A", directory=second)
        self.git("commit", "-q", "-m", "tracks the path", directory=second)
        # A path that only an earlier commit tracks is not exempt: the commit the checkout is at decides.
        self.git("rm", "-q", "--cached", "--", inside)
        self.git("commit", "-q", "-m", "the file is no longer tracked")
        self.assertTrue((self.root / public).is_file())
        self.assertIn(inside, self.git("ls-tree", "-r", "--name-only", "HEAD~1"))
        self.assertEqual(release.tracked_paths(self.root), frozenset())
        refused({member: 2})
        with patch.dict(os.environ, {"GIT_DIR": str(second / ".git")}):
            self.assertEqual(release.tracked_paths(self.root), frozenset())
            refused({member: 2})
        # Nor is one that only another branch tracks.
        self.git("branch", "other", "HEAD~1")
        refused({member: 2})
        self.assert_refused("published_member_names_non_published", target=target, output="not-tracked")

    def test_a_replace_ref_makes_no_path_a_tracked_one(self):
        # git can be told to read another object in place of one (a replace ref). The packager asks
        # for the commit's own objects: a path that only a replacing commit tracks exempts nothing.
        target = "linux-x86_64"
        self.withhold()
        lists = self.private_list()
        public = WITHHELD[1]
        member = "selected/provenance/linux-mcpb/contents.txt"
        self.add_source(member, (json.dumps({"files": {public: {"kind": "catalog"}}, "catalog": public}, indent=1) + "\n").encode())

        def refused(expected):
            with self.assertRaises(release.PackageError) as error:
                release.collect(self.root, target, non_published=lists)
            self.assertEqual((str(error.exception), error.exception.members),
                             ("published_member_names_non_published", expected))

        write(self.root / "kept.txt", b"a file the commit tracks\n")
        self.git("init", "-q")
        self.git("add", "--", self.root.name + "/kept.txt")
        self.git("commit", "-q", "-m", "one public file")
        head = self.git("rev-parse", "HEAD").strip()
        self.assertEqual(release.tracked_paths(self.root), frozenset({"kept.txt"}))
        refused({member: 2})
        inside = self.root.name + "/" + public
        write(self.root / public, b"a file no commit of the checkout tracks\n")
        self.git("add", "--", inside)
        other = self.git("commit-tree", self.git("write-tree").strip(), "-m", "tracks the path").strip()
        self.git("replace", head, other)
        # Asked without more, git now lists the path as the checkout's commit's.
        self.assertEqual(self.git("rev-parse", "HEAD").strip(), head)
        self.assertIn(inside, self.git("ls-tree", "-r", "--name-only", "HEAD").splitlines())
        self.assertEqual(release.tracked_paths(self.root), frozenset({"kept.txt"}))
        refused({member: 2})

    def test_only_a_regular_tracked_file_makes_its_path_a_tracked_one(self):
        # What exempts a path is a file of the commit. A link or another repository's commit that the
        # checkout's commit holds at that path is no file with that path's content: it exempts nothing.
        target = "linux-x86_64"
        self.withhold()
        lists = self.private_list()
        public = WITHHELD[1]
        inside = self.root.name + "/" + public
        member = "selected/provenance/linux-mcpb/contents.txt"
        text = json.dumps({"files": {public: {"kind": "catalog"}}, "catalog": public}, indent=1) + "\n"
        self.add_source(member, text.encode())

        def refused(expected):
            with self.assertRaises(release.PackageError) as error:
                release.collect(self.root, target, non_published=lists)
            self.assertEqual((str(error.exception), error.exception.members),
                             ("published_member_names_non_published", expected))

        def entry():
            """How the checkout's commit holds the path: the mode and kind of its tree entry."""
            rows = [row for row in self.git("ls-tree", "-r", "HEAD").splitlines() if row.endswith("\t" + inside)]
            self.assertEqual(len(rows), 1, rows)
            return " ".join(rows[0].split()[:2])

        write(self.root / "kept.txt", b"a file the commit tracks\n")
        self.git("init", "-q")
        self.git("add", "--", self.root.name + "/kept.txt")
        self.git("commit", "-q", "-m", "one public file")
        first = self.git("rev-parse", "HEAD").strip()
        # A link at the path, tracked as a link.
        (self.root / public).parent.mkdir(parents=True, exist_ok=True)
        (self.root / public).symlink_to("kept.txt")
        self.git("add", "--", inside)
        self.git("commit", "-q", "-m", "a link at the path")
        self.assertEqual(entry(), "120000 blob")
        self.assertEqual(release.tracked_paths(self.root), frozenset({"kept.txt"}))
        refused({member: 2})
        # A commit of another repository at the path (a submodule's entry).
        self.git("rm", "-q", "--cached", "--", inside)
        (self.root / public).unlink()
        self.git("update-index", "--add", "--cacheinfo", "160000," + first + "," + inside)
        self.git("commit", "-q", "-m", "a submodule at the path")
        self.assertEqual(entry(), "160000 commit")
        self.assertEqual(release.tracked_paths(self.root), frozenset({"kept.txt"}))
        refused({member: 2})
        # A regular file there, executable or not, is a tracked file: both references are exempt.
        self.git("rm", "-q", "--cached", "--", inside)
        for mode, expected in ((0o755, "100755 blob"), (0o644, "100644 blob")):
            write(self.root / public, b"the checkout's own public file\n")
            os.chmod(self.root / public, mode)
            self.git("add", "--", inside)
            self.git("commit", "-q", "-m", "a regular file at the path")
            self.assertEqual(entry(), expected)
            self.assertEqual(release.tracked_paths(self.root), frozenset({"kept.txt", public}))
            report = {}
            release.collect(self.root, target, non_published=lists, report=report)
            self.assertEqual(report["exempt_references"], {"public_repository_path": 2})

    def test_a_release_build_needs_the_private_list_and_a_proof_build_says_what_it_is(self):
        target = "linux-x86_64"
        error = self.refused_with(target, output="no-list")
        self.assertEqual(str(error), "non_published_list_required")
        result = release.package(self.root, target, self.base / "proof", proof_only=True)
        self.assertEqual((result["proof_only"], result["reference_check"]), (True, "not_run"))
        code, out, err = self.command("--target", target, "--output", self.base / "proof-cli", "--proof-only")
        self.assertEqual((code, json.loads(out)["proof_only"], json.loads(out)["reference_check"]), (0, True, "not_run"))
        code, out, err = self.command("--target", target, "--output", self.base / "release-cli")
        self.assertEqual((code, out), (1, ""))
        self.assertIn("non_published_list_required", err)
        self.assertFalse((self.base / "release-cli").exists())
        # With the list the check runs, and the result says so; the list cannot be combined with the override.
        self.withhold()
        result = release.package(self.root, target, self.base / "release", non_published_list=self.non_published)
        self.assertEqual((result["proof_only"], result["reference_check"]), (False, "passed"))
        self.assertEqual((result["projected_records"], result["restated"]), ([], {}))
        error = self.refused_with(target, output="both", non_published_list=self.non_published, proof_only=True)
        self.assertEqual(str(error), "invalid_proof_request")
        # A member the list names with another digest is not the member the list means.
        self.private_trees[SOURCE_TREE][WITHHELD[1]] = "0" * 64
        self.write_non_published()
        self.assert_refused("non_published_member_digest_mismatch", target=target, output="other-digest")

    def test_the_shipped_copy_of_a_submitted_selection_is_the_submission_after_restating(self):
        target = "linux-x86_64"
        self.reference_fixture()
        value = self.explicit_inputs(target)
        value["sources"][self.index_name] = {"path": self.python_kits[target]["sources"][self.index_name][0],
                                             "sha256": sha(self.record_inputs[self.index_name])}
        result = self.explicit_package(value, name="selection-copy")
        files = self.shipped(result)
        submitted = (self.base / "selection-copy-inputs.json").read_bytes()
        shipped = files["selected/provenance/current-input-selection.json"]
        # Equal after projection: the one digest that names a restated input is the shipped file's.
        self.assertNotEqual(shipped, submitted)
        self.assertEqual(shipped, submitted.replace(sha(self.record_inputs[self.index_name]).encode(),
                                                    sha(files[self.index_name]).encode()))
        # The submitted file's digest is in the build result, not in the library.
        self.assertEqual(result["input_selection"]["sha256"], sha(submitted))
        for name, data in files.items():
            self.assertNotIn(sha(submitted).encode(), data, name)

    def test_source_snapshot_outside_the_plain_ustar_form_is_refused(self):
        target = "linux-x86_64"
        member = "selected/provenance/linux-mcpb/source.tar.gz"
        rows = self.snapshot_rows[member]
        plain = self.snapshot_tars[member]
        end = sum(len(block) for _, block in member_blocks(plain))

        def directory(archive):
            item = tarfile.TarInfo("crates/sample/assets")
            item.type, item.mode = tarfile.DIRTYPE, 0o755
            archive.addfile(item)

        def extended(archive):
            item = tarfile.TarInfo("crates/sample/src/extended.rs")
            item.pax_headers = {"comment": "extended header"}
            archive.addfile(item)

        def repeated(archive):
            archive.addfile(tarfile.TarInfo(rows[1][0]))

        unsupported, invalid = "unsupported_provenance_member", "invalid_provenance_archive"
        cases = (("gnu-format", {"format": tarfile.GNU_FORMAT}, unsupported),
                 ("extended-header", {"format": tarfile.PAX_FORMAT, "extra": extended}, unsupported),
                 ("directory", {"extra": directory}, unsupported),
                 ("repeated-name", {"extra": repeated}, unsupported),
                 ("data-after-end", {"raw": gzip.compress(plain + b"x" * 512, mtime=0)}, invalid),
                 ("no-end-blocks", {"raw": gzip.compress(plain[:end], mtime=0)}, invalid),
                 ("not-a-tar", {"raw": gzip.compress(b"not a tar archive\n" * 64, mtime=0)}, invalid),
                 ("not-gzip", {"raw": plain}, invalid),
                 ("cut-gzip", {"raw": gzip.compress(plain, mtime=0)[:-12]}, invalid))
        for label, form, code in cases:
            with self.subTest(form=label):
                self.repin_snapshot(member, None if "raw" in form else rows, **form)
                self.assert_refused(code, target=target, output="form-" + label)
        self.repin_snapshot(member)
        self.package("plain-form", target)

    def test_snapshot_projection_check_refuses_any_difference_in_a_published_block(self):
        self.withhold()
        member, prefix = "selected/provenance/compose-migration/source.tar.gz", "source/"
        original = self.snapshot_tars[member]
        withheld = {prefix + name for name in WITHHELD}
        blocks = member_blocks(original)
        trailer = original[sum(len(block) for _, block in blocks):]
        kept = [block for name, block in blocks if name not in withheld]
        release.verify_provenance_projection(original, b"".join(kept) + trailer, withheld)
        changed = bytearray(kept[2])
        changed[512] ^= 1  # First data byte of a published member.
        rewritten = source_snapshot([row for row in self.snapshot_rows[member] if row[0] not in withheld])[1]
        cases = {"changed-data": [*kept[:2], bytes(changed), *kept[3:]],
                 "rewritten-headers": [block for _, block in member_blocks(rewritten)],
                 "dropped-member": kept[1:],
                 "reordered": [kept[1], kept[0], *kept[2:]],
                 "withheld-member-kept": [block for _, block in blocks]}
        for label, parts in cases.items():
            with self.subTest(case=label), \
                    self.assertRaisesRegex(release.PackageError, "^provenance_projection_mismatch$"):
                release.verify_provenance_projection(original, b"".join(parts) + trailer, withheld)
        # Packaging runs the check and stops on its refusal.
        with patch.object(release, "verify_provenance_projection",
                          side_effect=release.PackageError("provenance_projection_mismatch")):
            self.assert_refused("provenance_projection_mismatch", target="linux-x86_64", output="projection")

    def test_linux_guide_without_the_disclosure_sentence_is_refused(self):
        target = "linux-x86_64"
        selection = self.targets[target]
        guide = self.root / selection["guide"]
        cases = {"absent": b"Selected release guide.\n",
                 "reworded": DISCLOSURE.replace("except for", "without").encode() + b"\n",
                 "wrapped": DISCLOSURE.replace(" except for ", " except for\n").encode() + b"\n",
                 # A guide that points at the record for a list of members: the record lists none.
                 "member-list": DISCLOSURE.replace("members this library does not publish",
                                                   "the members listed in `" + RECORD + "`").encode() + b"\n"}
        self.assertEqual(len({*cases.values(), DISCLOSURE.encode() + b"\n"}), len(cases) + 1)
        self.assertNotIn(RECORD, DISCLOSURE)
        for label, raw in cases.items():
            with self.subTest(guide=label):
                guide.write_bytes(raw)
                selection["guide_sha256"] = sha(raw)
                self.assert_refused("provenance_disclosure_missing", target=target, output="guide-" + label)
        raw = b"# Guide\n\n" + DISCLOSURE.encode() + b"\n"
        guide.write_bytes(raw)
        selection["guide_sha256"] = sha(raw)
        self.assertEqual(release.collect(self.root, target)["README.md"], (raw, 0o644))
        # An explicitly selected guide is held to the same sentence.
        value = self.explicit_inputs(target)
        path = "docs/current-guide.md"
        write(self.root / path, b"Current guide.\n")
        value["sources"]["README.md"] = {"path": path, "sha256": sha(b"Current guide.\n")}
        with self.assertRaisesRegex(release.PackageError, "^provenance_disclosure_missing$"):
            self.explicit_package(value, name="guide-explicit")
        self.assertFalse((self.base / "guide-explicit").exists())
        self.assertEqual(release.PROVENANCE_DISCLOSURE, DISCLOSURE)
        self.assertEqual(release.PROVENANCE_DISCLOSURE_TARGETS, frozenset({"linux-x86_64"}))
        self.assertEqual(release.PROVENANCE_NOTE, NOTE)

    def test_the_member_lists_are_written_from_a_library_and_from_the_inputs(self):
        target = "linux-x86_64"
        self.withhold()
        library = self.package("lists-library", target)["archive"]
        linux_trees = (SOURCE_TREE, *SNAPSHOTS)
        published = {tree: self.published[tree] for tree in sorted(linux_trees)}
        document = self.private_document()
        private = {tree: document["trees"][tree] for tree in sorted(linux_trees)}
        originals = {tree: document["original_snapshots"][tree] for tree in sorted(SNAPSHOTS)}

        def write_list(name, *options, code=0, private=False):
            path = self.base / name
            done, out, err = self.command("--source-members", path, *options)
            self.assertEqual(done, code, err)
            if code:
                self.assertFalse(path.exists())
                return None
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            value = json.loads(path.read_bytes())
            self.assertEqual(value["schema"], PRIVATE_SCHEMA if private else LIST_SCHEMA)
            self.assertEqual(set(value), {"schema", "trees", "original_snapshots"} if private else {"schema", "trees"})
            # Standard output holds counts only.
            self.assertEqual(json.loads(out), {"trees": {tree: len(names) for tree, names in value["trees"].items()}})
            (release.read_non_published_list if private else release.read_source_member_lists)(path.read_bytes())
            return value

        # What a library publishes of its source trees is what the committed list is to hold.
        self.assertEqual(write_list("published.json", "--from-library", library)["trees"], published)
        # What the inputs hold beyond it is what the private list is to hold: each member with its digest,
        # and the digest of every original snapshot that holds one.
        value = write_list("private.json", "--non-published", "--target", target, "--from-inputs",
                           "--except-list", self.base / "published.json", private=True)
        self.assertEqual((value["trees"], value["original_snapshots"]), (private, originals))
        everything = write_list("inputs.json", "--target", target, "--from-inputs")["trees"]
        self.assertEqual(everything, {tree: sorted([*published[tree], *(row["name"] for row in private[tree])])
                                      for tree in published})
        self.assertEqual(write_list("union.json", "--from-list", self.base / "published.json",
                                    "--from-list", self.base / "private.json")["trees"], everything)
        # Private lists add up to one private list; a published list has no digests to add.
        value = write_list("private-again.json", "--non-published", "--from-list", self.base / "private.json", private=True)
        self.assertEqual((value["trees"], value["original_snapshots"]), (private, originals))
        write_list("private-from-library.json", "--non-published", "--from-library", library, code=1)
        # The lists written this way package the same library again.
        shutil.copyfile(self.base / "published.json", self.root / PUBLISHED_LIST)
        self.non_published = self.base / "private.json"
        again = self.package("lists-again", target)
        self.assertEqual(Path(again["archive"]).read_bytes(), Path(library).read_bytes())
        # Refusals: an existing destination, no source, inputs without a target, a library that is none.
        before = (self.base / "private.json").read_bytes()
        done, out, err = self.command("--source-members", self.base / "private.json", "--from-library", library)
        self.assertEqual((done, out, (self.base / "private.json").read_bytes()), (1, "", before))
        write_list("none.json", code=1)
        write_list("no-target.json", "--from-inputs", code=1)
        write(self.base / "not-a-library.tar.gz", b"not an archive")
        write_list("not-a-library.json", "--from-library", self.base / "not-a-library.tar.gz", code=1)
        write_list("with-output.json", "--from-library", library, "--output", self.base / "unused", code=1)


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


class SourceSnapshotReferenceTests(unittest.TestCase):
    """A published member inside a shipped source snapshot that names a member left out refuses the
    build. A snapshot is compressed, so its own bytes do not show such a reference: the check also
    reads what the snapshot holds. The fixture is the one of ReleaseTests; none of its rows runs here."""

    setUp = ReleaseTests.setUp
    provenance_fixture = ReleaseTests.provenance_fixture
    write_published = ReleaseTests.write_published
    private_document = ReleaseTests.private_document
    write_non_published = ReleaseTests.write_non_published
    private_list = ReleaseTests.private_list
    withhold = ReleaseTests.withhold
    repin_snapshot = ReleaseTests.repin_snapshot
    repin_donor = ReleaseTests.repin_donor

    def snapshot_with(self, member, target, text):
        """Pin the snapshot again with text as the content of one published member; the private list
        follows the new original. Returns the snapshot's bytes."""
        holder = SNAPSHOTS.get(member, "") + PUBLISHED[2]
        rows = [(name, text if name == holder else data) for name, data in self.snapshot_rows[member]]
        self.assertEqual([name for name, _ in rows].count(holder), 1)
        raw = self.repin_snapshot(member, rows, target=target)
        self.write_non_published()
        return raw

    def refusal(self, target):
        with self.assertRaises(release.PackageError) as refused:
            release.collect(self.root, target, non_published=self.private_list())
        return str(refused.exception), getattr(refused.exception, "members", None)

    def test_a_published_snapshot_member_that_names_a_member_left_out_refuses_the_build(self):
        self.withhold()
        for target, donor in sorted(self.python_kits.items()):
            members = sorted(name for name in donor.get("sources", {}) if name.endswith("/source.tar.gz"))
            self.assertTrue(members, target)
            member = members[0]
            left_out = SNAPSHOTS.get(member, "") + WITHHELD[2]
            digest = self.private_trees[member][left_out]
            cases = {"digest": (b"pinned " + digest.encode() + b"\n", 1),
                     "digest-in-upper-case": (b"pinned " + digest.upper().encode() + b"\n", 1),
                     "path": (b"include " + left_out.encode() + b" here\n", 1),
                     "path-and-digest": (left_out.encode() + b" " + digest.encode() + b"\n", 2)}
            for label, (text, count) in cases.items():
                with self.subTest(target=target, reference=label):
                    raw = self.snapshot_with(member, target, text)
                    lists = self.private_list()
                    # The compressed snapshot does not show the reference; the member it holds does.
                    self.assertEqual(release.count_references(raw, lists.names(), lists.digests()), 0)
                    self.assertEqual(release.count_references(text, lists.names(), lists.digests()), count)
                    self.assertIn(text, gzip.decompress(raw))
                    self.assertEqual(self.refusal(target), ("published_member_names_non_published", {member: count}))
            # The same member without a reference: the build's check passes.
            with self.subTest(target=target, reference="none"):
                self.snapshot_with(member, target, b"pinned nothing\n")
                files = release.collect(self.root, target, non_published=self.private_list())
                self.assertIn(b"pinned nothing\n", gzip.decompress(files[member][0]))

    def test_the_refusal_names_each_snapshot_that_holds_a_reference_and_its_count(self):
        target = "linux-x86_64"
        self.withhold()
        first, second = sorted(SNAPSHOTS)[:2]
        expected = {}
        for member, count in ((first, 1), (second, 3)):
            digest = self.private_trees[member][SNAPSHOTS[member] + WITHHELD[1]]
            self.snapshot_with(member, target, (digest + "\n").encode() * count)
            expected[member] = count
        self.assertEqual(self.refusal(target), ("published_member_names_non_published", expected))
        output = self.base / "refused-snapshot-reference"
        with self.assertRaisesRegex(release.PackageError, "^published_member_names_non_published$"):
            release.package(self.root, target, output, non_published_list=self.non_published)
        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
