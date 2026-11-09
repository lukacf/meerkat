"""Credential-free guidance checks, with no provider or MCP runtime dependency."""
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

from meerkat_toolkit.adapters.services_config import ConfigurationError, configuration_help


ROOT = Path(__file__).resolve().parents[3]


class AuthPrerequisitesTests(unittest.TestCase):
    def test_exact_profile_auth_relationship_and_unresolved_access(self):
        slack = configuration_help("slack")
        gmail = configuration_help("gmail")
        for result in (slack, gmail):
            kinds = {item["kind"] for item in result["prerequisites"]}
            self.assertEqual(kinds, {"account", "client_authentication", "application_binding",
                                     "registration", "permissions", "resource_access"})
            instructions = " ".join(item["instruction"] for item in result["prerequisites"])
            self.assertIn("config.token_env", instructions)
            self.assertIn("downstream service authentication", instructions)
            self.assertIn("stdio", instructions)
            self.assertIn("does not perform OAuth", instructions)
            self.assertIn("replacement process", instructions)
            self.assertIn("administrator", " ".join(result["unresolved_details"]))
            self.assertIn("provider scopes", " ".join(result["unresolved_details"]))
        self.assertIn("Slack Web API access token", json.dumps(slack))
        self.assertNotIn("Google access token", json.dumps(slack))
        self.assertIn("Google access token", json.dumps(gmail))
        self.assertIn("does not verify", json.dumps(gmail))
        self.assertIn("separate host operation approval", json.dumps(gmail))
        self.assertNotIn("Sending Gmail", json.dumps(slack))

    def test_slack_permissions_are_conditional_on_operation_and_conversation(self):
        rows = configuration_help("slack")["prerequisites"]
        permissions = [row["instruction"] for row in rows if row["kind"] == "permissions"]
        listing, history, no_extra, user_profile = permissions
        for scope in ("channels:read", "groups:read", "im:read", "mpim:read"):
            self.assertIn(scope, listing)
        self.assertIn("Do not request all four", listing)
        for scope in ("channels:history", "groups:history", "im:history", "mpim:history"):
            self.assertIn(scope, history)
        self.assertIn("read_interval uses both", history)
        self.assertIn("list bot and user tokens", history)
        self.assertIn("membership", history)
        self.assertIn("no additional OAuth scope", no_extra)
        self.assertIn("do not require chat:write", no_extra)
        self.assertIn("does not establish that a recipient", no_extra)
        self.assertIn("requires users:read", user_profile)
        self.assertIn("do not require users:read.email", user_profile)

    def test_slack_installation_and_rate_restrictions_do_not_claim_universal_admin_or_tier(self):
        rows = configuration_help("slack")["prerequisites"]
        registration = next(row["instruction"] for row in rows if row["kind"] == "registration")
        access = next(row["instruction"] for row in rows if row["kind"] == "resource_access")
        self.assertIn("If workspace app approval is enabled", registration)
        self.assertIn("install or reinstall", registration)
        self.assertIn("New commercially distributed non-Marketplace", access)
        self.assertIn("exempts existing installations", access)
        self.assertIn("internal customer-built", access)
        self.assertIn("explicit partial coverage", access)

    def test_gmail_metadata_limitations_and_send_draft_authority_are_distinct(self):
        permissions = [row["instruction"] for row in configuration_help("gmail")["prerequisites"]
                       if row["kind"] == "permissions"]
        reads, writes = permissions
        self.assertIn("gmail.readonly", reads)
        self.assertIn("list without a query", reads)
        self.assertIn("full/raw formats", reads)
        self.assertIn("get_message defaults to full", reads)
        self.assertIn("gmail.send does not authorize draft creation", writes)
        self.assertIn("gmail.compose also permits sending", writes)
        self.assertIn("neither grant provides general inbox reads", writes)
        self.assertIn("add the applicable read scope", writes)
        self.assertIn("not required by these operations", writes)
        self.assertIn("do not select tools or approve", writes)

    def test_gmail_registration_and_mime_remain_host_owned(self):
        text = " ".join(row["instruction"] for row in configuration_help("gmail")["prerequisites"])
        for expected in ("enable the Gmail API", "Desktop app", "Web application", "registered redirect URI",
                         "add test users", "exceptions are conditional", "administrators can block",
                         "does not impersonate", "super-administrator", "standard base64", "To/Cc/Bcc",
                         "do not compose a message", "not permission to resend"):
            self.assertIn(expected, text)

    def test_guidance_does_not_expand_configuration_or_operation_authority(self):
        from meerkat_toolkit.adapters.services_config import validate_configuration, tool_descriptions
        for profile in ("slack", "gmail"):
            help_value = configuration_help(profile)
            config = {"token_env": "HOST_TOKEN", "timeout_seconds": 5, "max_response_bytes": 2048}
            operation = "auth_test" if profile == "slack" else "send_raw"
            if profile == "gmail":
                config.update(user_id="me", max_message_bytes=1024)
            parsed = validate_configuration({"profile": profile, "operations": [operation], "config": config})
            self.assertEqual([item["name"] for item in tool_descriptions(parsed)], [f"{profile}_{operation}"])
            self.assertTrue(help_value["config_fields"]["token_env"]["credential_reference"])
            for row in help_value["prerequisites"]:
                self.assertLess(len(row["instruction"]), 8192)
                self.assertNotIn("\u2013", row["instruction"])
                self.assertNotIn("\u2014", row["instruction"])
            # Guidance is conditional prose, not an extra accepted permission/configuration field.
            with self.assertRaises(ConfigurationError):
                validate_configuration({"profile": profile, "operations": [operation],
                                        "config": {**config, "scopes": ["all"]}})

    def test_guidance_does_not_read_credentials(self):
        class UnreadableEnvironment(dict):
            def __getitem__(self, _key): raise AssertionError("unexpected credential read")
            def get(self, *_args): raise AssertionError("unexpected credential read")
            def __iter__(self): raise AssertionError("unexpected environment iteration")
        with patch.object(os, "environ", UnreadableEnvironment()):
            for profile in ("slack", "gmail"):
                json.dumps(configuration_help(profile))

    def test_results_are_detached_and_invalid_profile_is_not_reflected(self):
        pristine = configuration_help("slack")
        pristine["prerequisites"][0]["instruction"] = "contaminated"
        pristine["unresolved_details"].clear()
        fresh = configuration_help("slack")
        self.assertNotIn("contaminated", json.dumps(fresh))
        self.assertTrue(fresh["unresolved_details"])
        for profile in ("private-token-marker", None, [], True):
            with self.assertRaises(ConfigurationError) as caught:
                configuration_help(profile)
            self.assertEqual(str(caught.exception), "Invalid service MCP configuration")

    def test_real_cli_help_keeps_provider_imports_cold(self):
        code = '''
import importlib.abc, json, sys
class RefuseRuntime(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname.split('.')[0] in {'mcp', 'httpx', 'httpcore', 'anyio'} or fullname.startswith('meerkat_toolkit.connectors'):
            raise RuntimeError('provider runtime was imported')
sys.meta_path.insert(0, RefuseRuntime())
from meerkat_toolkit.adapters.services_mcp import main
raise SystemExit(main(['config-help', '--profile', sys.argv[1]]))
'''
        environment = {"PATH": "/usr/bin:/bin", "PYTHONPATH": str(ROOT / "sdk/python/src"),
                       "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1",
                       "HOST_SELECTED_TOKEN": "synthetic-private-marker"}
        for profile in ("slack", "gmail"):
            process = subprocess.run([sys.executable, "-c", code, profile], env=environment,
                                     capture_output=True, text=True, timeout=5)
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertEqual(json.loads(process.stdout), configuration_help(profile))
            self.assertNotIn("synthetic-private-marker", process.stdout + process.stderr)


if __name__ == "__main__":
    unittest.main()
