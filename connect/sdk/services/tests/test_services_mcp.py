"""Offline tests through the official MCP client and real connector owners."""

from __future__ import annotations

import asyncio
import base64
from copy import deepcopy
import hashlib
import importlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import traceback
import unittest
from unittest.mock import patch

import httpx
from mcp import Client, StdioServerParameters


ROOT = Path(__file__).resolve().parents[3]
NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
SOURCE_PATH = os.environ.get("PYTHONPATH", str(ROOT / "sdk/python/src"))
MODULE = NAMESPACE + ".adapters.services_mcp"
adapter = importlib.import_module(MODULE)
config_module = importlib.import_module(NAMESPACE + ".adapters.services_config")
ConfigurationError = config_module.ConfigurationError
CANARY = "fixture-value-must-not-appear-in-errors"
MIME = b"To: first@example.invalid\r\nCc: second@example.invalid\r\nBcc: third@example.invalid\r\nSubject: Fixture\r\n\r\nExact body\x00\xff"


def configuration(profile="gmail", operations=None, **overrides):
    options = {"token_env": "SELECTED_TOKEN", "timeout_seconds": 3, "max_response_bytes": 65536}
    if profile == "gmail":
        options.update(user_id="fixture@example.invalid", max_message_bytes=1024)
    operations = list(config_module.OPERATIONS[profile] if operations is None else operations)
    if "read_interval" in operations:
        options.update(max_interval_requests=3, max_interval_messages=20)
    options.update(overrides)
    return {"profile": profile, "operations": operations, "config": options}


def environment(**values):
    return {"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": SOURCE_PATH, **values}


async def eventually(predicate, timeout=5):
    async with asyncio.timeout(timeout):
        while not predicate():
            await asyncio.sleep(0.01)


# Test-only transport injection; neither the CLI nor configuration accepts it.
STDIO_FIXTURE = r'''
import asyncio, importlib, json, sys
from pathlib import Path
from unittest.mock import patch
import httpx
module, config_file, log_file, release_file, mode = sys.argv[1:]
adapter = importlib.import_module(module)
log, release = Path(log_file), Path(release_file)
def emit(event, **data):
    with log.open('a') as output:
        output.write(json.dumps({'event': event, **data}) + '\n')
class Stream(httpx.AsyncByteStream):
    def __init__(self, payload, gate=False):
        self.payload, self.gate = json.dumps(payload).encode(), gate
    async def __aiter__(self):
        yield self.payload
    async def aclose(self):
        emit('response_closing')
        if self.gate:
            async with asyncio.timeout(10):
                while not release.exists():
                    await asyncio.sleep(.01)
        emit('response_closed')
async def handle(request):
    emit('request', method=request.method, url=str(request.url),
         authorization=request.headers.get('authorization'), body=request.content.decode())
    path = request.url.path
    status = 200
    if path == '/api/auth.test':
        payload = {'ok': True, 'team_id': 'T1', 'user_id': 'U1'}
    elif path == '/api/conversations.list':
        payload = {'ok': True, 'channels': [{'id': 'C1'}], 'response_metadata': {'next_cursor': 'next'}}
    elif path == '/api/conversations.info':
        payload = {'ok': True, 'channel': {'id': request.url.params['channel']}}
    elif path == '/api/chat.getPermalink':
        payload = {'ok': True, 'channel': request.url.params['channel'],
                   'permalink': 'https://fixture.slack.com/archives/C1/p11000000?thread_ts=10.0&cid=C1'}
    elif path == '/api/conversations.history':
        payload = {'ok': True, 'messages': [{'ts': '11.0', 'text': 'fixture', 'reply_count': 1}], 'has_more': False}
    elif path == '/api/conversations.replies':
        payload = {'ok': True, 'messages': [{'ts': '11.0', 'thread_ts': '11.0'}, {'ts': '12.0', 'thread_ts': '11.0'}], 'has_more': False}
    elif path.endswith('/messages/send'):
        payload = {'id': 'm1', 'threadId': 't1'}
        if mode == 'malformed_write':
            payload = {'provider_secret': 'not-a-message'}
        if mode == 'rate_limit':
            status, payload = 429, {'private': 'provider-body-secret'}
        if mode == 'redirect':
            status, payload = 302, {}
    elif path.endswith('/drafts'):
        payload = {'id': 'd1', 'message': {'id': 'm1', 'threadId': 't1'}}
    elif path.endswith('/messages'):
        payload = {'messages': [{'id': 'm1', 'threadId': 't1'}], 'nextPageToken': 'opaque', 'resultSizeEstimate': 100}
    elif path.endswith('/messages/m1'):
        payload = {'id': 'm1', 'threadId': 't1', 'raw': 'aGk=', 'payload': {'headers': [{'name': 'Subject', 'value': 'Fixture'}]}}
    else:
        raise AssertionError('Unexpected fixture request')
    return httpx.Response(status, headers={'content-type': 'application/json', 'retry-after': '2', 'location': 'https://unselected.invalid/'},
                          stream=Stream(payload, mode == 'gated'), request=request)
class OwnedClient(httpx.AsyncClient):
    async def aclose(self):
        emit('client_closing')
        await super().aclose()
        emit('client_closed')
original = adapter.create_profile
async def create(configuration):
    def factory(**kwargs):
        emit('client_settings', base_url=kwargs['base_url'], trust_env=kwargs['trust_env'],
             follow_redirects=kwargs['follow_redirects'], timeout=kwargs['timeout'])
        return OwnedClient(transport=httpx.MockTransport(handle), **kwargs)
    return await original(configuration, client_factory=factory)
with patch.object(adapter, 'create_profile', side_effect=create):
    raise SystemExit(adapter.main(['serve', '--config', config_file]))
'''


class Fixture:
    def __init__(self, base, value, mode="normal", token="fixture-token-A"):
        self.base = Path(base)
        self.base.mkdir()
        self.config, self.log, self.release = [self.base / name for name in ("config.json", "events.jsonl", "release")]
        self.config.write_text(json.dumps(value))
        self.params = StdioServerParameters(command=sys.executable, args=["-c", STDIO_FIXTURE, MODULE,
            str(self.config), str(self.log), str(self.release), mode], env=environment(SELECTED_TOKEN=token))

    def events(self, kind=None):
        rows = [] if not self.log.exists() else [json.loads(line) for line in self.log.read_text().splitlines()]
        return rows if kind is None else [row for row in rows if row["event"] == kind]


class ConfigurationTests(unittest.TestCase):
    def test_strict_configuration_no_value_leaks(self):
        good = json.dumps(configuration())
        bad = [good.replace('"timeout_seconds": 3', '"timeout_seconds": true'),
               good.replace('"timeout_seconds": 3', '"timeout_seconds": 61'),
               good.replace('"timeout_seconds": 3', '"timeout_seconds": NaN'),
               good.replace('"timeout_seconds": 3', '"timeout_seconds": 1e999'),
               good.replace('"timeout_seconds": 3', '"timeout_seconds": 3, "timeout_seconds": 2'),
               good.replace('"token_env": "SELECTED_TOKEN"', '"token_env": "' + CANARY + '\\n"'),
               good.replace('"user_id": "fixture@example.invalid"', '"user_id": "' + CANARY + '"'),
               good.replace('"max_message_bytes": 1024', '"max_message_bytes": 0'),
               good.replace('"profile": "gmail"', '"profile": "' + CANARY + '"'),
               good.replace('"send_raw"', '"modify_message"'),
               good.replace('"send_raw"', '"get_message"'),
               good.replace('"config": {', '"config": {"base_url": "https://' + CANARY + '/",'),
               '{"profile":"gmail"}', '[]', 'null', CANARY, " " * 65537]
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "config.json"
            for body in bad:
                with self.subTest(body=body[:30]):
                    path.write_text(body)
                    with self.assertRaises(ConfigurationError) as caught:
                        config_module.load_configuration(path)
                    self.assertIsNone(caught.exception.__context__)
                    self.assertNotIn(CANARY, "".join(traceback.format_exception(caught.exception)))
            with self.assertRaises(ConfigurationError):
                config_module.load_configuration(folder)
            pipe = Path(folder) / "pipe"
            os.mkfifo(pipe)
            with self.assertRaises(ConfigurationError):
                config_module.load_configuration(pipe)

    def test_inert_commands_without_site_packages_credentials_or_runtime_imports(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "config.json"
            script = r'''
import importlib, os, sys
from unittest.mock import patch
module, *args = sys.argv[1:]
adapter = importlib.import_module(module)
class Guard(dict):
    def __getitem__(self, key):
        if key == 'SELECTED_TOKEN':
            raise AssertionError('credential resolution entered')
        return super().__getitem__(key)
with patch.object(os, 'environ', Guard(os.environ)):
    result = adapter.main(args)
assert not any(name == root or name.startswith(root + '.') for name in sys.modules
               for root in ('mcp', 'httpx', 'anyio', 'jsonschema', 'referencing'))
assert not any('.connectors.' in name for name in sys.modules)
raise SystemExit(result)
'''
            for profile in ("slack", "gmail"):
                path.write_text(json.dumps(configuration(profile)))
                commands = [["profiles"], ["config-help", "--profile", profile],
                            ["check-config", "--config", str(path)], ["describe", "--config", str(path)]]
                for command in commands:
                    with self.subTest(profile=profile, command=command[0]):
                        run = subprocess.run([sys.executable, "-S", "-c", script, MODULE, *command],
                            env=environment(), capture_output=True, text=True, timeout=10)
                        self.assertEqual(run.returncode, 0, run.stderr)
                        self.assertIsInstance(json.loads(run.stdout), dict)
                        self.assertEqual(run.stderr, "")

    def test_serve_modes_select_actual_handlers_with_validated_configuration(self):
        value = configuration("gmail", ["get_message"])
        expected = config_module.validate_configuration(value)
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "config.json"
            path.write_text(json.dumps(value))
            for mode, handler in (("serve-native", adapter.serve_native_stdio),
                                  ("serve", adapter.serve_stdio)):
                with self.subTest(mode=mode), patch("anyio.run") as run:
                    self.assertEqual(adapter.main([mode, "--config", str(path)]), 0)
                    run.assert_called_once_with(handler, expected, backend="asyncio")

    def test_staging_is_exact_and_excludes_private_and_test_payloads(self):
        spec = importlib.util.spec_from_file_location("service_stage", ROOT / "sdk/services/stage.py")
        stage = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(stage)
        with tempfile.TemporaryDirectory() as folder:
            target = Path(folder) / "component"
            manifest = stage.stage(target)
            for relative, entry in manifest["source_mapping"].items():
                self.assertEqual((target / relative).read_bytes(), (ROOT / entry["canonical_source"]).read_bytes())
                self.assertEqual(hashlib.sha256((target / relative).read_bytes()).hexdigest(), entry["sha256"])
            installed = {str(path.relative_to(target)) for path in target.rglob("*") if path.is_file()}
            module_names = {
                "__init__.py", "httpx_client.py", "connectors/__init__.py",
                "connectors/_httpx_stream.py", "connectors/gmail.py", "connectors/slack.py",
                "connectors/gmail_content.py", "connectors/_google_errors.py",
                "connectors/google_calendar.py", "connectors/google_drive.py",
                "adapters/__init__.py", "adapters/_mcp_admission.py", "adapters/services_config.py", "adapters/services_mcp.py",
                "adapters/gmail_composition.py", "adapters/_connector_call.py", "auth/__init__.py",
                "auth/google_oauth.py", "auth/slack_oauth.py",
            }
            guide_names = {"SERVICES-HOST-CLIENT.md", "GOOGLE-OAUTH.md", "SLACK-OAUTH.md"}
            prefix = "src/meerkat_toolkit_services/"
            self.assertEqual(installed, {
                "LICENSE-MIT", "LICENSE-APACHE", "pyproject.toml", "README.md", "THIRD_PARTY_NOTICES.md",
                prefix + "PACKAGE-SOURCES.json", prefix + "GUIDE.md",
            } | {prefix + name for name in module_names | guide_names})
            self.assertEqual(set(manifest["source_mapping"]), {prefix + name for name in module_names})
            self.assertEqual(set(manifest["guide_mapping"]), {prefix + name for name in guide_names})
            self.assertEqual(
                json.loads((target / (prefix + "PACKAGE-SOURCES.json")).read_text()), manifest)
            member = "meerkat_toolkit_services/adapters/services_mcp.py"
            member_sha256 = hashlib.sha256((target / ("src/" + member)).read_bytes()).hexdigest()
            self.assertEqual(manifest["native_connector"], {
                "schema_version": 1,
                "module": "meerkat_toolkit_services.adapters.services_mcp",
                "mode": "serve-native",
                "configuration_argument": "--config",
                "member": member,
                "member_sha256": member_sha256,
            })
            self.assertEqual(member_sha256, manifest["source_mapping"]["src/" + member]["sha256"])
            for relative, entry in manifest["guide_mapping"].items():
                original = (ROOT / entry["canonical_source"]).read_bytes()
                self.assertEqual(hashlib.sha256(original).hexdigest(), entry["canonical_sha256"])
                projected = original.decode("utf-8")
                for replacement in entry["literal_replacements"]:
                    self.assertIn(replacement["from"], projected)
                    projected = projected.replace(replacement["from"], replacement["to"])
                self.assertEqual((target / relative).read_text(), projected)
                self.assertEqual(hashlib.sha256(projected.encode()).hexdigest(), entry["sha256"])
                self.assertNotIn("meerkat_toolkit.", projected)
                self.assertNotIn("../../docs/coordination/", projected)
            self.assertIn(
                "https://github.com/lukacf/meerkat/issues/1631#issuecomment-5975028207",
                (target / (prefix + "GOOGLE-OAUTH.md")).read_text())
            self.assertIn(
                "[`httpx_client.py`](httpx_client.py)",
                (target / (prefix + "SERVICES-HOST-CLIENT.md")).read_text())
            self.assertTrue(all(not any(term in path for term in ("gmail_message", "frontend", "tests/", "agno", "host_control")) for path in installed))
            for notice in ("LICENSE-MIT", "LICENSE-APACHE"):
                self.assertEqual((target / notice).read_bytes(), (ROOT / "sdk/python" / notice).read_bytes())
            with self.assertRaises(FileExistsError):
                stage.stage(target)
            alias = Path(folder) / "alias"
            alias.symlink_to(target, target_is_directory=True)
            with self.assertRaises(FileExistsError):
                stage.stage(alias)


class ProfileTests(unittest.IsolatedAsyncioTestCase):
    async def test_validation_precedes_credentials_and_client_creation(self):
        bad = adapter.Configuration("gmail", ("send_raw",), {"token_env": CANARY})
        with patch.object(adapter, "selected_token", side_effect=AssertionError("credential entered")):
            with self.assertRaises(ConfigurationError):
                await adapter.create_profile(bad, client_factory=lambda **_: self.fail("client created"))
        good = config_module.validate_configuration(configuration())
        for token in (None, "", "space value", "line\nbreak", "\x7f", "x" * 4097):
            values = {} if token is None else {"SELECTED_TOKEN": token}
            with self.subTest(token=token and token[:20]), patch.dict(os.environ, values, clear=True):
                with self.assertRaises(ConfigurationError):
                    await adapter.create_profile(good, client_factory=lambda **_: self.fail("client created"))

    async def test_post_invocation_errors_and_result_encoding_keep_write_unknown(self):
        cases = [ValueError(CANARY), TypeError(CANARY), RuntimeError(CANARY),
                 {"id": "m1", "threadId": "t1", "unexpected": object()},
                 {"id": "m1", "threadId": "t1", "unexpected": float("nan")},
                 {"id": "m1", "threadId": "t1", "unexpected": "\ud800"}]
        for value in cases:
            with self.subTest(value=type(value).__name__):
                class Operations:
                    async def send_raw(self, **_):
                        if isinstance(value, Exception):
                            raise value
                        return value
                client = httpx.AsyncClient()
                profile = adapter.ServiceProfile(config_module.validate_configuration(configuration("gmail", ["send_raw"])), Operations(), client)
                try:
                    async with Client(adapter.create_server(profile)) as mcp:
                        result = await mcp.call_tool("gmail_send_raw", {"mime_base64": base64.b64encode(MIME).decode()})
                        self.assertTrue(result.is_error)
                        error = result.structured_content["error"]
                        self.assertTrue(error["operation_invoked"])
                        self.assertEqual(error["write_outcome"], "unknown")
                        self.assertNotIn(CANARY, result.model_dump_json())
                finally:
                    await profile.aclose()

    async def test_profile_close_joins_active_call_and_repeated_cancel(self):
        entered, release, closed = asyncio.Event(), asyncio.Event(), []
        class Operations:
            async def auth_test(self):
                entered.set()
                await release.wait()
                return {"ok": True, "team_id": "T1", "user_id": "U1"}
        class OwnedClient(httpx.AsyncClient):
            async def aclose(self):
                closed.append("closed")
                await super().aclose()
        profile = adapter.ServiceProfile(config_module.validate_configuration(configuration("slack", ["auth_test"])), Operations(), OwnedClient())
        async with Client(adapter.create_server(profile)) as mcp:
            active = asyncio.create_task(mcp.call_tool("slack_auth_test", {}))
            await entered.wait()
            closing = asyncio.create_task(profile.aclose())
            await asyncio.sleep(.02)
            closing.cancel()
            await asyncio.sleep(.02)
            closing.cancel()
            self.assertFalse(closing.done())
            self.assertFalse(closed)
            release.set()
            await active
            with self.assertRaises(asyncio.CancelledError):
                await closing
            self.assertEqual(closed, ["closed"])
            refused = await mcp.call_tool("slack_auth_test", {})
            self.assertEqual(refused.structured_content["error"]["kind"], "closed")
        await profile.aclose()
        self.assertEqual(closed, ["closed"])


class StdioTests(unittest.IsolatedAsyncioTestCase):
    async def test_slack_permalink_selected_wire_and_unselected_refusal(self):
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "links", configuration("slack", ["get_permalink"]))
            async with Client(fixture.params, mode="legacy") as client:
                listing = await client.list_tools()
                self.assertEqual([tool.name for tool in listing.tools], ["slack_get_permalink"])
                self.assertTrue(listing.tools[0].annotations.read_only_hint)
                result = await client.call_tool("slack_get_permalink", {"channel": "C1", "message_ts": "11.000000"})
                self.assertFalse(result.is_error)
                self.assertEqual(result.structured_content["value"]["permalink"],
                                 "https://fixture.slack.com/archives/C1/p11000000?thread_ts=10.0&cid=C1")
                for args in ({"channel": "C1"}, {"channel": "C1", "message_ts": 11},
                             {"channel": "C1", "message_ts": "11", "token": "unselected"}):
                    refused = await client.call_tool("slack_get_permalink", args)
                    self.assertTrue(refused.is_error)
                    self.assertFalse(refused.structured_content["error"]["operation_invoked"])
                self.assertTrue((await client.call_tool("slack_history_page", {"channel": "C1"})).is_error)
            requests = fixture.events("request")
            self.assertEqual(len(requests), 1)
            self.assertEqual(requests[0]["url"], "https://slack.com/api/chat.getPermalink?channel=C1&message_ts=11.000000")
            self.assertEqual(len(fixture.events("response_closed")), 1)
            self.assertEqual(len(fixture.events("client_closed")), 1)
            unselected = Fixture(Path(folder) / "no-links", configuration("slack", ["auth_test"]))
            async with Client(unselected.params, mode="legacy") as client:
                self.assertTrue((await client.call_tool("slack_get_permalink", {"channel": "C1", "message_ts": "11"})).is_error)
            self.assertEqual(unselected.events("request"), [])

    async def test_gmail_selected_listing_exact_mime_and_cleanup(self):
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "gmail", configuration())
            async with Client(fixture.params, mode="legacy") as client:
                listing = await client.list_tools()
                self.assertEqual([tool.name for tool in listing.tools], ["gmail_" + op for op in config_module.OPERATIONS["gmail"]])
                self.assertEqual([tool.annotations.read_only_hint for tool in listing.tools], [True, True, False, False, False, True, True, True, False])
                self.assertTrue((await client.call_tool("gmail_modify_message", {"message_id": "m1"})).is_error)
                page = await client.call_tool("gmail_list_messages_page", {"query": "in:inbox", "max_results": 2})
                self.assertEqual(page.structured_content["value"]["nextPageToken"], "opaque")
                message = await client.call_tool("gmail_get_message", {"message_id": "m1", "format": "metadata", "metadata_headers": ["Subject"]})
                self.assertEqual(message.structured_content["value"]["id"], "m1")
                for operation in ("send_raw", "create_draft_raw"):
                    result = await client.call_tool("gmail_" + operation, {"mime_base64": base64.b64encode(MIME).decode(), "thread_id": "t1"})
                    self.assertFalse(result.is_error)
                for encoded in ("", "YQ", "YR==", "YQ==\n", "%%%%", base64.b64encode(b"x" * 1025).decode()):
                    result = await client.call_tool("gmail_send_raw", {"mime_base64": encoded})
                    self.assertEqual(result.structured_content["error"]["write_outcome"], "not_attempted")
            requests = fixture.events("request")
            self.assertEqual(len(requests), 4)
            for request in requests:
                self.assertTrue(request["url"].startswith("https://gmail.googleapis.com/gmail/v1/users/fixture%40example.invalid/"))
                self.assertEqual(request["authorization"], "Bearer fixture-token-A")
            for request in requests[2:]:
                body = json.loads(request["body"])
                raw = body if "raw" in body else body["message"]
                self.assertEqual(base64.urlsafe_b64decode(raw["raw"]), MIME)
                self.assertEqual(raw["threadId"], "t1")
            self.assertEqual(fixture.events("client_settings"), [{"event": "client_settings", "base_url": "https://gmail.googleapis.com/", "trust_env": False, "follow_redirects": False, "timeout": None}])
            self.assertEqual(len(fixture.events("response_closed")), 4)
            self.assertEqual(len(fixture.events("client_closed")), 1)

    async def test_slack_all_reads_interval_partial_coverage_and_caps(self):
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "slack", configuration("slack"))
            interval = {"channel": "C1", "start": "10", "end": "20", "root_oldest": "5", "max_requests": 1, "max_messages": 20}
            async with Client(fixture.params, mode="legacy") as client:
                listing = await client.list_tools()
                self.assertEqual([tool.name for tool in listing.tools], ["slack_" + op for op in config_module.OPERATIONS["slack"]])
                for operation, arguments in (("auth_test", {}), ("list_conversations_page", {"types": ["private_channel"], "limit": 1}),
                    ("get_conversation", {"channel": "C1"}), ("history_page", {"channel": "C1", "oldest": "10"}),
                    ("replies_page", {"channel": "C1", "ts": "11.0"})):
                    self.assertFalse((await client.call_tool("slack_" + operation, arguments)).is_error)
                result = await client.call_tool("slack_read_interval", interval)
                value = result.structured_content["value"]
                self.assertEqual(value["termination"], "request_budget")
                self.assertEqual(value["requests_made"], 1)
                self.assertEqual([gap["kind"] for gap in value["coverage_gaps"]], ["older_roots_unsearched", "request_budget"])
                for key, amount in (("max_requests", 4), ("max_messages", 21)):
                    refused = await client.call_tool("slack_read_interval", {**interval, key: amount})
                    self.assertFalse(refused.structured_content["error"]["operation_invoked"])
                self.assertEqual(len(fixture.events("request")), 6)
                completed = await client.call_tool("slack_read_interval", {**interval, "max_requests": 3})
                self.assertEqual(completed.structured_content["value"]["termination"], "finished")
                self.assertEqual(completed.structured_content["value"]["coverage_gaps"], [{"kind": "older_roots_unsearched", "source": "history"}])
                self.assertTrue((await client.call_tool("slack_send_message", {})).is_error)
            self.assertEqual(len(fixture.events("request")), 8)
            self.assertEqual(len(fixture.events("client_closed")), 1)

    async def test_account_process_and_operation_subset_isolation(self):
        with tempfile.TemporaryDirectory() as folder:
            first = Fixture(Path(folder) / "a", configuration("gmail", ["get_message"], user_id="a@example.invalid"), token="token-A")
            second = Fixture(Path(folder) / "b", configuration("gmail", ["send_raw"], user_id="b@example.invalid"), token="token-B")
            async with Client(first.params, mode="legacy") as a, Client(second.params, mode="legacy") as b:
                self.assertEqual([tool.name for tool in (await a.list_tools()).tools], ["gmail_get_message"])
                self.assertEqual([tool.name for tool in (await b.list_tools()).tools], ["gmail_send_raw"])
                self.assertTrue((await a.call_tool("gmail_send_raw", {"mime_base64": base64.b64encode(MIME).decode()})).is_error)
                self.assertTrue((await b.call_tool("gmail_get_message", {"message_id": "m1"})).is_error)
                await a.call_tool("gmail_get_message", {"message_id": "m1"})
                await b.call_tool("gmail_send_raw", {"mime_base64": base64.b64encode(MIME).decode()})
            for fixture, identity in ((first, "a"), (second, "b")):
                requests = fixture.events("request")
                self.assertEqual(len(requests), 1)
                self.assertIn("/users/" + identity + "%40example.invalid/", requests[0]["url"])
                self.assertEqual(requests[0]["authorization"], "Bearer token-" + identity.upper())
                self.assertEqual(len(fixture.events("client_closed")), 1)

    async def test_entered_write_failure_does_not_retry_or_leak_body(self):
        with tempfile.TemporaryDirectory() as folder:
            for mode, kind in (("malformed_write", "invalid_response"), ("rate_limit", "rate_limited"), ("redirect", "redirect")):
                with self.subTest(mode=mode):
                    fixture = Fixture(Path(folder) / mode, configuration("gmail", ["send_raw"]), mode)
                    async with Client(fixture.params, mode="legacy") as client:
                        result = await client.call_tool("gmail_send_raw", {"mime_base64": base64.b64encode(MIME).decode()})
                        self.assertEqual(result.structured_content["error"]["kind"], kind)
                        self.assertEqual(result.structured_content["error"]["write_outcome"], "unknown")
                        self.assertNotIn("provider-body-secret", result.model_dump_json())
                    self.assertEqual(len(fixture.events("request")), 1)
                    self.assertEqual(len(fixture.events("response_closed")), 1)
                    self.assertEqual(len(fixture.events("client_closed")), 1)

    async def test_cancelled_write_waits_for_response_close_before_following_call(self):
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "cancel", configuration("gmail", ["send_raw"]), "gated")
            async with Client(fixture.params, mode="legacy") as client:
                active = asyncio.create_task(client.call_tool("gmail_send_raw", {"mime_base64": base64.b64encode(MIME).decode()}))
                following = None
                try:
                    await eventually(lambda: fixture.events("response_closing"))
                    active.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await active
                    await asyncio.wait_for(client.send_ping(), 1)
                    await asyncio.wait_for(client.list_tools(), 1)
                    following = asyncio.create_task(client.call_tool("gmail_send_raw", {"mime_base64": base64.b64encode(MIME).decode()}))
                    await asyncio.sleep(.1)
                    self.assertFalse(following.done())
                    self.assertEqual(len(fixture.events("request")), 1)
                    self.assertFalse(fixture.events("client_closed"))
                    fixture.release.touch()
                    self.assertFalse((await asyncio.wait_for(following, 3)).is_error)
                finally:
                    fixture.release.touch()
                    await asyncio.gather(*[task for task in (active, following) if task], return_exceptions=True)
            events = fixture.events()
            self.assertLess(next(i for i, row in enumerate(events) if row["event"] == "response_closed"),
                            [i for i, row in enumerate(events) if row["event"] == "request"][1])
            self.assertEqual(events[-1]["event"], "client_closed")
            self.assertEqual(len(fixture.events("client_closed")), 1)

    async def test_saturated_stdio_keeps_cancelled_write_owner_until_response_cleanup_joins(self):
        with tempfile.TemporaryDirectory() as folder:
            fixture = Fixture(Path(folder) / "admission", configuration("gmail", ["send_raw"]), "gated")
            arguments = {"mime_base64": base64.b64encode(MIME).decode()}
            tasks = []
            async with Client(fixture.params, mode="legacy") as client:
                try:
                    tasks.append(asyncio.create_task(client.call_tool("gmail_send_raw", arguments)))
                    await eventually(lambda: fixture.events("response_closing"))
                    tasks.extend(asyncio.create_task(client.call_tool("gmail_send_raw", arguments)) for _ in range(3))
                    await asyncio.wait_for(client.send_ping(), 1)
                    await asyncio.sleep(.05)
                    refused = await asyncio.wait_for(client.call_tool("gmail_send_raw", arguments), 1)
                    self.assertEqual(refused.structured_content["error"], {
                        "kind": "server_busy", "effect": "write", "operation_invoked": False,
                        "write_outcome": "not_attempted",
                    })
                    tasks[0].cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await tasks[0]
                    refused = await asyncio.wait_for(client.call_tool("gmail_send_raw", arguments), 1)
                    self.assertEqual(refused.structured_content["error"]["kind"], "server_busy")
                    tasks[2].cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await tasks[2]
                    await asyncio.wait_for(client.list_tools(), 1)
                    await asyncio.sleep(.05)
                    tasks.append(asyncio.create_task(client.call_tool("gmail_send_raw", arguments)))
                    await asyncio.sleep(.05)
                    self.assertFalse(tasks[-1].done())
                    self.assertEqual(len(fixture.events("request")), 1)
                    self.assertEqual(fixture.events("response_closed"), [])
                    self.assertEqual(fixture.events("client_closed"), [])
                    fixture.release.touch()
                    results = await asyncio.wait_for(asyncio.gather(*tasks, return_exceptions=True), 3)
                    self.assertEqual(sum(not result.is_error for result in results if not isinstance(result, BaseException)), 3)
                    self.assertEqual(len(fixture.events("request")), 4)
                finally:
                    fixture.release.touch()
                    await asyncio.gather(*tasks, return_exceptions=True)
            events = fixture.events()
            self.assertLess(next(i for i, row in enumerate(events) if row["event"] == "response_closed"),
                            [i for i, row in enumerate(events) if row["event"] == "request"][1])
            self.assertEqual(len(fixture.events("response_closed")), 4)
            self.assertEqual(len(fixture.events("client_closed")), 1)
            self.assertEqual(events[-1]["event"], "client_closed")


if __name__ == "__main__":
    unittest.main()
