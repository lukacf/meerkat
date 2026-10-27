"""Fixed Slack channel limits through configuration and actual stdio MCP."""

from copy import deepcopy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from mcp import Client
from test_services_mcp import (
    ConfigurationError, Fixture, adapter, config_module, configuration,
)


OPERATIONS = ["auth_test", "get_conversation", "get_permalink", "history_page", "replies_page", "read_interval"]


def scoped(channels):
    return configuration("slack", OPERATIONS, allowed_channel_ids=channels)


def channel_calls(channel):
    return [
        ("slack_get_conversation", {"channel": channel}),
        ("slack_get_permalink", {"channel": channel, "message_ts": "11.0"}),
        ("slack_history_page", {"channel": channel}),
        ("slack_replies_page", {"channel": channel, "ts": "11.0"}),
        ("slack_read_interval", {"channel": channel, "start": "10", "end": "20",
                                  "root_oldest": "0", "max_requests": 1, "max_messages": 20,
                                  "resumable": True}),
    ]


class ChannelConfigurationTests(unittest.TestCase):
    def test_copy_freeze_and_revalidation(self):
        original = scoped(["C1"])
        parsed = config_module.validate_configuration(original)
        original["config"]["allowed_channel_ids"].append("C2")
        self.assertEqual(parsed.config["allowed_channel_ids"], ("C1",))
        with self.assertRaises(TypeError):
            parsed.config["allowed_channel_ids"] = ("C2",)
        self.assertEqual(adapter._profile_configuration(parsed).config["allowed_channel_ids"], ("C1",))

    def test_strict_public_json_values(self):
        bad = [None, "C1", {}, ("C1",), [True], [1], [None], [""], ["C1", "C1"],
               [" C1"], ["C1\n"], ["C-1"], ["C" * 129], [f"C{i}" for i in range(257)]]
        for value in bad:
            with self.subTest(value=repr(value)[:60]), self.assertRaises(ConfigurationError) as error:
                config_module.validate_configuration(scoped(value))
            self.assertEqual(str(error.exception), "Invalid service MCP configuration")
            self.assertIsNone(error.exception.__context__)
        parsed = config_module.validate_configuration(scoped([f"C{i}" for i in range(256)]))
        self.assertEqual(len(parsed.config["allowed_channel_ids"]), 256)

    def test_empty_is_distinct_from_omitted(self):
        self.assertEqual(config_module.validate_configuration(scoped([])).config["allowed_channel_ids"], ())
        self.assertNotIn("allowed_channel_ids", config_module.validate_configuration(
            configuration("slack")).config)

    def test_restricted_broad_listing_and_other_profile_refused(self):
        for value in [[], ["C1"]]:
            with self.subTest(channels=value), self.assertRaises(ConfigurationError):
                config_module.validate_configuration(configuration("slack", allowed_channel_ids=value))
        with self.assertRaises(ConfigurationError):
            config_module.validate_configuration(configuration("gmail", allowed_channel_ids=["C1"]))

    def test_help_retains_optionality_and_visible_meaning(self):
        help_value = config_module.configuration_help("slack")
        self.assertEqual(help_value["config_fields"]["allowed_channel_ids"],
                         {"kind": "string_list", "required": False})
        self.assertNotIn("allowed_channel_ids", config_module.configuration_help("gmail")["config_fields"])
        notes = " ".join(row["instruction"] for row in help_value["prerequisites"])
        for text in ("empty array", "omitting", "list_conversations_page", "quoted message"):
            self.assertIn(text, notes)


class ChannelMCPTests(unittest.IsolatedAsyncioTestCase):
    async def test_all_selected_channel_calls_enforce_before_provider(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(Path(directory) / "limited", scoped(["C1"]))
            async with Client(fixture.params, mode="legacy") as client:
                tools = (await client.list_tools()).tools
                self.assertEqual({tool.name for tool in tools}, {"slack_" + name for name in OPERATIONS})
                for name, args in channel_calls("C2"):
                    result = await client.call_tool(name, args)
                    self.assertTrue(result.is_error, name)
                    self.assertEqual(result.structured_content["error"]["kind"], "permission", name)
                    self.assertEqual(fixture.events("request"), [], name)
                for name, args in channel_calls("C1"):
                    result = await client.call_tool(name, args)
                    self.assertFalse(result.is_error, name)
                self.assertEqual(len(fixture.events("request")), 5)

    async def test_empty_ceiling_allows_only_selected_account_observation(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(Path(directory) / "empty", scoped([]))
            async with Client(fixture.params, mode="legacy") as client:
                for name, args in channel_calls("C1"):
                    self.assertTrue((await client.call_tool(name, args)).is_error)
                self.assertEqual(fixture.events("request"), [])
                self.assertFalse((await client.call_tool("slack_auth_test", {})).is_error)
                self.assertEqual(len(fixture.events("request")), 1)

    async def test_continuation_from_other_instance_cannot_widen_ceiling(self):
        with tempfile.TemporaryDirectory() as directory:
            first = Fixture(Path(directory) / "first", scoped(["C1"]))
            second = Fixture(Path(directory) / "second", scoped(["C2"]))
            name, args = channel_calls("C1")[-1]
            async with Client(first.params, mode="legacy") as client:
                part = (await client.call_tool(name, args)).structured_content["value"]
                checkpoint = deepcopy(part["continuation"])
                self.assertIsNotNone(checkpoint)
            async with Client(second.params, mode="legacy") as client:
                result = await client.call_tool(name, {**args, "continuation": checkpoint})
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content["error"]["kind"], "permission")
            self.assertEqual(second.events("request"), [])
            async with Client(first.params, mode="legacy") as client:
                resumed = await client.call_tool(name, {**args, "continuation": checkpoint})
                self.assertFalse(resumed.is_error)
                self.assertIsNone(resumed.structured_content["value"]["continuation"])
            self.assertEqual(len(first.events("request")), 2)

    async def test_invalid_ceiling_refused_before_credentials(self):
        value = config_module.Configuration("slack", ("history_page",), scoped(None)["config"])
        with patch.object(adapter, "selected_token", side_effect=AssertionError("credential access")):
            with self.assertRaises(ConfigurationError):
                await adapter.create_profile(value)


if __name__ == "__main__":
    unittest.main()
