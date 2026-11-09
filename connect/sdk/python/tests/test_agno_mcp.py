"""Real MCP and upstream functions with controlled, offline HTTP responses."""

from __future__ import annotations

import asyncio
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

from agno.tools import Toolkit
import httpx
from mcp import Client, StdioServerParameters

from meerkat_toolkit.adapters.agno import AgnoToolkitAdapter
from meerkat_toolkit.adapters.agno_mcp import create_server
from meerkat_toolkit.adapters.agno_profiles import (
    AgnoMcpProfile, ProfileConfigError, build_profile, load_configuration, validate_configuration,
)


CANARY = "private-invalid-config-must-not-appear"
MODULE = "meerkat_toolkit.adapters.agno_mcp"


def configuration(profile: str = "hackernews") -> dict:
    if profile == "hackernews":
        return {"profile": profile, "operations": ["get_top_hackernews_stories"],
                "config": {"timeout": 2}}
    return {"profile": profile, "operations": ["search_pubmed"],
            "config": {"timeout": 2, "email": "fixture@example.invalid"}}


def custom_profile(function, *, effect="read", **kwargs) -> AgnoMcpProfile:
    adapter = AgnoToolkitAdapter(Toolkit(tools=[function]), functions={function.__name__: effect},
                                 **kwargs)
    return AgnoMcpProfile("fixture", adapter, tuple(adapter.describe()), thread_handoff_safe=True)


async def eventually(predicate, timeout=3.0) -> None:
    async with asyncio.timeout(timeout):
        while not predicate():
            await asyncio.sleep(0.01)


class ProfileConfigurationTests(unittest.TestCase):
    def test_strict_bounded_configuration_and_redacted_errors(self) -> None:
        valid = json.dumps(configuration())
        malformed = [
            valid.replace('"timeout": 2', '"timeout": 2, "timeout": 3'),
            valid.replace('"timeout": 2', '"timeout": NaN'),
            valid.replace('"timeout": 2', '"timeout": 1e999'),
            valid.replace('"timeout": 2', '"timeout": true'),
            valid.replace('"timeout": 2', '"timeout": 0'),
            valid.replace('"timeout": 2', '"timeout": 61'),
            valid.replace('"timeout": 2', '"timeout": 2, "secret": "' + CANARY + '"'),
            valid.replace('"profile": "hackernews"', '"profile": "' + CANARY + '"'),
            valid.replace('"get_top_hackernews_stories"', '"' + CANARY + '"'),
            '{"profile":"hackernews"}', '[]', CANARY, " " * 65_537,
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile.json"
            path.write_text(valid)
            selected = load_configuration(path)
            self.assertEqual(selected.name, "hackernews")
            self.assertEqual(selected.config, {"timeout": 2})
            for value in malformed:
                with self.subTest(value=value[:70]):
                    path.write_text(value)
                    with self.assertRaises(ProfileConfigError) as raised:
                        load_configuration(path)
                    error = raised.exception
                    self.assertNotIn(CANARY, "".join(traceback.format_exception(error)))
                    self.assertIsNone(error.__context__)

    def test_selection_is_explicit_and_pubmed_defaults_are_bounded(self) -> None:
        profile = validate_configuration("pubmed", operations=["search_pubmed"], config={
            "timeout": 5, "email": "fixture@example.invalid",
        })
        self.assertEqual(profile.config, {"timeout": 5, "email": "fixture@example.invalid",
                                         "max_results": 10, "results_expanded": True})
        for operations, config in (
            ([], {"timeout": 2}),
            (["search_pubmed", "search_pubmed"], {"timeout": 2}),
            (["search_pubmed"], {"timeout": 2}),
            (["search_pubmed"], {"timeout": 2, "email": "bad"}),
            (["search_pubmed"], {"timeout": 2, "email": "a@b", "max_results": 51}),
            (["search_pubmed"], {"timeout": 2, "email": "a@b", "results_expanded": 1}),
        ):
            with self.subTest(operations=operations, config=config):
                with self.assertRaises(ProfileConfigError):
                    validate_configuration("pubmed", operations=operations, config=config)

    def test_cli_describe_is_inert_and_has_one_json_document(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile.json"
            path.write_text(json.dumps(configuration()))
            # No network path may be entered even while constructing metadata.
            script = """
from unittest.mock import patch
from meerkat_toolkit.adapters.agno_mcp import main
import sys
with patch('httpx.get', side_effect=AssertionError('network entered')):
    raise SystemExit(main(['describe', '--config', sys.argv[1]]))
"""
            result = subprocess.run([sys.executable, "-c", script, str(path)],
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            metadata = json.loads(result.stdout)
            self.assertEqual(metadata["profile"], "hackernews")
            self.assertEqual([t["name"] for t in metadata["tools"]],
                             ["get_top_hackernews_stories"])
            path.write_text(CANARY)
            result = subprocess.run([sys.executable, "-m", MODULE, "describe", "--config", str(path)],
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertEqual(result.stdout, "")
            self.assertNotIn(CANARY, result.stderr)


class McpProfileTests(unittest.IsolatedAsyncioTestCase):
    async def test_real_hackernews_schema_selection_result_and_input_bounds(self) -> None:
        profile = build_profile("hackernews", operations=[
            "get_top_hackernews_stories", "get_user_details",
        ], config={"timeout": 7})
        requests = []

        def get(url: str, *, timeout: int) -> httpx.Response:
            self.assertEqual(timeout, 7)
            requests.append(url)
            fixtures = {
                "https://hacker-news.firebaseio.com/v0/topstories.json": [1],
                "https://hacker-news.firebaseio.com/v0/item/1.json": {
                    "id": 1, "by": "reader", "title": "Fixture", "deleted": False,
                },
                "https://hacker-news.firebaseio.com/v0/user/reader.json": {
                    "id": "reader", "karma": 2, "about": None, "submitted": [],
                },
            }
            return httpx.Response(200, json=fixtures[url], request=httpx.Request("GET", url))

        with patch("httpx.get", side_effect=get):
            async with Client(create_server(profile)) as client:
                listing = await client.list_tools()
                self.assertEqual(len(listing.tools), 2)
                self.assertTrue(all(t.annotations.read_only_hint for t in listing.tools))
                schema = listing.tools[0].input_schema
                self.assertEqual(schema["properties"]["num_stories"]["maximum"], 50)
                for name, arguments in (
                    ("get_top_hackernews_stories", {"num_stories": 0}),
                    ("get_top_hackernews_stories", {"num_stories": 51}),
                    ("get_user_details", {"username": "../private"}),
                    ("get_user_details", {"username": "r" * 65}),
                ):
                    result = await client.call_tool(name, arguments)
                    self.assertTrue(result.is_error)
                    self.assertFalse(result.structured_content["error"]["entered"])
                self.assertEqual(requests, [])
                result = await client.call_tool("get_top_hackernews_stories", {"num_stories": 1.0})
                self.assertFalse(result.is_error)
                value = result.structured_content["value"]
                self.assertEqual(result.content[0].text, value)
                self.assertEqual(json.loads(value)[0]["deleted"], False)
                user = await client.call_tool("get_user_details", {"username": "reader"})
                self.assertIsNone(json.loads(user.structured_content["value"])["about"])
        self.assertEqual(len(requests), 3)

    async def test_real_pubmed_config_and_queries_use_actual_parser(self) -> None:
        profile = build_profile("pubmed", operations=["search_pubmed"], config={
            "timeout": 5, "email": "fixture@example.invalid", "max_results": 3,
        })
        requests = []

        def get(url: str, *, params: dict, timeout: int) -> httpx.Response:
            self.assertEqual(timeout, 5)
            requests.append((url, params))
            fixtures = {
                "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi":
                    b"<eSearchResult><IdList><Id>123</Id></IdList></eSearchResult>",
                "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi":
                    b"<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID>123</PMID>"
                    b"<Article><ArticleTitle>Fixture result</ArticleTitle></Article>"
                    b"</MedlineCitation></PubmedArticle></PubmedArticleSet>",
            }
            return httpx.Response(200, content=fixtures[url], request=httpx.Request("GET", url))

        with patch("httpx.get", side_effect=get):
            async with Client(create_server(profile)) as client:
                for args in ({"query": ""}, {"query": "q" * 4097},
                             {"query": "q", "max_results": 51},
                             {"query": "q", "max_results": None}):
                    self.assertTrue((await client.call_tool("search_pubmed", args)).is_error)
                self.assertEqual(requests, [])
                result = await client.call_tool("search_pubmed", {"query": "fixture"})
                self.assertFalse(result.is_error)
                self.assertIn("Title: Fixture result", result.structured_content["value"])
        self.assertEqual(requests[0][1]["email"], "fixture@example.invalid")
        self.assertEqual(requests[0][1]["retmax"], 3)
        self.assertEqual(len(requests), 2)

    async def test_known_upstream_failure_strings_become_safe_mcp_errors(self) -> None:
        for name, operations, config, arguments in (
            ("pubmed", ["search_pubmed"], {"timeout": 2, "email": "fixture@example.invalid"},
             [{"query": "fixture"}]),
            ("hackernews", ["get_top_hackernews_stories", "get_user_details"], {"timeout": 2},
             [{"num_stories": 1}, {"username": "reader"}]),
        ):
            profile = build_profile(name, operations=operations, config=config)
            with patch("httpx.get", side_effect=httpx.ReadTimeout(CANARY)):
                async with Client(create_server(profile)) as client:
                    for operation, args in zip(operations, arguments, strict=True):
                        result = await client.call_tool(operation, args)
                        self.assertTrue(result.is_error)
                        self.assertEqual(result.structured_content["error"]["kind"],
                                         "upstream_operation_error")
                        self.assertNotIn(CANARY, result.model_dump_json())

    async def test_error_boundaries_preserve_write_uncertainty(self) -> None:
        entered = []

        def write(amount: int) -> dict:
            entered.append(amount)
            raise ValueError(CANARY)

        async with Client(create_server(custom_profile(write, effect="write"))) as client:
            invalid = await client.call_tool("write", {"amount": "bad"})
            self.assertEqual(invalid.structured_content["error"]["write_outcome"], "not_started")
            result = await client.call_tool("write", {"amount": 1})
            self.assertTrue(result.is_error)
            self.assertEqual(result.structured_content["error"]["write_outcome"], "unknown")
            self.assertNotIn(CANARY, result.model_dump_json())
        self.assertEqual(entered, [1])

    async def test_result_bound_applies_without_truncation_or_replay(self) -> None:
        entered = []

        def write() -> str:
            entered.append(True)
            return "x" * 30

        profile = custom_profile(write, effect="write", max_result_bytes=20)
        async with Client(create_server(profile)) as client:
            result = await client.call_tool("write", {})
        self.assertTrue(result.is_error)
        self.assertEqual(result.structured_content["error"]["write_outcome"], "unknown")
        self.assertEqual(entered, [True])

    async def test_unsafe_thread_handoff_is_refused(self) -> None:
        def read() -> str:
            return "value"

        safe = custom_profile(read)
        with self.assertRaises(ValueError):
            create_server(AgnoMcpProfile(safe.name, safe.adapter, safe.descriptions))

    async def test_cancellation_joins_active_worker_and_canceled_queue_never_enters(self) -> None:
        entered = []
        release = threading.Event()

        def read(label: str) -> str:
            entered.append(label)
            if label == "first" and not release.wait(5):
                raise RuntimeError("fixture timed out")
            return label

        async with Client(create_server(custom_profile(read))) as client:
            active = asyncio.create_task(client.call_tool("read", {"label": "first"}))
            queued = None
            following = None
            try:
                await eventually(lambda: entered == ["first"])
                queued = asyncio.create_task(client.call_tool("read", {"label": "canceled"}))
                await asyncio.sleep(0.03)
                queued.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await queued
                active.cancel()
                await asyncio.sleep(0.03)
                active.cancel()
                following = asyncio.create_task(client.call_tool("read", {"label": "second"}))
                await asyncio.sleep(0.03)
                self.assertFalse(active.done())
                self.assertFalse(following.done())
                self.assertEqual(entered, ["first"])
                await asyncio.wait_for(client.list_tools(), 1)
                release.set()
                with self.assertRaises(asyncio.CancelledError):
                    await active
                result = await asyncio.wait_for(following, 2)
                self.assertEqual(result.structured_content["value"], "second")
                self.assertEqual(entered, ["first", "second"])
            finally:
                release.set()
                await asyncio.gather(*[t for t in (active, queued, following) if t],
                                     return_exceptions=True)


# This subprocess-only fixture patches the actual HTTP protocol boundary. No
# production module installs a global patch or accepts arbitrary import paths.
STDIO_FIXTURE = r"""
import sys, time
from pathlib import Path
from unittest.mock import patch
import httpx
from meerkat_toolkit.adapters.agno_mcp import main
config, marker, release = map(Path, sys.argv[1:])
def get(url, *, timeout):
    assert url == 'https://hacker-news.firebaseio.com/v0/topstories.json'
    with marker.open('a') as output:
        output.write('entered\n')
    print('fixture upstream stdout is redirected', flush=True)
    until = time.monotonic() + 5
    while not release.exists():
        if time.monotonic() > until:
            raise RuntimeError('fixture timeout')
        time.sleep(0.01)
    return httpx.Response(200, json=[], request=httpx.Request('GET', url))
with patch('httpx.get', side_effect=get):
    raise SystemExit(main(['serve', '--config', str(config)]))
"""


class StdioTests(unittest.IsolatedAsyncioTestCase):
    async def test_real_stdio_ping_and_cancel_preserve_serialization(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            config, marker, release = [base / name for name in ("profile.json", "marker", "release")]
            config.write_text(json.dumps(configuration()))
            params = StdioServerParameters(command=sys.executable, args=[
                "-c", STDIO_FIXTURE, str(config), str(marker), str(release),
            ], env={"PYTHONPATH": os.environ.get("PYTHONPATH", "")})
            async with Client(params, mode="legacy") as client:
                listing = await client.list_tools()
                self.assertEqual([t.name for t in listing.tools], ["get_top_hackernews_stories"])
                active = asyncio.create_task(client.call_tool("get_top_hackernews_stories", {}))
                following = None
                try:
                    await eventually(marker.exists)
                    await asyncio.wait_for(client.send_ping(), 1)
                    await asyncio.wait_for(client.list_tools(), 1)
                    active.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await active
                    following = asyncio.create_task(client.call_tool(
                        "get_top_hackernews_stories", {"num_stories": 1},
                    ))
                    await asyncio.sleep(0.1)
                    self.assertFalse(following.done())
                    self.assertEqual(marker.read_text().splitlines(), ["entered"])
                    release.touch()
                    result = await asyncio.wait_for(following, 2)
                    self.assertEqual(result.structured_content["value"], "[]")
                    self.assertEqual(marker.read_text().splitlines(), ["entered", "entered"])
                finally:
                    release.touch()
                    await asyncio.gather(*[t for t in (active, following) if t],
                                         return_exceptions=True)

    async def test_legacy_2024_11_05_raw_stdio_initialize_and_list(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "profile.json"
            path.write_text(json.dumps(configuration()))
            process = await asyncio.create_subprocess_exec(
                sys.executable, "-m", MODULE, "serve", "--config", str(path),
                stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )

            async def send(value: dict) -> None:
                process.stdin.write(json.dumps(value).encode() + b"\n")
                await process.stdin.drain()

            async def response(request_id: int) -> dict:
                async with asyncio.timeout(5):
                    while True:
                        line = await process.stdout.readline()
                        self.assertTrue(line, "stdio server closed unexpectedly")
                        item = json.loads(line)
                        if item.get("id") == request_id:
                            return item

            try:
                await send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": {"name": "toolkit-wire-fixture", "version": "0"},
                }})
                initialized = await response(1)
                self.assertNotIn("error", initialized)
                self.assertEqual(initialized["result"]["protocolVersion"], "2024-11-05")
                await send({"jsonrpc": "2.0", "method": "notifications/initialized"})
                await send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
                listing = await response(2)
                self.assertEqual([t["name"] for t in listing["result"]["tools"]],
                                 ["get_top_hackernews_stories"])
                await send({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                    "name": "get_top_hackernews_stories", "arguments": {"num_stories": 0},
                }})
                rejected = await response(3)
                self.assertTrue(rejected["result"]["isError"])
            finally:
                process.stdin.close()
                try:
                    await asyncio.wait_for(process.wait(), 3)
                except TimeoutError:
                    process.kill()
                    await process.wait()
                diagnostics = (await process.stderr.read()).decode()
                self.assertEqual(process.returncode, 0, diagnostics)


if __name__ == "__main__":
    unittest.main()
