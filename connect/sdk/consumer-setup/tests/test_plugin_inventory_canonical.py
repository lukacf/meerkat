"""Real MCP and canonical inventory/file inspection, with inert installation only.

Run with TOOLKIT_PACKAGE_INSPECT and TOOLKIT_PACKAGE_INSPECT_SHA256 set to the
same reviewed canonical binary. The selected installer remains the existing
controlled fixture; this test qualifies inspection and source recovery, not
software installation, package execution or live runtime readiness.
"""
import os
from pathlib import Path
import secrets
import shutil
import stat
import unittest

from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend


class CanonicalPluginRunner(SelectedRunner):
    """Use the actual process owner for inspection and the inert selected installer."""
    def __init__(self, workspace):
        super().__init__(workspace)
        self.inspector = backend.Runner(workspace)
        self.inspections = []

    def run_compose(self, argv, **kwargs):
        request = backend.decode(kwargs["input_file"].read_bytes())
        self.inspections.append((list(argv), request))
        return self.inspector.run_compose(argv, **kwargs)


class CanonicalPluginInventoryTests(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        binary = os.environ.get("TOOLKIT_PACKAGE_INSPECT")
        expected = os.environ.get("TOOLKIT_PACKAGE_INSPECT_SHA256")
        if binary is None and expected is None:
            self.skipTest("set the reviewed canonical inspector path and SHA-256")
        self.assertIsNotNone(binary, "TOOLKIT_PACKAGE_INSPECT is required")
        self.assertIsNotNone(expected, "TOOLKIT_PACKAGE_INSPECT_SHA256 is required")
        inspector = Path(binary)
        self.assertTrue(inspector.is_absolute())
        self.assertTrue(stat.S_ISREG(inspector.lstat().st_mode))
        self.assertTrue(os.access(inspector, os.X_OK))
        self.assertEqual(backend.digest(inspector.read_bytes()), expected)
        super().setUp()
        self.profile["package_inspector"] = {"path": str(inspector), "sha256": expected}
        self.profile["plugin_inspection"] = True
        self.write_profile()
        self.plugin = self.staging / "ordinary"
        skill = self.plugin / "skills" / ("guide-" + secrets.token_hex(8))
        references = skill / "references"
        references.mkdir(parents=True)
        reference = references / ("details-" + secrets.token_hex(8) + ".md")
        reference.write_text("# Public reference\nReview the exact selected artifact.\n")
        (skill / "SKILL.md").write_text("# Public guide\nRead [details](references/" + reference.name + ").\n")
        (self.plugin / "plugin.json").write_bytes(backend.encoded({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name": "ordinary-plugin", "description": "cafe"}))
        (self.plugin / "mcp.json").write_bytes(backend.encoded({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json", "mcpServers": {}}))

    async def test_actual_mcp_discovers_names_reads_selected_source_and_recovers_without_staging(self):
        from mcp import Client
        c = self.controller(runner_factory=CanonicalPluginRunner)
        owner = adapter.SetupAdapter(c)
        inventory_directories = []
        try:
            async with Client(adapter.create_server(owner)) as client:
                tool = next(t for t in (await client.list_tools()).tools if t.name == "setup_inspect_plugin")
                self.assertIn("inventory", tool.input_schema["properties"])
                refused = await client.call_tool("setup_inspect_plugin", {
                    "staged_id": "ordinary", "inventory": "./", "files": ["./plugin.json"]})
                self.assertTrue(refused.is_error)
                self.assertEqual(c.runner.inspections, [])

                async def inventory(directory):
                    result = await client.call_tool("setup_inspect_plugin", {"staged_id": "ordinary", "inventory": directory})
                    self.assertFalse(result.is_error, result)
                    observed = result.structured_content
                    self.assertEqual(set(observed), {"schema_version", "scope", "format", "status", "directory", "inventory", "complete"})
                    self.assertEqual(observed["schema_version"], 1)
                    self.assertEqual(observed["scope"], "metadata_only")
                    self.assertEqual(observed["format"], "agent-plugin-files")
                    self.assertEqual(observed["status"], "inventoried")
                    self.assertEqual(observed["directory"], directory)
                    self.assertIs(observed["complete"], True)
                    rows = observed["inventory"]
                    self.assertEqual([r["path"] for r in rows], sorted(r["path"] for r in rows))
                    for row in rows:
                        self.assertEqual(set(row), {"path", "kind", "byte_length"} if row["kind"] == "file" else {"path", "kind"})
                    self.assertEqual(c.plugin_inspections, {})
                    self.assertEqual(list((c.workspace / "plugin-inspections").iterdir()), [])
                    inventory_directories.append(directory)
                    return rows

                root = await inventory("./")
                skills = next(r["path"] for r in root if r["path"] == "./skills" and r["kind"] == "directory")
                skill_rows = await inventory(skills)
                self.assertEqual(len(skill_rows), 1)
                self.assertEqual(skill_rows[0]["kind"], "directory")
                skill = skill_rows[0]["path"]
                contents = await inventory(skill)
                guide = next(r["path"] for r in contents if r["path"].endswith("/SKILL.md") and r["kind"] == "file")
                references = next(r["path"] for r in contents if r["path"].endswith("/references") and r["kind"] == "directory")
                reference_rows = await inventory(references)
                self.assertEqual(len(reference_rows), 1)
                self.assertEqual(reference_rows[0]["kind"], "file")
                files = ["./plugin.json", "./mcp.json", guide, reference_rows[0]["path"]]
                result = await client.call_tool("setup_inspect_plugin", {"staged_id": "ordinary", "files": files})
                self.assertFalse(result.is_error, result)
                observed = result.structured_content
                self.assertEqual(observed["status"], "accepted")
                self.assertEqual([row["path"] for row in observed["files"]], files)
                for row in observed["files"]:
                    raw = (self.plugin / row["path"]).read_bytes()
                    self.assertEqual(row["text"], raw.decode())
                    self.assertEqual(row["byte_length"], len(raw))
                    self.assertEqual(row["source_digest"], backend.digest(raw))
                self.assertEqual(len(c.plugin_inspections), 1)
                source = observed["plugin_source"]
                retained = c.plugin_source_bytes(source)
                self.assertEqual(backend.decode(retained)["inspector_sha256"], self.profile["package_inspector"]["sha256"])
                checked = await client.call_tool("setup_selected_check", {**self.stage(), "plugin_source": source})
                self.assertFalse(checked.is_error, checked)
                installed = await client.call_tool("setup_selected_install", self.selection(checked.structured_content))
                self.assertFalse(installed.is_error, installed)
                c.join()
                self.assertEqual(c.job["status"], "succeeded")
                self.assertEqual(len(c.runner.inspections), 5)
                for index, (argv, request) in enumerate(c.runner.inspections):
                    self.assertEqual(argv, [str(c.workspace / "selected-tools/package_inspector"), "agent-plugin-files"])
                    self.assertEqual(request, {"root": str(self.staging), "directory": "./ordinary",
                        **({"inventory": inventory_directories[index]} if index < 4 else {"files": files})})
                self.assertEqual(list((c.workspace / "tmp").iterdir()), [])
        finally:
            await owner.aclose()
        job = c.job
        shutil.rmtree(self.plugin)
        recovered = self.recover(c, job)
        recovery_owner = adapter.SetupAdapter(recovered)
        try:
            async with Client(adapter.create_server(recovery_owner)) as client:
                self.assertNotIn("setup_inspect_plugin", {t.name for t in (await client.list_tools()).tools})
                result = await client.call_tool("setup_plugin_source", {"job_id": job["id"]})
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content, {**observed, "job_id": job["id"]})
                self.assertEqual(recovered.runner.calls, [])
                self.assertEqual(c.plugin_source_bytes(source), retained)
                self.assertFalse(self.plugin.exists())
        finally:
            await recovery_owner.aclose()


if __name__ == "__main__":
    unittest.main()
