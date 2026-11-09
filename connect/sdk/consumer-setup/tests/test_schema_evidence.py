"""Exact supplied-schema evidence through the real MCP protocol, without execution."""
import hashlib
import json
import unittest

from mcp import Client
from test_selected_setup import SelectedFixture, adapter, backend


class SchemaEvidenceTests(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    def selected(self):
        self.profile["package_inspector"] = self.tools["python"]
        self.write_profile()
        return self.controller()

    async def test_real_mcp_hashes_exact_empty_object_without_package_validation(self):
        controller = self.selected()
        owner = adapter.SetupAdapter(controller)
        before = sorted(str(p.relative_to(controller.workspace)) for p in controller.workspace.rglob("*"))
        try:
            async with Client(adapter.create_server(owner)) as client:
                result = await client.call_tool("setup_inspect_schema", {"schema_json": "{}"})
                self.assertFalse(result.is_error, "supplied schema evidence must be available without a package")
                value = result.structured_content
                self.assertEqual(json.loads(result.content[0].text), value)
                self.assertEqual(value["schema_digest"],
                    "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a")
                self.assertEqual(value["bytes"], 2)
                self.assertEqual(value["scope"], "supplied_schema_evidence_only")
                self.assertEqual(value["hash_algorithm"], "sha256")
                self.assertEqual(value["byte_convention"], "exact_supplied_utf8")
                self.assertEqual(value["runtime_observation"], "not_established")
                self.assertEqual(value["schema_validation"], "not_performed")
                self.assertEqual(controller.runner.calls, [])
                self.assertEqual(controller.jobs, {})
                self.assertEqual(controller.selected_attempts, 0)
                self.assertEqual(before, sorted(str(p.relative_to(controller.workspace))
                    for p in controller.workspace.rglob("*")))
        finally:
            await owner.aclose()

    async def test_unicode_whitespace_and_escape_spelling_remain_exact_bytes(self):
        controller = self.selected()
        owner = adapter.SetupAdapter(controller)
        texts = ['{"title":"caf\u00e9"}', '{ "title": "caf\u00e9" }\n', '{"title":"caf\\u00e9"}']
        self.assertTrue(all(json.loads(text) == json.loads(texts[0]) for text in texts))
        try:
            async with Client(adapter.create_server(owner)) as client:
                hashes = []
                for text in texts:
                    result = await client.call_tool("setup_inspect_schema", {"schema_json": text})
                    self.assertFalse(result.is_error)
                    value = result.structured_content
                    self.assertEqual(value["schema_digest"], hashlib.sha256(text.encode("utf-8")).hexdigest())
                    self.assertEqual(value["bytes"], len(text.encode("utf-8")))
                    self.assertNotIn("caf", json.dumps(value))
                    hashes.append(value["schema_digest"])
                self.assertEqual(len(set(hashes)), len(texts))
                repeated = await client.call_tool("setup_inspect_schema", {"schema_json": texts[0]})
                self.assertEqual(repeated.structured_content["schema_digest"], hashes[0])
        finally:
            await owner.aclose()

    async def test_json_refusals_are_fixed_and_do_not_echo_input(self):
        controller = self.selected()
        owner = adapter.SetupAdapter(controller)
        invalid = ['{"PRIVATE":"unterminated', '{"PRIVATE":1,"PRIVATE":2}',
                   '{"x":{"PRIVATE":1,"PRIVATE":2}}', '{"PRIVATE":NaN}',
                   '{"PRIVATE":Infinity}', '{"PRIVATE":1e999}', '"PRIVATE"',
                   '[]', 'null', '42', '\ufeff{}']
        try:
            async with Client(adapter.create_server(owner)) as client:
                for text in invalid:
                    with self.subTest(text=text):
                        result = await client.call_tool("setup_inspect_schema", {"schema_json": text})
                        self.assertTrue(result.is_error)
                        self.assertEqual(result.structured_content, {"error": "invalid_schema_evidence"})
                        self.assertNotIn("PRIVATE", result.content[0].text)
                for args in ({}, {"schema_json": {}}, {"schema_json": "{}", "path": "/PRIVATE"}):
                    result = await client.call_tool("setup_inspect_schema", args)
                    self.assertTrue(result.is_error)
                    self.assertEqual(result.structured_content, {"error": "invalid_arguments"})
        finally:
            await owner.aclose()

    async def test_document_request_and_structure_bounds_refuse_before_any_effect(self):
        controller = self.selected()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                cases = [('{"title":"' + '\u00e9' * 16384 + '"}', "invalid_arguments"),
                         ('{"title":"' + '\\\\' * 16378 + '"}', "invalid_arguments"),
                         ('{"items":' * 34 + 'true' + '}' * 34, "invalid_schema_evidence"),
                         (json.dumps({"enum": [0] * 4097}), "invalid_schema_evidence")]
                for text, error in cases:
                    with self.subTest(error=error, bytes=len(text.encode())):
                        result = await client.call_tool("setup_inspect_schema", {"schema_json": text})
                        self.assertTrue(result.is_error)
                        self.assertEqual(result.structured_content, {"error": error})
                self.assertEqual(controller.runner.calls, [])
                self.assertEqual(controller.jobs, {})
        finally:
            await owner.aclose()

    async def test_boolean_and_unresolved_refs_are_evidence_not_schema_validation(self):
        controller = self.selected()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                for text in ('true', 'false', '{"$ref":"https://example.invalid/private-schema"}',
                             '{"type":"not-a-json-schema-type"}'):
                    result = await client.call_tool("setup_inspect_schema", {"schema_json": text})
                    self.assertFalse(result.is_error)
                    value = result.structured_content
                    self.assertEqual(value["schema_digest"], hashlib.sha256(text.encode()).hexdigest())
                    self.assertEqual(value["schema_validation"], "not_performed")
                    self.assertEqual(value["runtime_observation"], "not_established")
                    self.assertNotIn("private-schema", json.dumps(value))
                self.assertEqual(controller.runner.calls, [])
        finally:
            await owner.aclose()

    async def test_host_optin_and_recovery_surface_remain_closed(self):
        controller = self.controller()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertNotIn("setup_inspect_schema", {tool.name for tool in (await client.list_tools()).tools})
            with self.assertRaises(backend.SetupError) as caught:
                controller.dispatch("/api/inspect_schema", {"schema_json": "{}"})
            self.assertEqual(caught.exception.code, "schema_inspection_unavailable")
        finally:
            await owner.aclose()
        controller = self.selected()
        controller.recovery_only = True
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertNotIn("setup_inspect_schema", {tool.name for tool in (await client.list_tools()).tools})
            with self.assertRaises(backend.SetupError) as caught:
                controller.dispatch("/api/inspect_schema", {"schema_json": "{}"})
            self.assertEqual(caught.exception.code, "recovery_read_only")
        finally:
            await owner.aclose()

    async def test_changed_selected_owner_or_closed_admission_cannot_hash(self):
        controller = self.selected()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                inspector = controller.workspace / "selected-tools/package_inspector"
                inspector.write_bytes(b"changed pinned metadata owner")
                result = await client.call_tool("setup_inspect_schema", {"schema_json": "{}"})
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content, {"error": "selected_tool_changed"})
                controller.close_admission()
                result = await client.call_tool("setup_inspect_schema", {"schema_json": "{}"})
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content, {"error": "host_closing"})
                self.assertEqual(controller.runner.calls, [])
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
