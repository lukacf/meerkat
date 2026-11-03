"""Configured Drive reads through real MCP, with controlled HTTP only."""
from __future__ import annotations

import asyncio
import base64
from contextlib import asynccontextmanager
import importlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import httpx
from mcp import Client, StdioServerParameters

NAMESPACE = os.environ.get("TOOLKIT_SERVICES_TEST_NAMESPACE", "meerkat_toolkit")
adapter = importlib.import_module(NAMESPACE + ".adapters.services_mcp")
config_module = importlib.import_module(NAMESPACE + ".adapters.services_config")
prepare = importlib.import_module(NAMESPACE + ".httpx_client").prepare_httpx_client
SOURCE_PATH = os.environ.get("PYTHONPATH", str(Path(__file__).resolve().parents[3] / "sdk/python/src"))
PAGE = {"files": [{"id": "a/b", "name": "Report", "mimeType": "application/pdf",
    "description": "Selected report", "webViewLink": "https://drive.google.com/file/d/example/view",
    "capabilities": {"canDownload": False}, "future": [None, False]}],
    "nextPageToken": "opaque+/=", "incompleteSearch": True}


def selection(operations=None, maximum=65536):
    return {"profile": "google-drive", "operations": operations or [
        "list_files_page", "get_file", "download_content", "export_content"],
        "config": {"token_env": "DRIVE_TOKEN", "timeout_seconds": 2,
                   "max_response_bytes": 65536, "max_content_bytes": maximum}}


class Provider:
    def __init__(self):
        self.raw = json.dumps(PAGE).encode()
        self.headers = {"content-type": "application/json"}
        self.status = 200
        self.requests, self.events = [], []
        self.release = None
        self.closing = asyncio.Event()

    async def handle(self, request):
        self.requests.append(request)
        owner = self

        class Stream(httpx.AsyncByteStream):
            async def __aiter__(self):
                yield owner.raw[:3]
                yield owner.raw[3:]

            async def aclose(self):
                owner.closing.set()
                if owner.release is not None:
                    await owner.release.wait()
                owner.events.append("response_closed")

        return httpx.Response(self.status, request=request, stream=Stream(), headers=self.headers)

    def factory(self, **kwargs):
        self.settings = kwargs
        owner = self

        class OwnedClient(httpx.AsyncClient):
            async def aclose(self):
                await super().aclose()
                owner.events.append("client_closed")

        return OwnedClient(transport=httpx.MockTransport(self.handle), **kwargs)


@asynccontextmanager
async def connected(provider, **options):
    configuration = config_module.validate_configuration(selection(**options))
    with patch.dict(os.environ, {"DRIVE_TOKEN": "fixture-token"}, clear=True):
        profile = await adapter.create_profile(configuration, client_factory=provider.factory)
    try:
        async with Client(adapter.create_server(profile)) as client:
            yield client, profile
    finally:
        if provider.release is not None:
            provider.release.set()
        await profile.aclose()


class DriveProfileTests(unittest.IsolatedAsyncioTestCase):
    async def test_search_and_metadata_preserve_scope_pagination_and_fields(self):
        provider = Provider()
        async with connected(provider) as (client, _):
            tools = (await client.list_tools()).tools
            self.assertEqual([t.name for t in tools], ["google-drive_" + op for op in selection()["operations"]])
            self.assertTrue(all(t.annotations.read_only_hint and not t.annotations.destructive_hint for t in tools))
            self.assertEqual(provider.requests, [])
            args = {"query": "name contains 'Report' and trashed = false", "page_size": 2, "order_by": "name"}
            first = await client.call_tool("google-drive_list_files_page", args)
            self.assertEqual(first.structured_content, {"value": PAGE})
            provider.raw = b'{"files":[],"incompleteSearch":false}'
            second = await client.call_tool("google-drive_list_files_page", dict(args, page_token="opaque+/="))
            self.assertEqual(second.structured_content["value"], {"files": [], "incompleteSearch": False})
            provider.raw = json.dumps(dict(PAGE["files"][0], shortcutDetails={"targetId": "other"})).encode()
            detail = await client.call_tool("google-drive_get_file", {"file_id": "a/b"})
            self.assertEqual(detail.structured_content["value"]["shortcutDetails"], {"targetId": "other"})
            self.assertEqual(len(provider.requests), 3, "no traversal or shortcut following")
        for request in provider.requests:
            self.assertEqual(request.method, "GET")
            self.assertEqual(request.url.host, "www.googleapis.com")
            self.assertEqual(request.headers["authorization"], "Bearer fixture-token")
            for field in ("webViewLink", "description", "modifiedTime", "mimeType", "id", "name", "shortcutDetails", "capabilities"):
                self.assertIn(field, request.url.params["fields"])
        self.assertEqual(provider.requests[0].url.params["q"], args["query"])
        self.assertIn("nextPageToken", provider.requests[0].url.params["fields"])
        self.assertIn("incompleteSearch", provider.requests[0].url.params["fields"])
        self.assertEqual(provider.requests[1].url.params["pageToken"], "opaque+/=")
        self.assertEqual(provider.requests[2].url.raw_path.split(b"?")[0], b"/drive/v3/files/a%2Fb")
        self.assertFalse(provider.settings["trust_env"])
        self.assertFalse(provider.settings["follow_redirects"])
        self.assertEqual(provider.events, ["response_closed"] * 3 + ["client_closed"])

    async def test_content_returns_exact_text_binary_json_and_non_utf8(self):
        provider = Provider()
        async with connected(provider) as (client, _):
            for raw, mime, expected_kind in (("Hej \u2603\r\n".encode(), "text/plain; charset=utf-8", "text"),
                    (b'{"a":false}', "application/json", "text"),
                    (b"%PDF-1.7\x00\xff", "application/pdf", "blob"),
                    (b"caf\xe9", "text/plain; charset=iso-8859-1", "blob"),
                    (b"\x00" * 100, "text/plain; charset=utf-8", "blob"),
                    (b"\xffbad", "text/plain; charset=utf-8", "blob"),
                    (b"", "application/octet-stream", "blob")):
                provider.raw, provider.headers = raw, {"content-type": mime}
                result = await client.call_tool("google-drive_download_content", {"file_id": "a/b"})
                self.assertFalse(result.is_error, result)
                resource = result.content[1].resource
                self.assertEqual(result.content[1].type, "resource")
                self.assertEqual(resource.mime_type, mime)
                if expected_kind == "text":
                    self.assertEqual(resource.text.encode("utf-8"), raw)
                else:
                    self.assertEqual(base64.b64decode(resource.blob), raw)
                self.assertEqual(result.structured_content["value"]["bytes"], len(raw))
                self.assertEqual(result.structured_content["value"]["representation"], expected_kind)
                self.assertNotIn("output_path", result.model_dump_json())
            self.assertEqual(len(provider.requests), 7)
            self.assertTrue(all(dict(r.url.params) == {"alt": "media"} for r in provider.requests))

    async def test_export_exact_mime_and_no_automatic_fallback(self):
        provider = Provider()
        provider.raw, provider.headers = b"<h1>Title</h1>", {"content-type": "text/html; charset=UTF-8"}
        async with connected(provider) as (client, _):
            result = await client.call_tool("google-drive_export_content", {"file_id": "document", "mime_type": "text/html"})
            self.assertEqual(result.content[1].resource.text, "<h1>Title</h1>")
            self.assertEqual(dict(provider.requests[-1].url.params), {"mimeType": "text/html"})
            self.assertEqual(provider.requests[-1].url.path, "/drive/v3/files/document/export")
            provider.status, provider.raw = 403, b"private provider prose"
            failed = await client.call_tool("google-drive_export_content", {"file_id": "document", "mime_type": "text/plain"})
            self.assertEqual(failed.structured_content["error"]["kind"], "permission")
            self.assertEqual(len(provider.requests), 2)
            self.assertNotIn("private provider prose", failed.model_dump_json())

    async def test_bounds_length_encoding_and_http_failures_return_no_partial_content(self):
        provider = Provider()
        cases = [(200, b"12345", {}, "response_too_large"),
                 (200, b"12345", {"content-length": "5"}, "response_too_large"),
                 (200, b"12", {"content-length": "3"}, "invalid_response"),
                 (200, b"12", {"content-length": "1"}, "invalid_response"),
                 (200, b"12", {"content-encoding": "gzip"}, "unsupported_encoding"),
                 (302, b"", {"location": "https://other.invalid/"}, "redirect"),
                 (401, b"secret", {}, "authentication"),
                 (429, b"secret", {"retry-after": "3"}, "rate_limited")]
        async with connected(provider, maximum=4) as (client, _):
            for status, raw, headers, kind in cases:
                provider.status, provider.raw, provider.headers = status, raw, headers
                before = len(provider.requests)
                result = await client.call_tool("google-drive_download_content", {"file_id": "selected"})
                self.assertTrue(result.is_error, result)
                self.assertEqual(result.structured_content["error"]["kind"], kind)
                self.assertEqual(result.structured_content["error"]["write_outcome"], "not_applicable")
                self.assertEqual(len(result.content), 1)
                self.assertEqual(len(provider.requests), before + 1)
                self.assertNotIn("secret", result.model_dump_json())
            provider.status, provider.raw, provider.headers = 200, b"1234", {}
            self.assertFalse((await client.call_tool("google-drive_download_content", {"file_id": "selected"})).is_error)

    async def test_selection_and_arguments_cannot_select_paths_accounts_writes_or_masks(self):
        provider = Provider()
        async with connected(provider, operations=["download_content"]) as (client, _):
            for field in ("output_path", "base_url", "token_env", "fields", "supports_all_drives"):
                result = await client.call_tool("google-drive_download_content", {"file_id": "selected", field: "other"})
                self.assertTrue(result.is_error)
            for name in ("get_file", "create_file", "update_file", "delete_file", "create_permission"):
                self.assertTrue((await client.call_tool("google-drive_" + name, {"file_id": "selected"})).is_error)
            for file_id in ("..", ".", "bad\n", ""):
                self.assertTrue((await client.call_tool("google-drive_download_content", {"file_id": file_id})).is_error)
            self.assertEqual(provider.requests, [])
        for maximum in (0, True, 16777217):
            with self.assertRaises(config_module.ConfigurationError):
                config_module.validate_configuration(selection(maximum=maximum))
        self.assertIn("drive.readonly", json.dumps(config_module.configuration_help("google-drive")))

    async def test_borrowed_client_and_cancelled_close_retain_custody(self):
        provider = Provider()
        provider.raw = b"abcdef"
        provider.headers = {"content-type": "application/octet-stream"}
        provider.release = asyncio.Event()
        client = httpx.AsyncClient(base_url="https://www.googleapis.com/", headers={"Authorization": "Bearer host"},
                                   transport=httpx.MockTransport(provider.handle))
        prepare(client)
        with patch.dict(os.environ, {}, clear=True):
            profile = adapter.create_profile_from_client(config_module.validate_configuration(selection()), client)
        try:
            task = asyncio.create_task(profile.operations.download_content("file"))
            await asyncio.wait_for(provider.closing.wait(), 2)
            task.cancel()
            await asyncio.sleep(0)
            self.assertFalse(task.done())
            provider.release.set()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
            self.assertEqual(provider.events, ["response_closed"])
            await profile.aclose()
            self.assertFalse(client.is_closed)
            self.assertEqual(provider.requests[0].headers["authorization"], "Bearer host")
        finally:
            provider.release.set()
            await client.aclose()

    async def test_actual_stdio_binary_resource(self):
        script = '''import asyncio,importlib,sys,httpx
from mcp.server.stdio import stdio_server
a=importlib.import_module(sys.argv[1]); c=a.load_configuration(sys.argv[2])
async def main():
 p=await a.create_profile(c,client_factory=lambda **kw:httpx.AsyncClient(transport=httpx.MockTransport(lambda r:httpx.Response(200,content=(b"%PDF-1.7\\x00\\xff" if r.url.path.endswith("selected") else b"\\xff"*16777216),headers={"content-type":"application/pdf"})),**kw))
 try:
  async with stdio_server() as (read,write):
   s=a.create_server(p); await s.run(read,write,s.create_initialization_options())
 finally: await p.aclose()
asyncio.run(main())
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "host.py").write_text(script)
            (path / "config.json").write_text(json.dumps(selection(["download_content"], maximum=16777216)))
            params = StdioServerParameters(command=sys.executable,
                args=["-B", str(path / "host.py"), NAMESPACE + ".adapters.services_mcp", str(path / "config.json")],
                env={"PATH": "/usr/bin:/bin", "PYTHONPATH": SOURCE_PATH, "DRIVE_TOKEN": "fixture-token"})
            async with Client(params) as client:
                result = await client.call_tool("google-drive_download_content", {"file_id": "selected"})
                self.assertFalse(result.is_error, result)
                self.assertEqual(base64.b64decode(result.content[1].resource.blob), b"%PDF-1.7\x00\xff")
                maximum = await client.call_tool("google-drive_download_content", {"file_id": "maximum"})
                self.assertFalse(maximum.is_error, maximum)
                self.assertEqual(base64.b64decode(maximum.content[1].resource.blob), b"\xff" * 16777216)
                self.assertLess(len(maximum.model_dump_json().encode()), 4 * ((16777216 + 2) // 3) + 4096)


if __name__ == "__main__":
    unittest.main()
