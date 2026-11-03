"""Focused exporter checks. No installer, provider, adapter or native host runs."""
from __future__ import annotations

import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/export-library-consumers.py"
SPEC = importlib.util.spec_from_file_location("library_consumers", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
ENV = {"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1"}


def sha(data):
    return hashlib.sha256(data).hexdigest()


class ConsumerExportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory(prefix="toolkit-consumer-tests-")
        cls.addClassCleanup(cls.scratch.cleanup)
        cls.base = Path(cls.scratch.name).resolve()
        cls.source = cls.base / "source"
        cls.source.mkdir()
        cls.payloads, cls.origins = MODULE.collect(ROOT)
        sources = set(MODULE.PINS) | set(MODULE.DEPENDENCY_PINS)
        sources.update((MODULE.CATALOG, MODULE.DISCOVERY_CATALOG))
        sources.update(source["path"] for source in MODULE.ROUTE_SOURCES.values())
        sources.update(row["path"] for row in cls.origins.values() if "path" in row)
        component_inputs = json.loads((ROOT / MODULE.SERVICES_ACCEPTANCE).read_text())
        sources.add(component_inputs["retained_acceptance"]["path"])
        seed_requirements = (ROOT / MODULE.AGNO_SEED / "install-requirements.txt").read_text().splitlines()
        sources.update(MODULE.AGNO_SEED + "/" + name[2:] for name in seed_requirements
                       if name != "./" + MODULE.AGNO_WHEEL)
        for relative in sources:
            target = cls.source / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, target)

    def setUp(self):
        self.work = Path(tempfile.mkdtemp(prefix="case-", dir=self.base))
        self.output = self.work / "consumer"

    def cli(self, output=None):
        return subprocess.run([sys.executable, str(SCRIPT), "--root", str(self.source),
                               "--output", str(output if output is not None else self.output)],
                              env=ENV, capture_output=True, text=True, timeout=30)

    def refuse(self, output=None):
        result = self.cli(output)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertEqual(result.stdout, "")
        return result

    def test_public_export_preserves_exact_installation_inputs_and_documents(self):
        result = self.cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        summary = json.loads(result.stdout)
        self.assertEqual(summary["scope"], "consumer_export_only_no_installation_or_activation")
        contents = json.loads((self.output / "CONTENTS.json").read_bytes())
        self.assertEqual(contents["accepted_inputs"], {**MODULE.PINS, **MODULE.DEPENDENCY_PINS})
        self.assertEqual(contents["files"]["README.md"]["source"]["base"], "exporter_checkout")
        members = {str(p.relative_to(self.output)) for p in self.output.rglob("*") if p.is_file()}
        self.assertEqual(members, set(contents["files"]) | {"CONTENTS.json", "SHA256SUMS"})
        self.assertEqual(summary["files"], len(members))
        self.assertEqual(178, len(members))
        self.assertEqual(contents["files"]["install-library-consumer.py"]["source"],
                         {"path": MODULE.INSTALLER, "sha256": MODULE.INSTALLER_SHA})
        for path, record in contents["files"].items():
            actual = (self.output / path).read_bytes()
            self.assertEqual(record["sha256"], sha(actual), path)
            origin = record["source"]
            if "path" in origin:
                self.assertEqual(actual, (ROOT / origin["path"]).read_bytes(), path)
        checksums = dict(line.split("  ", 1)[::-1] for line in (self.output / "SHA256SUMS").read_text().splitlines())
        self.assertEqual(set(checksums), members - {"SHA256SUMS"})
        for path, expected in checksums.items():
            self.assertEqual(expected, sha((self.output / path).read_bytes()))
        self.assertEqual(summary["checksums_sha256"], sha((self.output / "SHA256SUMS").read_bytes()))
        with zipfile.ZipFile(self.output / "agno" / MODULE.AGNO_WHEEL) as archive:
            self.assertEqual((self.output / "agno/PROFILE-GUIDE.md").read_bytes(),
                             archive.read("meerkat_toolkit_agno/PROFILE-GUIDE.md"))
            self.assertFalse(any("paired_host" in name.lower() or name.startswith("meerkat_toolkit/")
                                 for name in archive.namelist()))
        with tarfile.open(self.output / "activepieces" / MODULE.AP_ARCHIVE) as archive:
            self.assertEqual((self.output / "activepieces/PROFILE-GUIDE.md").read_bytes(),
                             archive.extractfile("package/README.md").read())
        catalog = json.loads((self.output / "catalog.json").read_bytes())
        self.assertEqual(catalog[:101], json.loads((ROOT / MODULE.CATALOG).read_bytes()))
        self.assertEqual(catalog[101:], json.loads((ROOT / MODULE.DISCOVERY_CATALOG).read_bytes()))
        self.assertEqual(len(catalog), 111)
        self.assertEqual(len({tuple(row["identity"][k] for k in ("publisher", "name", "version")) for row in catalog}), len(catalog))
        self.assertEqual(contents["files"]["catalog.json"]["source"], {
            "kind": "composed_catalog_metadata", "sources": [
                {"path": MODULE.CATALOG, "sha256": MODULE.CATALOG_SHA, "scope": "adopted", "entries": 101},
                {"path": MODULE.DISCOVERY_CATALOG, "sha256": MODULE.DISCOVERY_CATALOG_SHA, "scope": "discovery", "entries": 10}]})
        for service in ("slack", "gmail", "google-calendar", "google-drive"):
            entry = next(row for row in catalog if row["identity"]["name"] == service)
            self.assertEqual({row["id"] for row in entry["strategies"]},
                             {"selected-local-mcp", "first-party-services-mcp"})
            self.assertIn(MODULE.SERVICES_SHA, entry["strategies"][1]["instructions"])
        self.assertEqual((self.output / "services/install-requirements.txt").read_text(),
                         "./" + MODULE.SERVICES_WHEEL + "\n")
        self.assertEqual(sha((self.output / "services" / MODULE.SERVICES_WHEEL).read_bytes()), MODULE.SERVICES_SHA)
        with zipfile.ZipFile(self.output / "services" / MODULE.SERVICES_WHEEL) as archive:
            self.assertEqual((self.output / "services/PROFILE-GUIDE.md").read_bytes(),
                             archive.read("meerkat_toolkit_services/GUIDE.md"))
        lock = json.loads((self.output / "activepieces/package-lock.json").read_bytes())
        integrity = "sha512-" + base64.b64encode(hashlib.sha512(
            (self.output / "activepieces" / MODULE.AP_ARCHIVE).read_bytes()).digest()).decode()
        self.assertEqual(lock["packages"]["node_modules/@rkat/toolkit-activepieces"]["integrity"], integrity)
        forbidden = ("fixture", "verifier", "verify-", "test_", "test-", "expected.json", "pgserver", "paired_host")
        self.assertFalse([name for name in members if any(word in name.lower() for word in forbidden)])
        self.assertEqual(json.loads((self.output / "activepieces/tally.json").read_text()),
                         {"profile": "tally.get-form", "token_env": "MY_TALLY_TOKEN"})
        self.assertEqual(json.loads((self.output / "agno/hackernews.json").read_text()),
                         {"profile": "hackernews", "operations": ["get_top_hackernews_stories"], "config": {"timeout": 10}})
        for service in ("slack", "gmail"):
            config = json.loads((self.output / ("services/" + service + ".json")).read_bytes())
            self.assertEqual(config["profile"], service)
            self.assertIn("token_env", config["config"])
            self.assertNotIn("token", config["config"])
        for family, count in (("agno", 112), ("services", 32)):
            lines = (self.output / family / "install-offline.txt").read_text().splitlines()
            self.assertEqual(len(lines), count)
            self.assertEqual(len(set(lines)), count)
            for line in lines:
                self.assertRegex(line, r"^[a-z0-9-]+==[A-Za-z0-9.!+_-]+ --hash=sha256:[0-9a-f]{64}$")
            self.assertEqual(len(list((self.output / family).glob("*.whl"))), count)
        cache = json.loads((self.output / "activepieces/offline-cache.json").read_bytes())
        self.assertEqual(cache["schema_version"], 1)
        self.assertEqual(len(cache["tarballs"]), 4)
        for row in cache["tarballs"]:
            data = (self.output / "activepieces" / row["path"]).read_bytes()
            self.assertEqual(row["sha256"], sha(data))
            self.assertEqual(row["integrity"], "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode())
            selected = lock["packages"]["node_modules/" + row["name"]]
            self.assertEqual(row["version"], selected["version"])
            self.assertEqual(row["integrity"], selected["integrity"])

    def test_corrupt_artifact_lock_requirement_or_anchor_refuses_before_output(self):
        targets = [MODULE.AGNO_ARTIFACT,
                   MODULE.AGNO_SEED + "/install-requirements.txt",
                   MODULE.AP_SEED + "/package-lock.json",
                   MODULE.AP_ARTIFACT,
                   MODULE.AGNO_SEED + "/seed-manifest.json",
                   MODULE.CATALOG,
                   MODULE.DISCOVERY_CATALOG,
                   MODULE.SERVICES_SEED + "/" + MODULE.SERVICES_WHEEL,
                   MODULE.SERVICES + "/runtime-constraints.txt",
                   MODULE.SERVICES_SEED + "/install-offline.txt",
                   MODULE.SERVICES_SEED + "/README.md",
                   MODULE.INSTALLER,
                   MODULE.INSTALLER_ACCEPTANCE,
                   *MODULE.DEPENDENCY_PINS,
                   self.origins["agno/mcp-2.2.0-py3-none-any.whl"]["path"],
                   self.origins["activepieces/undici-8.11.2.tgz"]["path"]]
        for relative in targets:
            with self.subTest(relative=relative):
                target = self.source / relative
                before = target.read_bytes()
                try:
                    target.write_bytes(before + b"corrupted")
                    self.refuse()
                    self.assertFalse(self.output.exists())
                finally:
                    target.write_bytes(before)

    def test_existing_directory_and_file_are_unchanged(self):
        self.output.mkdir()
        sentinel = self.output / "owned.txt"
        sentinel.write_bytes(b"keep me")
        self.refuse()
        self.assertEqual(list(self.output.iterdir()), [sentinel])
        self.assertEqual(sentinel.read_bytes(), b"keep me")
        file = self.work / "file"
        file.write_bytes(b"unchanged")
        self.refuse(file)
        self.assertEqual(file.read_bytes(), b"unchanged")

    def test_existing_and_dangling_output_symlinks_are_unchanged(self):
        other = self.work / "other"
        other.mkdir()
        self.output.symlink_to(other, target_is_directory=True)
        self.refuse()
        self.assertTrue(self.output.is_symlink())
        self.assertEqual(list(other.iterdir()), [])
        self.output.unlink()
        self.output.symlink_to(self.work / "missing")
        self.refuse()
        self.assertTrue(self.output.is_symlink())
        self.assertFalse((self.work / "missing").exists())

    def test_relative_dot_missing_parent_and_protected_output_paths_refuse(self):
        for output in ("relative", str(self.work) + "/../escape", str(self.work) + "/./escape",
                       str(self.work / "missing-parent/child"), str(self.source / MODULE.AGNO / "export"),
                       str(self.source / MODULE.CONFIG / "export"),
                       str(self.source / MODULE.AUTH / "export"),
                       str(self.source / MODULE.SERVICES_SEED / "export"),
                       str(self.source / MODULE.DEPENDENCIES / "export"),
                       str(self.source / MODULE.ACQUISITION / "export")):
            with self.subTest(output=output):
                self.refuse(output)
        link = self.work / "proof-link"
        link.symlink_to(self.source / MODULE.AP, target_is_directory=True)
        self.refuse(link / "export")
        self.assertFalse((self.source / MODULE.AP / "export").exists())

    def test_source_file_symlink_is_refused_even_when_target_has_exact_bytes(self):
        relative = MODULE.AGNO_ARTIFACT
        target = self.source / relative
        before = target.read_bytes()
        try:
            target.unlink()
            target.symlink_to(ROOT / relative)
            self.refuse()
            self.assertFalse(self.output.exists())
        finally:
            target.unlink()
            target.write_bytes(before)

    def test_source_directory_symlink_is_refused(self):
        target = self.source / MODULE.AGNO_SEED
        saved = target.with_name("saved-seed")
        target.rename(saved)
        try:
            target.symlink_to(saved, target_is_directory=True)
            self.refuse()
            self.assertFalse(self.output.exists())
        finally:
            target.unlink()
            saved.rename(target)

    def test_source_fifo_is_refused_without_waiting_for_a_writer(self):
        target = self.source / MODULE.AGNO_ACCEPTANCE
        before = target.read_bytes()
        try:
            target.unlink()
            os.mkfifo(target)
            self.refuse()
            self.assertFalse(self.output.exists())
        finally:
            target.unlink()
            target.write_bytes(before)

    def test_new_destination_appearing_during_collection_is_not_replaced(self):
        def competing_destination(_root, _target):
            self.output.mkdir()
            (self.output / "owned.txt").write_bytes(b"other owner")
            return dict(self.payloads), dict(self.origins)
        with patch.object(MODULE, "collect", side_effect=competing_destination):
            with self.assertRaises(FileExistsError):
                MODULE.export(self.source, str(self.output))
        self.assertEqual((self.output / "owned.txt").read_bytes(), b"other owner")
        self.assertEqual(len(list(self.output.iterdir())), 1)

    def test_interrupted_write_has_no_completion_marker_and_cannot_be_reused(self):
        original = MODULE.write_file
        calls = 0
        def interrupted(directory, name, data):
            nonlocal calls
            calls += 1
            if calls == 3:
                raise OSError("simulated disk failure")
            return original(directory, name, data)
        with patch.object(MODULE, "write_file", side_effect=interrupted):
            with self.assertRaises(OSError):
                MODULE.export(self.source, str(self.output))
        self.assertTrue(self.output.is_dir())
        self.assertFalse((self.output / "SHA256SUMS").exists())
        self.refuse()

    def test_unselected_private_and_fixture_files_do_not_enter_collection(self):
        extra = self.source / MODULE.AGNO_SEED / "private-household-fixture.txt"
        extra.write_text("not part of any selected input")
        try:
            files, origins = MODULE.collect(self.source)
            self.assertNotIn(extra.name, " ".join(files))
            self.assertEqual(set(files), set(self.payloads))
            # Guidance and the installer explicitly come from the current
            # exporter checkout; another owner can revise them during a test.
            for path, data in files.items():
                if origins[path].get("base") == "exporter_checkout":
                    self.assertEqual(sha(data), origins[path]["sha256"], path)
                else:
                    self.assertEqual(sha(data), sha(self.payloads[path]), path)
        finally:
            extra.unlink()

    def test_dependency_manifest_shape_cannot_change_selection_or_escape_local_paths(self):
        target = self.source / MODULE.DEPENDENCIES / "closure.json"
        before = target.read_bytes()
        original = json.loads(before)
        mutations = {
            "traversal": lambda c: c["artifacts"][0].update(path="../outside.whl"),
            "filename": lambda c: c["artifacts"][0].update(filename="../../outside.whl"),
            "duplicate": lambda c: c["artifacts"].__setitem__(1, c["artifacts"][0]),
            "missing": lambda c: c["artifacts"].pop(),
            "version": lambda c: c["artifacts"][0].update(version="9999.0.0"),
            "family": lambda c: c["artifacts"][0].update(families=["services"]),
            "platform": lambda c: c["python_platform"].update(machine="x86_64"),
            "inputs": lambda c: c["accepted_inputs"].update({"services/runtime-constraints.txt": "0" * 64}),
            "too_large": lambda c: c["artifacts"][0].update(bytes=MODULE.MAX_FILE + 1),
            "npm_integrity": lambda c: c["artifacts"][-1].update(integrity="sha512-foreign"),
        }
        try:
            for label, mutate in mutations.items():
                with self.subTest(label=label):
                    candidate = json.loads(json.dumps(original))
                    mutate(candidate)
                    data = MODULE.encoded(candidate)
                    target.write_bytes(data)
                    pins = {**MODULE.DEPENDENCY_PINS, MODULE.DEPENDENCIES + "/closure.json": sha(data)}
                    with patch.object(MODULE, "DEPENDENCY_PINS", pins):
                        with self.assertRaises(MODULE.ExportError):
                            MODULE.collect(self.source)
                    self.assertFalse(self.output.exists())
        finally:
            target.write_bytes(before)

    def test_nonlocal_python_requirement_is_rejected_even_if_manifest_is_rebound(self):
        target = self.source / MODULE.DEPENDENCIES / "closure.json"
        before = target.read_bytes()
        closure = json.loads(before)
        files, origins = dict(self.payloads), dict(self.origins)
        files["agno/runtime-constraints.txt"] = b"malicious @ https://example.invalid/package.whl\n"
        closure["accepted_inputs"]["agno/runtime-constraints.txt"] = sha(files["agno/runtime-constraints.txt"])
        data = MODULE.encoded(closure)
        try:
            target.write_bytes(data)
            pins = {**MODULE.DEPENDENCY_PINS, MODULE.DEPENDENCIES + "/closure.json": sha(data)}
            with patch.object(MODULE, "DEPENDENCY_PINS", pins):
                with self.assertRaises(MODULE.ExportError):
                    MODULE.collect_dependencies(self.source, files, origins)
        finally:
            target.write_bytes(before)

    def test_dependency_object_symlink_refuses_even_with_exact_bytes(self):
        relative = self.origins["agno/mcp-2.2.0-py3-none-any.whl"]["path"]
        target = self.source / relative
        before = target.read_bytes()
        try:
            target.unlink()
            target.symlink_to(ROOT / relative)
            self.refuse()
            self.assertFalse(self.output.exists())
        finally:
            target.unlink()
            target.write_bytes(before)


if __name__ == "__main__":
    unittest.main()
