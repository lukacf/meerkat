"""Generic typed lists survive exact configuration and historical review."""
import json
import unittest
from unittest.mock import patch

from test_backend import FakeRunner, FixtureMixin, backend, help_source
from test_successor import historical, proposal, source


def list_help():
    value = help_source("services")
    value["config_fields"]["allowed_channel_ids"] = {"kind": "string_list", "required": False}
    return value


class ListRunner(FakeRunner):
    def run(self, argv, capture=False):
        if "config-help" in argv:
            self.calls.append((list(argv), capture))
            return 0, backend.encoded(list_help())
        return super().run(argv, capture)


def selection(values):
    return {"family": "services", "profile": "slack", "operations": ["auth_test"],
            "values": {"token_env": "FIXTURE_TOKEN", "timeout_seconds": 10, **values}}


class StringListTests(FixtureMixin, unittest.TestCase):
    def prepared_list(self, **kwargs):
        c = self.controller(runner_factory=ListRunner, **kwargs)
        c.prepare({"family": "services"}); c.join()
        return c

    def test_help_preserves_optional_fields_and_legacy_required_default(self):
        value = backend.normalize_help("services", "slack", list_help(), "fixture")
        fields = {row["name"]: row for row in value["fields"]}
        self.assertFalse(fields["allowed_channel_ids"]["required"])
        self.assertTrue(fields["token_env"]["required"])
        self.assertFalse(fields["allowed_channel_ids"]["credential_reference"])
        for change in ({"required": "false"}, {"credential_reference": True}):
            bad = list_help(); bad["config_fields"]["allowed_channel_ids"].update(change)
            with self.assertRaisesRegex(backend.SetupError, "inspection_unavailable"):
                backend.normalize_help("services", "slack", bad, "fixture")

    def test_omitted_empty_and_populated_lists_check_install_and_replay_exactly(self):
        c = self.prepared_list(); hashes = set()
        for values in ({}, {"allowed_channel_ids": []}, {"allowed_channel_ids": ["C1", "C2"]}):
            with self.subTest(values=values):
                request = selection(values); checked = c.check(request)
                hashes.add(checked["configuration_sha256"])
                install = {**request, "validation_id": checked["validation_id"],
                           "configuration_sha256": checked["configuration_sha256"]}
                job = c.install(install)["job"]; c.join()
                self.assertEqual(c.jobs[job["id"]]["status"], "succeeded")
                stored = c.workspace / "installations" / checked["validation_id"] / "configuration/selected.json"
                self.assertEqual(stored.read_bytes(), backend.configuration_bytes(request))
                self.assertEqual(json.loads(stored.read_bytes())["config"], request["values"])
                self.assertEqual(c.install(install)["job"]["id"], job["id"])
        self.assertEqual(len(hashes), 3)

    def test_invalid_lists_refuse_before_parser_or_configuration_write(self):
        c = self.prepared_list()
        invalid = [None, "[]", {}, [1], [True], [[]], ("C1",), ["x"] * 257,
                   ["x" * 2049], ["a\nb"], ["\x00"], ["\x7f"]]
        for value in invalid:
            with self.subTest(value=repr(value)[:80]):
                count = sum("check-config" in argv for argv, _ in c.runner.calls)
                files = list((c.workspace / "configurations").iterdir())
                with self.assertRaisesRegex(backend.SetupError, "invalid_configuration"):
                    c.check(selection({"allowed_channel_ids": value}))
                self.assertEqual(sum("check-config" in argv for argv, _ in c.runner.calls), count)
                self.assertEqual(list((c.workspace / "configurations").iterdir()), files)
        # Provider-specific syntax and duplicate policy do not belong here.
        for value in (["", "duplicate", "duplicate"], ["x"] * 256, ["\U0001f600" * 2048]):
            c.check(selection({"allowed_channel_ids": value}))

    def test_checked_list_is_copied_and_mutation_requires_fresh_validation(self):
        c = self.prepared_list(); request = selection({"allowed_channel_ids": ["C1"]})
        checked = c.check(request); request["values"]["allowed_channel_ids"].append("C2")
        self.assertEqual(c.validations[checked["validation_id"]]["request"]["values"]["allowed_channel_ids"], ["C1"])
        with self.assertRaisesRegex(backend.SetupError, "validation_changed"):
            c.install({**request, "validation_id": checked["validation_id"],
                       "configuration_sha256": checked["configuration_sha256"]})

    def test_historical_lists_preserve_types_digest_and_current_projection(self):
        for values in ({}, {"allowed_channel_ids": []}, {"allowed_channel_ids": ["C1", "C2"]}):
            with self.subTest(values=values):
                c = self.prepared_list(**source(self, historical(selection(values))))
                projected = proposal(c)
                self.assertEqual(projected["values"], selection(values)["values"])
                if values:
                    projected["values"]["allowed_channel_ids"].append("changed")
                    self.assertEqual(proposal(c)["values"], selection(values)["values"])
                c.check({**selection(values), "values": proposal(c)["values"]})
        for value in ([True], ["\n"], ["x"] * 257, ["x" * 2049]):
            with self.assertRaisesRegex(backend.SetupError, "invalid_prior_request"):
                backend.prior_request(backend.encoded(historical(selection({"allowed_channel_ids": value}))))
        record = historical(selection({"allowed_channel_ids": []}))
        record["selection"]["values"]["allowed_channel_ids"].append("tampered")
        with self.assertRaisesRegex(backend.SetupError, "invalid_prior_request"):
            backend.prior_request(backend.encoded(record))

    def test_removed_or_changed_list_is_unapplied_without_disclosing_items(self):
        old = selection({"removed": ["PRIVATE_ITEM"], "timeout_seconds": ["PRIVATE_ITEM"]})
        c = self.prepared_list(**source(self, historical(old))); projected = proposal(c)
        self.assertNotIn("PRIVATE_ITEM", json.dumps(projected))
        self.assertEqual({(row["name"], row["prior_type"], row["reason"]) for row in projected["unapplied"]},
                         {("removed", "string_list", "not_in_current_profile"),
                          ("timeout_seconds", "string_list", "current_type_differs")})


class StringListMcpTests(FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_successor_preserves_empty_list_and_exact_configuration(self):
        from test_mcp_adapter import adapter, connection
        request = selection({"allowed_channel_ids": []})
        c = self.controller(runner_factory=ListRunner, **source(self, historical(request)))
        c.prepare({"family": "services"}); c.join()
        with patch.object(adapter, "backend", backend):
            async with connection(c) as (client, _):
                result = await client.call_tool("setup_successor", {"prior_request_sha256": c.prior_request_sha256})
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["values"], request["values"])
                self.assertEqual(c.validations, {})
                proposed = {**request, "values": result.structured_content["values"]}
                checked = await client.call_tool("setup_check", proposed)
                self.assertFalse(checked.is_error, checked)
                self.assertEqual(checked.structured_content["configuration_sha256"],
                                 backend.digest(backend.configuration_bytes(request)))

    async def test_real_mcp_schema_current_check_and_install_preserve_lists(self):
        from test_mcp_adapter import adapter, connection
        c = self.controller(runner_factory=ListRunner)
        c.prepare({"family": "services"}); c.join()
        with patch.object(adapter, "backend", backend):
            async with connection(c) as (client, _):
                for values in ({}, {"allowed_channel_ids": []}, {"allowed_channel_ids": ["C1"]}):
                    request = selection(values)
                    result = await client.call_tool("setup_check", request)
                    self.assertFalse(result.is_error, result)
                    checked = result.structured_content
                    install = await client.call_tool("setup_install", {**request,
                        "validation_id": checked["validation_id"], "configuration_sha256": checked["configuration_sha256"]})
                    self.assertFalse(install.is_error, install); c.join()
                    self.assertEqual(c.jobs[install.structured_content["job"]["id"]]["status"], "succeeded")
                for value in (None, [1], [[]], ["\n"], ["x"] * 257, ["x" * 2049]):
                    count = len(c.validations)
                    result = await client.call_tool("setup_check", selection({"allowed_channel_ids": value}))
                    self.assertTrue(result.is_error)
                    self.assertEqual(len(c.validations), count)
