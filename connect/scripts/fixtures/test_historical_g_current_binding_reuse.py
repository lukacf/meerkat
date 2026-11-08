"""Synthetic current-reader proof tests, not native or historical acceptance.

The original fresh-source fixture/tests are unchanged. These inert temporary
records exercise only the closed retained-component evidence boundary.
"""
from pathlib import Path
import copy
import json
import os
import tempfile
import unittest

from historical_g_current_binding import CurrentBindingRefused, Documents, _same_build_selection, verify_current
from test_historical_g_current_binding import StructuralFixture, artifact, pin


class ReuseFixture(StructuralFixture):
    def __init__(self, root, changes=None, *, doc_delta=False, selection_alias=False):
        super().__init__(root)
        self.changes = changes or {}
        current = self.current
        source = root / "source"
        prior_root = root / "prior-source"
        head, tree, prior_head, prior_tree = "a" * 40, "d" * 40, "1" * 40, "2" * 40
        core, meta = current["core"], current["meta"]
        self.file("sdk", source / "sdk/example.py", b"# unchanged SDK\n")
        self.file("packager", source / "scripts/package-connect-resources.py", b"# inert producer\n")
        self.file("producer", source / "scripts/connect-candidate.py", b"# inert resource source owner\n")
        self.file("rust", source / "crates/toolkit-mobkit/src/private_originals.rs", b"// current native code\n")
        toolkit_manifest = self.file("toolkit_manifest", source / "crates/toolkit-mobkit/Cargo.toml", b"# toolkit\n")
        policy = self.file("policy", source / ".cargo/config.toml", b"# inert selected policy\n")
        self.file("current_reader", source / "scripts/fixtures/historical_g_current_binding.py", b"# current reader\n")
        if doc_delta:
            self.file("current_guide", source / "docs/site/guides/host-composition.mdx", b"Current reader guide\n")
        old, new = {}, {}
        for path in source.rglob("*"):
            if not path.is_file():
                continue
            name = path.relative_to(source).as_posix()
            row = pin(path)
            new[name] = {"sha256": row["sha256"], "size": row["size"], "mode": "100644"}
            data = (b"// prior native code\n" if name.endswith("private_originals.rs") else
                b"# prior reader\n" if name == "scripts/fixtures/historical_g_current_binding.py" else
                b"Prior reader guide\n" if name == "docs/site/guides/host-composition.mdx" else path.read_bytes())
            saved = self.file("prior_" + name, prior_root / name, data)
            old[name] = {"sha256": saved["sha256"], "size": saved["size"], "mode": "100644"}
        old_inventory = self.document("prior_inventory", old)
        new_inventory = self.document("current_inventory", new)
        commit = self.document("commit", {"head": head, "tree": tree})
        expected_sdk = self.document("expected_sdk", {"connect": {"repository": str(prior_root),
            "revision": prior_head, "files": {"sdk/example.py": old["sdk/example.py"]}}})
        resource_build = pin(self.paths["resource_build"])
        resource_run = self.document("retained_resource_run", {"exit_code": 0, "timed_out": False,
            "cwd": str(prior_root), "source_head": prior_head,
            "argv": ["python", "-B", str(prior_root / "scripts/package-connect-resources.py")]})
        resource_inputs = self.document("resource_inputs", {"source_root": str(prior_root), "source_tree": prior_tree,
            "connect_source": prior_head, "expected_sdk_files": expected_sdk,
            "producer_sources": {name: pin(prior_root / name) for name in
                ("scripts/package-connect-resources.py", "scripts/connect-candidate.py")}})
        payload = self.document("payload_review", {"status": "source_payloads_verified",
            "source_head": prior_head, "source_tree": prior_tree, "build": resource_build,
            "manifest": current["resource_manifest"], "files": [resource_inputs]})
        resource = self.document("retained_resource", {"status": "actual_fresh_resource_composition_verified",
            "findings": [], "source_head": prior_head, "source_tree": prior_tree, "build": resource_build,
            "manifest": current["resource_manifest"], "resource_locator": current["resource_executable"],
            "run": resource_run, "source_payload_review": payload,
            "full_resource_tree_verified": True, "archive_matches_resource_tree": True})
        resource_peer = self.document("resource_peer", {"status": "actual_fresh_resource_composition_verified",
            "findings": [], "source_head": prior_head, "build": resource_build,
            "manifest": current["resource_manifest"], "source_payload_review": payload})
        composition = self.document("retained_composition", {"source_head": prior_head, "core_head": core,
            "meta_head": meta, "sdk_expected_files": expected_sdk, "resource_review": resource})
        prior_inputs = self.document("prior_inputs", {"source_root": str(prior_root), "source_head": prior_head,
            "source_tree": prior_tree, "core_head": core, "meta_head": meta, "composition": composition})
        reuse = self.document("reuse", {"status": "source_delta_allows_reuse_of_unchanged_components_only",
            "current_commit": commit, "current_inventory": new_inventory, "prior_inventory": old_inventory,
            "changed_paths": ["crates/toolkit-mobkit/src/private_originals.rs",
                "scripts/fixtures/historical_g_current_binding.py"] +
                (["docs/site/guides/host-composition.mdx"] if doc_delta else []), "prior_resources": resource})
        packages = json.loads(self.paths["metadata"].read_text())["packages"]
        packages.append({"name": "toolkit-mobkit", "id": "path+" + toolkit_manifest["path"],
            "source": None, "manifest_path": toolkit_manifest["path"]})
        metadata = self.document("current_metadata", {"packages": packages})
        stderr = self.file("metadata_stderr", root / "metadata.stderr", b"")
        metadata_command = self.document("metadata_command", {"exit_code": 0, "timed_out": False,
            "cwd": str(source), "stdout": metadata, "stderr": stderr})
        metadata_result = self.document("current_metadata_result", {"status": "actual_locked_metadata_complete",
            "source_root": str(source), "source_head": head, "source_tree": tree,
            "metadata": metadata, "command": metadata_command})
        identity = {"source_root": str(source), "source_head": head, "source_tree": tree,
            "source_inventory": new_inventory, "cargo_manifest_inventory": pin(self.paths["cargo_manifest_inventory"]),
            "core_head": core, "meta_head": meta, "manifest_sha256": current["native_manifest"]["sha256"],
            "lock_sha256": current["native_lock"]["sha256"], "metadata": metadata, "metadata_result": metadata_result}
        graph_peer = self.document("current_graph_peer", dict(identity, status="actual_final_path_graph_verified", findings=[]))
        graph_root = self.document("current_graph_root", {"status": "root_graph_go_for_development_build", "findings": [],
            "source_commit": commit, "metadata_result": metadata_result,
            "exact_graph_equal_after_candidate_root_relocation": True})
        context = self.document("context", dict(identity, root_graph_review=graph_root, peer_graph_review=graph_peer,
            source_commit=commit, cargo_policy=policy, prior_inputs=prior_inputs, reuse=reuse,
            composition=composition, resource_build=resource_build, resource_peer_review=resource_peer))
        argv = ["wrapper", "--config", policy["path"], "test", "--locked", "--offline", "--manifest-path",
            current["native_manifest"]["path"], "--no-run"]
        command_spec = self.document("command_spec", {"argv": argv, "cwd": str(source)})
        build_source = self.document("build_source", {"context": context, "command": command_spec,
            "source_root": str(source), "source_head": head, "source_tree": tree, "core_head": core, "meta_head": meta,
            "files": [context, command_spec]})
        source_reviews = {k: self.document(k, {"status": "source_go_unexecuted", "findings": [], "source": build_source})
            for k in ("current_source_root_review", "current_source_peer_review")}
        output = root / "build"
        inputs = self.document("build_inputs", {"source": build_source,
            "root_source_review": source_reviews["current_source_root_review"],
            "peer_source_review": source_reviews["current_source_peer_review"], "output": str(output)})
        runner = self.file("runner", root / "run.py", b"# inert driver\n")
        idle = self.document("idle", {"forbidden_processes": []})
        admission = self.document("admission", {"status": "actual_retention_build_admitted", "findings": [],
            "lane_idle": True, "source": build_source, "inputs": inputs, "idle_evidence": idle,
            "output": str(output), "argv": argv, "cwd": str(source)})
        selection_value = {"source": build_source, "context": context, "inputs": inputs,
            "admission": admission, "source_root": str(source), "source_head": head, "source_tree": tree,
            "core": core, "meta": meta, "manifest": current["native_manifest"], "lockfile": current["native_lock"],
            "metadata": metadata, "graph_root": graph_root, "graph_peer": graph_peer, "runner": runner,
            "cargo_policy": policy, "argv": argv, "resource_build": resource_build,
            "resource_manifest": current["resource_manifest"], "resources_sha256": current["resources_sha256"]}
        selection = self.file("current_selection", output / "selection.json",
            (json.dumps(self.changed("current_selection", selection_value)) + "\n").encode())
        observed_selection = selection
        if selection_alias:
            alias = output / "SELECTION.json"
            if not alias.is_file() or not alias.samefile(output / "selection.json"):
                raise unittest.SkipTest("filesystem does not expose the actual selection case alias")
            observed_selection = pin(alias)
        binaries, events = {}, []
        targets = {"meerkat_connect": ("meerkat-connect", "lib", "meerkat_connect"),
            "toolkit_mobkit": ("toolkit-mobkit", "lib", "toolkit_mobkit"),
            "meerkat_connect_bin": ("meerkat-connect", "bin", "meerkat-connect")}
        for key, (name, kind, target) in targets.items():
            package = next(p for p in packages if p["name"] == name)
            binary = pin(self.paths["binary"]) if key == "meerkat_connect" else self.file(key, root / (key + ".inert"), b"not executable\n")
            row = {"package_id": package["id"], "manifest_path": package["manifest_path"], "path": binary["path"],
                "binary": binary, "target": {"kind": [kind], "name": target}}
            binaries[key] = row
            events.append({"reason": "compiler-artifact", "package_id": row["package_id"],
                "manifest_path": row["manifest_path"], "executable": row["path"], "target": row["target"], "profile": {"test": True}})
        events.append({"reason": "build-finished", "success": True})
        events = self.changed("current_events", events)
        log = self.file("current_log", root / "current.log", "".join(json.dumps(r) + "\n" for r in events).encode())
        cargo = self.changed("current_cargo", {"exit_code": 0, "timed_out": False, "argv": argv, "cwd": str(source),
            "env": {"MEERKAT_CONNECT_RESOURCES_SHA256": current["resources_sha256"]}, "log": log})
        command = self.document("current_command", cargo)
        child = self.document("child", {"schema": "actual-recorder-child-observation-v1", "command_name": "build",
            "command": command, "selection": observed_selection, "log": log, "expected_argv": argv, "actual_argv": argv,
            "cwd": str(source), "wait_hook_observed": True, "owner_run_finished": True,
            "owner_returned_normally": True, "owner_exception_class": None, "returncode": 0,
            "child_pid": 123, "direct_child_reaped": True})
        old_build = json.loads(self.paths["build"].read_text())
        old_build["core_dependency"]["sources"] = {name: self.file("core_" + name, root / "core" / name, b"// inert upstream\n")
            for name in ("crates/meerkat-runtime/src/meerkat_machine/session_management.rs",
                "crates/meerkat-mob/src/runtime/session_service.rs")}
        old_build["meta_dependency"]["gateway"] = self.file("meta_gateway",
            root / "meta/crates/meerkat-meta/src/rpc_gateway.rs", b"// inert gateway\n")
        build = self.document("current_build", {"selection": selection, "cargo": cargo, "direct_child": child,
            "binaries": binaries, "core_dependency": old_build["core_dependency"], "meta_dependency": old_build["meta_dependency"]})
        postflight = self.document("postflight", {"status": "passed", "source": build_source, "inputs": inputs,
            "census": {"forbidden_processes": []}})
        emission = self.file("current_emission", output / "RESULT.json", (json.dumps(self.changed("current_emission", {
            "status": "passed", "selection": selection, "build": build, "postflight": postflight})) + "\n").encode())
        build_root = self.document("build_root_review", {"status": "terminal_pass_reviewed", "source_head": head,
            "source": build_source, "result": emission, "findings": []})
        build_peer = self.document("build_peer_review", {"status": "terminal_pass_reviewed", "source_head": head,
            "source": build_source, "result": emission, "root_review": build_root, "findings": []})
        outer_stdout = self.file("outer_stdout", root / "outer.stdout", b"")
        outer = self.document("outer", {"status": "terminal", "exit_code": 0, "direct_child_reaped": True,
            "pid": 456, "deadline_expired": False, "received_signals": [], "primary_failure": None,
            "cooperative_interrupt_sent": False, "cwd": str(source), "stdout": outer_stdout, "stderr": stderr,
            "argv": ["python", runner["path"], "--inputs-sha256", inputs["sha256"], "--admission-sha256", admission["sha256"]]})
        supervision = self.document("supervision", {"status": "passed", "exit_code": 0, "source": build_source,
            "inputs": inputs, "output": str(output), "first_signal": None, "signal_counts": {},
            "restore_errors": [], "exception_class": None})
        evidence = self.document("reader_evidence", {"schema": "historical-g-current-reader-build-v1", "context": context,
            "build_bindings": {"build_result": emission, "root_build_review": build_root,
                "peer_build_review": build_peer, "outer_terminal": outer},
            "supervision": supervision, "resource_inputs": resource_inputs})
        current.update(source=artifact(build_source), root_source_review=artifact(source_reviews["current_source_root_review"]),
            peer_source_review=artifact(source_reviews["current_source_peer_review"]), emission=artifact(emission),
            resource_receipt=artifact(resource), reader_evidence=artifact(evidence))


class ReusedCurrentBindingTests(unittest.TestCase):
    def fixture(self, changes=None, **kwargs):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        return ReuseFixture(Path(directory.name).resolve(), changes, **kwargs)

    def refused(self, changes):
        with self.assertRaises(CurrentBindingRefused):
            verify_current(self.fixture(changes).current)

    def test_exact_reused_components_keep_old_identity_and_create_nothing(self):
        f = self.fixture()
        before = {str(p): p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
        self.assertIsNone(verify_current(f.current))
        self.assertEqual(before, {str(p): p.read_bytes() for p in f.root.rglob("*") if p.is_file()})

    def test_exact_unconsumed_guide_delta_is_allowed(self):
        self.assertIsNone(verify_current(self.fixture(doc_delta=True).current))

    def test_current_only_guide_cannot_hide_consumed_source(self):
        with self.assertRaises(CurrentBindingRefused):
            verify_current(self.fixture({"resource_inputs": lambda r: r["producer_sources"].update({
                "docs/site/guides/host-composition.mdx": next(iter(r["producer_sources"].values()))})},
                doc_delta=True).current)

    def test_actual_case_alias_joins_the_same_selection_file(self):
        self.assertIsNone(verify_current(self.fixture(selection_alias=True).current))

    def test_rejects_unrelated_selection_paths_and_incompatible_output(self):
        f = self.fixture()
        selected = f.paths["current_selection"]
        output = selected.parent
        same_bytes = output / "unrelated.json"
        same_bytes.write_bytes(selected.read_bytes())
        different = output / "mismatched.json"
        different.write_bytes(b"{}\n")
        same_inode = output / "same-inode.json"
        os.link(selected, same_inode)
        for path in (same_bytes, different, same_inode):
            with self.subTest(path=path.name), self.assertRaises(CurrentBindingRefused):
                _same_build_selection(pin(path), pin(selected), str(output), Documents())
        other = output / "other"
        other.mkdir()
        with self.assertRaises(CurrentBindingRefused):
            _same_build_selection(pin(selected), pin(selected), str(other), Documents())

    def test_rejects_distinct_case_sibling_with_equal_or_different_bytes(self):
        f = self.fixture()
        selected = f.paths["current_selection"]
        alias = selected.with_name("SELECTION.json")
        if alias.exists():
            self.skipTest("filesystem aliases case siblings; distinct same-name files cannot be created")
        for data in (selected.read_bytes(), b"{}\n"):
            with self.subTest(same_bytes=data == selected.read_bytes()):
                alias.write_bytes(data)
                self.assertFalse(alias.samefile(selected))
                with self.assertRaises(CurrentBindingRefused):
                    _same_build_selection(pin(alias), pin(selected), str(selected.parent), Documents())

    def test_closed_reader_and_build_evidence(self):
        self.refused({"reader_evidence": lambda r: r.update(extra=True)})
        self.refused({"reader_evidence": lambda r: r["build_bindings"].update(extra=True)})
        self.refused({"reader_evidence": lambda r: r.update(schema="pending")})

    def test_cross_source_context_graph_and_selection(self):
        for name in ("context", "current_graph_peer", "current_selection"):
            with self.subTest(name=name):
                self.refused({name: lambda r: r.update(source_head="0" * 40)})
        self.refused({"current_graph_root": lambda r: r.update(exact_graph_equal_after_candidate_root_relocation=False)})

    def test_root_cannot_masquerade_as_six_field_peer(self):
        self.refused({"reader_evidence": lambda r: r["build_bindings"].update(peer_build_review=r["build_bindings"]["root_build_review"])})
        self.refused({"build_peer_review": lambda r: r.update(root_review={"path": "/wrong", "sha256": "0" * 64, "size": 0})})
        self.refused({"build_peer_review": lambda r: r.update(extra=True)})

    def test_rejects_nonterminal_or_interrupted_owner(self):
        for change in ({"exit_code": 1}, {"direct_child_reaped": False}, {"deadline_expired": True},
                       {"received_signals": [2]}, {"primary_failure": "OSError"}):
            with self.subTest(change=change):
                self.refused({"outer": lambda r, change=change: r.update(change)})
        self.refused({"supervision": lambda r: r.update(first_signal=2)})
        self.refused({"supervision": lambda r: r.update(exception_class="OSError")})
        self.refused({"child": lambda r: r.update(direct_child_reaped=False)})
        self.refused({"child": lambda r: r.update(owner_returned_normally=False)})

    def test_rejects_compiler_and_binary_mismatches(self):
        self.refused({"current_cargo": lambda r: r.update(timed_out=True)})
        self.refused({"current_build": lambda r: r["binaries"]["meerkat_connect"].update(path="/wrong")})
        self.refused({"current_build": lambda r: r["binaries"].pop("toolkit_mobkit")})
        self.refused({"current_events": lambda r: r[-1].update(success=False)})
        self.refused({"current_events": lambda r: r.append(copy.deepcopy(r[0]))})
        self.refused({"current_build": lambda r: r["core_dependency"].update(revision="0" * 40)})

    def test_rejects_fresh_identity_relabel_and_wrong_resource_terminal(self):
        self.refused({"retained_resource": lambda r: r.update(source_head="a" * 40)})
        self.refused({"retained_resource_run": lambda r: r.update(cwd="/wrong")})
        self.refused({"retained_resource_run": lambda r: r.update(timed_out=True)})
        self.refused({"resource_peer": lambda r: r.update(findings=["unresolved"])})

    def test_rejects_reuse_inventory_and_consumed_source_mismatches(self):
        self.refused({"reuse": lambda r: r.update(changed_paths=[])})
        self.refused({"current_inventory": lambda r: r["sdk/example.py"].update(sha256="0" * 64)})
        self.refused({"current_inventory": lambda r: r["sdk/example.py"].update(sha256="0" * 64),
            "reuse": lambda r: r["changed_paths"].append("sdk/example.py")})
        self.refused({"expected_sdk": lambda r: r["connect"].update(revision="a" * 40)})
        self.refused({"resource_inputs": lambda r: r["producer_sources"].update({
            "scripts/fixtures/historical_g_current_binding.py": next(iter(r["producer_sources"].values()))})})

    def test_rejects_rebound_truncated_resource_input_map(self):
        f = self.fixture()
        original = json.loads(f.paths["resource_inputs"].read_text())
        original["producer_sources"].pop("scripts/connect-candidate.py")
        replacement = f.document("truncated_resource_inputs", original)
        evidence = json.loads(f.paths["reader_evidence"].read_text())
        evidence["resource_inputs"] = replacement
        f.current["reader_evidence"] = artifact(f.document("rebound_reader_evidence", evidence))
        with self.assertRaises(CurrentBindingRefused):
            verify_current(f.current)
        self.refused({"payload_review": lambda r: r.update(source_head="a" * 40)})
        self.refused({"payload_review": lambda r: r.update(files=[])})

    def test_rejects_postflight_and_changed_physical_binary(self):
        self.refused({"postflight": lambda r: r["census"].update(forbidden_processes=["unjoined"])})
        f = self.fixture()
        f.paths["binary"].write_bytes(b"changed")
        with self.assertRaises(CurrentBindingRefused):
            verify_current(f.current)


if __name__ == "__main__":
    unittest.main()
