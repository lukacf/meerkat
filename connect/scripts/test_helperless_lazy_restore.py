"""Pure oracle and callback-forwarding regressions; no SDK or verifier import.

The AST loader compiles only the selected pure functions and observer method.
These synthetic observations are negative/positive oracle tests, never native
Restore, callback, process, cleanup or fixed-binary acceptance evidence.
"""
import ast
import copy
import json
from pathlib import Path
import threading
import types
import unittest


def require(value, message):
    if not value:
        raise AssertionError(message)


def selected_source():
    path = Path(__file__).with_name("verify_supplied_shared_workflow.py")
    tree = ast.parse(path.read_text())
    functions = [node for node in tree.body if isinstance(node, ast.FunctionDef)
                 and node.name in ("require_helperless_restore_pre", "require_helperless_restore_wake")]
    scenario = next(node for node in tree.body if isinstance(node, ast.ClassDef) and node.name == "HelperlessScenario")
    observe = next(node for node in scenario.body if isinstance(node, ast.FunctionDef) and node.name == "observe_runtime")
    require(len(functions) == 2, "exact oracle definitions missing")
    module = ast.Module(body=functions + [observe], type_ignores=[])
    namespace = {"require": require, "copy": copy, "json": json,
                 "C3_ID": "acceptance-ordinary-member", "C3_CLASS": "acceptance-ordinary-class",
                 "C3_TOOL": "acceptance_nonce_read"}
    exec(compile(module, str(path) + ":selected-pure-test", "exec"), namespace)
    return namespace


class ColdRestoreOracleTests(unittest.TestCase):
    def setUp(self):
        self.source = selected_source()
        build = {"method": "callback/agent_customizer/customize_build", "completed": True,
                 "identity": "acceptance-ordinary-member", "spec_identity": "acceptance-ordinary-member",
                 "profile": "acceptance-ordinary-class", "scope_id": "scope-1",
                 "context": {"identity": "acceptance-ordinary-member", "active_peers": ["peer"],
                             "managed_edges": [{"from": "a", "to": "b"}, {"from": "b", "to": "c"}]}}
        self.before = {"phase": "prepared", "builds": [{"identity": "acceptance-ordinary-member", "profile": "acceptance-ordinary-class"}],
            "build_contexts": [{"model": "fixture", "labels": {}, "system_prompt": None}],
            "created": [{"identity": "acceptance-ordinary-member", "session_id": "same-session"}],
            "created_contexts": [{"model": "fixture"}],
            "calls": [{"nonce": "before", "handler_build": 1}], "model_requests": 2,
            "callbacks": [copy.deepcopy(build)], "callback_overflow": False}
        self.pre = copy.deepcopy(self.before)
        self.pre.update(phase="restored", callbacks=[])
        self.after = copy.deepcopy(self.pre)
        self.after.update(builds=self.before["builds"] * 2, build_contexts=self.before["build_contexts"] * 2,
            calls=[{"nonce": "before", "handler_build": 1}, {"nonce": "after", "handler_build": 2}],
            model_requests=4, callbacks=[copy.deepcopy(build),
                {"method": "callback/call_tool", "completed": True, "scope_id": "scope-1",
                 "tool": "acceptance_nonce_read", "nonce": "after",
                 "handler_result": {"nonce": "after", "handler_build": 2}}])

    def verify(self):
        return self.source["require_helperless_restore_wake"](self.before, self.pre, self.after)

    def test_exact_cold_wake_and_repeated_cross_process_scope_string_pass(self):
        self.assertEqual(self.verify(), {"scope_id": "scope-1", "new_builds": 1,
            "restored_after_create": 0, "handler_result": {"nonce": "after", "handler_build": 2}})

    def test_no_pre_input_build_or_work(self):
        for key in ("builds", "calls", "model_requests", "callbacks"):
            with self.subTest(key=key):
                original = copy.deepcopy(self.pre)
                self.pre[key] = self.after[key]
                with self.assertRaises(AssertionError): self.verify()
                self.pre = original

    def test_missing_or_duplicate_build_and_after_create_are_rejected(self):
        for rows in (self.after["callbacks"][1:], self.after["callbacks"][:1] * 2 + self.after["callbacks"][1:],
                     self.after["callbacks"] + [{"method": "callback/agent_customizer/after_create", "completed": True}]):
            with self.subTest(rows=rows):
                original = self.after["callbacks"]
                self.after["callbacks"] = rows
                with self.assertRaises(AssertionError): self.verify()
                self.after["callbacks"] = original

    def test_wrong_scope_nonce_or_handler_is_rejected(self):
        for key, value in (("scope_id", "old-scope"), ("nonce", "before"),
                           ("handler_result", {"nonce": "after", "handler_build": 1})):
            with self.subTest(key=key):
                original = copy.deepcopy(self.after["callbacks"][1])
                self.after["callbacks"][1][key] = value
                with self.assertRaises(AssertionError): self.verify()
                self.after["callbacks"][1] = original

    def test_managed_edges_are_a_set_but_peers_and_draft_remain_exact(self):
        self.after["callbacks"][0]["context"]["managed_edges"].reverse()
        self.verify()
        self.after["callbacks"][0]["context"]["active_peers"] = ["foreign"]
        with self.assertRaises(AssertionError): self.verify()
        self.after["callbacks"][0]["context"]["active_peers"] = ["peer"]
        self.after["build_contexts"][1] = {"model": "changed", "labels": {}, "system_prompt": None}
        with self.assertRaises(AssertionError): self.verify()

    def test_incomplete_exception_and_overflow_cannot_pass(self):
        for update in ({"completed": False}, {"exception": "RuntimeError"}):
            original = copy.deepcopy(self.after["callbacks"][0])
            self.after["callbacks"][0].update(update)
            with self.assertRaises(AssertionError): self.verify()
            self.after["callbacks"][0] = original
        self.after["callback_overflow"] = True
        with self.assertRaises(AssertionError): self.verify()

    def test_historical_creation_must_remain_exact(self):
        self.after["created"] = self.before["created"] * 2
        with self.assertRaises(AssertionError): self.verify()


class CallbackForwardingTests(unittest.IsolatedAsyncioTestCase):
    async def test_forwarded_once_same_result_and_exception_objects(self):
        source = selected_source()
        calls, returned = [], {"opaque": object()}
        error = RuntimeError("actual callback failure")
        async def original(method, params):
            calls.append((method, params))
            if method == "callback/agent_customizer/after_create": raise error
            return returned
        dispatcher = types.SimpleNamespace(handle_callback=original)
        runtime = types.SimpleNamespace(_dispatcher=dispatcher)
        fixture = types.SimpleNamespace(phase="restored", callback_observations={},
            callback_dispatchers=[], callback_lock=threading.Lock())
        source["observe_runtime"](fixture, runtime)
        # Mirrors the inspected bootstrap assignment, not real native routing.
        registered = dispatcher.handle_callback
        params = {"scope_id": "current", "tool": "acceptance_nonce_read", "arguments": {"nonce": "after"}}
        self.assertIs(await registered("callback/call_tool", params), returned)
        self.assertIs(calls[0][1], params)
        with self.assertRaises(RuntimeError) as caught:
            await registered("callback/agent_customizer/after_create", {"identity": "member", "session_id": "session"})
        self.assertIs(caught.exception, error)
        self.assertEqual(len(calls), 2)
        rows = fixture.callback_observations["restored"]["rows"]
        self.assertTrue(rows[0]["completed"])
        self.assertFalse(rows[1]["completed"])
        self.assertEqual(rows[1]["exception"], "RuntimeError")

    async def test_overflow_is_observed_without_changing_original_dispatch(self):
        source = selected_source()
        calls = []
        async def original(method, params):
            calls.append((method, params))
            return None
        fixture = types.SimpleNamespace(phase="restored", callback_observations={},
            callback_dispatchers=[], callback_lock=threading.Lock())
        runtime = types.SimpleNamespace(_dispatcher=types.SimpleNamespace(handle_callback=original))
        source["observe_runtime"](fixture, runtime)
        for _ in range(65):
            self.assertIsNone(await runtime._dispatcher.handle_callback("callback/agent_customizer/after_create", {}))
        self.assertEqual(len(calls), 65)
        self.assertEqual(len(fixture.callback_observations["restored"]["rows"]), 64)
        self.assertTrue(fixture.callback_observations["restored"]["overflow"])


if __name__ == "__main__":
    unittest.main()
