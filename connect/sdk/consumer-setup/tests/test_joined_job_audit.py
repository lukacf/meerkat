"""Joined Controller diagnostics with inert package effects, never native proof."""
import asyncio
import importlib.util
import json
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

from test_backend import FixtureMixin, backend


def load(name, filename, **injected):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).resolve().parents[1] / filename)
    module = importlib.util.module_from_spec(spec)
    for key, value in injected.items():
        setattr(module, key, value)
    spec.loader.exec_module(module)
    return module


adapter = load("joined_audit_adapter", "mcp_adapter.py", _retained_backend=backend)
local = load("joined_audit_local", "local_session.py")


class JoinedJobAudit(FixtureMixin, unittest.IsolatedAsyncioTestCase):
    def owner(self):
        controller = self.controller()
        census = adapter.SharedEntryCensus(self.root / ("census-" + str(self.attempts)), "a" * 64)
        owner = adapter.ControllerConnections(controller, _entry_census=census)
        return controller, owner, owner.open(), census

    async def call(self, owner, peer, route, arguments, **extra):
        command = {"operation": "prepare_entry", "peer": 0, "connection": "1",
            "tool": "setup_" + route.removeprefix("/api/"), "arguments": arguments,
            "preparation": None, **extra}
        token = owner.owner_control(command)["entry"]
        try:
            value = await peer.dispatch(route, arguments, _entry=token)
            return value, token
        finally:
            owner.owner_control({"operation": "close_entry", "peer": 0, "entry": token})
            owner.owner_control({"operation": "finish_entry", "peer": 0, "entry": token})

    async def settled(self, failed=False):
        controller, owner, peer, census = self.owner()
        controller.runner.nonzero_after_publish = failed
        try:
            value, token = await self.call(owner, peer, "/api/prepare", {"family": "agno"})
            await asyncio.to_thread(controller.join)
        finally:
            await owner.aclose()
        return controller, owner, census, value["job"]["id"], token

    async def test_preparation_only_has_direct_terminal_row_only_after_real_join(self):
        controller, owner, peer, census = self.owner()
        controller.runner.block = threading.Event()
        try:
            value, token = await self.call(owner, peer, "/api/prepare", {"family": "agno"})
            identifier = value["job"]["id"]
            self.assertTrue(await asyncio.to_thread(controller.runner.entered.wait, 2))
            self.assertEqual(controller.job_detail({"job_id": identifier})["job"]["status"], "running")
            self.assertIsNone(owner.joined_job_audit())
            controller.runner.block.set()
            await asyncio.to_thread(controller.join)
        finally:
            controller.runner.block.set()
            await owner.aclose()
        row = owner.joined_job_audit()["jobs"][0]
        self.assertEqual((row["id"], row["kind"], row["status"], row["terminal"], row["worker_join"]),
                         (identifier, "prepare", "succeeded", True, "joined"))
        self.assertEqual(row["origin"], {"entry": token,
            "request_sha256": backend.digest((census.directory / (token + ".request.json")).read_bytes()),
            "physical_intent": None})
        self.assertEqual(row["error"], {"present": False, "category": None})
        self.assertTrue(owner.joined_job_audit()["complete"])
        original = owner.closed_snapshot()
        self.assertEqual(set(original), {"schema_version", "scope", "rounds"})
        self.assertEqual(original["rounds"][0]["requests"], [])
        self.assertEqual(original["rounds"][0]["preparations"], [])
        self.assertEqual(original["rounds"][0]["views"][0]["jobs"], [])

    async def test_unconfirmed_worker_is_diagnostic_not_reusable_close(self):
        controller, owner, census, identifier, token = await self.settled(failed=True)
        row = owner.joined_job_audit()["jobs"][0]
        self.assertEqual((row["id"], row["status"], row["terminal"], row["worker_join"]),
                         (identifier, "unconfirmed", True, "joined"))
        self.assertEqual(row["error"], {"present": True, "category": "operation_unconfirmed"})
        with self.assertRaises(backend.SetupError) as caught:
            owner.closed_snapshot()
        self.assertEqual(caught.exception.code, "shared_close_unavailable")
        self.assertNotIn("untrusted child output", json.dumps(owner.joined_job_audit()))
        # The existing succeeded-only writer returns without a reuse record;
        # the additive writer still retains the actual joined failure row.
        local.observe_shared_close(adapter, self.root, {}, owner, "d" * 64)
        self.assertFalse((self.root / "shared-close.json").exists())
        local.observe_shared_job_audit(self.root, owner, "d" * 64)
        retained = json.loads((self.root / ("shared-jobs-audit-" + "d" * 64 + ".json")).read_bytes())
        self.assertEqual(retained["audit"]["jobs"][0]["status"], "unconfirmed")

    async def test_observation_keeps_original_creation_entry_and_physical_pin(self):
        controller, owner, peer, census = self.owner()
        physical = {"id": "b" * 64, "sha256": "c" * 64}
        try:
            value, token = await self.call(owner, peer, "/api/prepare", {"family": "agno"},
                physical_intent=physical, native_request={"call_id": "not-retained-by-audit"})
            identifier = value["job"]["id"]
            await asyncio.to_thread(controller.join)
            observed, later = await self.call(owner, peer, "/api/job", {"job_id": identifier})
            self.assertEqual(observed["job"]["status"], "succeeded")
            self.assertNotEqual(token, later)
        finally:
            await owner.aclose()
        row = owner.joined_job_audit()["jobs"][0]
        self.assertEqual(row["origin"]["entry"], token)
        self.assertEqual(row["origin"]["physical_intent"], physical)
        self.assertNotIn("not-retained-by-audit", json.dumps(row))

    async def test_corrupted_or_missing_request_is_not_a_claimed_origin(self):
        controller, owner, census, identifier, token = await self.settled()
        path = census.directory / (token + ".request.json")
        raw = path.read_bytes()
        for data in (b"{}", None):
            if data is None:
                path.unlink()
            else:
                path.write_bytes(data)
            audit = owner.joined_job_audit()
            self.assertIsNone(audit["jobs"][0]["origin"])
            self.assertFalse(audit["complete"])
            self.assertEqual(audit["jobs"][0]["status"], "succeeded")
            path.write_bytes(raw)
        self.assertTrue(owner.joined_job_audit()["complete"])

    async def test_error_projection_never_retains_untrusted_payload(self):
        controller, owner, census, identifier, token = await self.settled(failed=True)
        for value in ("SECRET-TOKEN\nconfiguration payload", {"credential": "SECRET-TOKEN"}):
            controller.jobs[identifier]["error"] = value
            row = owner.joined_job_audit()["jobs"][0]
            self.assertEqual(row["error"], {"present": True, "category": "other"})
            self.assertNotIn("SECRET-TOKEN", json.dumps(row))
        self.assertNotIn("result", row)
        self.assertNotIn("family", row)

    async def test_bound_and_capture_failure_cannot_change_existing_job(self):
        controller, owner, census, identifier, token = await self.settled()
        before = dict(controller.jobs[identifier])
        # Synthetic rows exercise only the diagnostic bound, not real workers.
        for index in range(257):
            controller.jobs[f"{index:032x}"] = {"id": f"{index:032x}",
                "kind": "prepare", "status": "running"}
        audit = owner.joined_job_audit()
        self.assertEqual(len(audit["jobs"]), 256)
        self.assertTrue(audit["truncated"])
        self.assertFalse(audit["complete"])
        self.assertEqual(controller.jobs[identifier], before)
        owner._job_audit_entries = {f"{index:032x}": "a" * 64 for index in range(256)}
        owner._remember_job_audit(identifier, token)
        self.assertFalse(owner._job_audit_complete)
        self.assertEqual(len(owner._job_audit_entries), 256)
        self.assertEqual(controller.jobs[identifier], before)

    async def test_separate_write_preserves_old_close_and_swallows_only_diagnostic_errors(self):
        controller, owner, census, identifier, token = await self.settled()
        original = backend.encoded(owner.closed_snapshot())
        old = self.root / "shared-close.json"
        old.write_bytes(original)
        identity = "d" * 64
        local.observe_shared_job_audit(self.root, owner, identity)
        path = self.root / ("shared-jobs-audit-" + identity + ".json")
        saved = path.read_bytes()
        value = json.loads(saved)
        self.assertEqual(value["owner"], identity)
        self.assertEqual(value["audit"]["jobs"][0]["id"], identifier)
        self.assertEqual(old.read_bytes(), original)
        local.observe_shared_job_audit(self.root, owner, identity)
        self.assertEqual(path.read_bytes(), saved)
        with patch.object(local, "write_new", side_effect=OSError("inert fixture")):
            local.observe_shared_job_audit(self.root, owner, "e" * 64)
        self.assertEqual(old.read_bytes(), original)
        with patch.object(owner, "joined_job_audit", side_effect=asyncio.CancelledError):
            with self.assertRaises(asyncio.CancelledError):
                local.observe_shared_job_audit(self.root, owner, "e" * 64)

    async def test_incomplete_owner_join_refuses_audit_publication(self):
        controller, owner, peer, census = self.owner()
        try:
            local.observe_shared_job_audit(self.root, owner, "f" * 64)
            self.assertFalse((self.root / ("shared-jobs-audit-" + "f" * 64 + ".json")).exists())
        finally:
            await owner.aclose()
        self.assertIsNotNone(owner.joined_job_audit())
        for field, value in (("_pending", {"unsettled": True}), ("_job_waits", {"unsettled": True})):
            original = getattr(owner, field)
            setattr(owner, field, value)
            self.assertIsNone(owner.joined_job_audit())
            setattr(owner, field, original)


if __name__ == "__main__":
    unittest.main()
