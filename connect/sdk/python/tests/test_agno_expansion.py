"""Actual selected donor methods over offline HTTP protocol boundaries."""

from __future__ import annotations

import asyncio
from contextlib import ExitStack
import inspect
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import traceback
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

from agno.tools.financial_datasets import FinancialDatasetsTools
import httpx
from mcp import Client, StdioServerParameters
import requests

from meerkat_toolkit.adapters.agno_mcp import _joined_worker, create_server
from meerkat_toolkit.adapters.agno_profiles import (
    ProfileConfigError, build_profile, validate_configuration,
)
from meerkat_toolkit.adapters.agno_transports import BorrowedSessionFinancialDatasetsTools


KEY = "fixture-selected-key-never-disclose"
CANARY = "fixture-ambient-account-never-select"
REFERENCE = "TOOLKIT_TEST_SELECTED_KEY"
HOSTILE = {
    REFERENCE: KEY, "TAVILY_API_KEY": CANARY, "FINANCIAL_DATASETS_API_KEY": CANARY,
    "TAVILY_PROJECT": CANARY, "TAVILY_ORG_ID": CANARY,
    "TAVILY_API_BASE_URL": "https://ambient.invalid",
    "TAVILY_HTTP_PROXY": "http://ambient.invalid", "TAVILY_HTTPS_PROXY": "http://ambient.invalid",
    "HTTP_PROXY": "http://ambient.invalid", "HTTPS_PROXY": "http://ambient.invalid",
    "REQUESTS_CA_BUNDLE": CANARY,
}
OPERATIONS = {
    "searxng": ["search_web", "news_search"],
    "tavily": ["web_search_using_tavily"],
    "financial-datasets": ["get_income_statements", "get_balance_sheets"],
}


def config(name: str) -> dict:
    return {"timeout": 7, **({"host": "https://search.example.invalid/"} if name == "searxng"
                            else {"api_key_env": REFERENCE})}


def response(request, *, status=200, value=None, headers=None) -> requests.Response:
    result = requests.Response()
    result.status_code = status
    result.url = request.url
    result.request = request
    result._content = json.dumps(value).encode()
    result.headers.update(headers or {"Content-Type": "application/json"})
    return result


async def eventually(predicate) -> None:
    async with asyncio.timeout(3):
        while not predicate():
            await asyncio.sleep(0.01)


class OfflineMixin:
    def setUp(self) -> None:
        super().setUp()
        self.guards = ExitStack()
        self.addCleanup(self.guards.close)
        for target in ("socket.socket.connect", "socket.socket.connect_ex", "socket.getaddrinfo"):
            self.guards.enter_context(patch(target, side_effect=AssertionError("network forbidden")))


class ConfigurationTests(OfflineMixin, unittest.TestCase):
    def test_explicit_reference_and_origin_validation_is_inert_and_bounded(self) -> None:
        for name in OPERATIONS:
            selected = validate_configuration(name, operations=OPERATIONS[name], config=config(name))
            self.assertNotIn(KEY, repr(selected))
        for name, setting in (
            ("tavily", {"timeout": 7, "api_key": KEY}),
            ("tavily", {"timeout": 7, "api_key_env": ""}),
            ("tavily", {"timeout": 7, "api_key_env": "a=b"}),
            ("financial-datasets", {"timeout": 7, "api_key_env": "x" * 129}),
            *( ("searxng", {"timeout": 7, "host": host}) for host in (
                "http://search.invalid", "https://user:password@search.invalid",
                "https://search.invalid/path", "https://search.invalid/?secret=value",
                "https://search.invalid/#fragment", "https://search.invalid:99999",
                "https://search.invalid\\path", "https://search.invalid\n", "https://",
            )),
        ):
            with self.subTest(name=name, setting=setting):
                with self.assertRaises(ProfileConfigError) as caught:
                    validate_configuration(name, operations=OPERATIONS[name], config=setting)
                self.assertNotIn(KEY, str(caught.exception))
                self.assertIsNone(caught.exception.__context__)
        for name in ("tavily", "financial-datasets"):
            for value in (None, "", KEY + "\n", "x" * 4097):
                with self.subTest(name=name, value=value), patch.dict(os.environ, HOSTILE, clear=True):
                    if value is None:
                        os.environ.pop(REFERENCE)
                    else:
                        os.environ[REFERENCE] = value
                    with self.assertRaises(ProfileConfigError) as caught:
                        build_profile(name, operations=OPERATIONS[name], config=config(name))
                    self.assertNotIn(KEY, "".join(traceback.format_exception(caught.exception)))
                    self.assertIsNone(caught.exception.__context__)

    def test_inherited_operation_bodies_and_borrowed_session_ownership(self) -> None:
        for name in OPERATIONS["financial-datasets"]:
            self.assertIs(getattr(BorrowedSessionFinancialDatasetsTools, name),
                          getattr(FinancialDatasetsTools, name))
        session = requests.Session()
        self.addCleanup(session.close)
        with patch.object(session, "close") as close:
            donor = BorrowedSessionFinancialDatasetsTools(KEY, 7, session=session)
            self.assertIs(donor._session, session)
            close.assert_not_called()

    def test_partial_construction_and_adapter_rejection_close_owned_clients(self) -> None:
        from agno.tools.tavily import TavilyTools

        closed = []
        original_close = requests.Session.close

        def close(session):
            closed.append(session)
            original_close(session)

        original_init = TavilyTools.__init__

        def partial_init(toolkit, *args, **kwargs):
            original_init(toolkit, *args, **kwargs)
            raise ValueError(KEY)

        with patch.dict(os.environ, HOSTILE, clear=True), patch.object(requests.Session, "close", close):
            with patch.object(TavilyTools, "__init__", partial_init):
                with self.assertRaises(ProfileConfigError) as caught:
                    build_profile("tavily", operations=OPERATIONS["tavily"], config=config("tavily"))
                self.assertEqual(len(closed), 1)
                self.assertNotIn(KEY, "".join(traceback.format_exception(caught.exception)))
            for name in ("tavily", "financial-datasets"):
                before = len(closed)
                with patch("meerkat_toolkit.adapters.agno.AgnoToolkitAdapter", side_effect=ValueError(KEY)):
                    with self.assertRaises(ProfileConfigError):
                        build_profile(name, operations=OPERATIONS[name], config=config(name))
                self.assertEqual(len(closed), before + 1)
            for name in ("tavily", "financial-datasets"):
                before = len(closed)
                profile = build_profile(name, operations=OPERATIONS[name], config=config(name))
                profile.close()
                profile.close()
                self.assertEqual(len(closed), before + 1)


class ExpansionProtocolTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_keys_bind_to_each_profile_and_do_not_follow_later_environment(self) -> None:
        seen = []

        def send(adapter, request, **kwargs):
            seen.append(request.headers["Authorization"])
            return response(request, value={"results": []})

        profiles = []
        try:
            with patch.dict(os.environ, HOSTILE, clear=True), \
                    patch.object(requests.adapters.HTTPAdapter, "send", send):
                for key in ("first-explicit-fixture-key", "second-explicit-fixture-key"):
                    os.environ[REFERENCE] = key
                    profiles.append(build_profile("tavily", operations=OPERATIONS["tavily"],
                                                  config=config("tavily")))
                os.environ[REFERENCE] = CANARY
                for profile in profiles:
                    async with Client(create_server(profile)) as client:
                        result = await client.call_tool("web_search_using_tavily", {"query": "fixture"})
                        self.assertFalse(result.is_error)
            self.assertEqual(seen, ["Bearer first-explicit-fixture-key", "Bearer second-explicit-fixture-key"])
        finally:
            for profile in profiles:
                profile.close()

    async def test_actual_searxng_methods_and_input_limits(self) -> None:
        calls = []

        def get(url, *, timeout):
            self.assertEqual(timeout, 7)
            calls.append(url)
            return httpx.Response(200, json={"results": [{"title": "one"}, {"title": "two"}],
                                              "unresponsive_engines": []},
                                  request=httpx.Request("GET", url))

        profile = build_profile("searxng", operations=OPERATIONS["searxng"], config=config("searxng"))
        self.addCleanup(profile.close)
        with patch("httpx.get", side_effect=get):
            async with Client(create_server(profile)) as client:
                for op in OPERATIONS["searxng"]:
                    for arguments in ({"query": ""}, {"query": "q", "max_results": 6},
                                      {"query": "q" * 4097}):
                        rejected = await client.call_tool(op, arguments)
                        self.assertTrue(rejected.is_error)
                        self.assertFalse(rejected.structured_content["error"]["entered"])
                    result = await client.call_tool(op, {"query": "bounded & query", "max_results": 1})
                    self.assertFalse(result.is_error)
                    self.assertEqual(len(json.loads(result.structured_content["value"])["results"]), 1)
        self.assertEqual(len(calls), 2)
        self.assertEqual(parse_qs(urlsplit(calls[0]).query)["q"], ["bounded & query"])
        self.assertEqual(parse_qs(urlsplit(calls[1]).query)["categories"], ["news"])

    async def test_actual_tavily_sdk_excludes_ambient_state_and_preserves_result(self) -> None:
        calls = []

        def send(adapter, request, **kwargs):
            calls.append(request)
            self.assertEqual(adapter.max_retries.total, 0)
            self.assertEqual(request.url, "https://api.tavily.com/search")
            self.assertEqual(request.headers["Authorization"], f"Bearer {KEY}")
            self.assertNotIn("X-Project-ID", request.headers)
            self.assertNotIn("X-Tavily-Orgid", request.headers)
            self.assertEqual(kwargs["proxies"], {})
            self.assertIs(kwargs["verify"], True)
            self.assertEqual(kwargs["timeout"], 7)
            payload = json.loads(request.body)
            self.assertEqual(payload["search_depth"], "basic")
            self.assertIs(payload["include_answer"], False)
            self.assertEqual(payload["max_results"], 1)
            return response(request, value={"results": [
                {"title": "Fixture", "url": "https://example.invalid/result", "content": "text", "score": 0},
            ]})

        with patch.dict(os.environ, HOSTILE, clear=True), \
                patch("requests.sessions.get_netrc_auth", side_effect=AssertionError("netrc forbidden")), \
                patch.object(requests.adapters.HTTPAdapter, "send", send):
            profile = build_profile("tavily", operations=OPERATIONS["tavily"], config=config("tavily"))
            try:
                async with Client(create_server(profile)) as client:
                    listing = await client.list_tools()
                    self.assertNotIn(KEY, listing.model_dump_json())
                    self.assertNotIn(CANARY, listing.model_dump_json())
                    invalid = await client.call_tool("web_search_using_tavily", {"query": "q", "max_results": 6})
                    self.assertTrue(invalid.is_error)
                    result = await client.call_tool("web_search_using_tavily", {"query": "fixture", "max_results": 1})
                    self.assertFalse(result.is_error)
                    self.assertEqual(json.loads(result.structured_content["value"])["results"][0]["score"], 0)
            finally:
                profile.close()
        self.assertEqual(len(calls), 1)

    async def test_actual_financial_methods_prepare_selected_key_and_endpoint(self) -> None:
        calls = []

        def send(adapter, request, **kwargs):
            calls.append(request)
            self.assertEqual(adapter.max_retries.total, 0)
            self.assertEqual(request.headers["X-API-KEY"], KEY)
            self.assertNotIn("Authorization", request.headers)
            self.assertEqual(kwargs["proxies"], {})
            self.assertIs(kwargs["verify"], True)
            self.assertEqual(kwargs["timeout"], 7)
            self.assertEqual(parse_qs(urlsplit(request.url).query),
                             {"ticker": ["FIXT"], "period": ["quarterly"], "limit": ["1"]})
            return response(request, value={"rows": [{"amount": 0, "audited": False}],
                                            "next_page_url": "https://api.financialdatasets.ai/page/2"})

        with patch.dict(os.environ, HOSTILE, clear=True), \
                patch("requests.sessions.get_netrc_auth", side_effect=AssertionError("netrc forbidden")), \
                patch.object(requests.adapters.HTTPAdapter, "send", send):
            profile = build_profile("financial-datasets", operations=OPERATIONS["financial-datasets"],
                                    config=config("financial-datasets"))
            try:
                for name, binding in profile.adapter._bindings.items():
                    self.assertIs(inspect.unwrap(binding.entrypoint).__func__,
                                  getattr(FinancialDatasetsTools, name))
                async with Client(create_server(profile)) as client:
                    for op in OPERATIONS["financial-datasets"]:
                        for arguments in ({"ticker": ""}, {"ticker": "FIXT", "limit": 11},
                                          {"ticker": "FIXT", "period": "all"}, {"ticker": "x" * 33}):
                            self.assertTrue((await client.call_tool(op, arguments)).is_error)
                        result = await client.call_tool(op, {"ticker": "FIXT", "period": "quarterly", "limit": 1})
                        self.assertFalse(result.is_error)
                        self.assertIs(json.loads(result.structured_content["value"])["rows"][0]["audited"], False)
            finally:
                profile.close()
        self.assertEqual([urlsplit(item.url).path for item in calls],
                         ["/financials/income-statements", "/financials/balance-sheets"])

    async def test_failure_and_redirects_are_redacted_without_second_request(self) -> None:
        for name in ("tavily", "financial-datasets"):
            for status in (401, 429, 500, 302):
                calls = []

                def send(adapter, request, **kwargs):
                    calls.append(request)
                    return response(request, status=status, value={"detail": {"error": KEY + CANARY}},
                                    headers={"Location": "https://wrong-account.invalid/"} if status == 302 else None)

                with self.subTest(name=name, status=status), patch.dict(os.environ, HOSTILE, clear=True), \
                        patch.object(requests.adapters.HTTPAdapter, "send", send):
                    profile = build_profile(name, operations=OPERATIONS[name][:1], config=config(name))
                    try:
                        async with Client(create_server(profile)) as client:
                            args = {"query": "fixture"} if name == "tavily" else {"ticker": "FIXT"}
                            result = await client.call_tool(OPERATIONS[name][0], args)
                            self.assertTrue(result.is_error)
                            self.assertNotIn(KEY, result.model_dump_json())
                            self.assertNotIn(CANARY, result.model_dump_json())
                    finally:
                        profile.close()
                    self.assertEqual(len(calls), 1)
        profile = build_profile("searxng", operations=["search_web"], config=config("searxng"))
        with patch("httpx.get", side_effect=httpx.ReadTimeout(CANARY)):
            async with Client(create_server(profile)) as client:
                result = await client.call_tool("search_web", {"query": "fixture"})
                self.assertTrue(result.is_error)
                self.assertNotIn(CANARY, result.model_dump_json())
        profile.close()

    async def test_cancelled_worker_finishes_before_client_close_and_cleanup_joins(self) -> None:
        entered, release, closing, close_release = [threading.Event() for _ in range(4)]
        events = []
        original_close = requests.Session.close

        def send(adapter, request, **kwargs):
            events.append("entered")
            entered.set()
            if not release.wait(5):
                raise RuntimeError("fixture timeout")
            events.append("returned")
            return response(request, value={"rows": []})

        def close(session):
            events.append("closing")
            closing.set()
            if not close_release.wait(5):
                raise RuntimeError("fixture timeout")
            original_close(session)
            events.append("closed")

        with patch.dict(os.environ, HOSTILE, clear=True), \
                patch.object(requests.adapters.HTTPAdapter, "send", send), \
                patch.object(requests.Session, "close", close):
            profile = build_profile("financial-datasets", operations=["get_income_statements"],
                                    config=config("financial-datasets"))
            active = cleanup = None
            try:
                async with Client(create_server(profile)) as client:
                    active = asyncio.create_task(client.call_tool("get_income_statements", {"ticker": "FIXT"}))
                    await eventually(entered.is_set)
                    active.cancel()
                    await asyncio.sleep(0.03)
                    active.cancel()
                    self.assertFalse(active.done())
                    self.assertFalse(closing.is_set())
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await active
                cleanup = asyncio.create_task(_joined_worker(profile.close))
                await eventually(closing.is_set)
                cleanup.cancel()
                await asyncio.sleep(0.03)
                cleanup.cancel()
                self.assertFalse(cleanup.done())
                close_release.set()
                with self.assertRaises(asyncio.CancelledError):
                    await cleanup
                self.assertEqual(events, ["entered", "returned", "closing", "closed"])
            finally:
                release.set()
                close_release.set()
                await asyncio.gather(*[task for task in (active, cleanup) if task], return_exceptions=True)
                profile.close()


# The test subprocess patches only HTTP boundaries, never a production factory.
STDIO = r'''
import json, socket, sys
from pathlib import Path
from unittest.mock import patch
import httpx, requests
from meerkat_toolkit.adapters.agno_mcp import main
config, marker = map(Path, sys.argv[1:])
original_close = requests.Session.close
def close(session):
    with marker.open('a') as output:
        output.write('closed\n')
    original_close(session)
def send(adapter, request, **kwargs):
    assert kwargs['proxies'] == {}
    assert kwargs['verify'] is True
    result = requests.Response()
    result.status_code = 200
    result.url = request.url
    result.request = request
    value = {'results': []} if '/search' in request.url else {'rows': []}
    result._content = json.dumps(value).encode()
    return result
def get(url, *, timeout):
    return httpx.Response(200, json={'results': []}, request=httpx.Request('GET', url))
with patch('socket.socket.connect', side_effect=AssertionError('network forbidden')), \
     patch('socket.getaddrinfo', side_effect=AssertionError('network forbidden')), \
     patch.object(requests.adapters.HTTPAdapter, 'send', send), \
     patch.object(requests.Session, 'close', close), patch('httpx.get', side_effect=get):
    raise SystemExit(main(['serve', '--config', str(config)]))
'''


STDIO_JOIN = r'''
import json, sys, time
from pathlib import Path
from unittest.mock import patch
import requests
from meerkat_toolkit.adapters.agno_mcp import main
config, marker, body_release, close_release = map(Path, sys.argv[1:])
original_close = requests.Session.close
def mark(value):
    with marker.open('a') as output:
        output.write(value + '\n')
def wait(path):
    until = time.monotonic() + 8
    while not path.exists():
        if time.monotonic() > until:
            raise RuntimeError('fixture timeout')
        time.sleep(0.01)
def send(adapter, request, **kwargs):
    mark('entered')
    wait(body_release)
    mark('returned')
    result = requests.Response()
    result.status_code = 200
    result.url = request.url
    result.request = request
    result._content = b'{"rows":[]}'
    return result
def close(session):
    mark('closing')
    wait(close_release)
    original_close(session)
    mark('closed')
with patch('socket.socket.connect', side_effect=AssertionError('network forbidden')), \
     patch('socket.getaddrinfo', side_effect=AssertionError('network forbidden')), \
     patch.object(requests.adapters.HTTPAdapter, 'send', send), \
     patch.object(requests.Session, 'close', close):
    raise SystemExit(main(['serve', '--config', str(config)]))
'''


class StdioExpansionTests(OfflineMixin, unittest.IsolatedAsyncioTestCase):
    async def test_stdio_eof_joins_active_body_then_pending_session_close(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path, marker, body_release, close_release = [root / name for name in (
                "profile.json", "events", "body-release", "close-release",
            )]
            path.write_text(json.dumps({"profile": "financial-datasets",
                                        "operations": ["get_income_statements"],
                                        "config": config("financial-datasets")}))
            process = await asyncio.create_subprocess_exec(
                sys.executable, "-c", STDIO_JOIN, str(path), str(marker), str(body_release), str(close_release),
                env={**os.environ, **HOSTILE}, stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            )

            async def send(value):
                process.stdin.write(json.dumps(value).encode() + b"\n")
                await process.stdin.drain()

            def events():
                return marker.read_text().splitlines() if marker.exists() else []

            try:
                await send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": {"name": "expansion-shutdown-fixture", "version": "0"},
                }})
                initialized = json.loads(await asyncio.wait_for(process.stdout.readline(), 5))
                self.assertNotIn("error", initialized)
                await send({"jsonrpc": "2.0", "method": "notifications/initialized"})
                await send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                    "name": "get_income_statements", "arguments": {"ticker": "FIXT"},
                }})
                await eventually(lambda: events() == ["entered"])
                process.stdin.close()
                await asyncio.sleep(0.05)
                self.assertIsNone(process.returncode)
                self.assertEqual(events(), ["entered"])
                body_release.touch()
                await eventually(lambda: "closing" in events())
                self.assertEqual(events(), ["entered", "returned", "closing"])
                self.assertIsNone(process.returncode)
                close_release.touch()
                await asyncio.wait_for(process.wait(), 3)
                self.assertEqual(events(), ["entered", "returned", "closing", "closed"])
                diagnostics = (await process.stderr.read()).decode()
                self.assertEqual(process.returncode, 0, diagnostics)
                self.assertNotIn(KEY, diagnostics)
            finally:
                body_release.touch()
                close_release.touch()
                if process.returncode is None:
                    process.stdin.close()
                    try:
                        await asyncio.wait_for(process.wait(), 3)
                    except TimeoutError:
                        process.kill()
                        await process.wait()

    async def test_new_profiles_run_via_ordinary_stdio_config_and_owned_clients_close(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in OPERATIONS:
                path, marker = root / f"{name}.json", root / f"{name}.closed"
                operation = OPERATIONS[name][0]
                path.write_text(json.dumps({"profile": name, "operations": [operation], "config": config(name)}))
                environment = {"PYTHONPATH": os.environ.get("PYTHONPATH", ""), **HOSTILE}
                params = StdioServerParameters(command=sys.executable,
                                               args=["-c", STDIO, str(path), str(marker)], env=environment)
                async with Client(params, mode="legacy") as client:
                    listing = await client.list_tools()
                    self.assertEqual([tool.name for tool in listing.tools], [operation])
                    self.assertNotIn(KEY, listing.model_dump_json())
                    arguments = {"ticker": "FIXT"} if name == "financial-datasets" else {"query": "fixture"}
                    result = await client.call_tool(operation, arguments)
                    self.assertFalse(result.is_error)
                if name == "searxng":
                    self.assertFalse(marker.exists())
                else:
                    self.assertEqual(marker.read_text().splitlines(), ["closed"])

    async def test_describe_requires_explicit_key_but_never_sends_or_discloses_it(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile.json"
            script = """
import socket,sys
from unittest.mock import patch
from meerkat_toolkit.adapters.agno_mcp import main
with patch('socket.socket.connect', side_effect=AssertionError('network forbidden')), \\
     patch('socket.getaddrinfo', side_effect=AssertionError('network forbidden')):
    raise SystemExit(main(['describe','--config',sys.argv[1]]))
"""
            for name in OPERATIONS:
                path.write_text(json.dumps({"profile": name, "operations": OPERATIONS[name], "config": config(name)}))
                result = subprocess.run([sys.executable, "-c", script, str(path)],
                                        env={**os.environ, **HOSTILE}, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["profile"], name)
                self.assertNotIn(KEY, result.stdout + result.stderr)
                self.assertNotIn(CANARY, result.stdout + result.stderr)
                if name != "searxng":
                    environment = {**os.environ, **HOSTILE}
                    environment.pop(REFERENCE, None)
                    result = subprocess.run([sys.executable, "-c", script, str(path)], env=environment,
                                            capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 1)
                    self.assertEqual(result.stdout, "")
                    self.assertNotIn(CANARY, result.stderr)


if __name__ == "__main__":
    unittest.main()
