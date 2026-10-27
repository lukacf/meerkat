"""Bounded projection of the pinned package owner's parse diagnostics."""
import copy
import json
import unittest
from pathlib import Path

import test_selected_inline as inline_fixture
from test_selected_setup import SelectedFixture, adapter, backend


def report(document="{}", path=None, category="data"):
    return {"schema_version": 1, "scope": "metadata_only", "status": "invalid",
            "error": {"kind": "invalid_metadata", "path": "package",
                      "message": "invalid integration package", "diagnostic": {
                          "version": 1, "category": category,
                          "field_path": ["strategies", 0, "known_requirements", 0] if path is None else path,
                          "source_digest": backend.digest(document.encode())}}}


class ProjectionTests(unittest.TestCase):
    def project(self, value, document="{}", code=2):
        with self.assertRaises(backend.SetupError) as caught:
            backend.package_inspection(backend.encoded(value), backend.digest(document.encode()), code)
        return backend.setup_error_payload(caught.exception)

    def test_exact_owner_diagnostic_keeps_fixed_guidance_and_input_binding(self):
        document = '{"description":"private caf\u00e9"}'
        value = report(document)
        result = self.project(value, document)
        self.assertEqual(result["error"], "selected_invalid_package")
        for key, item in value["error"]["diagnostic"].items():
            self.assertEqual(result["diagnostic"][key], item)
        self.assertEqual(result["diagnostic"]["code"], "invalid_package")
        self.assertIn("Preserve valid notices", result["diagnostic"]["message"])
        self.assertNotIn("private", json.dumps(result))
        for category in ("data", "syntax", "eof"):
            self.assertEqual(self.project(report(category=category, path=[]))["diagnostic"]["category"], category)

    def test_context_specific_paths_allow_only_canonical_output_vocabulary(self):
        for path in (["identity", "publisher"], ["provenance", "notices", 0],
                     ["strategies", 1048576, "known_requirements", 0, "version", "minimum_minor"],
                     ["strategies", 0, "known_requirements", 0, "availability"],
                     ["known_exports", 0, "role"], ["assessments", 0, "evidence", 0, "owner"]):
            with self.subTest(path=path):
                self.assertEqual(self.project(report(path=path))["diagnostic"]["field_path"], path)
        for path in (["PRIVATE"], ["identity", "known_requirements"], ["provenance", "publisher"],
                     ["strategies", "0"], ["strategies", True], ["strategies", -1],
                     ["strategies", 1048577], ["strategies", 0, "kind", "local_mcp"],
                     ["strategies", 0, "sources", "PRIVATE"], ["identity", 0],
                     ["description", "PRIVATE"], ["identity"] * 17, None, "PRIVATE"):
            with self.subTest(path=path):
                value = report()
                value["error"]["diagnostic"]["field_path"] = path
                result = self.project(value)
                self.assertEqual(result, {"error": "package_inspection_unavailable"})
                self.assertNotIn("PRIVATE", json.dumps(result))

    def test_unknown_version_digest_fields_and_wrong_envelopes_cannot_be_projected(self):
        valid = report()
        mutations = [
            lambda v: v["error"]["diagnostic"].update(version=True),
            lambda v: v["error"]["diagnostic"].update(version=2),
            lambda v: v["error"]["diagnostic"].update(category="PRIVATE"),
            lambda v: v["error"]["diagnostic"].update(source_digest="0" * 64),
            lambda v: v["error"]["diagnostic"].update(source_digest="sha256:" + "0" * 64),
            lambda v: v["error"]["diagnostic"].update(message="PRIVATE"),
            lambda v: v["error"].update(path="PRIVATE"),
            lambda v: v["error"].update(message="PRIVATE"),
            lambda v: v["error"].update(kind="PRIVATE"),
            lambda v: v["error"].update(diagnostic=None),
            lambda v: v.update(format="integration-package"),
            lambda v: v.update(scope="PRIVATE"),
            lambda v: v.update(status="accepted"),
            lambda v: v.update(schema_version=True),
        ]
        for mutate in mutations:
            value = copy.deepcopy(valid)
            mutate(value)
            self.assertEqual(self.project(value), {"error": "package_inspection_unavailable"})
        for code in (0, 1, -1, True, 2.0):
            self.assertEqual(self.project(valid, code=code), {"error": "package_inspection_unavailable"})
        raw = backend.encoded(valid).replace(b'"version":1', b'"version":1,"version":1')
        with self.assertRaisesRegex(backend.SetupError, "package_inspection_unavailable"):
            backend.package_inspection(raw, backend.digest(b"{}"), 2)

    def test_legacy_errors_keep_generic_help_without_child_text(self):
        value = report()
        del value["error"]["diagnostic"]
        value["error"].update(path="PRIVATE", message="PRIVATE")
        result = self.project(value)
        self.assertEqual(result["error"], "selected_invalid_package")
        self.assertEqual(set(result["diagnostic"]), {"phase", "code", "message"})
        self.assertNotIn("PRIVATE", json.dumps(result))


class ControllerTests(SelectedFixture, unittest.TestCase):
    inspection_controller = inline_fixture.InlineTests.inspection_controller

    def test_failure_retry_readback_and_no_installation_mutation(self):
        c = self.inspection_controller()
        document = json.dumps({"strategies": [{"known_requirements": ["PRIVATE setup prose"],
                              "unresolved_details": ["PRIVATE unknown fact"]}],
                               "provenance": {"notices": ["PRIVATE attribution"]}})
        c.runner.inspection_code = 2
        c.runner.inspection_output = backend.encoded(report(document))
        for _ in range(2):
            with self.assertRaises(backend.SetupError) as caught:
                c.inspect_package({"package_json": document})
            self.assertEqual(backend.setup_error_payload(caught.exception)["diagnostic"]["source_digest"],
                             backend.digest(document.encode()))
        self.assertEqual(c.runner.inspection_calls[0][1], document.encode())
        self.assertEqual(c.runner.inspection_calls[1][1], document.encode())
        self.assertEqual(list((c.workspace / "tmp").iterdir()), [])
        self.assertEqual(list((c.workspace / "selections").iterdir()), [])
        self.assertIsNone(c.job)
        self.assertEqual(c.jobs, {})
        corrected = json.loads(document)
        corrected["strategies"][0]["instructions"] = corrected["strategies"][0]["known_requirements"][0]
        corrected["strategies"][0]["known_requirements"] = []
        c.runner.inspection_code, c.runner.inspection_output = 0, None
        raw = json.dumps(corrected)
        result = c.inspect_package({"package_json": raw})
        self.assertEqual(result["source_digest"], backend.digest(raw.encode()))
        submitted = json.loads(c.runner.inspection_calls[-1][1])
        self.assertEqual(submitted["provenance"], json.loads(document)["provenance"])
        self.assertEqual(submitted["strategies"][0]["unresolved_details"], ["PRIVATE unknown fact"])

    def test_input_and_tool_changes_suppress_diagnostic_after_dispatch(self):
        for mutation in ("input", "tool"):
            with self.subTest(mutation=mutation):
                c = self.inspection_controller()
                c.runner.inspection_code = 2
                c.runner.inspection_output = backend.encoded(report())
                original = c.runner.run_compose
                def changed(*args, **kwargs):
                    result = original(*args, **kwargs)
                    if mutation == "input":
                        kwargs["input_file"].write_text("PRIVATE changed document")
                    else:
                        Path(args[0][0]).write_text("PRIVATE changed tool")
                    return result
                c.runner.run_compose = changed
                with self.assertRaises(backend.SetupError) as caught:
                    c.inspect_package({"package_json": "{}"})
                self.assertEqual(backend.setup_error_payload(caught.exception),
                                 {"error": "package_inspection_unavailable"})

    def test_changed_frozen_tool_refuses_before_dispatch(self):
        c = self.inspection_controller()
        (c.workspace / "selected-tools" / "package_inspector").write_text("PRIVATE changed tool")
        with self.assertRaises(backend.SetupError) as caught:
            c.inspect_package({"package_json": "{}"})
        self.assertEqual(backend.setup_error_payload(caught.exception), {"error": "selected_tool_changed"})
        self.assertEqual(c.runner.inspection_calls, [])
        self.assertEqual(c.selected_attempts, 0)


class McpTests(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    inspection_controller = inline_fixture.InlineTests.inspection_controller

    async def test_mcp_exposes_the_same_bound_diagnostic(self):
        from mcp import Client
        c = self.inspection_controller()
        c.runner.inspection_code = 2
        c.runner.inspection_output = backend.encoded(report())
        owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                result = await client.call_tool("setup_inspect_package", {"package_json": "{}"})
                self.assertTrue(result.is_error)
                self.assertEqual(result.structured_content["error"], "selected_invalid_package")
                self.assertEqual(result.structured_content["diagnostic"]["field_path"],
                                 ["strategies", 0, "known_requirements", 0])
                self.assertEqual(result.structured_content["diagnostic"]["source_digest"], backend.digest(b"{}"))
                self.assertIsNone(c.job)
        finally:
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
