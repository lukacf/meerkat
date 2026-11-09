"""Fixed service-owner diagnostics without input or exception reflection."""

from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import importlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
config = importlib.import_module(NAMESPACE + ".adapters.services_config")
adapter = importlib.import_module(NAMESPACE + ".adapters.services_mcp")
CANARY = "private-input-must-not-be-reflected"
GENERIC = "Service MCP command failed; check the selected configuration and dependencies.\n"


def selection(profile="slack", operations=None):
    values = {"token_env": "HOST_TOKEN", "timeout_seconds": 5, "max_response_bytes": 1024}
    if profile == "gmail":
        values.update(user_id="me", max_message_bytes=1024)
    return {"profile": profile, "operations": operations or ["auth_test" if profile == "slack" else "send_message"], "config": values}


def invoke(arguments):
    output, error = io.StringIO(), io.StringIO()
    with redirect_stdout(output), redirect_stderr(error):
        code = adapter.main(arguments)
    return code, output.getvalue(), error.getvalue()


class ConfigurationDiagnosticsTests(unittest.TestCase):
    def reject(self, value, code, path):
        with self.assertRaises(config.ConfigurationError) as caught:
            config.validate_configuration(value)
        error = caught.exception
        self.assertEqual(str(error), "Invalid service MCP configuration")
        self.assertIsNone(error.__cause__)
        self.assertIsNone(error.__context__)
        self.assertEqual(error.diagnostic, {"version": 1, "code": code, "field_path": path})
        self.assertIn(tuple(path), config.CONFIGURATION_DIAGNOSTIC_PATHS[code])
        self.assertNotIn(CANARY, repr(error) + repr(vars(error)) + json.dumps(error.diagnostic))
        return error

    def test_document_profile_operations_and_fields(self):
        for value in (None, [], {}, {**selection(), CANARY: CANARY}):
            with self.subTest(value=value):
                self.reject(value, "invalid_document", [])
        for name in (None, 1, CANARY):
            value = selection()
            value["profile"] = name
            self.reject(value, "invalid_profile", ["profile"])
        for operations in ([], (), [CANARY], [1], ["auth_test", "auth_test"]):
            value = selection()
            value["operations"] = operations
            self.reject(value, "invalid_operations", ["operations"])
        for fields in ([], {}, {**selection()["config"], CANARY: CANARY}):
            value = selection()
            value["config"] = fields
            self.reject(value, "invalid_config_fields", ["config"])

    def test_conditional_fields_remain_exact(self):
        value = selection(operations=["read_interval"])
        self.reject(value, "invalid_config_fields", ["config"])
        value["config"].update(max_interval_requests=1, max_interval_messages=1)
        self.assertEqual(config.validate_configuration(value).operations, ("read_interval",))
        value["operations"] = ["auth_test"]
        self.reject(value, "invalid_config_fields", ["config"])

    def test_channel_ids_and_operation_conflict_are_distinct(self):
        for channels in (CANARY, [None], ["C1", "C1"], ["bad/channel"], ["C1"] * 257):
            value = selection()
            value["config"]["allowed_channel_ids"] = channels
            self.reject(value, "invalid_channel_ids", ["config", "allowed_channel_ids"])
        for channels in ([], ["C1"]):
            value = selection(operations=["list_conversations_page"])
            value["config"]["allowed_channel_ids"] = channels
            self.reject(value, "channel_list_conflict", ["operations"])
            value["operations"] = ["history_page"]
            observed = config.validate_configuration(value)
            self.assertEqual(observed.config["allowed_channel_ids"], tuple(channels))

    def test_recipient_syntax_and_operation_conflict_are_distinct(self):
        for recipients in (CANARY, [CANARY], [None], ["Display <one@example.invalid>"],
                           ["one@example.invalid", "one@EXAMPLE.invalid"]):
            value = selection("gmail")
            value["config"]["allowed_recipient_addresses"] = recipients
            self.reject(value, "invalid_recipient_addresses", ["config", "allowed_recipient_addresses"])
        for operation in ("send_raw", "create_draft_raw"):
            value = selection("gmail", [operation])
            value["config"]["allowed_recipient_addresses"] = []
            self.reject(value, "recipient_operation_conflict", ["operations"])
            value["operations"] = ["send_message"]
            self.assertEqual(config.validate_configuration(value).config["allowed_recipient_addresses"], ())

    def test_credential_reference_and_mailbox(self):
        for reference in (CANARY, "", 1, "token\nvalue"):
            value = selection()
            value["config"]["token_env"] = reference
            self.reject(value, "invalid_credential_reference", ["config", "token_env"])
        for mailbox in (CANARY, "", 1, "x" * 321):
            value = selection("gmail")
            value["config"]["user_id"] = mailbox
            self.reject(value, "invalid_mailbox", ["config", "user_id"])

    def test_each_numeric_limit_and_type_comes_from_owner(self):
        bounds = {"timeout_seconds": 60, "max_response_bytes": 16_777_216,
                  "max_message_bytes": 4_194_304, "max_interval_requests": 100,
                  "max_interval_messages": 10_000}
        for field, maximum in bounds.items():
            value = selection("gmail" if field == "max_message_bytes" else "slack")
            if field.startswith("max_interval"):
                value["operations"] = ["read_interval"]
                value["config"].update(max_interval_requests=1, max_interval_messages=1)
            for invalid in (True, 1.5, 0, maximum + 1, CANARY):
                with self.subTest(field=field, invalid=invalid):
                    value["config"][field] = invalid
                    self.reject(value, "integer_out_of_range", ["config", field])
            for accepted in (1, maximum):
                value["config"][field] = accepted
                self.assertEqual(config.validate_configuration(value).config[field], accepted)

    def test_diagnostic_cannot_be_mutated_or_use_unknown_path(self):
        error = self.reject({}, "invalid_document", [])
        error.diagnostic["field_path"].append(CANARY)
        self.assertEqual(error.diagnostic["field_path"], [])
        with self.assertRaises(TypeError):
            config.CONFIGURATION_DIAGNOSTIC_PATHS[CANARY] = ((),)
        for code, path in ((CANARY, ()), ("invalid_document", (CANARY,)),
                           ("integer_out_of_range", ("config", CANARY))):
            with self.assertRaisesRegex(ValueError, "Invalid configuration diagnostic"):
                config.ConfigurationError(code, path)

    def test_unexpected_validator_failure_stays_generic_without_context(self):
        with patch.object(config.re, "fullmatch", side_effect=RuntimeError(CANARY)):
            with self.assertRaises(config.ConfigurationError) as caught:
                config.validate_configuration(selection())
        self.assertIsNone(caught.exception.diagnostic)
        self.assertIsNone(caught.exception.__cause__)
        self.assertIsNone(caught.exception.__context__)
        with patch.object(adapter, "load_configuration", side_effect=RuntimeError(CANARY)):
            self.assertEqual(invoke(["check-config", "--config", CANARY]), (1, "", GENERIC))

    def test_file_errors_use_no_file_path_or_decode_detail(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / CANARY
            for raw in (b"{", b"\xff", b'{"duplicate":1,"duplicate":2}', b'{"x":NaN}', b" " * 65_537):
                path.write_bytes(raw)
                with self.assertRaises(config.ConfigurationError) as caught:
                    config.load_configuration(path)
                self.assertEqual(caught.exception.diagnostic,
                                 {"version": 1, "code": "invalid_config_file", "field_path": []})
                self.assertIsNone(caught.exception.__context__)
                code, output, error = invoke(["check-config", "--config", str(path)])
                self.assertEqual(code, 1)
                self.assertEqual(error, GENERIC)
                self.assertNotIn(CANARY, output + error)
                self.assertEqual(json.loads(output), {"valid": False, "scope": "configuration_only",
                                                      "diagnostic": caught.exception.diagnostic})
            path.unlink()
            self.assertEqual(json.loads(invoke(["check-config", "--config", str(path)])[1])["diagnostic"]["code"],
                             "invalid_config_file")

    def test_other_modes_and_bad_arguments_remain_generic(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "invalid.json"
            path.write_text("{}")
            for arguments in (["describe", "--config", str(path)], ["serve", "--config", str(path)],
                              ["config-help", "--profile", CANARY], ["check-config"],
                              ["check-config", "--config", str(path), "--unexpected", CANARY]):
                self.assertEqual(invoke(arguments), (1, "", GENERIC))

    def test_real_cli_correction_retains_restriction_and_success_shape(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "selection.json"
            value = selection(operations=["list_conversations_page"])
            value["config"]["allowed_channel_ids"] = ["C1"]
            before = deepcopy(value)
            path.write_text(json.dumps(value))
            command = [sys.executable, "-m", NAMESPACE + ".adapters.services_mcp", "check-config", "--config", str(path)]
            failed = subprocess.run(command, text=True, capture_output=True, timeout=10, check=False)
            self.assertEqual(failed.returncode, 1)
            self.assertEqual(json.loads(failed.stdout), {
                "valid": False, "scope": "configuration_only",
                "diagnostic": {"version": 1, "code": "channel_list_conflict", "field_path": ["operations"]}})
            self.assertEqual(failed.stderr, GENERIC)
            value["operations"] = ["history_page"]
            self.assertEqual(value["config"], before["config"])
            path.write_text(json.dumps(value))
            success = subprocess.run(command, text=True, capture_output=True, timeout=10, check=False)
            self.assertEqual(success.returncode, 0, success.stderr)
            self.assertEqual(success.stderr, "")
            self.assertEqual(json.loads(success.stdout), {
                "profile": "slack", "valid": True, "scope": "configuration_only",
                "credential_environment_references": [{"field_path": ["config", "token_env"], "environment_variable": "HOST_TOKEN"}]})


if __name__ == "__main__":
    unittest.main()
