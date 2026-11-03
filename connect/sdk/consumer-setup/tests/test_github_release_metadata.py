"""Exact public release metadata uses the existing owned research operation."""
import asyncio
import copy
import threading
import unittest
import urllib.parse
from unittest.mock import patch

from mcp import Client

import test_public_github as research

M, A, B, F = research.M, research.A, research.B, research.F
REQUEST = {"owner": "example", "repository": "project", "release_tag": "v1.2.3"}


def release_document(tag="v1.2.3", count=3):
    full = "Example/Project"
    encoded = urllib.parse.quote(tag, safe="")
    assets = []
    for index in range(count):
        name = "package-" + str(index) + ".tar.gz"
        assets.append({"id": index + 10, "name": name, "state": "uploaded", "size": 1024 + index,
            "url": "https://api.github.com/repos/" + full + "/releases/assets/" + str(index + 10),
            "browser_download_url": "https://github.com/" + full + "/releases/download/" + encoded + "/" + name})
    if assets:
        assets[0]["digest"] = "sha256:" + "a" * 64
    if len(assets) > 1:
        assets[1]["digest"] = None
    return {"id": 42, "tag_name": tag, "draft": False, "prerelease": False,
        "url": "https://api.github.com/repos/" + full + "/releases/42",
        "html_url": "https://github.com/" + full + "/releases/tag/" + encoded,
        "assets": assets, "body": "UNTRUSTED_RELEASE_BODY_NOT_REQUESTED"}


class ReleaseRunner(research.ResearchRunner):
    def run(self, argv, **kwargs):
        with patch.object(research, "file_document", return_value=release_document()):
            return super().run(argv, **kwargs)


class ReleaseProjectionTests(unittest.TestCase):
    def execute(self, document, request=REQUEST):
        with patch.object(M, "fetch", return_value=document):
            return M.execute("read", request)

    def test_exact_tag_asset_order_and_digest_absence_are_preserved(self):
        tag = "release/v1+build"
        document = release_document(tag)
        with patch.object(M, "fetch", return_value=document) as fetch:
            result = M.execute("read", {**REQUEST, "release_tag": tag})
        self.assertEqual(fetch.call_args.args,
                         ("/repos/example/project/releases/tags/release%2Fv1%2Bbuild",))
        self.assertEqual(result["scope"], "untrusted_public_github_release_metadata")
        self.assertEqual(result["source_url"], M.ORIGIN + fetch.call_args.args[0])
        self.assertEqual(result["release_api_url"], document["url"])
        self.assertEqual(result["release_url"], document["html_url"])
        self.assertEqual(result["release_tag"], tag)
        self.assertEqual(result["asset_count"], 3)
        self.assertEqual([row["name"] for row in result["assets"]], [row["name"] for row in document["assets"]])
        self.assertEqual([row["download_url"] for row in result["assets"]],
                         [row["browser_download_url"] for row in document["assets"]])
        self.assertEqual([row["bytes"] for row in result["assets"]], [1024, 1025, 1026])
        self.assertEqual([row["sha256"] for row in result["assets"]], ["a" * 64, None, None])
        self.assertEqual([row["reported_digest"] for row in result["assets"]], ["sha256:" + "a" * 64, None, None])
        self.assertTrue(result["selection_required"])
        self.assertFalse(result["assets_downloaded"])
        self.assertEqual(result["digest_provenance"], "reported_by_github_not_locally_verified")
        self.assertNotIn(document["body"], M.wire(result).decode())

    def test_other_digest_algorithm_is_reported_without_an_invented_sha256(self):
        document = release_document(count=1)
        document["assets"][0]["digest"] = "sha512:" + "b" * 128
        row = self.execute(document)["assets"][0]
        self.assertEqual(row["reported_digest"], document["assets"][0]["digest"])
        self.assertIsNone(row["sha256"])
        empty = self.execute(release_document(count=0))
        self.assertEqual(empty["assets"], [])
        self.assertEqual(empty["asset_count"], 0)

    def test_mixed_modes_and_invalid_tags_refuse_before_fetch(self):
        requests = [{**REQUEST, "path": "README.md"}, {**REQUEST, "ref": "main"},
                    {**REQUEST, "url": "https://evil.invalid"}]
        requests.extend({**REQUEST, "release_tag": value} for value in
                        (None, True, "", " ", "v1\nprivate", "a" * 257, ".", "..", "../v1", "v1/../v2", "a//b", "/v1", "a\\b"))
        with patch.object(M, "fetch") as fetch:
            for request in requests:
                with self.subTest(request=request), self.assertRaises(ValueError):
                    M.execute("read", request)
            fetch.assert_not_called()

    def test_release_identity_and_numeric_metadata_refuse_malformed_values(self):
        changes = [lambda d: d.update(tag_name="other"), lambda d: d.update(id=True),
            lambda d: d.update(id=0), lambda d: d.update(draft=True),
            lambda d: d.update(prerelease=1), lambda d: d.update(assets={}),
            lambda d: d["assets"][0].update(id=True), lambda d: d["assets"][0].update(size=True),
            lambda d: d["assets"][0].update(size=-1), lambda d: d["assets"][0].update(name="../asset"),
            lambda d: d["assets"][0].update(name="with\x00control"), lambda d: d["assets"][0].update(state=None),
            lambda d: d["assets"][0].update(digest="sha256:missing"),
            lambda d: d["assets"][0].update(digest="sha256:" + "A" * 64),
            lambda d: d["assets"][0].update(digest=True),
            lambda d: d["assets"][1].update(id=d["assets"][0]["id"]),
            lambda d: d["assets"][1].update(name=d["assets"][0]["name"])]
        for index, change in enumerate(changes):
            document = release_document()
            change(document)
            with self.subTest(index=index), self.assertRaises(ValueError):
                self.execute(document)

    def test_observed_urls_are_exactly_bound_and_never_followed(self):
        changes = [
            ("url", "https://api.github.com/repos/other/project/releases/42"),
            ("html_url", "https://github.com/example/project/releases/tag/other"),
            ("asset.url", "https://api.github.com/repos/example/project/releases/assets/99"),
            ("asset.browser_download_url", "https://github.com/other/project/releases/download/v1.2.3/package-0.tar.gz"),
            ("asset.browser_download_url", "https://github.com/example/project/releases/download/other/package-0.tar.gz"),
            ("asset.browser_download_url", "https://github.com/example/project/releases/download/v1.2.3/other.tar.gz"),
            ("asset.browser_download_url", "https://github.com.evil.invalid/example/project/releases/download/v1.2.3/package-0.tar.gz"),
            ("asset.browser_download_url", "https://user:PRIVATE@github.com/example/project/releases/download/v1.2.3/package-0.tar.gz"),
            ("asset.browser_download_url", "https://github.com:443/example/project/releases/download/v1.2.3/package-0.tar.gz"),
            ("asset.browser_download_url", "https://github.com/example/project/releases/download/v1.2.3/package-0.tar.gz?token=PRIVATE"),
            ("asset.browser_download_url", "https://github.com/example/project/releases/download/v1.2.3/package-0.tar.gz#fragment"),
            ("asset.browser_download_url", "http://github.com/example/project/releases/download/v1.2.3/package-0.tar.gz"),
        ]
        for key, value in changes:
            document = release_document()
            if key.startswith("asset."):
                document["assets"][0][key[6:]] = value
            else:
                document[key] = value
            with self.subTest(key=key, value=value), patch.object(M, "fetch", return_value=document) as fetch:
                with self.assertRaises(ValueError):
                    M.execute("read", REQUEST)
                self.assertEqual(fetch.call_count, 1)
                self.assertEqual(fetch.call_args.args, ("/repos/example/project/releases/tags/v1.2.3",))

    def test_asset_and_output_limits_refuse_without_silent_truncation(self):
        document = release_document(count=M.RELEASE_ASSET_LIMIT)
        result = self.execute(document)
        self.assertEqual(result["asset_count"], M.RELEASE_ASSET_LIMIT)
        self.assertEqual(result["assets"][-1]["name"], document["assets"][-1]["name"])
        with self.assertRaises(M.Failure) as failure:
            self.execute(release_document(count=M.RELEASE_ASSET_LIMIT + 1))
        self.assertEqual(failure.exception.code, "github_read_too_large")
        with patch.object(M, "RESPONSE_LIMIT", 1024), self.assertRaises(M.Failure) as failure:
            self.execute(document)
        self.assertEqual(failure.exception.code, "github_read_too_large")


class ReleaseMcpTests(F.FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_schema_dispatch_and_pre_dispatch_mode_refusal(self):
        controller = self.controller(families=("services",), allow_public_github_research=True,
                                     runner_factory=ReleaseRunner)
        before = copy.deepcopy((controller.job, controller.jobs, controller.validations))
        owner = A.SetupAdapter(controller)
        try:
            async with Client(A.create_server(owner)) as client:
                tools = {tool.name: tool for tool in (await client.list_tools()).tools}
                tool = tools["setup_read_github"]
                self.assertIn("release_tag", tool.input_schema["properties"])
                self.assertTrue(tool.annotations.read_only_hint)
                self.assertIn("mutually exclusive", tool.description)
                for other in ("path", "ref"):
                    result = await client.call_tool("setup_read_github", {**REQUEST, other: "PRIVATE_CANARY"})
                    self.assertTrue(result.is_error)
                    self.assertNotIn("PRIVATE_CANARY", str(result))
                self.assertEqual(controller.runner.calls, [])
                result = await client.call_tool("setup_read_github", REQUEST)
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["scope"], "untrusted_public_github_release_metadata")
                self.assertEqual(result.structured_content["assets"][0]["sha256"], "a" * 64)
                self.assertIsNone(result.structured_content["assets"][1]["sha256"])
            self.assertEqual(len(controller.runner.calls), 1)
            self.assertEqual((controller.job, controller.jobs, controller.validations), before)
            self.assertEqual(list((controller.workspace / "tmp").iterdir()), [])
        finally:
            await owner.aclose()

    async def test_cancelled_release_read_joins_the_same_owned_worker(self):
        controller = self.controller(families=("services",), allow_public_github_research=True,
                                     runner_factory=ReleaseRunner)
        controller.runner.block = threading.Event()
        owner = A.SetupAdapter(controller)
        task = asyncio.create_task(owner.dispatch("/api/read_github", REQUEST))
        close = None
        try:
            self.assertTrue(await asyncio.to_thread(controller.runner.entered.wait, 3))
            task.cancel()
            close = asyncio.create_task(owner.aclose())
            await asyncio.sleep(0.02)
            self.assertFalse(task.done())
            self.assertFalse(close.done())
            controller.runner.block.set()
            with self.assertRaises(asyncio.CancelledError):
                await task
            await close
            self.assertFalse(controller.operation_lock.locked())
            self.assertEqual(len(controller.runner.calls), 1)
            self.assertEqual(list((controller.workspace / "tmp").iterdir()), [])
        finally:
            controller.runner.block.set()
            await asyncio.gather(task, return_exceptions=True)
            await owner.aclose()

    async def test_release_rate_limit_stays_an_error_and_blocks_another_worker(self):
        controller = self.controller(families=("services",), allow_public_github_research=True,
                                     runner_factory=ReleaseRunner)
        controller.runner.error = {"error": "github_rate_limited", "retry_after_seconds": 60}
        owner = A.SetupAdapter(controller)
        try:
            async with Client(A.create_server(owner)) as client:
                for _ in range(2):
                    result = await client.call_tool("setup_read_github", REQUEST)
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content["error"], "github_rate_limited")
            self.assertEqual(len(controller.runner.calls), 1)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
