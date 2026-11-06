"""Structural contract tests only, never native emission or acceptance evidence.

Only the read-only join helper is imported. All files below are synthetic
temporary records, and the binary is an inert marker, not a runnable program.
"""
from pathlib import Path
import copy
import hashlib
import json
import tempfile
import unittest

from historical_g_current_binding import CurrentBindingRefused, verify_current


def pin(path):
    raw = path.read_bytes()
    return {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(), "size": len(raw)}


def artifact(row):
    return {"path": row["path"], "sha256": row["sha256"], "bytes": row["size"]}


class StructuralFixture:
    def __init__(self, root, changes=None, *, core="b" * 40, meta="c" * 40):
        self.root = root
        self.changes = changes or {}
        self.paths = {}
        source = root / "source"
        manifest = self.file("manifest", source / "native-candidate/Cargo.toml", b"# synthetic manifest\n")
        lock = self.file("lock", source / "native-candidate/Cargo.lock", b"# synthetic lock\n")
        native = self.file("binary", root / "emission/inert-binary", b"structural test marker; never execute\n")
        resource_root = root / "resources/libexec/meerkat-connect"
        resource_manifest = self.file("resource_manifest", resource_root / "resources.json", b"{}\n")
        locator = self.file("locator", root / "resources/bin/locator", b"inert structural locator\n")
        digest = resource_manifest["sha256"]
        packages = []
        upstreams = {}
        for name in ("meerkat-connect", "meerkat-core", "meerkat-runtime", "meerkat-session", "meerkat-mob", "meerkat-meta"):
            directory = source if name == "meerkat-connect" else root / ("meta" if name == "meerkat-meta" else "core")
            package_manifest = self.file(name, directory / "crates" / name / "Cargo.toml", ("# " + name + "\n").encode())
            package = {"name": name, "id": "path+" + package_manifest["path"], "source": None,
                       "manifest_path": package_manifest["path"]}
            packages.append(package)
            upstreams[name] = {"package_id": package["id"], "manifest": package_manifest}
        metadata = self.document("metadata", {"packages": packages})
        identity = {"source_root": str(source), "source_head": "a" * 40, "source_tree": "d" * 40,
            "core_head": core, "meta_head": meta, "manifest_sha256": manifest["sha256"], "lock_sha256": lock["sha256"]}
        for name in ("source_inventory", "cargo_manifest_inventory", "definition_inventory", "historical_native_check"):
            identity[name] = self.document(name, {"structural_fixture": name})
        identity["metadata"] = metadata
        identity["metadata_result"] = self.document("metadata_result", {
            "exit_code": 0, "timed_out": False, "cwd": str(source),
            "source_head": identity["source_head"], "source_tree": identity["source_tree"], "metadata": metadata})
        candidate = dict(identity, schema="connect-local-development-native-source-v1",
            status="local_development_native_source_reviewed")
        for name in ("root_source_review", "independent_source_review", "graph_root_review", "graph_peer_review"):
            candidate[name] = self.document(name, dict(identity, findings=[],
                status="actual_final_path_graph_verified" if name.startswith("graph") else "source_go_unexecuted"))
        candidate_pin = self.document("candidate", candidate)
        source_pin = self.document("source", {key: identity[key] for key in ("source_root", "source_head", "source_tree")} | {
            "candidate": candidate_pin})
        prepared = {}
        for name in ("prepared_root", "prepared_peer"):
            prepared[name] = self.document(name, {key: identity[key]
                for key in ("source_head", "source_tree", "core_head", "meta_head")} | {
                    "source": source_pin, "status": "source_go_unexecuted", "findings": []})
        resource_build = self.document("resource_build", {"resources": str(resource_root), "resources_sha256": digest})
        resource_run = self.document("resource_run", {"exit_code": 0, "cwd": str(source),
            "source_head": identity["source_head"], "argv": ["python", "-B", str(source / "scripts/package-connect-resources.py")]})
        resource = self.document("resource", {"status": "actual_fresh_resource_composition_verified", "findings": [],
            "source_head": identity["source_head"], "source_tree": identity["source_tree"], "build": resource_build,
            "manifest": resource_manifest, "resource_locator": locator, "run": resource_run,
            "full_resource_tree_verified": True, "archive_matches_resource_tree": True})
        selected = self.document("selection", {key: identity[key] for key in ("source_root", "source_head", "source_tree")} | {
            "manifest": manifest, "lockfile": lock, "resource_manifest": resource_manifest,
            "resources_sha256": digest, "resource_build": resource_build})
        binary = dict(upstreams["meerkat-connect"], binary=native, path=native["path"],
            manifest_path=packages[0]["manifest_path"], target={"kind": ["lib"], "name": "meerkat_connect"})
        binary.pop("manifest")
        binary = self.changed("binary_row", binary)
        events = self.changed("compiler_events", [
            {"reason": "compiler-artifact", "executable": native["path"], "package_id": packages[0]["id"],
             "manifest_path": packages[0]["manifest_path"], "target": {"kind": ["lib"], "name": "meerkat_connect"}, "profile": {"test": True}},
            {"reason": "build-finished", "success": True}])
        log = self.file("log", root / "compiler.log", "".join(json.dumps(row) + "\n" for row in events).encode())
        cargo = self.changed("cargo", {"exit_code": 0, "timed_out": False, "cwd": str(source),
            "env": {"MEERKAT_CONNECT_RESOURCES_SHA256": digest}, "log": log,
            "argv": ["wrapper", "test", "--locked", "--offline", "--manifest-path", manifest["path"], "--no-run"]})
        build = self.document("build", {"selection": selected, "cargo": cargo, "binaries": {"meerkat_connect": binary},
            "core_dependency": {"revision": core, "packages": {name: upstreams[name] for name in
                ("meerkat-core", "meerkat-runtime", "meerkat-session", "meerkat-mob")}},
            "meta_dependency": dict(upstreams["meerkat-meta"], revision=meta)})
        emission = self.document("emission", {"status": "passed", "selection": selected, "build": build})
        self.current = {"source": artifact(source_pin), "root_source_review": artifact(prepared["prepared_root"]),
            "peer_source_review": artifact(prepared["prepared_peer"]), "emission": artifact(emission),
            "resource_receipt": artifact(resource), "executable": artifact(native),
            "resource_manifest": artifact(resource_manifest), "resource_executable": artifact(locator),
            "native_manifest": artifact(manifest), "native_lock": artifact(lock), "resources_sha256": digest,
            "core": core, "meta": meta}

    def changed(self, name, value):
        value = copy.deepcopy(value)
        if name in self.changes:
            self.changes[name](value)
        return value

    def file(self, name, path, data):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        self.paths[name] = path
        return pin(path)

    def document(self, name, value):
        return self.file(name, self.root / (name + ".json"),
                         (json.dumps(self.changed(name, value)) + "\n").encode())


class CurrentBindingTests(unittest.TestCase):
    def fixture(self, changes=None, **kwargs):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        return StructuralFixture(Path(directory.name), changes, **kwargs)

    def refused(self, changes):
        with self.assertRaises(CurrentBindingRefused):
            verify_current(self.fixture(changes).current)

    def test_exact_structural_join_reads_without_creating_results(self):
        fixture = self.fixture()
        before = {str(p): p.read_bytes() for p in fixture.root.rglob("*") if p.is_file()}
        self.assertIsNone(verify_current(fixture.current))
        self.assertEqual(before, {str(p): p.read_bytes() for p in fixture.root.rglob("*") if p.is_file()})

    def test_reviewed_tuple_is_not_a_current_hash_allowlist(self):
        self.assertIsNone(verify_current(self.fixture(core="e" * 40, meta="f" * 40).current))

    def test_rejects_current_tuple_mismatch(self):
        for key in ("core", "meta"):
            with self.subTest(key=key):
                fixture = self.fixture()
                fixture.current[key] = "0" * 40
                with self.assertRaises(CurrentBindingRefused):
                    verify_current(fixture.current)

    def test_rejects_source_and_candidate_identity_mismatches(self):
        for key in ("source_root", "source_head", "source_tree"):
            with self.subTest(key=key):
                self.refused({"source": lambda row, key=key: row.update({key: "/wrong" if key == "source_root" else "0" * 40})})
        self.refused({"candidate": lambda row: row.update(status="pending")})

    def test_rejects_source_and_graph_review_mismatches(self):
        for name in ("prepared_root", "prepared_peer", "root_source_review", "independent_source_review", "graph_root_review", "graph_peer_review"):
            with self.subTest(name=name):
                self.refused({name: lambda row: row.update(source_head="0" * 40)})
                self.refused({name: lambda row: row.update(findings=["unresolved"])})
        self.refused({"prepared_peer": lambda row: row["source"].update(path="/wrong/source.json")})

    def test_rejects_metadata_terminal_and_package_mismatches(self):
        self.refused({"metadata_result": lambda row: row.update(exit_code=1)})
        self.refused({"metadata_result": lambda row: row.update(timed_out=True)})
        self.refused({"metadata": lambda row: row["packages"][1].update(id="different")})
        self.refused({"metadata": lambda row: row["packages"].append(copy.deepcopy(row["packages"][0]))})

    def test_rejects_manifest_lock_and_resource_selection_mismatches(self):
        for key in ("manifest", "lockfile", "resource_manifest"):
            with self.subTest(key=key):
                self.refused({"selection": lambda row, key=key: row[key].update(sha256="0" * 64)})
        self.refused({"selection": lambda row: row.update(source_tree="0" * 40)})
        self.refused({"selection": lambda row: row.update(resources_sha256="0" * 64)})

    def test_rejects_emitted_binary_and_tuple_mismatches(self):
        self.refused({"binary_row": lambda row: row.update(path="/different/binary")})
        self.refused({"binary_row": lambda row: row["binary"].update(sha256="0" * 64)})
        self.refused({"build": lambda row: row["core_dependency"].update(revision="0" * 40)})
        self.refused({"build": lambda row: row["meta_dependency"].update(revision="0" * 40)})

    def test_rejects_failed_or_interrupted_emission(self):
        self.refused({"emission": lambda row: row.update(status="failed")})
        for updates in ({"exit_code": 1}, {"timed_out": True}, {"failure": "KeyboardInterrupt"}):
            with self.subTest(updates=updates):
                self.refused({"cargo": lambda row, updates=updates: row.update(updates)})

    def test_rejects_missing_or_different_actual_compiler_event(self):
        self.refused({"compiler_events": lambda rows: rows[1].update(success=False)})
        self.refused({"compiler_events": lambda rows: rows[0].update(executable="/wrong/binary")})
        self.refused({"compiler_events": lambda rows: rows.append(copy.deepcopy(rows[0]))})
        self.refused({"cargo": lambda row: row.update(cwd="/wrong/source")})

    def test_rejects_resource_receipt_and_terminal_mismatches(self):
        for updates in ({"status": "pending"}, {"source_head": "0" * 40}, {"full_resource_tree_verified": False}):
            with self.subTest(updates=updates):
                self.refused({"resource": lambda row, updates=updates: row.update(updates)})
        self.refused({"resource": lambda row: row["build"].update(sha256="0" * 64)})
        self.refused({"resource": lambda row: row["resource_locator"].update(path="/wrong/locator")})
        self.refused({"resource_run": lambda row: row.update(exit_code=1)})

    def test_rejects_changed_file_bytes_and_duplicate_keys(self):
        fixture = self.fixture()
        fixture.paths["binary"].write_bytes(b"changed")
        with self.assertRaises(CurrentBindingRefused):
            verify_current(fixture.current)
        fixture = self.fixture()
        path = fixture.paths["source"]
        path.write_bytes(b'{"candidate":{},"candidate":{}}')
        fixture.current["source"] = artifact(pin(path))
        with self.assertRaises(CurrentBindingRefused):
            verify_current(fixture.current)


if __name__ == "__main__":
    unittest.main()
