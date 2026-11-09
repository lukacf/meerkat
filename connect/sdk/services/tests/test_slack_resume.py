"""Resume through the real MCP boundary with the selected interval tool alone."""
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest

from mcp import Client
from test_services_mcp import Fixture, configuration


class ResumeTests(unittest.IsolatedAsyncioTestCase):
    async def test_resume_only_interval_and_refuse_wrong_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(Path(directory) / "resume", configuration("slack", ["read_interval"]))
            args = {"channel": "C1", "start": "10", "end": "20", "root_oldest": "0",
                    "max_requests": 1, "max_messages": 20, "resumable": True}
            async with Client(fixture.params, mode="legacy") as client:
                self.assertEqual([t.name for t in (await client.list_tools()).tools], ["slack_read_interval"])
                first = (await client.call_tool("slack_read_interval", args)).structured_content["value"]
                self.assertEqual(first["termination"], "request_budget")
                checkpoint = deepcopy(first["continuation"])
                bad = await client.call_tool("slack_read_interval", {**args, "channel": "C2", "continuation": checkpoint})
                self.assertTrue(bad.is_error)
                self.assertEqual(len(fixture.events("request")), 1)
                wrong = deepcopy(checkpoint)
                wrong["pending"][0]["foreign"] = True
                self.assertTrue((await client.call_tool("slack_read_interval", {**args, "continuation": wrong})).is_error)
                self.assertEqual(len(fixture.events("request")), 1)
                second = (await client.call_tool("slack_read_interval", {**args, "continuation": checkpoint})).structured_content["value"]
                self.assertIsNone(second["continuation"])
                self.assertEqual(second["history"]["state"], "not_requested")
                self.assertEqual(second["requests_made"], 1)
                self.assertEqual(second["observation_scope"], "current_call_only")
                self.assertEqual([m["message"]["ts"] for m in second["messages"]], ["11.0", "12.0"])
                self.assertEqual(checkpoint, first["continuation"])
            requests = fixture.events("request")
            self.assertEqual(len(requests), 2)
            self.assertIn("conversations.history", requests[0]["url"])
            self.assertIn("conversations.replies", requests[1]["url"])
            self.assertTrue(all(r["authorization"] == "Bearer fixture-token-A" for r in requests))
            self.assertEqual(len(fixture.events("client_closed")), 1)


if __name__ == "__main__":
    unittest.main()
