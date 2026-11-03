"""Controlled provider decisions from model-visible proof, not native acceptance."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest
from unittest import mock


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


V = load("currentness_verifier", "verify_native_gateway_subprocess.py")
C = load("currentness_console", "verify_installed_console.py")
PROOF = {"error": "operation_refused", "message": "opaque display text",
         "data": {"kind": "reprepare_required", "tool_invoked": False}}


def initialize(provider, _directory, events):
    # Exercise the real response method without starting its HTTP listener.
    provider.requests, provider.results, provider.issued = [], {}, set()
    provider.events = events


def provider(scenario_type=C.ConsoleScenario):
    scenario = object.__new__(scenario_type)
    scenario.v = V
    with mock.patch.object(V.InstalledAcquisitionProvider, "__init__", initialize):
        return scenario.provider(Path("unused"), mock.Mock())


def body():
    return {"model": "fixture-helper-model",
            "tools": [{"function": {"name": name}}
                      for name in ("tool_catalog_search", "tool_catalog_load")],
            "messages": [{"role": "user", "content": V.ACQUISITION_REQUEST}]}


def respond(owner, request):
    owner.requests.append(copy.deepcopy(request))
    return owner.response(request)["delta"]["tool_calls"][0]


def result(request, call, value):
    request["messages"].extend([
        {"role": "assistant", "content": None,
         "tool_calls": [{key: value for key, value in call.items() if key != "index"}]},
        {"role": "tool", "tool_call_id": call["id"], "content": json.dumps(value)},
    ])


def successful_search():
    return {"catalog_exact": True, "pending_sources": [],
            "results": [{"name": name, "visibility_status": "deferred"}
                        for name in V.ACQUISITION_MCP_TOOLS]}


def successful_load():
    return {"catalog_exact": True, "accepted_names": list(V.ACQUISITION_MCP_TOOLS),
            "noop_names": [], "resolutions": [
                {"name": name, "accepted": True} for name in V.ACQUISITION_MCP_TOOLS]}


class ConsoleHelperCurrentnessTests(unittest.TestCase):
    def first_refusal(self, scenario_type=C.ConsoleScenario):
        owner, request = provider(scenario_type), body()
        first = respond(owner, request)
        self.assertEqual(first["function"]["name"], "tool_catalog_search")
        self.assertEqual(json.loads(first["function"]["arguments"]), {"query": "setup", "limit": 50})
        result(request, first, copy.deepcopy(PROOF))
        return owner, request, first

    def check_fresh(self, owner, request, first):
        fresh = respond(owner, request)
        self.assertNotEqual(fresh["id"], first["id"])
        self.assertEqual(fresh["function"], first["function"])
        self.assertEqual(owner.issued, {first["id"], fresh["id"]})
        self.assertEqual(owner.results[first["id"]], PROOF)
        self.assertEqual(json.loads(request["messages"][-1]["content"]), PROOF)
        return fresh

    def test_console_and_browser_continue_only_after_fresh_search_and_load(self):
        for scenario in (C.ConsoleScenario, C.BrowserConsoleScenario):
            with self.subTest(scenario=scenario.__name__):
                owner, request, first = self.first_refusal(scenario)
                fresh = self.check_fresh(owner, request, first)
                result(request, fresh, successful_search())
                loaded = respond(owner, request)
                self.assertEqual(loaded["function"]["name"], "tool_catalog_load")
                self.assertEqual(json.loads(loaded["function"]["arguments"]),
                                 {"names": list(V.ACQUISITION_MCP_TOOLS)})
                result(request, loaded, successful_load())
                request["tools"].extend({"function": {"name": name}}
                                        for name in V.ACQUISITION_MCP_TOOLS)
                schema = respond(owner, request)
                self.assertEqual(schema["function"]["name"], "setup_inspect_schema")
                self.assertEqual(owner.results[first["id"]], PROOF)
                self.assertEqual(len([row for row in owner.events.add.call_args_list
                                      if row.args[0] == "controlled_model_tool"]), 4)

    def test_visible_tools_do_not_turn_refusal_into_successful_search_or_load(self):
        owner, request, first = self.first_refusal()
        request["tools"].extend({"function": {"name": name}} for name in V.ACQUISITION_MCP_TOOLS)
        fresh = self.check_fresh(owner, request, first)
        result(request, fresh, successful_search())
        loaded = respond(owner, request)
        self.assertEqual(loaded["function"]["name"], "tool_catalog_load")
        result(request, loaded, successful_load())
        self.assertEqual(respond(owner, request)["function"]["name"], "setup_inspect_schema")

    def test_older_load_cannot_satisfy_the_fresh_search_decision(self):
        owner, request, first = self.first_refusal()
        # Supply older, valid load evidence adversarially. Visibility alone
        # must not bypass the search/load sequence after the fresh decision.
        older = {"id": "helper-catalog-load-older", "type": "function",
                 "function": {"name": "tool_catalog_load",
                              "arguments": json.dumps({"names": list(V.ACQUISITION_MCP_TOOLS)})}}
        history = {"messages": []}
        result(history, older, successful_load())
        request["messages"][1:1] = history["messages"]
        owner.issued.add(older["id"])
        fresh = respond(owner, request)
        self.assertNotEqual(fresh["id"], first["id"])
        self.assertEqual(fresh["function"], first["function"])
        result(request, fresh, successful_search())
        request["tools"].extend({"function": {"name": name}} for name in V.ACQUISITION_MCP_TOOLS)
        loaded = respond(owner, request)
        self.assertEqual(loaded["function"]["name"], "tool_catalog_load")
        self.assertNotEqual(loaded["id"], older["id"])
        result(request, loaded, successful_load())
        self.assertEqual(respond(owner, request)["function"]["name"], "setup_inspect_schema")

    def test_second_proven_refusal_stops_without_a_third_search(self):
        owner, request, first = self.first_refusal()
        fresh = self.check_fresh(owner, request, first)
        result(request, fresh, copy.deepcopy(PROOF))
        before = set(owner.issued)
        with self.assertRaisesRegex(AssertionError, "fresh helper search.*bound"):
            respond(owner, request)
        self.assertEqual(owner.issued, before)

    def test_repeated_request_cannot_reissue_the_fresh_call(self):
        owner, request, first = self.first_refusal()
        self.check_fresh(owner, request, first)
        before = set(owner.issued)
        with self.assertRaisesRegex(AssertionError, "does not replay tool effects"):
            respond(owner, request)
        self.assertEqual(owner.issued, before)

    def test_changed_historical_refusal_remains_fatal(self):
        owner, request, first = self.first_refusal()
        fresh = self.check_fresh(owner, request, first)
        changed = copy.deepcopy(PROOF)
        changed["message"] = "changed historical display"
        request["messages"][-1]["content"] = json.dumps(changed)
        result(request, fresh, successful_search())
        with self.assertRaisesRegex(AssertionError, "retained tool result changed"):
            respond(owner, request)

    def test_only_exact_closed_nonentry_data_permits_a_fresh_search(self):
        values = [
            {"error": "operation_refused", "message": "reprepare_required tool_invoked false"},
            {**PROOF, "error": "another_error"},
            {**PROOF, "data": None},
            {**PROOF, "data": []},
            {**PROOF, "data": {"kind": "reprepare_required"}},
            {**PROOF, "data": {"tool_invoked": False}},
            {**PROOF, "data": {**PROOF["data"], "extra": True}},
        ]
        values += [{**PROOF, "data": {"kind": kind, "tool_invoked": False}}
                   for kind in ("denied", "malformed_facts", "unknown", None)]
        values += [{**PROOF, "data": {"kind": "reprepare_required", "tool_invoked": invoked}}
                   for invoked in (True, "unknown", "false", None, 0, 1, [], {})]
        for value in values:
            with self.subTest(value=value):
                owner, request, first = self.first_refusal()
                request["messages"][-1]["content"] = json.dumps(value)
                # Hidden corroboration must never authorize this decision.
                request["authorization_audit"] = {"kind": "reprepare_required", "tool_invoked": False}
                before = set(owner.issued)
                with self.assertRaisesRegex(AssertionError, "actual tool result .* refused"):
                    respond(owner, request)
                self.assertEqual(owner.issued, before)

    def test_foreign_unissued_and_other_operations_remain_fatal(self):
        for case in ("original_lane", "unissued", "load", "install", "access", "business"):
            with self.subTest(case=case):
                owner, request, first = self.first_refusal()
                if case == "original_lane":
                    request["model"] = "fixture-original-model"
                elif case == "unissued":
                    owner.issued.clear()
                else:
                    identifier = {"load": "helper-catalog-load-1", "install": "install-once",
                                  "access": "public-task-access", "business": "business-read"}[case]
                    request["messages"][-1]["tool_call_id"] = identifier
                    owner.issued.add(identifier)
                before = set(owner.issued)
                with self.assertRaisesRegex(AssertionError, "actual tool result .* refused"):
                    respond(owner, request)
                self.assertEqual(owner.issued, before)

    def test_default_nonconsole_provider_does_not_opt_in(self):
        with mock.patch.object(V.InstalledAcquisitionProvider, "__init__", initialize):
            owner = V.InstalledAcquisitionProvider(Path("unused"), mock.Mock())
        request = body()
        first = respond(owner, request)
        result(request, first, copy.deepcopy(PROOF))
        with self.assertRaisesRegex(AssertionError, "actual tool result .* refused"):
            respond(owner, request)

    def test_nonentry_proof_does_not_restore_missing_request_or_tool(self):
        for missing in ("request", "tool"):
            with self.subTest(missing=missing):
                owner, request, first = self.first_refusal()
                if missing == "request":
                    request["messages"][0]["content"] = "unrelated setup request"
                else:
                    request["tools"] = []
                before = set(owner.issued)
                with self.assertRaises(AssertionError):
                    respond(owner, request)
                self.assertEqual(owner.issued, before)


if __name__ == "__main__":
    unittest.main()
