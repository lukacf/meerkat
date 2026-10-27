"""Historical intent projection through existing current setup owners."""
import asyncio
import copy
import json
import unittest

from test_backend import FixtureMixin, backend

CANARY = "PRIVATE_UNSUPPORTED_VALUE_MUST_NOT_ESCAPE"


def historical(selection=None):
    selection = copy.deepcopy(selection or {"family": "agno", "profile": "searxng", "operations": ["search_web"],
        "values": {"host": "https://127.0.0.1:1234", "timeout": 10, "optional_flag": False}})
    return {"schema_version": 1, "kind": "toolkit.consumer_setup.configured_install_request",
        "job_id": "a" * 32, "validation_id": "b" * 32, "workspace": "/DO_NOT_READ_PRIOR_WORKSPACE",
        "selection": selection, "configuration_sha256": backend.digest(backend.configuration_bytes(selection)),
        "server_name": "toolkit-" + selection["family"] + "-" + "b" * 12,
        "contents_sha256": "c" * 64, "installer_sha256": "d" * 64,
        "runtimes": {"python": "/DO_NOT_EXECUTE_PRIOR_RUNTIME", "node": None, "npm_cli": None}}


def source(fixture, record=None, raw=None):
    path = fixture.root / "historical-request.json"
    data = raw if raw is not None else backend.encoded(record or historical())
    path.write_bytes(data)
    return {"prior_request_path": path, "prior_request_sha256": backend.digest(data)}


def proposal(controller):
    return controller.successor({"prior_request_sha256": controller.prior_request_sha256})


class SuccessorTests(FixtureMixin, unittest.TestCase):
    def test_explicit_frozen_input_requires_current_preparation_and_review(self):
        args = source(self); c = self.controller(**args)
        state = c.state()
        self.assertEqual(state["prior_selection"], {"family": "agno", "profile": "searxng", "request_sha256": args["prior_request_sha256"]})
        self.assertNotIn("values", json.dumps(state))
        self.assertEqual(state["family_contents_sha256"]["agno"], self.sha)
        self.assertEqual(proposal(c)["status"], "preparation_required"); self.assertEqual(c.runner.calls, [])
        args["prior_request_path"].write_text("changed after host freeze")
        c.prepare({"family": "agno"}); c.join()
        value = proposal(c)
        self.assertEqual(value["status"], "review_required")
        self.assertEqual(value["scope"], "successor_configuration_candidate")
        self.assertEqual(value["values"], historical()["selection"]["values"])
        self.assertEqual(value["prior_operations"], ["search_web"]); self.assertEqual(value["unapplied"], [])
        self.assertEqual(c.validations, {}); self.assertEqual(len(c.jobs), 1)
        self.assertNotIn("DO_NOT", json.dumps(value))
        self.assertEqual(value["reported_prior_contents_sha256"], "c" * 64)
        self.assertEqual(value["current_contents_sha256"], self.sha)

    def test_unapplied_fields_types_bounds_refs_and_operations_are_never_silently_lost(self):
        selection = historical()["selection"]
        selection["operations"].append("removed_operation")
        selection["values"].update(timeout="ten", optional_flag=0, removed_field=CANARY, host="")
        args = source(self, historical(selection)); c = self.controller(**args)
        before = args["prior_request_path"].read_bytes()
        c.prepare({"family": "agno"}); c.join(); value = proposal(c)
        self.assertEqual(value["values"], {})
        self.assertEqual({(r["kind"], r["name"]) for r in value["unapplied"]},
            {("operation", "removed_operation"), ("field", "timeout"), ("field", "optional_flag"), ("field", "removed_field"), ("field", "host")})
        self.assertNotIn(CANARY, json.dumps(value))
        self.assertEqual(args["prior_request_path"].read_bytes(), before)
        self.assertEqual((c.workspace / "prior-request.json").read_bytes(), before)
        self.assertEqual(c.validations, {})
        # Current owner alone accepts the explicitly repaired complete selection.
        fixed = self.request(); checked = c.check(fixed)
        self.assertEqual(checked["scope"], "configuration_only")
        c.install({**fixed, "validation_id": checked["validation_id"], "configuration_sha256": checked["configuration_sha256"]}); c.join()
        self.assertEqual(args["prior_request_path"].read_bytes(), before)
        self.assertNotEqual(checked["validation_id"], "b" * 32)

    def test_current_reference_and_integer_bounds_are_structural_not_semantic_proof(self):
        sel = {"family": "services", "profile": "slack", "operations": ["auth_test"],
            "values": {"token_env": CANARY + " invalid token", "timeout_seconds": 1000}}
        c = self.controller(**source(self, historical(sel))); c.prepare({"family": "services"}); c.join()
        value = proposal(c)
        self.assertEqual(value["values"], {})
        self.assertEqual({r["reason"] for r in value["unapplied"]}, {"current_reference_name_required", "outside_current_bounds"})
        self.assertNotIn(CANARY, json.dumps(value))

    def test_missing_profile_disabled_family_and_unconfirmed_never_choose_or_prepare(self):
        record = historical(); record["selection"]["profile"] = "missing"
        record["configuration_sha256"] = backend.digest(backend.configuration_bytes(record["selection"]))
        c = self.controller(**source(self, record)); c.prepare({"family": "agno"}); c.join()
        self.assertEqual(proposal(c)["status"], "profile_unavailable")
        self.assertEqual(len(c.jobs), 1)
        c = self.controller(families=("services",), **source(self))
        with self.assertRaisesRegex(backend.SetupError, "unknown_family"): proposal(c)
        c = self.controller(**source(self))
        for status in ["preparing", "unconfirmed"]:
            c.families["agno"]["status"] = status
            with self.assertRaisesRegex(backend.SetupError, "preparation_unconfirmed"): proposal(c)
        self.assertEqual(c.runner.calls, [])

    def test_bad_digest_tamper_unknown_fields_and_legacy_absence_refuse(self):
        args = source(self)
        with self.assertRaisesRegex(backend.SetupError, "prior_request_changed"):
            self.controller(**{**args, "prior_request_sha256": "f" * 64})
        c = self.controller(**args)
        with self.assertRaisesRegex(backend.SetupError, "prior_request_changed"):
            c.successor({"prior_request_sha256": "f" * 64})
        with self.assertRaises(backend.SetupError): c.successor({"prior_request_sha256": c.prior_request_sha256, "path": "/not/allowed"})
        (c.workspace / "prior-request.json").write_text("{}");
        with self.assertRaisesRegex(backend.SetupError, "prior_request_changed"): proposal(c)
        legacy = self.controller()
        self.assertNotIn("prior_selection", legacy.state())
        with self.assertRaisesRegex(backend.SetupError, "unavailable"): legacy.successor({"prior_request_sha256": "f" * 64})
        c.recovery_only = True
        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"): proposal(c)

    def test_host_source_arguments_are_paired_and_recovery_is_separate(self):
        args = source(self)
        for values in ({"prior_request_path": args["prior_request_path"]},
                       {"prior_request_sha256": args["prior_request_sha256"]},
                       {**args, "prior_request_path": args["prior_request_path"].relative_to(self.root)},
                       {**args, "recover_workspace": self.root / "unread-old-workspace"}):
            with self.subTest(values=list(values)), self.assertRaisesRegex(backend.SetupError, "invalid_prior_selection"):
                self.controller(**values)

    def test_strict_v1_envelope_and_scalar_types_no_old_runtime_execution(self):
        edits = [lambda r: r.update(schema_version=True), lambda r: r.update(schema_version=2),
            lambda r: r.update(extra=True), lambda r: r["selection"].update(family="official-github"),
            lambda r: r["selection"].update(operations=[]), lambda r: r["selection"]["operations"].append("search_web"),
            lambda r: r["selection"]["values"].update(timeout=9007199254740992),
            lambda r: r["selection"]["values"].update(timeout=[10]), lambda r: r.update(configuration_sha256="f" * 64)]
        for edit in edits:
            record = historical(); edit(record)
            with self.subTest(index=edits.index(edit)), self.assertRaisesRegex(backend.SetupError, "invalid_prior_request"):
                self.controller(**source(self, record))
        raw = backend.encoded(historical()).replace(b'"schema_version":1', b'"schema_version":1,"schema_version":1')
        with self.assertRaisesRegex(backend.SetupError, "invalid_prior_request"): self.controller(**source(self, raw=raw))


class SuccessorMcpTests(FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_proposal_then_existing_check_install_and_replay(self):
        import test_mcp_adapter as m
        args = source(self); c = self.controller(**args); original = m.adapter.backend; m.adapter.backend = backend
        before = args["prior_request_path"].read_bytes()
        try:
            async with m.connection(c) as (client, owner):
                self.assertIn("setup_successor", [t.name for t in (await client.list_tools()).tools])
                async def call(name, arguments):
                    result = await client.call_tool(name, arguments)
                    self.assertFalse(result.is_error, result); return result.structured_content
                params = {"prior_request_sha256": args["prior_request_sha256"]}
                first = await call("setup_successor", params)
                self.assertEqual(first["status"], "preparation_required"); self.assertEqual(c.runner.calls, [])
                await call("setup_prepare", {"family": "agno"}); await asyncio.to_thread(c.join)
                result = await call("setup_successor", params)
                self.assertEqual(result["status"], "review_required")
                selected = {"family": "agno", "profile": "searxng", "operations": ["search_web"], "values": result["values"]}
                checked = await call("setup_check", selected)
                install = {**selected, "validation_id": checked["validation_id"], "configuration_sha256": checked["configuration_sha256"]}
                job = await call("setup_install", install); await asyncio.to_thread(c.join)
                repeated = await call("setup_install", install)
                self.assertEqual(job["job"]["id"], repeated["job"]["id"])
                self.assertEqual(repeated["job"]["status"], "succeeded")
                self.assertEqual(args["prior_request_path"].read_bytes(), before)
                self.assertTrue((await client.call_tool("setup_successor", {**params, "path": "/wrong"})).is_error)
        finally: m.adapter.backend = original
