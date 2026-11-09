"""Review-derived context controls; inert transport, no native acceptance claim."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import threading
import unittest

from test_browser_console_status import C, ObserverHarness, observation


SPEC = importlib.util.spec_from_file_location(
    "context_gateway_verifier", Path(__file__).with_name("verify_native_gateway_subprocess.py"))
V = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(V)


def review_fixture(principal):
    operation = V.ACQUISITION_OPERATION
    scope = {"realm": "mob.fixture", "owner": "toolkit/acquisition/installed-fixture"}
    source = V.installed_fixture_principal("event-source")
    attachment = {"destination": {"context": "session",
        "logical_owner": V.installed_fixture_principal("original-agent"),
        "audience": {"audience": "principal", "principal": principal}}}
    context = {"fresh_original": "fresh", "prior_original": {"authority": source, "work": "prior"},
        "business_task": {**scope, "id": "task"}, "live_attachment": attachment,
        "operator_and_requester": principal, "represented_subject": principal,
        "proposed_fresh_instruction": V.ACQUISITION_REQUEST}
    decision = {"route": V.ACQUISITION_ROUTE, "decision": "fresh", "choice": "approve",
        "connection_revision": "a" * 64, "live_attachment": attachment,
        "permission": {"target": "original", "connection": "server", "resource": "server",
            "operations": [operation], "lifetime": {
                "scope": {"kind": "task", "task": "fresh"}, "expires_at_ms": 1900000000000}}}
    review = {"review_id": 42, "can_start": False, "prompt": {"kind": "approval",
        "approval_id": "public-approval", "allowed_decisions": ["approve", "deny"],
        "body": {"access_request": {"software": {"job_id": "job", "family": "agno",
            "profile": "hackernews", "server_name": "server"},
            "phase": "installed_public_service_access", "authentication": {"kind": "none"},
            "application_selection": {"agent": "original", "operations": [operation], "expires_in_ms": None},
            "service_instruction": context}, "service_access": decision}}}
    results = {"original-request": {"operation": "task"}, "installed-job": {"job": {"id": "job"}},
        "reconciled-install": {"observation": {"result": {
            "mcp_configuration": {"mcpServers": {"server": {}}}}}}}
    return review, results, scope, source


class ContextHarness(ObserverHarness):
    def __init__(self, directory):
        super().__init__(directory, [], [])
        scenario = self.scenario
        scenario.application_tool_policy = None
        review, results, scenario.task_scope, scenario.source_authority = review_fixture(scenario.human())
        self.review = review
        self.provider.results = results
        self.provider.lock = threading.RLock()
        self.provider.expected_service_context = None
        for name in ("ACQUISITION_OPERATION", "ACQUISITION_ROUTE", "installed_fixture_principal",
                     "installed_approved_service_context", "canonical"):
            setattr(scenario.v, name, getattr(V, name))
        scenario.v.ACQUISITION_REQUEST = V.ACQUISITION_REQUEST
        self.before_answer_checked = False

    async def start(self, native_url):
        await super().start(native_url)
        row = observation(value={"review": self.review})
        # This runs inside front.start, before the Browser run-loop drains the
        # accepted event or any answer response. The hook must already bind.
        await self.emit([row, row])
        context = self.provider.expected_service_context
        assert context is not None
        user = lambda value: {"role": "user", "content": value}
        messages = [user(V.ACQUISITION_REQUEST),
                    {"role": "assistant", "content": V.ORIGINAL_A_FINISH},
                    user(V.canonical(context).decode()), user(V.ACQUISITION_REQUEST)]
        V.InstalledAcquisitionProvider.checked_original_catalog_history(
            self.provider_for_history(), messages, 3)
        for changed in (messages[:2] + messages[3:],
                        messages[:2] + [user("{}"), messages[3]]):
            try:
                V.InstalledAcquisitionProvider.checked_original_catalog_history(
                    self.provider_for_history(), changed, len(changed) - 1)
            except AssertionError:
                pass
            else:
                raise AssertionError("missing or substituted context was accepted")
        self.before_answer_checked = True

    def provider_for_history(self):
        provider = object.__new__(V.InstalledAcquisitionProvider)
        provider.controller_reprepare = False
        provider.expected_service_context = self.provider.expected_service_context
        return provider


class BrowserContextTests(unittest.TestCase):
    def test_review_hook_binds_before_answer_response_and_is_idempotent(self):
        with tempfile.TemporaryDirectory(prefix="inert-browser-context-") as scratch:
            harness = ContextHarness(Path(scratch))
            report, error = harness.run()
        self.assertIsNone(error)
        self.assertTrue(harness.before_answer_checked)
        self.assertEqual(len(report["http"]), 2)
        self.assertTrue(all(row["path"].endswith("/data") for row in report["http"]))
        self.assertEqual(report["decisions"], [])
        self.assertEqual(harness.scenario._approved_service_context,
                         V.canonical(harness.provider.expected_service_context).decode())

    def test_changed_or_foreign_review_cannot_replace_binding(self):
        with tempfile.TemporaryDirectory(prefix="inert-browser-context-") as scratch:
            harness = ContextHarness(Path(scratch))
            scenario = harness.scenario
            scenario.bind_access_context(harness.review, {"session_id": "session"},
                                         harness.provider.results, harness.provider)
            initial = copy.deepcopy(harness.provider.expected_service_context)
            for foreign in (False, True):
                changed = copy.deepcopy(harness.review)
                if foreign:
                    changed["prompt"]["body"]["service_access"]["permission"]["operations"] = ["foreign"]
                else:
                    changed["prompt"]["body"]["service_access"]["connection_revision"] = "b" * 64
                with self.assertRaises(AssertionError):
                    scenario.bind_access_context(changed, {"session_id": "session"},
                                                 harness.provider.results, harness.provider)
                self.assertEqual(harness.provider.expected_service_context, initial)

    def test_foreign_review_hook_fails_before_disclosure_or_binding(self):
        with tempfile.TemporaryDirectory(prefix="inert-browser-context-") as scratch:
            harness = ContextHarness(Path(scratch))
            harness.review["prompt"]["body"]["service_access"]["permission"]["operations"] = ["foreign"]
            report, error = harness.run()
        self.assertIsNotNone(error)
        self.assertFalse(harness.before_answer_checked)
        self.assertIsNone(harness.provider.expected_service_context)
        self.assertEqual(report["status"], "failed")
        self.assertEqual(len(report["observation_errors"]), 1)
        harness.events.add.assert_not_called()

    def test_native_completion_requires_the_same_selected_context(self):
        # Use the existing actual native-frame fixture and real completion
        # validator; only public history and result bytes are fabricated.
        from test_assemble_connect_product import InstalledConsoleBoundaryTests
        frames, accepted, context, business = InstalledConsoleBoundaryTests().journey()
        expected = '{"selected":"review"}'
        b = next(row for row in frames if row["id"] == "b-start")
        b["payload"]["input"]["content"].insert(0, {"type": "text", "text": expected})
        self.assertIsNotNone(C.completed_journey(frames, accepted, context, business, V,
                                               fresh_context=expected))
        for selected in (None, '{"selected":"different"}'):
            with self.assertRaises(AssertionError):
                C.completed_journey(frames, accepted, context, business, V, fresh_context=selected)
        b["payload"]["input"]["content"].pop(0)
        with self.assertRaises(AssertionError):
            C.completed_journey(frames, accepted, context, business, V, fresh_context=expected)


if __name__ == "__main__":
    unittest.main()
