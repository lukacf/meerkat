"""Maintained regression coverage promoted from the independent diagnostics review."""

import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest


spec = importlib.util.spec_from_file_location("diagnostics_backend", Path(__file__).resolve().parents[1] / "backend.py")
backend = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backend)
CANARY = "PRIVATE_VALUE_6ae720_DO_NOT_REFLECT"


def expected(code, path):
    return {"valid": False, "scope": "configuration_only",
            "diagnostic": {"version": 1, "code": code, "field_path": path}}


def selection(**updates):
    values = {"token_env": "DIAGNOSTICS_TOKEN", "timeout_seconds": 10,
              "max_response_bytes": 65536}
    values.update(updates)
    return {"profile": "slack", "operations": ["history_page"], "config": values}


class SetupDiagnosticBoundaryTests(unittest.TestCase):
    backend = backend

    def observe(self, value, family="services", profile="slack", suffix=b""):
        raw = value if isinstance(value, bytes) else json.dumps(value).encode()
        return self.backend.configuration_diagnostic(raw + suffix, family, profile, "a" * 64, "b" * 64)

    def test_exact_envelope_bound_to_current_selection_without_child_text(self):
        value = expected("channel_list_conflict", ["operations"])
        result = self.observe(value)
        self.assertEqual(set(result), {"version", "phase", "code", "field_path", "message", "family",
                                       "profile", "configuration_sha256", "contents_sha256"})
        self.assertEqual(result["configuration_sha256"], "a" * 64)
        self.assertEqual(result["contents_sha256"], "b" * 64)
        self.assertEqual(result["family"], "services")
        self.assertEqual(result["profile"], "slack")
        result["field_path"].append(CANARY)
        self.assertEqual(self.observe(value)["field_path"], ["operations"])
        self.assertIsNone(self.observe(value, family="agno"))
        self.assertIsNone(self.observe(value, family="activepieces"))
        self.assertIsNone(self.observe(value, profile="gmail"))

    def test_malformed_child_diagnostics_remain_generic(self):
        valid = expected("channel_list_conflict", ["operations"])
        mutations = [[], None, False, {}, {**valid, "valid": 0}, {**valid, "valid": True},
                     {**valid, "scope": CANARY}, {**valid, "message": CANARY},
                     {"valid": False, "scope": "configuration_only"}]
        for version in (True, False, 1.0, "1", 2, None):
            mutations.append({**valid, "diagnostic": {**valid["diagnostic"], "version": version}})
        for code in ("", CANARY, None, False, 0, [], {}):
            mutations.append({**valid, "diagnostic": {**valid["diagnostic"], "code": code}})
        for path in (None, "operations", [], [CANARY], ["config", "token_env"], [0], [False], [["operations"]]):
            mutations.append({**valid, "diagnostic": {**valid["diagnostic"], "field_path": path}})
        mutations.append({**valid, "diagnostic": {**valid["diagnostic"], "message": CANARY}})
        for value in mutations:
            with self.subTest(index=mutations.index(value)):
                self.assertIsNone(self.observe(value))
        raw = json.dumps(valid).encode()
        for malformed in (raw + CANARY.encode(), b"x" * 1025, b"\xff", raw.replace(b'"version": 1', b'"version": 1, "version": 1')):
            self.assertIsNone(self.observe(malformed))

    def test_calendar_diagnostics_bind_only_the_selected_profile(self):
        value = expected("invalid_calendar_id", ["config", "calendar_id"])
        result = self.observe(value, profile="google-calendar")
        self.assertEqual(result["code"], "invalid_calendar_id")
        self.assertEqual(result["profile"], "google-calendar")
        self.assertEqual(result["field_path"], ["config", "calendar_id"])
        for profile in ("slack", "gmail", "calendar", CANARY):
            with self.subTest(profile=profile):
                self.assertIsNone(self.observe(value, profile=profile))
        for code, path in (
            ("invalid_channel_ids", ["config", "allowed_channel_ids"]),
            ("invalid_mailbox", ["config", "user_id"]),
            ("integer_out_of_range", ["config", "max_message_bytes"]),
            ("integer_out_of_range", ["config", "max_interval_requests"]),
        ):
            with self.subTest(code=code, path=path):
                self.assertIsNone(self.observe(expected(code, path), profile="google-calendar"))
        common = expected("invalid_credential_reference", ["config", "token_env"])
        self.assertEqual(self.observe(common, profile="google-calendar")["code"], common["diagnostic"]["code"])
        reflected = {**value, "diagnostic": {**value["diagnostic"], "value": CANARY}}
        self.assertIsNone(self.observe(reflected, profile="google-calendar"))

    def test_wrong_profile_field_pairs_are_not_promoted(self):
        for profile, code, path in (
            ("slack", "invalid_recipient_addresses", ["config", "allowed_recipient_addresses"]),
            ("slack", "recipient_operation_conflict", ["operations"]),
            ("slack", "invalid_mailbox", ["config", "user_id"]),
            ("slack", "integer_out_of_range", ["config", "max_message_bytes"]),
            ("gmail", "invalid_channel_ids", ["config", "allowed_channel_ids"]),
            ("gmail", "integer_out_of_range", ["config", "max_interval_requests"]),
        ):
            with self.subTest(profile=profile, code=code):
                self.assertIsNone(self.observe(expected(code, path), profile=profile))

    def test_slack_result_limit_is_bound_to_its_selected_profile(self):
        value = expected("integer_out_of_range", ["config", "max_interval_result_bytes"])
        result = self.observe(value, profile="slack")
        self.assertEqual(result["field_path"], ["config", "max_interval_result_bytes"])
        for profile in ("gmail", "google-calendar", "google-drive", CANARY):
            self.assertIsNone(self.observe(value, profile=profile))

    def test_drive_content_limit_is_bound_to_its_selected_profile(self):
        value = expected("integer_out_of_range", ["config", "max_content_bytes"])
        result = self.observe(value, profile="google-drive")
        self.assertEqual(result["field_path"], ["config", "max_content_bytes"])
        self.assertEqual(result["profile"], "google-drive")
        for profile in ("slack", "gmail", "google-calendar", "drive", CANARY):
            self.assertIsNone(self.observe(value, profile=profile))
        for code, path in (("invalid_calendar_id", ["config", "calendar_id"]),
                           ("invalid_mailbox", ["config", "user_id"]),
                           ("integer_out_of_range", ["config", "max_interval_requests"])):
            self.assertIsNone(self.observe(expected(code, path), profile="google-drive"))
        self.assertIsNone(self.observe({**value, "message": CANARY}, profile="google-drive"))

    def test_parser_failure_requires_exact_exit_status_and_unchanged_input(self):
        b = self.backend
        selected = {"family": "services", "profile": "slack", "operations": ["history_page", "list_conversations_page"],
                    "values": selection(allowed_channel_ids=["C1"])["config"]}
        data = b.configuration_bytes(selected)
        raw = json.dumps(expected("channel_list_conflict", ["operations"])).encode()
        with tempfile.TemporaryDirectory(prefix="diagnostic-readback-") as directory:
            path = Path(directory) / "selection.json"
            for code, mutate in ((1, False), (2, False), (-15, False), (True, False), (1, True)):
                with self.subTest(code=code, mutate=mutate):
                    path.write_bytes(data)
                    def run(argv, capture=False):
                        if mutate:
                            path.write_text("{}")
                        return code, raw
                    owner = SimpleNamespace(commands={"services": ["fixture-not-executed"]},
                                            help_locked=lambda *args: {}, _run=run,
                                            input_binding=lambda family: ("b" * 64, "c" * 64))
                    with self.assertRaises(b.SetupError) as caught:
                        b.Controller.parser_check(owner, "services", path, selected)
                    payload = b.setup_error_payload(caught.exception)
                    self.assertNotIn(CANARY, json.dumps(payload))
                    if mutate:
                        self.assertEqual(payload, {"error": "configuration_changed"})
                    elif type(code) is int and code == 1:
                        self.assertEqual(payload["diagnostic"]["configuration_sha256"], b.digest(data))
                    else:
                        self.assertEqual(payload, {"error": "invalid_configuration"})


if __name__ == "__main__":
    unittest.main()
