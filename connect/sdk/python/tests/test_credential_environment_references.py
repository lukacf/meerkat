"""Check-config exposes selected credential names without reading their values."""

from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parents[1] / "src"
CANARY = "synthetic-credential-value-must-not-appear"
GUARD = r'''
import importlib.abc, os, runpy, socket, sys
sys.path.insert(0, sys.argv.pop(1))
module = sys.argv.pop(1)
class Imports(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname.split('.')[0] in {'agno','anyio','jsonschema','mcp','referencing','requests','httpx','httpcore','psycopg'} or fullname.startswith('meerkat_toolkit.connectors') or fullname.endswith(('.agno','.agno_transports')):
            raise AssertionError('runtime import forbidden')
sys.meta_path.insert(0, Imports())
class Environment(dict):
    allowed = {'LANG','LANGUAGE','LC_ALL','LC_MESSAGES','COLUMNS','LINES'}
    def get(self, name, default=None):
        if name not in self.allowed: raise AssertionError('credential lookup forbidden')
        return super().get(name, default)
    def __getitem__(self, name):
        if name not in self.allowed: raise AssertionError('credential lookup forbidden')
        return super().__getitem__(name)
    def __contains__(self, name):
        if name not in self.allowed: raise AssertionError('credential membership forbidden')
        return super().__contains__(name)
    def __iter__(self): raise AssertionError('environment iteration forbidden')
    def keys(self): raise AssertionError('environment iteration forbidden')
    def items(self): raise AssertionError('environment iteration forbidden')
    def values(self): raise AssertionError('environment iteration forbidden')
    def copy(self): raise AssertionError('environment copying forbidden')
os.environ = Environment({'LANG':'C.UTF-8','SELECTED_NAME':'synthetic-credential-value-must-not-appear'})
def deny(*args, **kwargs): raise AssertionError('network forbidden')
socket.socket.connect = socket.socket.connect_ex = socket.getaddrinfo = deny
sys.argv[0] = module
runpy.run_module(module, run_name='__main__')
'''


def agno(profile, operations, **config):
    return {"profile": profile, "operations": operations, "config": {"timeout": 10, **config}}


def service(profile, reference, **extra):
    return {"profile": profile, "operations": ["auth_test" if profile == "slack" else "get_message"],
            "config": {"token_env": reference, "timeout_seconds": 10, "max_response_bytes": 100000,
                       **({"user_id": "me", "max_message_bytes": 10000} if profile == "gmail" else {}),
                       **extra}}


def row(name, value):
    return {"field_path": ["config", name], "environment_variable": value}


class CredentialEnvironmentReferencesTests(unittest.TestCase):
    def command(self, family, value, expected=0, diagnostic=None):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config.json"
            raw = json.dumps(value).encode()
            path.write_bytes(raw)
            result = subprocess.run(
                [sys.executable, "-I", "-S", "-B", "-c", GUARD, str(SOURCE),
                 f"meerkat_toolkit.adapters.{family}_mcp", "check-config", "--config", str(path)],
                env={"LANG": "C.UTF-8"}, capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(result.returncode, expected, result.stderr)
            self.assertEqual(path.read_bytes(), raw)
            self.assertNotIn(CANARY, result.stdout + result.stderr)
            if expected:
                if diagnostic is None:
                    self.assertEqual(result.stdout, "")
                else:
                    code, field_path = diagnostic
                    self.assertEqual(json.loads(result.stdout), {
                        "valid": False, "scope": "configuration_only",
                        "diagnostic": {"version": 1, "code": code, "field_path": field_path},
                    })
                    self.assertEqual(result.stderr,
                        "Service MCP command failed; check the selected configuration and dependencies.\n")
                return None
            self.assertEqual(result.stderr, "")
            parsed = json.loads(result.stdout)
            self.assertEqual(set(parsed), {"profile", "valid", "scope", "credential_environment_references"})
            self.assertEqual(parsed["profile"], value["profile"])
            self.assertIs(parsed["valid"], True)
            self.assertEqual(parsed["scope"], "configuration_only")
            return parsed["credential_environment_references"]

    def test_public_profiles_return_an_empty_array_without_environment_reads(self):
        for value in [agno("hackernews", ["get_top_hackernews_stories"]),
                      agno("pubmed", ["search_pubmed"], email="SELECTED_NAME@example.invalid"),
                      agno("searxng", ["search_web"], host="https://search.invalid"),
                      agno("arxiv", ["search_arxiv_and_return_articles"]),
                      agno("youtube", ["get_youtube_video_data"]),
                      agno("wikipedia", ["search_wikipedia"], language="en")]:
            with self.subTest(profile=value["profile"]):
                self.assertEqual(self.command("agno", value), [])

    def test_trello_keeps_literal_names_paths_and_alias_rows(self):
        value = agno("trello", ["list_boards"], api_key_env="AccountA_TrelloKey", token_env="accountA_Token")
        self.assertEqual(self.command("agno", value),
                         [row("api_key_env", "AccountA_TrelloKey"), row("token_env", "accountA_Token")])
        value["config"] |= {"api_key_env": "PATH", "token_env": "PATH"}
        # A reference named PATH is a description, not permission to inject it.
        self.assertEqual(self.command("agno", value), [row("api_key_env", "PATH"), row("token_env", "PATH")])

    def test_aws_omits_absent_optional_reference_and_retains_present_one(self):
        value = agno("aws-lambda", ["list_functions"], api_key_env="Selected_Secret",
                     access_key_id_env="Selected_Access", region="us-east-1")
        expected = [row("access_key_id_env", "Selected_Access"), row("api_key_env", "Selected_Secret")]
        self.assertEqual(self.command("agno", value), expected)
        value["config"]["session_token_env"] = "Selected_Session"
        self.assertEqual(self.command("agno", value), expected + [row("session_token_env", "Selected_Session")])

    def test_postgres_and_services_keep_separate_selected_account_references(self):
        postgres = agno("postgres", ["show_tables"], socket_directory="/tmp/fixture-socket",
                        port=65432, database="fixture", user="reader", password_env="Selected_Password",
                        schema="public", max_rows=10)
        self.assertEqual(self.command("agno", postgres), [row("password_env", "Selected_Password")])
        for profile in ("slack", "gmail"):
            for reference in ("ACCOUNT_A", "Account_B", "PATH", "ACCOUNT_A"):
                with self.subTest(profile=profile, reference=reference):
                    self.assertEqual(self.command("services", service(profile, reference)), [row("token_env", reference)])

    def test_invalid_configuration_never_emits_a_projection(self):
        values = [("agno", agno("trello", ["list_boards"], api_key_env="SELECTED_NAME", token_env="OTHER")),
                  ("services", service("gmail", "SELECTED_NAME"))]
        for family, original in values:
            for change in ("invalid-reference", "unknown-field", "invalid-bound"):
                value = deepcopy(original)
                if change == "invalid-reference":
                    value["config"]["token_env"] = CANARY
                elif change == "unknown-field":
                    value["config"][CANARY] = CANARY
                else:
                    value["config"]["timeout" if family == "agno" else "timeout_seconds"] = 0
                with self.subTest(family=family, change=change):
                    diagnostic = {
                        "invalid-reference": ("invalid_credential_reference", ["config", "token_env"]),
                        "unknown-field": ("invalid_config_fields", ["config"]),
                        "invalid-bound": ("integer_out_of_range", ["config", "timeout_seconds"]),
                    }[change] if family == "services" else None
                    self.command(family, value, expected=1, diagnostic=diagnostic)

    def test_projection_uses_declared_flags_and_the_single_validated_load(self):
        from meerkat_toolkit.adapters import agno_mcp, services_mcp
        from meerkat_toolkit.adapters.agno_profiles import validate_configuration as validate_agno
        from meerkat_toolkit.adapters.services_config import validate_configuration as validate_service
        cases = [
            (agno_mcp, validate_agno("trello", operations=["list_boards"],
                                    config={"timeout": 10, "api_key_env": "KEY", "token_env": "TOKEN"}),
             {"config_fields": [{"name": "api_key_env", "credential_reference": False},
                                {"name": "token_env", "credential_reference": True}]}, [row("token_env", "TOKEN")]),
            (services_mcp, validate_service(service("slack", "TOKEN")),
             {"config_fields": {"token_env": {"credential_reference": False}, "timeout_seconds": {}}}, []),
        ]
        for module, selected, help_, expected in cases:
            with self.subTest(module=module.__name__), patch.object(module, "load_configuration", return_value=selected) as load:
                with patch.object(module, "configuration_help", return_value=help_):
                    out, err = io.StringIO(), io.StringIO()
                    with redirect_stdout(out), redirect_stderr(err):
                        code = module.main(["check-config", "--config", "/not-read-again.json"])
                    self.assertEqual(code, 0, err.getvalue())
                    self.assertEqual(json.loads(out.getvalue())["credential_environment_references"], expected)
                    load.assert_called_once_with("/not-read-again.json")
            with patch.object(module, "load_configuration", side_effect=ValueError()), patch.object(module, "configuration_help") as help_call:
                with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    self.assertEqual(module.main(["check-config", "--config", "/invalid.json"]), 1)
                help_call.assert_not_called()


if __name__ == "__main__":
    unittest.main()
