"""Read exact setup contracts through real MCP without a host filesystem tool."""
import json
from pathlib import Path
import unittest
from unittest.mock import patch

from mcp import Client

from test_selected_setup import SelectedFixture, adapter, backend
from test_selected_mcpb import McpbFixture
from test_hosted_recipes import HostedFixture, adapter as hosted_adapter


class GuideTests(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    def server(self, controller, module_file=None):
        owner = adapter.SetupAdapter(controller)
        self.addAsyncCleanup(owner.aclose)
        with patch.object(adapter, "__file__", str(module_file or adapter.__file__)):
            return adapter.create_server(owner)

    async def call(self, client, topic):
        result = await client.call_tool("setup_guide", {"topic": topic})
        self.assertFalse(result.is_error, result)
        self.assertEqual(json.loads(result.content[0].text), result.structured_content)
        return result.structured_content

    def layout(self, name="packed", kind="combined", data=b"# Inert guide\r\nExact UTF-8: caf\xc3\xa9.\n"):
        root = self.root / name
        (root / "setup").mkdir(parents=True)
        guides = root / ("selected/contracts" if kind == "combined" else "contracts")
        guides.mkdir(parents=True)
        (guides / "METADATA.md").write_bytes(data)
        return root / "setup/mcp_adapter.py", guides

    async def test_source_contracts_discoverable_before_any_preparation(self):
        controller = self.controller()
        before = controller.state()
        async with Client(self.server(controller)) as client:
            tools = {item.name: item for item in (await client.list_tools()).tools}
            self.assertIn("setup_guide", tools)
            self.assertEqual(tools["setup_guide"].input_schema["properties"]["topic"]["enum"],
                             ["selected-metadata", "python-install", "node-install"])
            root = Path(adapter.__file__).resolve().parent
            for topic, path in [("selected-metadata", root / "METADATA.md"),
                                ("python-install", root.parents[1] / "crates/toolkit-package/PYTHON-INSTALL.md"),
                                ("node-install", root.parents[1] / "crates/toolkit-package/NODE-INSTALL.md")]:
                result = await self.call(client, topic)
                raw = path.read_bytes()
                self.assertEqual(result, {"topic": topic, "scope": "documentation_only",
                    "media_type": "text/markdown", "text": raw.decode("utf-8"),
                    "bytes": len(raw), "sha256": backend.digest(raw)})
        self.assertEqual(controller.runner.calls, [])
        self.assertEqual(controller.state(), before)

    async def test_packed_layouts_preserve_bytes_and_freeze_for_the_session(self):
        for kind in ("combined", "standalone"):
            with self.subTest(kind=kind):
                module, guides = self.layout(kind, kind)
                raw = (guides / "METADATA.md").read_bytes()
                server = self.server(self.controller(), module)
                (guides / "METADATA.md").write_text("changed after server creation")
                with patch.object(backend, "read_file", side_effect=AssertionError("No request-time file reads")):
                    async with Client(server) as client:
                        first = await self.call(client, "selected-metadata")
                        self.assertEqual(first["text"].encode(), raw)
                        self.assertEqual(await self.call(client, "selected-metadata"), first)
                        unavailable = await client.call_tool("setup_guide", {"topic": "python-install"})
                        self.assertEqual(unavailable.structured_content, {"error": "guide_unavailable"})

    async def test_operation_scope_guidance_reaches_helper_in_supported_layouts(self):
        source_root = Path(adapter.__file__).resolve().parents[2]
        topics = ("selected-metadata", "python-install", "node-install")
        for kind in ("source", "combined", "standalone"):
            with self.subTest(layout=kind):
                module = None
                if kind != "source":
                    module, guides = self.layout("operation-scope-" + kind, kind)
                    for topic in topics:
                        filename, source = adapter.GUIDE_FILES[topic]
                        (guides / filename).write_bytes((source_root / source).read_bytes())
                controller = self.controller()
                before = controller.state()
                async with Client(self.server(controller, module)) as client:
                    for topic in topics:
                        with self.subTest(topic=topic):
                            raw = (source_root / adapter.GUIDE_FILES[topic][1]).read_bytes()
                            result = await self.call(client, topic)
                            self.assertEqual(result["text"].encode(), raw)
                            self.assertEqual(result["sha256"], backend.digest(raw))
                            self.assertLessEqual(result["bytes"], adapter.GUIDE_LIMIT)
                            text = " ".join(result["text"].split())
                            for distinction in ("native protected", "operation-name allowlist",
                                                "query semantics", "startup effects",
                                                "filesystem confinement"):
                                self.assertTrue(distinction in text,
                                                f"{kind}/{topic} omits {distinction!r}")
                self.assertEqual(controller.runner.calls, [])
                self.assertEqual(controller.state(), before)

    async def test_limit_is_exact_utf8_bytes_and_empty_contract_refuses(self):
        for label, data, valid in (("limit", b"# " + b"X" * (32 * 1024 - 2), True),
                                   ("empty", b"", False),
                                   ("multibyte", "\u00e9".encode() * (16 * 1024 + 1), False)):
            module, _ = self.layout(label, data=data)
            async with Client(self.server(self.controller(), module)) as client:
                result = await client.call_tool("setup_guide", {"topic": "selected-metadata"})
                self.assertEqual(result.is_error, not valid)
                if valid:
                    self.assertEqual(result.structured_content["text"].encode(), data)
                    self.assertEqual(result.structured_content["bytes"], 32 * 1024)
                else:
                    self.assertEqual(result.structured_content, {"error": "guide_unavailable"})

    async def test_disabled_topics_and_caller_paths_refuse_before_dispatch(self):
        controller = self.controller()
        async with Client(self.server(controller)) as client:
            for args in ({"topic": "hosted-metadata"}, {"topic": "mcpb-install"},
                         {"topic": "../../private.sqlite3"}, {"topic": None},
                         {"topic": "selected-metadata", "path": str(self.root)}):
                with self.subTest(args=args):
                    result = await client.call_tool("setup_guide", args)
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content, {"error": "invalid_arguments"})
        self.assertEqual(controller.runner.calls, [])

    async def test_missing_unsafe_or_oversized_contract_is_fixed_unavailable(self):
        import os
        for kind in ("missing", "utf8", "control", "oversized", "directory", "fifo", "symlink", "parent_symlink"):
            with self.subTest(kind=kind):
                module, guides = self.layout(kind)
                path = guides / "METADATA.md"
                path.unlink()
                if kind == "utf8":
                    path.write_bytes(b"\xffPRIVATE")
                elif kind == "control":
                    path.write_bytes(b"PRIVATE\0")
                elif kind == "oversized":
                    path.write_bytes(b"X" * (32 * 1024 + 1))
                elif kind == "directory":
                    path.mkdir()
                elif kind == "fifo":
                    os.mkfifo(path)
                elif kind in ("symlink", "parent_symlink"):
                    secret = self.root / (kind + "-private")
                    secret.write_text("PRIVATE_HOST_CONTENT")
                    if kind == "symlink":
                        path.symlink_to(secret)
                    else:
                        guides.rmdir()
                        guides.symlink_to(self.root, target_is_directory=True)
                async with Client(self.server(self.controller(), module)) as client:
                    result = await client.call_tool("setup_guide", {"topic": "selected-metadata"})
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content, {"error": "guide_unavailable"})
                    self.assertNotIn("PRIVATE", str(result))

    async def test_ambiguous_or_unknown_layout_has_no_fallback(self):
        module, guides = self.layout("ambiguous")
        (module.parent.parent / "contracts").mkdir()
        (module.parent.parent / "contracts/METADATA.md").write_text("wrong layout")
        for location in (module, guides / "mcp_adapter.py"):
            async with Client(self.server(self.controller(), location)) as client:
                result = await client.call_tool("setup_guide", {"topic": "selected-metadata"})
                self.assertEqual(result.structured_content, {"error": "guide_unavailable"})

    async def test_recovery_does_not_add_tools_or_read_guides(self):
        controller, _, job = self.installed()
        recovered = self.recover(controller, job)
        with patch.object(backend, "read_file", side_effect=AssertionError("Recovery must not load guides")):
            async with Client(self.server(recovered)) as client:
                self.assertNotIn("setup_guide", {item.name for item in (await client.list_tools()).tools})
                result = await client.call_tool("setup_guide", {"topic": "selected-metadata"})
                self.assertEqual(result.structured_content, {"error": "unknown_tool"})


class HostedGuideTests(HostedFixture, unittest.IsolatedAsyncioTestCase):
    async def test_remote_opt_in_alone_gates_hosted_guide_without_reading_database(self):
        for enabled in (False, True):
            controller = self.controller(allow_remote_recipes=enabled)
            owner = hosted_adapter.SetupAdapter(controller)
            self.addAsyncCleanup(owner.aclose)
            async with Client(hosted_adapter.create_server(owner)) as client:
                tools = {item.name: item for item in (await client.list_tools()).tools}
                if not enabled:
                    self.assertNotIn("setup_guide", tools)
                else:
                    self.assertEqual(tools["setup_guide"].input_schema["properties"]["topic"]["enum"], ["hosted-metadata"])
                    result = await client.call_tool("setup_guide", {"topic": "hosted-metadata"})
                    raw = Path(hosted_adapter.__file__).with_name("HOSTED-METADATA.md").read_bytes()
                    self.assertFalse(result.is_error)
                    self.assertEqual(result.structured_content["text"].encode(), raw)
                    text = " ".join(result.structured_content["text"].split())
                    for distinction in ("native protected", "operation-name allowlist",
                                        "query semantics", "startup effects", "filesystem confinement"):
                        self.assertTrue(distinction in text, f"hosted-metadata omits {distinction!r}")
            self.assertEqual(controller.runner.calls, [])


class McpbGuideTests(McpbFixture, unittest.IsolatedAsyncioTestCase):
    async def test_mcpb_topic_requires_its_selected_validator(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        self.addAsyncCleanup(owner.aclose)
        async with Client(adapter.create_server(owner)) as client:
            tools = {item.name: item for item in (await client.list_tools()).tools}
            self.assertIn("mcpb-install", tools["setup_guide"].input_schema["properties"]["topic"]["enum"])
            result = await client.call_tool("setup_guide", {"topic": "mcpb-install"})
            raw = (Path(adapter.__file__).resolve().parents[2] / "crates/toolkit-package/MCPB-INSTALL.md").read_bytes()
            self.assertFalse(result.is_error)
            self.assertEqual(result.structured_content["text"].encode(), raw)
            text = " ".join(result.structured_content["text"].split())
            for distinction in ("native protected", "operation-name allowlist",
                                "query semantics", "startup effects", "filesystem confinement"):
                self.assertTrue(distinction in text, f"mcpb-install omits {distinction!r}")
        self.assertEqual(controller.runner.calls, [])


if __name__ == "__main__":
    unittest.main()
