"""Credential-specific review context tests; no credentials or native I/O."""
import copy
import hashlib
from pathlib import Path
import threading
import unittest
from unittest import mock

from test_browser_console_context import C, V, review_fixture
import test_assemble_connect_product as assembly_tests


def fixture():
    scenario = C.TrelloConsoleScenario(V, "http://127.0.0.1:18444")
    review, results, scenario.task_scope, scenario.source_authority = review_fixture(C.human())
    scenario._review_owner = {"operator": "operator", "application": "application",
        "realm": "mob.fixture", "route": V.ACQUISITION_ROUTE,
        "purpose": "operations", "represented_subject": C.human()}
    body = review["prompt"]["body"]
    body.update(scenario._review_owner)
    body["connector_selection"] = {"server": "server", "connector": None}
    request, decision = body["access_request"], body["service_access"]
    context = request["service_instruction"]
    context["proposed_fresh_instruction"] = C.TRELLO_REQUEST
    configuration = {"name": "server", "command": "/inert/python", "args": [],
        "env": {}, "secret_env": sorted(C.TRELLO_SLOTS),
        "access": {"allowed_operations": [C.TRELLO_OPERATION]}}
    context["configuration_persistence"] = {
        "kind": "requested_defining_realm_persistence", "defining_realm": "mob.fixture",
        "expected_configuration_generation": 3, "configuration": configuration,
        "credential_slots": sorted(C.TRELLO_SLOTS),
        "requested_effects": "persist this exact configuration, then privately store the declared credentials",
        "authority": "requires current authenticated native realm administration at each mutation entry",
        "partial_effects": "configuration or credentials can remain after cancellation or later failure"}
    decision["permission"]["operations"] = [C.TRELLO_OPERATION]
    decision["permission"]["lifetime"]["expires_at_ms"] = 4102444800000
    decision["connection_revision"] = hashlib.sha256(V.canonical(configuration)).hexdigest()
    request.update(phase="installed_credential_service_access",
        authentication={"kind": "declared_secret_slots"},
        operation_basis={"kind": "package_published_metadata"}, installation=configuration,
        application_selection={"agent": "original", "operations": [C.TRELLO_OPERATION], "expires_in_ms": None})
    request["software"].update(profile="trello", validation_id="validation",
        receipt_sha256="receipt", configuration_sha256="checked")
    results["checked-install"] = {"validation_id": "validation", "configuration_sha256": "checked"}
    results["installed-job"]["job"]["status"] = "succeeded"
    results["reconciled-install"]["observation"].update(status="matched", job_id="job")
    results["reconciled-install"]["observation"]["result"].update(
        family="agno", profile="trello", receipt_sha256="receipt", configuration_sha256="checked",
        mcp_configuration={"mcpServers": {"server": {"command": "/inert/python"}}})
    review["credential_input"] = {"kind": "declared_secret_slots", "slots": [
        {"slot": C.TRELLO_SLOTS[0], "label": "config.api_key_env"},
        {"slot": C.TRELLO_SLOTS[1], "label": "config.token_env"}]}
    return scenario, review, results, {"identity": "original", "session_id": "session"}


class CredentialContextTests(unittest.TestCase):
    def test_binds_before_any_protected_submission_without_secret_values(self):
        scenario, review, results, accepted = fixture()
        calls = []

        async def http(method, path, body=None, **options):
            self.assertIsNotNone(scenario._credential_service_context)
            calls.append((method, path))
            return {"review": review} if method == "GET" else {}

        work = scenario.access_answer(http, review, accepted, results, {})
        try:
            work.send(None)
        except StopIteration as done:
            answer, context, path, kind = done.value
        self.assertEqual((path, kind), (C.PANEL + "/credentials", "credential_access"))
        expected = V.installed_approved_service_context({
            "decision": review["prompt"]["body"]["service_access"],
            "acquisition": context["business_task"]})
        self.assertEqual(scenario._credential_service_context, expected)
        raw = V.canonical(expected)
        self.assertTrue(all(value.encode() not in raw for value in scenario._credential_values.values()))
        self.assertEqual(len(calls), 12)
        self.assertEqual(len(answer["credentials"]), 2)

    def test_foreign_credential_review_never_binds(self):
        for target in ("operation", "session", "slot"):
            scenario, review, results, accepted = fixture()
            body = review["prompt"]["body"]
            if target == "operation":
                body["service_access"]["permission"]["operations"] = ["foreign"]
            elif target == "session":
                accepted["session_id"] = "foreign"
            else:
                review["credential_input"]["slots"][0]["slot"] = "foreign"
            with self.subTest(target=target), self.assertRaises(AssertionError):
                scenario.credential_answer(review, accepted, results)
            self.assertIsNone(scenario._credential_service_context)

    def test_provider_requires_exact_reviewed_context_without_vendor_call(self):
        for change in ("none", "missing", "substituted", "unbound"):
            scenario, review, results, accepted = fixture()
            scenario.credential_answer(review, accepted, results)
            with mock.patch.object(V.InstalledAcquisitionProvider, "__init__", return_value=None):
                provider = scenario.provider(Path("unused"), None)
            result = {"operation": "task", "created": True, "worker": {"state": "input_admitted",
                "deduplicated": False, "source_settlement": "fresh_acceptance_promoted"}}
            provider.results, provider.issued, provider.requests = (
                {"original-request": result}, {"original-request"}, [])
            provider.events, provider.business_observation = mock.Mock(), None
            provider.business_response_prepared = threading.Event()
            messages = [{"role": "user", "content": C.TRELLO_REQUEST},
                {"role": "tool", "tool_call_id": "original-request", "content": V.canonical(result).decode()},
                {"role": "assistant", "content": V.ORIGINAL_A_FINISH},
                {"role": "user", "content": V.canonical(scenario._credential_service_context).decode()},
                {"role": "user", "content": C.TRELLO_REQUEST}]
            if change == "missing": messages.pop(3)
            if change == "substituted": messages[3]["content"] = "{}"
            if change == "unbound": scenario._credential_service_context = None
            body = {"model": "fixture-original-model", "messages": messages,
                "tools": [{"function": {"name": C.TRELLO_OPERATION}}]}
            with self.subTest(change=change):
                if change == "none":
                    self.assertEqual(provider.response(body)["delta"]["content"], C.TRELLO_FINISH)
                    self.assertEqual(provider.business_observation["vendor_operation"], "not_requested")
                else:
                    with self.assertRaises(AssertionError): provider.response(body)

    def test_native_completion_requires_exact_context(self):
        scenario, review, results, accepted = fixture()
        scenario.credential_answer(review, accepted, results)
        fixture_case = assembly_tests.InstalledConsoleBoundaryTests()
        fixture_case.console = C
        frames, accepted, context, business = fixture_case.credential_journey()
        # Its fixture module loads a distinct copy with the same contract.
        expected = V.canonical(scenario._credential_service_context).decode()
        b = next(row for row in frames if row["id"] == "b-start")
        b["payload"]["input"]["content"] = [
            {"type": "text", "text": expected}, {"type": "text", "text": C.TRELLO_REQUEST}]
        self.assertIsNotNone(scenario.completed(frames, accepted, context, business))
        for replacement in ("{}", None):
            wrong = copy.deepcopy(frames)
            content = next(row for row in wrong if row["id"] == "b-start")["payload"]["input"]["content"]
            if replacement is None: content.pop(0)
            else: content[0]["text"] = replacement
            with self.assertRaises(AssertionError):
                scenario.completed(wrong, accepted, context, business)


if __name__ == "__main__":
    unittest.main()
