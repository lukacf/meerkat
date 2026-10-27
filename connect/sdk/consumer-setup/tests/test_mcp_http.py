"""Hosted checks through the real pinned MCP SDK, HTTP client and local TLS."""
import copy
import hashlib
import importlib.util
import json
import logging
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import anyio
import httpx2

from http_verification_server import INPUT, OUTPUT, Peer

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("http_checker_under_test", ROOT / "verify_mcp.py")
checker = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = checker
spec.loader.exec_module(checker)


def pin(path):
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


class HttpVerificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mcp-http-")
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name).resolve()
        self.config = self.work / "mcp.json"
        self.request_path = self.work / "request.json"
        self.env = {"PATH": "/usr/bin:/bin", "HOME": str(self.work)}
        self.peer = None

    def plan(self, mode="normal", action="discover", hostname="localhost"):
        if self.peer is not None:
            self.peer.close()
        self.peer = Peer(self.work, mode, hostname)
        self.addCleanup(self.peer.close)
        self.config.write_text(json.dumps({"$schema": checker.PORTABLE_SCHEMA, "mcpServers": {"selected": {
            "type": "streamable-http", "url": self.peer.url,
            "headers": {"X-Configured": "${PRIVATE_LITERAL}"}}}}))
        result = {"schema_version": 3, "action": action,
                  "connection": {"transport": "streamable-http", "mcp_json": pin(self.config),
                                 "server": "selected", "headers": {"Authorization": "Bearer PRIVATE_TOKEN"},
                                 "mode": "auto", "ca_certificate": pin(self.peer.ca)},
                  "timeout_seconds": 4, "max_report_bytes": 4096}
        if action == "call":
            result.update(expected_tools=[{"name": "increment", "input_schema": copy.deepcopy(INPUT),
                                           "output_schema": copy.deepcopy(OUTPUT)}],
                          calls=[{"name": "increment", "arguments": {"n": 7}}])
        return result

    def change_config(self, plan, mutate):
        config = json.loads(self.config.read_text())
        mutate(config)
        self.config.write_text(json.dumps(config))
        plan["connection"]["mcp_json"] = pin(self.config)

    def start(self, plan, env=None):
        self.request_path.write_text(json.dumps(plan))
        return subprocess.Popen([sys.executable, "-I", "-B", str(ROOT / "verify_mcp.py"),
                                 "--request", str(self.request_path), "--request-sha256", pin(self.request_path)["sha256"]],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env or self.env, cwd=self.work)

    def finish(self, process):
        try:
            out, err = process.communicate(timeout=20)
        except BaseException:
            process.kill(); process.wait(); raise
        evidence = os.environ.get("TOOLKIT_HTTP_TEST_EVIDENCE")
        if evidence:
            directory = Path(evidence)
            directory.mkdir(parents=True, exist_ok=True)
            index = len(list(directory.glob(self._testMethodName + "-*.json")))
            (directory / (self._testMethodName + "-" + str(index) + ".json")).write_text(json.dumps({
                "exit_code": process.returncode, "stdout": out.decode(), "stderr": err.decode(),
                "wire_events": self.peer.events}, indent=2) + "\n")
        self.assertEqual(err, b"")
        self.assertNotIn(b"PRIVATE_", out)
        self.assertNotIn(self.peer.url.encode(), out)
        result = json.loads(out)
        self.assertLessEqual(len(out), 4097)
        self.assertEqual(process.returncode, 0 if result["status"] == "observed" else 2)
        return result

    def check(self, plan, env=None):
        return self.finish(self.start(plan, env))

    def messages(self, method):
        return [event for event in self.peer.events if event["message"] and event["message"].get("method") == method]

    def test_exported_config_runtime_discovery_and_literal_headers(self):
        result = self.check(self.plan())
        self.assertEqual(result["status"], "observed")
        self.assertEqual(result["schema_version"], 3)
        self.assertEqual(result["transport"], "streamable-http")
        self.assertEqual(result["transport_context_scope"], "local_http_and_mcp_contexts")
        self.assertTrue(result["transport_context_closed"])
        self.assertEqual(result["tls_trust"], "pinned_ca")
        self.assertEqual(result["ca_certificate_sha256"], pin(self.peer.ca)["sha256"])
        self.assertEqual(result["scope"], "untrusted_sdk_visible_tool_declarations")
        self.assertEqual(result["tools"][0]["description"], self.peer.tag)
        self.assertEqual(self.messages("tools/call"), [])
        for event in self.peer.events:
            headers = {name.lower(): value for name, value in event["headers"].items()}
            self.assertEqual(headers["authorization"], "Bearer PRIVATE_TOKEN")
            self.assertEqual(headers["x-configured"], "${PRIVATE_LITERAL}")
            self.assertEqual(event["path"], "/mcp?bound=PRIVATE_QUERY")

    def test_explicit_calls_match_schemas_before_each_call(self):
        plan = self.plan(action="call")
        plan["calls"].append({"name": "increment", "arguments": {"n": 9}})
        result = self.check(plan)
        self.assertEqual(result["status"], "observed")
        self.assertEqual([call["result"]["structuredContent"]["value"] for call in result["calls"]], [8, 10])
        methods = [e["message"]["method"] for e in self.peer.events if e["message"]]
        self.assertEqual([m for m in methods if m.startswith("tools/")],
                         ["tools/list", "tools/call", "tools/list", "tools/call"])

    def test_empty_null_absent_and_paginated_discovery(self):
        for mode in ("empty", "nulls", "absent", "pages"):
            with self.subTest(mode=mode):
                result = self.check(self.plan(mode))
                self.assertEqual(result["status"], "observed")
                self.assertEqual(self.messages("tools/call"), [])
                if mode == "empty":
                    self.assertEqual(result["tools"], [])
                elif mode == "nulls":
                    self.assertIsNone(result["tools"][0]["outputSchema"])
                elif mode == "absent":
                    self.assertNotIn("outputSchema", result["tools"][0])
                else:
                    self.assertEqual(len(result["tools"]), 2)

    def test_ambiguous_nonfinal_and_oversized_discovery_never_returns_partial(self):
        for mode in ("duplicate", "cycle", "nonfinal", "overflow"):
            with self.subTest(mode=mode):
                result = self.check(self.plan(mode))
                self.assertEqual(result["status"], "stopped")
                self.assertNotIn("tools", result)
                self.assertEqual(self.messages("tools/call"), [])

    def test_changed_schema_stops_before_next_call(self):
        plan = self.plan("later_changed", action="call")
        plan["calls"] *= 2
        result = self.check(plan)
        self.assertEqual(result["reason"], "selected_tool_missing_or_changed")
        self.assertEqual(len(self.messages("tools/call")), 1)

    def test_sdk_param_headers_come_from_selected_schema_and_arguments(self):
        plan = self.plan("param_header", action="call")
        plan["expected_tools"][0]["input_schema"]["properties"]["n"]["x-mcp-header"] = "Number"
        result = self.check(plan)
        self.assertEqual(result["status"], "observed")
        headers = {key.lower(): val for key, val in self.messages("tools/call")[0]["headers"].items()}
        self.assertEqual(headers["mcp-param-number"], "7")

    def test_no_redirect_follow_or_business_replay(self):
        for mode in ("redirect", "cross_redirect"):
            with self.subTest(mode=mode):
                result = self.check(self.plan(mode, action="call"))
                self.assertEqual(result["status"], "stopped")
                self.assertEqual(result["calls"][0]["outcome"], "unknown")
                self.assertEqual(len(self.messages("tools/call")), 1)
                self.assertTrue(all(e["path"] == "/mcp?bound=PRIVATE_QUERY" for e in self.peer.events))

    def test_sse_resume_uses_get_and_never_replays_tool_post(self):
        result = self.check(self.plan("resume", action="call"))
        self.assertEqual(result["status"], "observed")
        self.assertEqual(result["calls"][0]["result"]["structuredContent"]["value"], 8)
        self.assertEqual(len(self.messages("tools/call")), 1)
        self.assertTrue(any(e["http_method"] == "GET" for e in self.peer.events))

    def test_endless_resume_is_bounded_by_deadline_without_replay(self):
        plan = self.plan("endless_resume", action="call"); plan["timeout_seconds"] = .5
        result = self.check(plan)
        self.assertEqual(result["reason"], "deadline_exceeded")
        self.assertEqual(result["calls"][0]["outcome"], "unknown")
        self.assertEqual(len(self.messages("tools/call")), 1)

    def test_input_required_rpc_error_and_dropped_call_do_not_retry(self):
        for mode in ("input", "rpc_error", "dropped"):
            with self.subTest(mode=mode):
                result = self.check(self.plan(mode, action="call"))
                self.assertEqual(result["status"], "stopped")
                self.assertEqual(len(self.messages("tools/call")), 1)
                self.assertEqual(result["calls"][0]["outcome"], "input_required" if mode == "input" else "unknown")

    def test_cookies_are_not_retained_across_legacy_session_requests(self):
        plan = self.plan("resume", action="call"); plan["connection"]["mode"] = "legacy"
        result = self.check(plan)
        self.assertEqual(result["status"], "observed")
        self.assertTrue(any(e["http_method"] == "DELETE" for e in self.peer.events))
        for event in self.peer.events:
            self.assertNotIn("cookie", {name.lower() for name in event["headers"]})

    def test_delete_failure_does_not_claim_remote_session_retirement(self):
        plan = self.plan("delete_error"); plan["connection"]["mode"] = "legacy"
        result = self.check(plan)
        self.assertEqual(result["status"], "observed")
        self.assertTrue(result["transport_context_closed"])
        self.assertEqual(result["transport_context_scope"], "local_http_and_mcp_contexts")
        self.assertNotIn("remote_session_closed", result)
        self.assertTrue(any(e["http_method"] == "DELETE" for e in self.peer.events))

    def test_ambient_proxy_ca_and_credentials_do_not_control_client(self):
        plan = self.plan()
        (self.work / ".netrc").write_text("machine 127.0.0.1 login PRIVATE_USER password PRIVATE_PASSWORD\n")
        env = dict(self.env, HTTPS_PROXY="http://PRIVATE_PROXY:1", HTTP_PROXY="http://PRIVATE_PROXY:1",
                   ALL_PROXY="http://PRIVATE_PROXY:1", SSL_CERT_FILE="/PRIVATE_CA", SSL_CERT_DIR="/PRIVATE_DIR",
                   PRIVATE_LITERAL="expanded")
        self.assertEqual(self.check(plan, env)["status"], "observed")
        plan["connection"].pop("ca_certificate")
        env["SSL_CERT_FILE"] = str(self.peer.ca)
        self.peer.events.clear()
        result = self.check(plan, env)
        self.assertEqual(result["status"], "stopped")
        self.assertEqual(result["tls_trust"], "system")
        self.assertEqual(self.peer.events, [])

    def test_unknown_ca_and_wrong_hostname_refused_before_http_messages(self):
        plan = self.plan(hostname="different.invalid")
        result = self.check(plan)
        self.assertEqual(result["status"], "stopped")
        self.assertEqual(self.peer.events, [])

    def test_auth_challenge_remains_redacted_and_never_calls_tools(self):
        result = self.check(self.plan("unauthorized"))
        self.assertEqual(result["status"], "stopped")
        self.assertEqual(self.messages("tools/call"), [])
        self.assertTrue(all(e["path"] == "/mcp?bound=PRIVATE_QUERY" for e in self.peer.events))

    def test_interrupt_preserves_dispatched_unknown_without_replay(self):
        process = self.start(self.plan("timeout", action="call"))
        self.assertTrue(self.peer.call_seen.wait(10))
        process.send_signal(signal.SIGTERM)
        result = self.finish(process)
        self.assertEqual(result["reason"], "interrupted_outcome_unknown")
        self.assertEqual(result["calls"][0]["outcome"], "unknown")
        self.assertEqual(len(self.messages("tools/call")), 1)

    def test_v3_fields_transport_and_call_permissions_are_explicit(self):
        original = self.plan()
        variants = []
        for field, val in (("launch", {}), ("calls", []), ("expected_tools", [])):
            plan = copy.deepcopy(original); plan[field] = val; variants.append(plan)
        for field, val in (("transport", "stdio"), ("mode", "unknown"), ("env", {}), ("url", self.peer.url), ("verify", False)):
            plan = copy.deepcopy(original); plan["connection"][field] = val; variants.append(plan)
        for plan in variants:
            with self.subTest(plan_fields=list(plan["connection"])):
                self.assertEqual(self.check(plan)["phase"], "preflight")
        self.assertEqual(self.peer.events, [])

    def test_endpoint_and_selected_entry_validation_prevent_network(self):
        for endpoint in ("http://127.0.0.1/", "https://PRIVATE_USER:PRIVATE_PASSWORD@localhost/",
                         "https://@localhost/", "https://localhost/#PRIVATE_FRAGMENT", "https://localhost/\n",
                         "https://localhost\\PRIVATE_PATH/", "https://localhost:PRIVATE_PORT/"):
            plan = self.plan()
            self.change_config(plan, lambda config: config["mcpServers"]["selected"].update(url=endpoint))
            self.assertEqual(self.check(plan)["phase"], "preflight")
            self.assertEqual(self.peer.events, [])
        for field, value in (("type", "sse"), ("command", "/PRIVATE_EXEC")):
            plan = self.plan()
            self.change_config(plan, lambda config: config["mcpServers"]["selected"].update({field: value}))
            self.assertEqual(self.check(plan)["phase"], "preflight")
            self.assertEqual(self.peer.events, [])

    def test_header_overlap_reserved_duplicates_and_limits_prevent_network(self):
        original = self.plan()
        bad_maps = [{"x-configured": "same-or-different"}, {"X-Duplicate": "a", "x-duplicate": "b"},
                    {"X-Bad": "PRIVATE_CR\r\nInjected: true"}, {"Bad Name": "x"}, {"X-Bad": "\t"},
                    {"X-Bad": "x" * 8193}, {"X-" + str(i): "x" for i in range(65)},
                    {"X-" + str(i): "x" * 8192 for i in range(9)}]
        bad_maps += [{name: "PRIVATE_VALUE"} for name in checker.RESERVED_HEADERS | {"Mcp-Param-Anything"}]
        for headers in bad_maps:
            plan = copy.deepcopy(original); plan["connection"]["headers"] = headers
            with self.subTest(header_names=list(headers)):
                self.assertEqual(self.check(plan)["phase"], "preflight")
        self.assertEqual(self.peer.events, [])

    def test_ca_and_config_hashes_regular_files_and_known_schema_required(self):
        original = self.plan()
        for field in ("ca_certificate", "mcp_json"):
            plan = copy.deepcopy(original); plan["connection"][field]["sha256"] = "0" * 64
            self.assertEqual(self.check(plan)["reason"], "input_size_or_digest_mismatch")
        link = self.work / "ca-link.pem"; link.symlink_to(self.peer.ca)
        plan = copy.deepcopy(original); plan["connection"]["ca_certificate"]["path"] = str(link)
        self.assertEqual(self.check(plan)["phase"], "preflight")
        plan = copy.deepcopy(original)
        self.change_config(plan, lambda config: config.update({"$schema": "https://PRIVATE_SCHEMA"}))
        self.assertEqual(self.check(plan)["reason"], "unsupported_mcp_schema")
        self.assertEqual(self.peer.events, [])

    def test_owned_client_is_closed_and_exact_endpoint_hook_refuses_change(self):
        plan = self.plan()
        self.request_path.write_text(json.dumps(plan))
        actual, captured = httpx2.AsyncClient, []
        def factory(**kwargs):
            client = actual(**kwargs)
            captured.append((client, kwargs))
            return client
        logging.disable(logging.CRITICAL)
        with patch.object(checker.httpx2, "AsyncClient", factory):
            result = anyio.run(checker.verify, str(self.request_path), pin(self.request_path)["sha256"])
        self.assertEqual(result["status"], "observed")
        client, kwargs = captured[0]
        self.assertTrue(client.is_closed)
        self.assertFalse(kwargs["trust_env"])
        self.assertEqual(kwargs["max_redirects"], 0)
        self.assertIsNone(kwargs["auth"])
        self.assertEqual(list(client.cookies), [])
        with self.assertRaisesRegex(checker.Rejected, "^http_endpoint_changed$"):
            anyio.run(kwargs["event_hooks"]["request"][0], httpx2.Request("GET", self.peer.url + "changed"))
        _, transport, _ = checker.load_request(str(self.request_path), pin(self.request_path)["sha256"])
        self.assertNotIn("PRIVATE_", repr(transport))
        self.assertTrue(transport.verify.check_hostname)


if __name__ == "__main__":
    unittest.main()
