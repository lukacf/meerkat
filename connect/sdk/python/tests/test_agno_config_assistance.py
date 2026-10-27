"""Inert public CLI configuration assistance; no provider or donor execution."""
from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from meerkat_toolkit.adapters.agno_profiles import (
    configuration_fields, configuration_help, list_profiles, validate_configuration, ProfileConfigError,
)

CANARY = "unselected-account-value-must-not-appear"
EXTRAS = {
    "pubmed": {"email": "operator@example.invalid"}, "searxng": {"host": "https://search.invalid"},
    "wikipedia": {"language": "en"}, "bitbucket": {"account_email": "a@example.invalid", "workspace": "a", "repo_slug": "b"},
    "gitlab": {"host": "https://gitlab.invalid"}, "clickup": {"workspace_id": "42"},
    "shopify": {"shop_name": "selected", "api_version": "2026-07"}, "spotify": {"market": "SE"},
    "calcom": {"event_type_id": 42, "user_timezone": "Europe/Stockholm"},
    "zendesk": {"subdomain": "selected", "account_email": "a@example.invalid"},
    "bright-data": {"web_unlocker_zone": "selected-web", "serp_zone": "selected-serp"},
    "trello": {"token_env": "SELECTED_TOKEN"}, "salesforce": {"instance_url": "https://selected.salesforce.com"},
    "twilio": {"account_sid": "AC" + "a" * 32}, "plivo": {"auth_id": "MA" + "a" * 18},
    "reddit": {"user_agent": "Selected toolkit test operator"}, "oxylabs": {"username": "selected"},
    "aws-lambda": {"access_key_id_env": "SELECTED_ACCESS_ID", "session_token_env": "SELECTED_SESSION", "region": "us-east-1"},
    "postgres": {"socket_directory": "/tmp/selected", "port": 65432, "database": "selected", "user": "reader",
                 "password_env": "SELECTED_PASSWORD", "schema": "public", "max_rows": 10},
}
PUBLIC = {"hackernews", "pubmed", "searxng", "arxiv", "youtube", "wikipedia", "postgres"}

# Literal names independently traced through real adapter/donor controls in
# test_agno_mcp.py, test_agno_expansion.py, test_agno_library_batch.py and
# test_agno_http_readers.py. Do not derive these expectations from publisher help.
PUBLIC_OPERATION_TOOLS = {
    "hackernews": {"get_top_hackernews_stories": "get_top_hackernews_stories",
                   "get_user_details": "get_user_details"},
    "pubmed": {"search_pubmed": "search_pubmed"},
    "searxng": {"search_web": "search_web", "news_search": "news_search"},
    "arxiv": {"search_arxiv_and_return_articles": "search_arxiv_and_return_articles"},
    "youtube": {"get_youtube_video_data": "get_youtube_video_data"},
    "wikipedia": {"search_wikipedia": "search_wikipedia"},
}


def configurations():
    # Deliberately independent of the new field declarations.
    for profile in list_profiles()["profiles"]:
        name = profile["profile"]
        yield {"profile": name, "operations": profile["operations"],
               "config": {"timeout": 7, **({} if name in PUBLIC else {"api_key_env": "SELECTED_KEY"}), **EXTRAS.get(name, {})}}


GUARD = r'''
import importlib.abc, json, os, runpy, socket, sys
sys.path.insert(0, sys.argv.pop(1))
class Imports(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname.split('.')[0] in {'agno','anyio','jsonschema','mcp','referencing','requests','httpx','psycopg'} or fullname.endswith(('.agno','.agno_transports')):
            raise AssertionError('runtime import forbidden')
sys.meta_path.insert(0, Imports())
class Environment(dict):
    def get(self, name, default=None):
        if name not in {'LANG','LANGUAGE','LC_ALL','LC_MESSAGES','COLUMNS','LINES'}:
            raise AssertionError('environment read forbidden')
        return super().get(name, default)
    def __getitem__(self, name):
        if name not in {'LANG','LANGUAGE','LC_ALL','LC_MESSAGES','COLUMNS','LINES'}:
            raise AssertionError('environment read forbidden')
        return super().__getitem__(name)
os.environ = Environment({'LANG':'C.UTF-8'})
def deny(*args, **kwargs): raise AssertionError('network forbidden')
socket.socket.connect = socket.socket.connect_ex = socket.getaddrinfo = deny
sys.argv[0] = 'meerkat-toolkit-agno-mcp'
runpy.run_module('meerkat_toolkit.adapters.agno_mcp', run_name='__main__')
'''


class ConfigAssistanceTests(unittest.TestCase):
    def test_auth_guidance_covers_profiles_without_changing_field_or_operation_selection(self):
        kinds = {"account", "client_authentication", "administrator", "developer_registration",
                 "host_deployment", "artifact_preparation", "application_binding"}
        public = {"hackernews", "pubmed", "searxng", "arxiv", "youtube", "wikipedia"}
        for profile in list_profiles()["profiles"]:
            name = profile["profile"]
            with self.subTest(profile=name):
                help_ = configuration_help(name)
                self.assertEqual(help_["operations"], profile["operations"])
                self.assertEqual(help_["config_fields"], configuration_fields(name))
                rows = help_["prerequisites"]
                self.assertTrue(rows)
                self.assertTrue(help_["unresolved_details"])
                for row in rows:
                    self.assertEqual(set(row), {"kind", "instruction"})
                    self.assertIn(row["kind"], kinds)
                    self.assertIs(type(row["instruction"]), str)
                    self.assertTrue(row["instruction"])
                self.assertTrue(all(type(value) is str and value for value in help_["unresolved_details"]))
                instructions = " ".join(row["instruction"] for row in rows)
                self.assertIn("local stdio MCP server", instructions)
                self.assertIn("never send it as bearer", instructions)
                references = [f["name"] for f in help_["config_fields"] if f["credential_reference"]]
                self.assertEqual(not references, name in public)
                for reference in references:
                    self.assertIn(reference, instructions)
                self.assertNotIn("\u2013", instructions)
                self.assertNotIn("\u2014", instructions)

    def test_public_auth_declaration_and_raw_names_are_explicit_publisher_facts(self):
        # PostgreSQL is local but still authenticates; no-auth is an explicit fact.
        observed_public = set()
        for row in list_profiles()["profiles"]:
            name = row["profile"]
            with self.subTest(profile=name):
                help_ = configuration_help(name)
                if name in PUBLIC_OPERATION_TOOLS:
                    expected_names = PUBLIC_OPERATION_TOOLS[name]
                    self.assertEqual(set(row["operations"]), set(expected_names))
                    self.assertEqual(row["operation_tools"], expected_names)
                    self.assertEqual(help_["operation_tools"], expected_names)
                    self.assertEqual(help_["authentication"], {"kind": "none"})
                    self.assertFalse(any(field["credential_reference"] for field in help_["config_fields"]))
                    observed_public.add(name)
                    help_["authentication"]["kind"] = "changed"
                    self.assertEqual(configuration_help(name)["authentication"], {"kind": "none"})
                else:
                    self.assertNotIn("authentication", help_)
        self.assertEqual(observed_public, set(PUBLIC_OPERATION_TOOLS))
        self.assertNotIn("authentication", configuration_help("postgres"))

    def assertOperationMappingShape(self, mapping, operations):
        # Transport-shape coverage only; this does not prove donor publication.
        self.assertIs(type(mapping), dict)
        self.assertEqual(set(mapping), set(operations))
        for raw_name in mapping.values():
            self.assertIs(type(raw_name), str)
            self.assertRegex(raw_name, r"\A[A-Za-z0-9_.-]{1,128}\Z")
        self.assertEqual(len(set(mapping.values())), len(mapping))

    def test_operation_mapping_transport_shape_and_detached_values(self):
        for row in list_profiles()["profiles"]:
            name = row["profile"]
            with self.subTest(profile=name):
                help_ = configuration_help(name)
                self.assertOperationMappingShape(row["operation_tools"], row["operations"])
                self.assertOperationMappingShape(help_["operation_tools"], row["operations"])
                expected_observation = dict(row["operation_tools"])
                self.assertEqual(help_["operation_tools"], expected_observation)
                help_["operation_tools"].clear()
                row["operation_tools"].clear()
                self.assertEqual(configuration_help(name)["operation_tools"], expected_observation)

    def test_auth_guidance_distinguishes_credentials_and_bootstrap_without_claiming_acquisition(self):
        def instructions(name):
            return " ".join(row["instruction"] for row in configuration_help(name)["prerequisites"])
        self.assertIn("no provider credential", instructions("hackernews"))
        self.assertIn("contact address, not an account credential", instructions("pubmed"))
        self.assertIn("no credential field or login path", instructions("searxng"))
        self.assertIn("Tavily account API key", instructions("tavily"))
        self.assertIn("Spotify OAuth access token", instructions("spotify"))
        self.assertIn("user-top-read", instructions("spotify"))
        self.assertIn("not established", " ".join(configuration_help("tavily")["unresolved_details"]))
        self.assertIn("does not acquire, refresh", instructions("spotify"))
        self.assertIn("account password", instructions("oxylabs"))
        self.assertIn("not an API key", instructions("oxylabs"))
        self.assertIn("SCRAM SHA-256", instructions("postgres"))
        self.assertTrue(any(row["kind"] == "administrator" for row in configuration_help("postgres")["prerequisites"]))
        self.assertIn("api_key_env names the AWS secret access key", instructions("aws-lambda"))
        self.assertIn("access_key_id_env names its matching access key ID", instructions("aws-lambda"))
        self.assertIn("session_token_env optionally names", instructions("aws-lambda"))
        self.assertIn("token_env names its separately selected user token", instructions("trello"))
        self.assertIn("read:pullrequest:bitbucket", instructions("bitbucket"))
        self.assertIn("personal access token with read_api", instructions("gitlab"))
        self.assertIn("does not filter get_upcoming_bookings", instructions("calcom"))

    def test_auth_guidance_is_detached_under_nested_mutation(self):
        for name in (row["profile"] for row in list_profiles()["profiles"]):
            original = configuration_help(name)
            changed = configuration_help(name)
            changed["prerequisites"][0]["instruction"] = CANARY
            changed["prerequisites"][0]["kind"] = CANARY
            changed["prerequisites"].append({"kind": CANARY, "instruction": CANARY})
            changed["unresolved_details"][:] = [CANARY]
            self.assertEqual(configuration_help(name), original)

    def test_auth_guidance_cli_is_cold_inert_and_never_reads_secrets(self):
        root = str(Path(__file__).resolve().parents[1] / "src")
        # Synthetic values only. The existing guard refuses any credential read,
        # external runtime import, DNS lookup or socket connection in the child.
        guard = GUARD.replace("Environment({'LANG':'C.UTF-8'})",
                              "Environment({'LANG':'C.UTF-8','SELECTED_KEY':" + repr(CANARY)
                              + ",'SELECTED_PASSWORD':" + repr(CANARY) + "})")
        for profile in list_profiles()["profiles"]:
            name = profile["profile"]
            with self.subTest(profile=name):
                result = subprocess.run([sys.executable, "-S", "-c", guard, root,
                                         "config-help", "--profile", name],
                                        env={"LANG": "C.UTF-8"}, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stderr, "")
                self.assertEqual(json.loads(result.stdout), configuration_help(name))
                self.assertNotIn(CANARY, result.stdout + result.stderr)

    def test_auth_guidance_rejects_profile_guesses_without_disclosure(self):
        from meerkat_toolkit.adapters.agno_mcp import main
        for value in ("Spotify", "spotify ", "spotify/" + CANARY, "api_key=" + CANARY):
            with self.assertRaises(ProfileConfigError) as caught:
                configuration_help(value)
            self.assertNotIn(value, str(caught.exception))
            out, err = io.StringIO(), io.StringIO()
            with redirect_stdout(out), redirect_stderr(err):
                code = main(["config-help", "--profile", value])
            self.assertEqual(code, 1)
            self.assertEqual(out.getvalue(), "")
            self.assertNotIn(value, err.getvalue())
            self.assertNotIn(CANARY, err.getvalue())

    def test_help_fields_match_valid_configurations_and_required_omissions(self):
        seen = []
        for value in configurations():
            name = value["profile"]
            with self.subTest(profile=name):
                selected = validate_configuration(name, operations=value["operations"], config=value["config"])
                help_ = configuration_help(name)
                self.assertEqual(help_["operations"], value["operations"])
                fields = {field["name"]: field for field in help_["config_fields"]}
                self.assertLessEqual(set(value["config"]), fields.keys())
                for field in fields.values():
                    changed = deepcopy(value["config"])
                    changed.pop(field["name"], None)
                    # Zendesk's flat help lists account_email as optional because
                    # OAuth forbids it; the legacy mode still requires it.
                    conditional_legacy_email = name == "zendesk" and field["name"] == "account_email"
                    if field["required"] or conditional_legacy_email:
                        with self.assertRaises(ProfileConfigError):
                            validate_configuration(name, operations=value["operations"], config=changed)
                    else:
                        validate_configuration(name, operations=value["operations"], config=changed)
                    if field["credential_reference"]:
                        self.assertEqual(field["kind"], "string")
                self.assertEqual(selected.name, name)
                seen.append(name)
        self.assertEqual(len(seen), 36)

    def check_predecessor_equivalence(self, path):
        spec = importlib.util.spec_from_file_location("config_assistance_predecessor", path)
        baseline = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = baseline
        spec.loader.exec_module(baseline)
        def outcome(module, value):
            try:
                result = module.validate_configuration(value["profile"], operations=value["operations"], config=value["config"])
                return (result.name, result.operations, result.config)
            except module.ProfileConfigError:
                return None
        current = sys.modules[validate_configuration.__module__]
        for original in configurations():
            values = [original]
            for key in original["config"]:
                missing = deepcopy(original); missing["config"].pop(key); values.append(missing)
                for replacement in (None, True, 0, "", [], {}, CANARY):
                    mutated = deepcopy(original); mutated["config"][key] = replacement; values.append(mutated)
            values += [{**original, "operations": []}, {**original, "operations": [CANARY]},
                       {**original, "config": {**original["config"], CANARY: CANARY}}]
            for value in values:
                with self.subTest(profile=original["profile"], config=value["config"]):
                    self.assertEqual(outcome(current, value), outcome(baseline, value))
        for name, expected in {"pubmed": {"max_results":10,"results_expanded":True}, "jina": {"max_content_length":10000,"search_query_content":True},
                               "openweather": {"units":"metric"}, "linkup": {"depth":"standard","output_type":"searchResults"}, "x": {"include_post_metrics":False}}.items():
            value = next(item for item in configurations() if item["profile"] == name)
            result = validate_configuration(name, operations=value["operations"], config=value["config"])
            self.assertEqual({key: result.config[key] for key in expected}, expected)

    def test_public_commands_work_without_runtime_imports_credentials_or_network(self):
        root = str(Path(__file__).resolve().parents[1] / "src")
        def command(args):
            return subprocess.run([sys.executable, "-S", "-c", GUARD, root, *args],
                                  env={"LANG":"C.UTF-8"}, capture_output=True, text=True, timeout=10)
        listing = command(["profiles"])
        self.assertEqual(listing.returncode, 0, listing.stderr)
        self.assertEqual(json.loads(listing.stdout), list_profiles())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile.json"
            for value in configurations():
                path.write_text(json.dumps(value)); before = path.read_bytes()
                for args in (["config-help", "--profile", value["profile"]], ["check-config", "--config", str(path)]):
                    result = command(args)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stderr, "")
                    parsed = json.loads(result.stdout)
                    self.assertEqual(parsed["profile"], value["profile"])
                    no_auth = value["profile"] in PUBLIC_OPERATION_TOOLS
                    if args[0] == "config-help":
                        self.assertOperationMappingShape(parsed["operation_tools"], value["operations"])
                        if no_auth:
                            self.assertEqual(parsed["operation_tools"], PUBLIC_OPERATION_TOOLS[value["profile"]])
                            self.assertEqual(parsed["authentication"], {"kind": "none"})
                        else:
                            self.assertNotIn("authentication", parsed)
                    elif no_auth:
                        self.assertEqual(parsed["credential_environment_references"], [])
                    else:
                        self.assertTrue(parsed["credential_environment_references"])
                self.assertEqual(path.read_bytes(), before)

    def test_public_failures_are_value_free_and_never_accept_guesses(self):
        from meerkat_toolkit.adapters.agno_mcp import main
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config.json"
            for content in (CANARY, '[1]', '{"profile":"hackernews"}', '{"profile":"hackernews","operations":[],"config":{}}',
                            '{"profile":"hackernews","profile":"pubmed","operations":["search_pubmed"],"config":{}}'):
                path.write_text(content)
                out, err = io.StringIO(), io.StringIO()
                with redirect_stdout(out), redirect_stderr(err):
                    code = main(["check-config", "--config", str(path)])
                self.assertEqual(code, 1); self.assertEqual(out.getvalue(), ""); self.assertNotIn(CANARY, err.getvalue())
            for args in (["config-help","--profile",CANARY], ["config-help","--profile","Hacker News"],
                         ["profiles",CANARY], [CANARY], ["check-config","--profile",CANARY]):
                out, err = io.StringIO(), io.StringIO()
                with redirect_stdout(out), redirect_stderr(err): code = main(args)
                self.assertEqual(code, 1); self.assertEqual(out.getvalue(), ""); self.assertNotIn(CANARY, err.getvalue())

    def test_help_and_lists_are_detached_from_future_calls(self):
        first = configuration_help("postgres"); first["config_fields"].clear(); first["operations"].clear()
        self.assertTrue(configuration_help("postgres")["config_fields"])
        self.assertEqual(configuration_help("postgres")["operations"], ["show_tables", "describe_table"])


if __name__ == "__main__": unittest.main()
