"""Publisher tool names are optional help metadata, never selection aliases."""
import copy
import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

from test_backend import FakeRunner, FixtureMixin, backend, help_source


def publisher():
    path = Path(__file__).resolve().parents[2] / "python/src/meerkat_toolkit/adapters/services_config.py"
    spec = importlib.util.spec_from_file_location("operation_names_services", path)
    module = importlib.util.module_from_spec(spec)
    with patch.dict(sys.modules, {spec.name: module}):
        spec.loader.exec_module(module)
    return module


class OperationNameTests(FixtureMixin, unittest.TestCase):
    def prepared_source(self, source, family="services"):
        class Runner(FakeRunner):
            def run(self, argv, capture=False):
                if "profiles" in argv or "config-help" in argv:
                    self.calls.append((list(argv), capture))
                    value = {"profiles": [source]} if "profiles" in argv else source
                    return 0, backend.encoded(value)
                return super().run(argv, capture)

        controller = self.controller(runner_factory=Runner)
        controller.prepare({"family": family})
        controller.join()
        return controller

    def test_agno_explicit_names_and_no_auth_survive_owned_help_without_selecting_aliases(self):
        source = help_source("agno")
        source["operation_tools"] = {"search_web": "Owner.Search-v2"}
        source["authentication"] = {"kind": "none"}
        original = copy.deepcopy(source)
        controller = self.prepared_source(source, "agno")
        request = {"family": "agno", "profile": "searxng"}
        observed = controller.help(request)
        self.assertEqual(observed["authentication"], {"kind": "none"})
        self.assertEqual(observed["operations"], [
            {"id": "search_web", "tool": "Owner.Search-v2", "effect": "unspecified"}])
        observed["authentication"]["kind"] = "changed"
        observed["operations"][0]["tool"] = "changed"
        self.assertEqual(source, original)
        self.assertEqual(controller.help(request)["authentication"], {"kind": "none"})
        self.assertEqual(controller.help(request)["operations"][0]["tool"], "Owner.Search-v2")
        selected = self.request()
        selected["operations"] = ["Owner.Search-v2"]
        self.error("invalid_configuration", lambda: controller.check(selected))
        self.assertFalse(any("check-config" in argv for argv, _ in controller.runner.calls))
        selected["operations"] = ["search_web"]
        self.assertEqual(controller.check(selected)["profile"], "searxng")

    def test_agno_legacy_missing_declarations_stay_unknown_despite_no_credential_fields(self):
        source = help_source("agno")
        source["tool"] = "not_a_mapping"
        normalized = backend.normalize_help("agno", "searxng", source, "")
        self.assertNotIn("authentication", normalized)
        self.assertFalse(any(field["credential_reference"] for field in normalized["fields"]))
        self.assertEqual(normalized["operations"], [{"id": "search_web", "effect": "unspecified"}])

    def test_agno_raw_name_mapping_must_cover_exact_ids_without_alias_collisions(self):
        source = help_source("agno")
        source["operations"] = ["search_web", "news_search"]
        valid = {"search_web": "Owner.Search-v2", "news_search": "Owner.News-v2"}
        invalid = [None, True, 1, [], {}, {"search_web": "only_one"},
                   dict(valid, extra="extra"),
                   {"search_web": "same", "news_search": "same"}]
        invalid += [{**valid, "search_web": value} for value in
                    (None, True, 1, [], {}, "", "a/b", "a b", "a\nb", "x" * 129)]
        for mapping in invalid:
            with self.subTest(mapping=mapping):
                source["operation_tools"] = mapping
                self.error("inspection_unavailable", lambda: backend.operation_rows("agno", source))
        source["operation_tools"] = valid
        self.assertEqual(backend.operation_rows("agno", source), [
            {"id": "search_web", "tool": "Owner.Search-v2", "effect": "unspecified"},
            {"id": "news_search", "tool": "Owner.News-v2", "effect": "unspecified"}])

    def test_no_auth_declaration_refuses_malformed_values_and_optional_credentials(self):
        for declaration in (None, True, "none", [], {}, {"kind": "None"},
                            {"kind": "oauth"}, {"kind": "none", "extra": "untrusted"}):
            with self.subTest(declaration=declaration):
                source = help_source("agno")
                source["authentication"] = declaration
                self.error("inspection_unavailable", lambda:
                           backend.normalize_help("agno", "searxng", source, ""))
        for required in (False, True):
            with self.subTest(required=required):
                source = help_source("agno")
                source["authentication"] = {"kind": "none"}
                source["config_fields"].append({"name": "token_env", "kind": "string",
                                                "required": required, "credential_reference": True})
                self.error("inspection_unavailable", lambda:
                           backend.normalize_help("agno", "searxng", source, ""))

    def test_agno_changed_publisher_mapping_after_prepare_refuses_current_help(self):
        for change in ("changed", "added", "removed"):
            with self.subTest(change=change):
                source = help_source("agno")
                if change != "added":
                    source["operation_tools"] = {"search_web": "Owner.Search-v2"}
                controller = self.prepared_source(source, "agno")
                if change == "removed":
                    del source["operation_tools"]
                else:
                    source["operation_tools"] = {"search_web": "Other.Search-v2"}
                self.error("inspection_unavailable", lambda:
                           controller.help({"family": "agno", "profile": "searxng"}))

    def test_current_services_publisher_help_matches_declared_mcp_tools(self):
        owner = publisher()
        for profile in owner.profiles()["profiles"]:
            with self.subTest(profile=profile["profile"]):
                source = owner.configuration_help(profile["profile"])
                before = copy.deepcopy(source)
                normalized = backend.normalize_help("services", source["profile"], source, "guide")
                declared = owner.tool_descriptions(owner.Configuration(
                    source["profile"], tuple(row["operation"] for row in source["operations"]),
                    {"max_message_bytes": 4096}))
                self.assertEqual(normalized["operations"], [
                    {"id": row["operation"], "tool": row["name"], "effect": row["effect"]}
                    for row in declared])
                self.assertEqual(backend.operation_rows("services", profile), normalized["operations"])
                self.assertEqual(source, before)

    def test_literal_nonprefix_names_survive_public_help_but_do_not_select(self):
        source = help_source("services")
        source["operations"][0]["tool"] = "Owner.Status-v2"
        controller = self.prepared_source(source)
        expected = [{"id": "auth_test", "tool": "Owner.Status-v2", "effect": "read"},
                    {"id": "read_interval", "tool": "slack_read_interval", "effect": "read"}]
        help_value = controller.help({"family": "services", "profile": "slack"})
        self.assertEqual(help_value["operations"], expected)
        prepared = next(row for row in controller.state()["families"] if row["id"] == "services")
        self.assertEqual(prepared["profiles"][0]["operations"], expected)
        help_value["operations"][0]["tool"] = "changed"
        self.assertEqual(controller.help({"family": "services", "profile": "slack"})["operations"], expected)
        request = {"family": "services", "profile": "slack", "operations": ["Owner.Status-v2"],
                   "values": {"token_env": "SLACK_TOKEN", "timeout_seconds": 10}}
        self.error("invalid_configuration", lambda: controller.check(request))
        self.assertFalse(any("check-config" in argv for argv, _ in controller.runner.calls))
        request["operations"] = ["auth_test"]
        checked = controller.check(request)
        stored = controller.validations[checked["validation_id"]]["path"]
        self.assertEqual(backend.decode(stored.read_bytes())["operations"], ["auth_test"])

    def test_absent_names_remain_absent_for_historical_owner(self):
        source = help_source("services")
        for row in source["operations"]:
            del row["tool"]
        controller = self.prepared_source(source)
        self.assertEqual(controller.help({"family": "services", "profile": "slack"})["operations"],
                         [{"id": "auth_test", "effect": "read"}, {"id": "read_interval", "effect": "read"}])

    def test_malformed_present_names_refuse_without_echo(self):
        invalid = [None, True, 1, [], {}, "", " leading", "trailing ", "a b", "a/b", "a:b",
                   "a\nb", "a\x00b", "a\x7fb", "caf\u00e9", "x" * 129]
        for value in invalid:
            with self.subTest(value=value):
                source = help_source("services")
                source["operations"][0]["tool"] = value
                with self.assertRaises(backend.SetupError) as caught:
                    backend.normalize_help("services", "slack", source, "")
                self.assertEqual(str(caught.exception), "inspection_unavailable")
                self.assertEqual(caught.exception.status, 500)

    def test_safe_names_preserve_case_punctuation_and_length_boundary(self):
        for name in ("x", "A.b-C_9", "x" * 128):
            with self.subTest(name=name):
                source = help_source("services")
                source["operations"][0]["tool"] = name
                self.assertEqual(backend.operation_rows("services", source)[0]["tool"], name)

    def test_duplicate_present_names_refuse_but_missing_names_do_not_collide(self):
        source = help_source("services")
        source["operations"][1]["tool"] = source["operations"][0]["tool"]
        self.error("inspection_unavailable", lambda: backend.operation_rows("services", source))
        del source["operations"][1]["tool"]
        self.assertEqual(backend.operation_rows("services", source), [
            {"id": "auth_test", "tool": "slack_auth_test", "effect": "read"},
            {"id": "read_interval", "effect": "read"}])

    def test_changed_added_or_removed_name_after_prepare_refuses_help(self):
        for change in ("changed", "added", "removed"):
            with self.subTest(change=change):
                source = help_source("services")
                if change == "added":
                    del source["operations"][0]["tool"]
                controller = self.prepared_source(source)
                if change == "removed":
                    del source["operations"][0]["tool"]
                else:
                    source["operations"][0]["tool"] = "different"
                self.error("inspection_unavailable", lambda: controller.help({"family": "services", "profile": "slack"}))

    def test_invalid_publisher_listing_never_becomes_prepared(self):
        source = help_source("services")
        source["operations"][0]["tool"] = None
        controller = self.prepared_source(source)
        state = controller.state()
        self.assertEqual(state["job"]["status"], "unconfirmed")
        prepared = next(row for row in state["families"] if row["id"] == "services")
        self.assertEqual(prepared["profiles"], [])
        self.error("family_not_prepared", lambda: controller.help({"family": "services", "profile": "slack"}))

    def test_other_family_operation_projections_unchanged(self):
        for family in ("agno", "activepieces", "official-github", "official-playwright"):
            with self.subTest(family=family):
                source = {"operations": ["fetch"], "tool": "must_not_infer"}
                self.assertEqual(backend.operation_rows(family, source), [{"id": "fetch", "effect": "unspecified"}])
                source["effect"] = "write"
                self.assertEqual(backend.operation_rows(family, source), [{"id": "fetch", "effect": "write"}])


if __name__ == "__main__":
    unittest.main()
