"""Focused error transport checks; real CLI/store composition is reviewed separately."""
import copy
import unittest
from pathlib import Path
from unittest.mock import patch
from mcp import Client
import test_hosted_recipes as hosted

backend, adapter = hosted.backend, hosted.adapter


def refusal(code="mcp_schema", **extra):
    return hosted.fixtures.envelope("error", error="invalid_bundle",
                                   diagnostic={"phase": "before_store_open", "code": code}, **extra)


class DiagnosticTests(hosted.HostedFixture, unittest.TestCase):
    def test_known_pre_store_diagnostics_have_fixed_repair_help(self):
        for code in backend.RECIPE_REJECTION_HELP:
            with self.subTest(code=code):
                c = self.controller()
                with patch.object(c.runner, "run", return_value=(1, backend.encoded(refusal(code)))):
                    with self.assertRaises(backend.SetupError) as caught:
                        c.remember_remote(hosted.REQUEST)
                error = caught.exception
                self.assertEqual(error.status, 400)
                payload = backend.setup_error_payload(error)
                self.assertEqual(payload["error"], "recipe_" + code + "_rejected")
                self.assertEqual(payload["diagnostic"], {
                    "phase": "before_store_open", "code": code,
                    "message": backend.RECIPE_REJECTION_HELP[code]})

    def test_malformed_legacy_or_success_code_refusals_remain_unconfirmed(self):
        values = [refusal("PRIVATE_CANARY"), refusal(message="PRIVATE_CANARY"),
                  {**refusal(), "diagnostic": {"phase": "after_save", "code": "mcp_schema"}},
                  {**refusal(), "diagnostic": {"phase": "before_store_open", "code": "mcp_schema", "detail": "PRIVATE_CANARY"}},
                  hosted.fixtures.envelope("error", error="invalid_bundle"),
                  {**refusal(), "error": "save_unconfirmed"}]
        for code, raw in [(1, backend.encoded(v)) for v in values] + [(0, backend.encoded(refusal())), (1, b"PRIVATE_CANARY")]:
            c = self.controller()
            with patch.object(c.runner, "run", return_value=(code, raw)):
                self.assert_refused(lambda: c.remember_remote(hosted.REQUEST), "retention_unconfirmed")

    def test_changed_submitted_bytes_suppress_pre_store_diagnostic(self):
        c = self.controller()
        def changed(argv, **kwargs):
            Path(argv[argv.index("--mcp") + 1]).write_text("PRIVATE_CANARY")
            return 1, backend.encoded(refusal())
        with patch.object(c.runner, "run", side_effect=changed):
            self.assert_refused(lambda: c.remember_remote(hosted.REQUEST), "retention_unconfirmed")

    def test_readback_diagnostic_after_successful_save_does_not_claim_rejection(self):
        c = self.controller()
        original = c.runner.run
        def late(argv, **kwargs):
            return (1, backend.encoded(refusal())) if argv[1] == "read" else original(argv, **kwargs)
        with patch.object(c.runner, "run", side_effect=late):
            self.assert_refused(lambda: c.remember_remote(hosted.REQUEST), "retention_unconfirmed")
        self.assertTrue(c.runner.records)


class DiagnosticMcpTests(hosted.HostedFixture, unittest.IsolatedAsyncioTestCase):
    async def test_fixed_schema_repair_reaches_agent_through_actual_mcp(self):
        c = self.controller()
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                with patch.object(c.runner, "run", return_value=(1, backend.encoded(refusal()))):
                    response = await client.call_tool("setup_remember_remote", hosted.REQUEST)
                self.assertTrue(response.is_error)
                self.assertEqual(response.structured_content, backend.setup_error_payload(
                    backend.SetupError("recipe_mcp_schema_rejected", 400)))
                self.assertIn("$schema", response.structured_content["diagnostic"]["message"])
        finally:
            await owner.aclose()


class DiagnosticHttpTests(hosted.HostedFixture, unittest.TestCase):
    def test_http_uses_same_fixed_help_without_internal_values(self):
        import http.client
        import threading
        c = self.controller()
        server = backend.Server(c, assets=self.root)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            with patch.object(c.runner, "run", return_value=(1, backend.encoded(refusal()))):
                connection = http.client.HTTPConnection(server.host, timeout=3)
                connection.request("POST", "/api/remember_remote", backend.encoded(hosted.REQUEST), {
                    "Host": server.host, "Origin": server.origin,
                    "X-Toolkit-Session": server.capability, "Content-Type": "application/json"})
                response = connection.getresponse()
                self.assertEqual(response.status, 400)
                value = backend.decode(response.read())
                connection.close()
                self.assertEqual(value, backend.setup_error_payload(backend.SetupError("recipe_mcp_schema_rejected", 400)))
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
