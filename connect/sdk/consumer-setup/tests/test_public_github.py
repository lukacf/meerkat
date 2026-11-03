"""Public research uses the actual MCP/controller boundary and owned runner."""
import asyncio
import base64
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import threading
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


M = load("public_github_test", ROOT / "public_github.py")
A = load("github_mcp_test", ROOT / "mcp_adapter.py")
B = A.backend
F = load("github_backend_fixture", ROOT / "tests/test_backend.py")
F.backend = B


def search_document():
    return {"total_count": 21, "incomplete_results": True, "items": [
        {"owner": {"login": "example"}, "name": name, "full_name": "example/" + name,
         "private": False, "description": "Untrusted evidence"} for name in ("second", "first")]}


def file_document(raw=b"# Public project\n", path="README.md"):
    blob = b"blob " + str(len(raw)).encode("ascii") + b"\0" + raw
    return {"type": "file", "encoding": "base64", "size": len(raw), "sha": hashlib.sha1(blob).hexdigest(),
            "path": path, "content": base64.b64encode(raw).decode("ascii")}


class ResearchRunner(B.Runner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.calls = []
        self.block = None
        self.entered = threading.Event()
        self.error = None

    def run(self, argv, **kwargs):
        self.calls.append((list(argv), kwargs, dict(self._compose_context.settings)))
        self.entered.set()
        if self.block is not None:
            self.block.wait(5)
        raw = os.read(self._compose_context.settings["stdin"], 4096)
        value = M.decode(raw)
        if self.error:
            return 0, M.wire(self.error)
        with patch.object(M, "fetch", return_value=search_document() if value["operation"] == "search" else file_document()):
            result = M.execute(value["operation"], value["request"])
        return 0, M.wire(result)


class TransportTests(unittest.TestCase):
    def test_literal_query_order_pagination_and_no_selection(self):
        with patch.object(M, "fetch", return_value=search_document()) as fetch:
            result = M.execute("search", {"query": "protocol language:python", "page": 2})
        self.assertEqual(fetch.call_args.args, ("/search/repositories?q=protocol+language%3Apython&per_page=20&page=2",))
        self.assertEqual([row["repository"] for row in result["repositories"]], ["example/second", "example/first"])
        self.assertTrue(result["selection_required"])
        self.assertTrue(result["incomplete_results"])
        with patch.object(M, "fetch") as fetch:
            for request in ({"query": "x", "url": "https://evil.invalid"}, {"query": "x", "page": True},
                            {"query": "x", "page": 6}, {"query": "x\nsecret"}, {"query": "x" * 257}):
                with self.subTest(request=request), self.assertRaises(ValueError):
                    M.execute("search", request)
            fetch.assert_not_called()

    def test_text_identity_ref_limits_and_no_caller_url(self):
        raw = b"# Research evidence\n"
        with patch.object(M, "fetch", return_value=file_document(raw, "docs/guide.md")) as fetch:
            result = M.execute("read", {"owner": "example", "repository": "project", "path": "docs/guide.md", "ref": "feature/docs"})
        self.assertEqual(fetch.call_args.args, ("/repos/example/project/contents/docs/guide.md?ref=feature%2Fdocs",))
        self.assertEqual(result["content_sha256"], hashlib.sha256(raw).hexdigest())
        self.assertEqual(result["text"].encode(), raw)
        self.assertEqual(result["blob_sha"], file_document(raw)["sha"])
        for document in (file_document(b"\x00binary"), file_document(b"x" * (M.TEXT_LIMIT + 1)),
                         {**file_document(), "type": "dir"}):
            with patch.object(M, "fetch", return_value=document), self.assertRaises(M.Failure):
                M.execute("read", {"owner": "example", "repository": "project"})
        with patch.object(M, "fetch") as fetch:
            for path in ("../secret", "/etc/passwd", "a/../b", "a\\b", "a//b"):
                with self.assertRaises(ValueError):
                    M.execute("read", {"owner": "example", "repository": "project", "path": path})
            fetch.assert_not_called()

    def test_https_owner_no_auth_redirects_and_bounded_error_projection(self):
        class Response:
            status = 200
            body = M.wire(search_document())
            headers = {"Content-Type": "application/json; charset=utf-8"}
            def getheader(self, name, default=None):
                return self.headers.get(name, default)
            def read(self, limit):
                return self.body[:limit]
        response = Response()
        with patch.object(M.http.client, "HTTPSConnection") as factory:
            connection = factory.return_value
            connection.getresponse.return_value = response
            self.assertEqual(M.fetch("/search/repositories?q=literal"), search_document())
            self.assertEqual(factory.call_args.args, ("api.github.com",))
            request = connection.request.call_args
            self.assertEqual(request.args, ("GET", "/search/repositories?q=literal"))
            self.assertFalse({"Authorization", "Cookie", "Proxy-Authorization"} & set(request.kwargs["headers"]))
            for status, code in ((302, "github_redirect_refused"), (429, "github_rate_limited"),
                                 (404, "github_not_found"), (500, "github_request_rejected")):
                response.status = status
                with self.assertRaises(M.Failure) as failure:
                    M.fetch("/readme")
                self.assertEqual(failure.exception.code, code)
            response.status, response.body = 200, b"x" * (M.RESPONSE_LIMIT + 1)
            with self.assertRaises(M.Failure) as failure:
                M.fetch("/readme")
            self.assertEqual(failure.exception.code, "github_read_too_large")
            self.assertEqual(connection.request.call_count, 6)
            self.assertEqual(connection.close.call_count, 6)


class ControllerTests(F.FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_default_disabled_and_selected_operations(self):
        from mcp import Client
        ordinary = self.controller(families=("services",))
        ordinary_owner = A.SetupAdapter(ordinary)
        try:
            async with Client(A.create_server(ordinary_owner)) as client:
                self.assertNotIn("setup_search_github", {tool.name for tool in (await client.list_tools()).tools})
            with self.assertRaisesRegex(B.SetupError, "github_research_unavailable"):
                ordinary.search_github({"query": "protocol"})
        finally:
            await ordinary_owner.aclose()
        for choice in (1, "true", None):
            with self.assertRaisesRegex(B.SetupError, "invalid_github_research_scope"):
                self.controller(families=("services",), allow_public_github_research=choice)
        controller = self.controller(families=("services",), allow_public_github_research=True, runner_factory=ResearchRunner)
        before = copy.deepcopy((controller.job, controller.jobs, controller.validations))
        owner = A.SetupAdapter(controller)
        try:
            async with Client(A.create_server(owner)) as client:
                tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                self.assertTrue({"setup_search_github", "setup_read_github"} <= tools.keys())
                self.assertNotIn("setup_selected_install", tools)
                self.assertTrue(tools["setup_read_github"].annotations.open_world_hint)
                search = await client.call_tool("setup_search_github", {"query": "protocol"})
                self.assertFalse(search.is_error, search)
                self.assertEqual(search.structured_content["repositories"][0]["repository"], "example/second")
                read = await client.call_tool("setup_read_github", {"owner": "example", "repository": "project"})
                self.assertFalse(read.is_error, read)
                self.assertEqual(read.structured_content["blob_sha"], file_document()["sha"])
                invalid = await client.call_tool("setup_search_github", {"query": "x", "token": "PRIVATE_CANARY"})
                self.assertTrue(invalid.is_error)
                self.assertNotIn("PRIVATE_CANARY", str(invalid))
            self.assertEqual(len(controller.runner.calls), 2)
            self.assertEqual((controller.job, controller.jobs, controller.validations), before)
        finally:
            await owner.aclose()

    async def test_cancel_busy_and_close_join_owned_work(self):
        controller = self.controller(families=("services",), allow_public_github_research=True, runner_factory=ResearchRunner)
        controller.runner.block = threading.Event()
        owner = A.SetupAdapter(controller)
        task = asyncio.create_task(owner.dispatch("/api/search_github", {"query": "protocol"}))
        await asyncio.to_thread(controller.runner.entered.wait, 3)
        self.assertTrue(controller.runner.entered.is_set())
        with self.assertRaisesRegex(B.SetupError, "operation_in_progress"):
            controller.read_github({"owner": "example", "repository": "project"})
        task.cancel()
        close = asyncio.create_task(owner.aclose())
        await asyncio.sleep(0.05)
        self.assertFalse(task.done())
        self.assertFalse(close.done())
        controller.runner.block.set()
        with self.assertRaises(asyncio.CancelledError):
            await task
        await close
        self.assertFalse(controller.operation_lock.locked())
        self.assertEqual(list((controller.workspace / "tmp").iterdir()), [])
        self.assertEqual(len(controller.runner.calls), 1)

    async def test_rate_and_changed_source_refuse_before_another_worker(self):
        controller = self.controller(families=("services",), allow_public_github_research=True, runner_factory=ResearchRunner)
        controller.runner.error = {"error": "github_rate_limited", "retry_after_seconds": 60}
        for _ in range(2):
            with self.assertRaisesRegex(B.SetupError, "github_rate_limited"):
                controller.search_github({"query": "protocol"})
        self.assertEqual(len(controller.runner.calls), 1)
        controller.public_github.retry_at = 0
        controller.public_github.worker.write_bytes(b"changed")
        with self.assertRaisesRegex(B.SetupError, "github_research_source_changed"):
            controller.search_github({"query": "protocol"})
        self.assertEqual(len(controller.runner.calls), 1)

    async def test_actual_runner_deadline_reaps_child(self):
        controller = self.controller(families=("services",), allow_public_github_research=True, runner_factory=ResearchRunner)
        pid_path = self.root / "child.pid"
        class DeadlineRunner(B.Runner):
            def run_compose(self, argv, **kwargs):
                program = "import os,pathlib,time; pathlib.Path(" + repr(str(pid_path)) + ").write_text(str(os.getpid())); time.sleep(10)"
                return super().run_compose([sys.executable, "-I", "-S", "-c", program],
                    output_limit=4096, timeout=0.3)
        with self.assertRaisesRegex(B.SetupError, "inspection_unavailable"):
            controller.public_github.run("search", {"query": "protocol"}, DeadlineRunner(controller.workspace))
        with self.assertRaises(ProcessLookupError):
            os.kill(int(pid_path.read_text()), 0)
        self.assertEqual(list((controller.workspace / "tmp").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
