"""Read-only recovery bookmarks use existing immutable request anchors."""
import copy
import http.client
import json
import os
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

from test_reconciliation import ReconciliationMixin, backend
from test_recovery import inventory
import test_recovery_selection as selection
import test_mcp_adapter as mcp


class RecoveryManifestTests(ReconciliationMixin, unittest.TestCase):
    def controller(self, **changes):
        return super().controller(**{"runner_factory": selection.MixedRunner, **changes})

    pair = selection.RecoverySelectionTests.pair

    def test_installed_request_exports_exact_canonical_bookmark_without_effects(self):
        owner, _, identifier = self.installed()
        before = inventory(owner.workspace)
        calls = copy.deepcopy(owner.runner.calls)
        result = owner.dispatch("/api/recovery_manifest", {"job_ids": [identifier]})
        raw = backend.encoded({"schema_version": 1, "workspace": str(owner.workspace),
            "requests": [owner.jobs[identifier]["recovery_request"]]})
        self.assertEqual(result, {"scope": "retained_setup_requests_only",
            "filename": "toolkit-recovery.json", "media_type": "application/json",
            "text": raw.decode(), "bytes": len(raw), "sha256": backend.digest(raw)})
        self.assertEqual(owner.state()["recovery_manifest"], {"available": True, "max_requests": 256})
        self.assertEqual(owner.runner.calls, calls)
        self.assertEqual(inventory(owner.workspace), before)

    def test_recovery_uses_original_root_and_selected_order_with_no_reconcile(self):
        original, identifiers = self.pair()
        ordered = identifiers[::-1]
        raw = original.recovery_manifest({"job_ids": ordered})["text"].encode()
        path = self.root / "bookmark.json"
        path.write_bytes(raw)
        recovered = self.controller(recover_manifest=path, recover_manifest_sha256=backend.digest(raw))
        before = inventory(original.workspace), inventory(recovered.workspace)
        jobs = copy.deepcopy(recovered.jobs)
        with patch.object(recovered, "_run", side_effect=AssertionError("worker entered")):
            result = recovered.recovery_manifest({"job_ids": ordered})
        self.assertEqual(result["text"].encode(), raw)
        value = json.loads(result["text"])
        self.assertEqual(value["workspace"], str(original.workspace))
        self.assertNotEqual(value["workspace"], str(recovered.workspace))
        self.assertEqual([row["id"] for row in value["requests"]], ordered)
        self.assertEqual(recovered.jobs, jobs)
        self.assertTrue(all(row["error"] == "historical_outcome_unknown" for row in jobs.values()))
        self.assertEqual((inventory(original.workspace), inventory(recovered.workspace)), before)
        recovered.close_admission()
        self.assertEqual(recovered.recovery_manifest({"job_ids": ordered}), result)

    def test_running_request_is_exportable_before_worker_settlement(self):
        owner = self.prepared()
        request = self.checked(owner)
        owner.runner.block = threading.Event()
        try:
            job = owner.install(request)["job"]
            self.assertEqual(owner.jobs[job["id"]]["status"], "running")
            result = owner.recovery_manifest({"job_ids": [job["id"]]})
            root, rows = backend.read_recovery_selection(result["text"].encode())
            self.assertEqual(root, owner.workspace)
            self.assertEqual(rows[0]["id"], job["id"])
            self.assertEqual(owner.jobs[job["id"]]["status"], "running")
        finally:
            owner.runner.block.set()
            owner.join()

    def test_mixed_configured_selected_and_export_requests(self):
        selected = type("Selected", (selection.selected.SelectedFixture, unittest.TestCase), {})()
        selected.setUp(); self.addCleanup(selected.tearDown)
        exported = type("Exported", (selection.hosted.HostedFixture, unittest.TestCase), {})()
        exported.setUp(); self.addCleanup(exported.tearDown)
        settings = {"selected_install_profile": selected.profile_path,
                    "selected_install_profile_sha256": selected.profile_sha,
                    "local_recipes_profile": exported.profile_path,
                    "local_recipes_profile_sha256": exported.profile_sha}
        owner, identifiers = self.pair(**settings, allow_remote_recipes=True)
        checked = owner.selected_check(selected.stage())
        identifiers.append(owner.selected_install(selected.selection(checked))["job"]["id"])
        owner.join()
        owner.remember_remote(selection.hosted.REQUEST)
        identifiers.append(owner.export_recipe(selection.hosted.EXPORT)["job"]["id"])
        owner.join()
        calls, before = copy.deepcopy(owner.runner.calls), inventory(owner.workspace)
        result = owner.recovery_manifest({"job_ids": identifiers})
        _, entries = backend.read_recovery_selection(result["text"].encode())
        self.assertEqual([row["kind"] for row in entries],
            ["toolkit.consumer_setup.configured_install_request"] * 2 +
            [backend.SELECTED_REQUEST, backend.REMOTE_EXPORT_REQUEST])
        self.assertEqual(owner.runner.calls, calls)
        self.assertEqual(inventory(owner.workspace), before)

    def test_acquisitions_reexport_without_retired_workers_or_payloads(self):
        for kind in ("python", "node", "compose"):
            with self.subTest(kind=kind):
                case = selection.acquisition.AcquisitionRecoveryTests()
                self.addCleanup(case.doCleanups)
                fixture, original, _ = case.publication(kind)
                identifier = original.job["id"]
                original_result = original.recovery_manifest({"job_ids": [identifier]})
                # Historical immutable requests remain, but execution assets need not.
                for path in (original.workspace / "acquisitions" / identifier).rglob("*"):
                    if path.is_file():
                        path.unlink()
                recovered = case.recovered(original)
                self.assertIsNone(getattr(recovered, kind + "_acquisition"))
                with patch.object(recovered, "_run", side_effect=AssertionError("worker entered")):
                    self.assertEqual(recovered.recovery_manifest({"job_ids": [identifier]}), original_result)
                self.assertEqual(recovered.job["error"], "historical_outcome_unknown")

    def test_invalid_selection_unknown_and_unanchored_refuse_without_partial_export(self):
        owner, _, identifier = self.installed()
        preparation = next(key for key, job in owner.jobs.items() if job["kind"] == "prepare")
        for value in ({}, {"job_ids": []}, {"job_ids": [identifier], "extra": True},
                      {"job_ids": None}, {"job_ids": "all"}, {"job_ids": [identifier] * 2},
                      {"job_ids": [identifier + "\n"]}, {"job_ids": [False]},
                      {"job_ids": [f"{i:032x}" for i in range(257)]}):
            with self.subTest(value=value), patch.object(backend, "read_file", side_effect=AssertionError("read")):
                self.error("invalid_request", lambda: owner.recovery_manifest(value))
        for value, code in (([identifier, "0" * 32], "unknown_job"),
                            ([identifier, preparation], "recovery_manifest_unavailable")):
            with patch.object(backend, "read_file", side_effect=AssertionError("partial read")):
                self.error(code, lambda: owner.recovery_manifest({"job_ids": value}))

    def test_anchor_identity_hash_and_same_root_required(self):
        owner, identifiers = self.pair()
        identifier = identifiers[1]
        intent = owner.install_intents[identifier]
        original = copy.deepcopy(intent["recovery_anchor"])
        for change in ({"id": identifiers[0]}, {"sha256": "0" * 64},
                       {"root": self.root}, {"root": str(owner.workspace)}):
            intent["recovery_anchor"] = {**original, **change}
            self.error("recovery_manifest_unavailable", lambda: owner.recovery_manifest({"job_ids": identifiers}))
        intent["recovery_anchor"] = original
        del intent["recovery_anchor"]
        self.error("recovery_manifest_unavailable", lambda: owner.recovery_manifest({"job_ids": identifiers}))

    def test_missing_tampered_symlink_fifo_and_oversized_anchors_refuse(self):
        owner, identifiers = self.pair()
        path = owner.workspace / "requests" / (identifiers[1] + ".json")
        raw = path.read_bytes()
        for mode in ("missing", "tampered", "symlink", "fifo", "oversized"):
            with self.subTest(mode=mode):
                path.unlink()
                if mode == "tampered":
                    path.write_bytes(raw + b" ")
                elif mode == "symlink":
                    path.symlink_to(owner.workspace / "requests" / (identifiers[0] + ".json"))
                elif mode == "fifo":
                    os.mkfifo(path)
                elif mode == "oversized":
                    path.write_bytes(b" " * (backend.MAX_BODY * 2 + 1))
                with self.assertRaises((backend.SetupError, OSError)):
                    owner.recovery_manifest({"job_ids": identifiers})
                if path.exists() or path.is_symlink():
                    path.unlink()
                path.write_bytes(raw)

    def test_loader_and_projection_share_request_envelope_binding(self):
        owner, _, identifier = self.installed()
        path = owner.workspace / "requests" / (identifier + ".json")
        original = backend.decode(path.read_bytes())
        for change in ({"job_id": "0" * 32}, {"workspace": str(self.root)},
                       {"kind": "unknown_kind"}):
            with self.subTest(change=change):
                raw = backend.encoded({**original, **change})
                path.write_bytes(raw)
                sha = backend.digest(raw)
                owner.install_intents[identifier]["recovery_anchor"]["sha256"] = sha
                owner.jobs[identifier]["recovery_request"]["sha256"] = sha
                manifest = backend.encoded({"schema_version": 1, "workspace": str(owner.workspace),
                    "requests": [{"id": identifier, "sha256": sha}]})
                selected = self.root / "selection.json"
                selected.write_bytes(manifest)
                self.error("invalid_recovery_request", lambda: owner.recovery_manifest({"job_ids": [identifier]}))
                self.error("invalid_recovery_request", lambda: backend.load_recovery_selection(selected, backend.digest(manifest)))

    def test_output_loss_does_not_block_bookmark_or_claim_readiness(self):
        owner, _, identifier = self.installed()
        before = owner.recovery_manifest({"job_ids": [identifier]})
        job = copy.deepcopy(owner.jobs[identifier])
        Path(job["result"]["receipt_path"]).unlink()
        Path(job["result"]["mcp_path"]).unlink()
        with patch.object(owner, "_run", side_effect=AssertionError("verifier entered")):
            self.assertEqual(owner.recovery_manifest({"job_ids": [identifier]}), before)
        self.assertEqual(owner.jobs[identifier], job)
        self.assertEqual(set(json.loads(before["text"])), {"schema_version", "workspace", "requests"})

    def test_maximum_real_anchor_selection_and_output_bound(self):
        owner, _, identifier = self.installed()
        original = backend.decode((owner.workspace / "requests" / (identifier + ".json")).read_bytes())
        identifiers = [f"{i:032x}" for i in range(256)]
        for key in identifiers:
            raw = backend.encoded({**original, "job_id": key})
            (owner.workspace / "requests" / (key + ".json")).write_bytes(raw)
            anchor = {"id": key, "sha256": backend.digest(raw)}
            owner.jobs[key] = {"recovery_request": anchor}
            owner.install_intents[key] = {"recovery_anchor": {**anchor, "root": owner.workspace}}
        result = owner.recovery_manifest({"job_ids": identifiers})
        self.assertEqual(len(json.loads(result["text"])["requests"]), 256)
        self.assertLessEqual(result["bytes"], backend.MAX_BODY)
        with patch.object(backend, "MAX_BODY", result["bytes"] - 1), patch.object(
                backend, "read_file", side_effect=AssertionError("read before bound")):
            self.error("invalid_recovery_manifest", lambda: owner.recovery_manifest({"job_ids": identifiers}))

    def test_http_export_uses_existing_origin_and_session_authority(self):
        owner, _, identifier = self.installed()
        server = backend.Server(owner)
        worker = threading.Thread(target=server.serve_forever)
        worker.start()
        try:
            for token, status in (("wrong", 403), (server.capability, 200)):
                connection = http.client.HTTPConnection(server.host, timeout=3)
                connection.request("POST", "/api/recovery_manifest", backend.encoded({"job_ids": [identifier]}),
                    {"Host": server.host, "Origin": server.origin, "X-Toolkit-Session": token,
                     "Content-Type": "application/json"})
                response = connection.getresponse()
                raw = response.read(); connection.close()
                self.assertEqual(response.status, status)
                if status == 200:
                    self.assertEqual(json.loads(raw), owner.recovery_manifest({"job_ids": [identifier]}))
        finally:
            server.shutdown(); server.server_close(); worker.join()


class RecoveryManifestMCPTests(mcp.reconciliation.ReconciliationMixin, unittest.IsolatedAsyncioTestCase):
    async def test_selected_only_surface_retains_manifest_tool(self):
        fixture = type("Selected", (selection.selected.SelectedFixture, unittest.TestCase), {})()
        fixture.setUp(); self.addCleanup(fixture.tearDown)
        original, _, job = fixture.installed()
        recovered = fixture.recover(original, job)
        for owner in (original, recovered):
            async with mcp.connection(owner) as (client, _):
                self.assertIn("setup_recovery_manifest", {row.name for row in (await client.list_tools()).tools})
                result = await client.call_tool("setup_recovery_manifest", {"job_ids": [job["id"]]})
                self.assertFalse(result.is_error)
                self.assertEqual(json.loads(result.structured_content["text"])["workspace"], str(original.workspace))

    async def test_real_mcp_live_and_single_recovery_export_without_mutation_tools(self):
        original, _, identifier = self.installed()
        expected = original.recovery_manifest({"job_ids": [identifier]})
        recovered = self.controller(recover_workspace=original.workspace, recover_request=identifier,
            recover_request_sha256=original.jobs[identifier]["recovery_request"]["sha256"])
        for owner in (original, recovered):
            calls, before = copy.deepcopy(owner.runner.calls), inventory(owner.workspace)
            async with mcp.connection(owner) as (client, _):
                tools = {item.name: item for item in (await client.list_tools()).tools}
                tool = tools["setup_recovery_manifest"]
                self.assertTrue(tool.annotations.read_only_hint)
                self.assertTrue(tool.annotations.idempotent_hint)
                if owner.recovery_only:
                    self.assertEqual(set(tools), {"setup_state", "setup_reconcile", "setup_recovery_manifest"})
                result = await client.call_tool("setup_recovery_manifest", {"job_ids": [identifier]})
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content, expected)
                for arguments in ({"job_ids": []}, {"job_ids": [identifier] * 2},
                                  {"job_ids": [identifier + "\n"]}, {"job_ids": ["0" * 32]}):
                    refused = await client.call_tool("setup_recovery_manifest", arguments)
                    self.assertTrue(refused.is_error)
            self.assertEqual(owner.runner.calls, calls)
            self.assertEqual(inventory(owner.workspace), before)


if __name__ == "__main__":
    unittest.main()
