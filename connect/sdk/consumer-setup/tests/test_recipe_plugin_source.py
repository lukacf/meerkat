"""Retain exact imported guidance through the existing recipe-owner boundary.

The controlled recipe owner verifies dispatch and binding only. Canonical Rust
and fresh-process SQLite acceptance cover the stored record and scope behavior.
"""
import copy
import json
import shutil
import threading
import unittest

from mcp import Client
from test_plugin_inspection import PluginRunner
from test_recipe_retention import RetentionFixture, RetentionRunner, adapter, backend


class SourceRetentionRunner(RetentionRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.inspections = []
        self.override = None
        self.inspection_block = None
        self.inspection_entered = threading.Event()

    run_compose = PluginRunner.run_compose


class SourceRetentionTests(RetentionFixture, unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        super().setUp()
        self.profile["package_inspector"] = self.tools["python"]
        self.profile["plugin_inspection"] = True
        self.write_profile()
        self.plugin = self.staging / "ordinary"
        (self.plugin / "skills" / "guide").mkdir(parents=True)
        (self.plugin / "plugin.json").write_text('{"name":"ordinary-plugin"}\n')
        (self.plugin / "skills/guide/SKILL.md").write_text("# Guide\nCount completed rows only. Preserve caf\u00e9 and line endings.\r\n", newline="")

    def controller(self, **changes):
        changes.setdefault("runner_factory", SourceRetentionRunner)
        return super().controller(**changes)

    def with_source(self):
        controller = self.controller()
        observed = controller.inspect_plugin({"staged_id": "ordinary", "files": ["./plugin.json", "./skills/guide/SKILL.md"]})
        checked = controller.selected_check({**self.stage("python"), "plugin_source": observed["plugin_source"]})
        job = controller.selected_install(self.selection(checked))["job"]
        controller.join()
        return controller, checked, job, observed

    async def test_actual_mcp_retains_frozen_source_after_staging_is_removed(self):
        controller, checked, job, observed = self.with_source()
        sidecar = controller.workspace / "selections" / checked["validation_id"] / "plugin-source.json"
        original = sidecar.read_bytes()
        shutil.rmtree(self.staging)
        bridge = adapter.SetupAdapter(controller)
        async with Client(adapter.create_server(bridge)) as client:
            result = await client.call_tool("setup_remember", self.request(job))
            self.assertFalse(result.is_error)
            value = result.structured_content
            self.assertEqual(json.loads(result.content[0].text), value)
            self.assertEqual(value["recipe"]["plugin_source_digest"], observed["plugin_source"]["inspection_sha256"])
        argv = controller.runner.save_calls[0]
        self.assertEqual(argv[argv.index("--plugin-source") + 1], str(sidecar))
        self.assertEqual(sidecar.read_bytes(), original)
        self.assertNotIn("plugin-source.json", checked["files"])
        await bridge.aclose()

    def test_changed_or_missing_source_refuses_before_save(self):
        controller, checked, job, _ = self.with_source()
        sidecar = controller.workspace / "selections" / checked["validation_id"] / "plugin-source.json"
        original = sidecar.read_bytes()
        sidecar.write_bytes(b"changed")
        self.assert_error(lambda: controller.remember(self.request(job)), "retention_source_unavailable")
        sidecar.unlink()
        self.assert_error(lambda: controller.remember(self.request(job)), "retention_source_unavailable")
        self.assertEqual(controller.runner.save_calls, [])
        sidecar.write_bytes(original)
        self.assertEqual(controller.remember(self.request(job))["status"], "retained")

    def test_owner_must_acknowledge_the_exact_source_digest(self):
        controller, _, job, _ = self.with_source()
        good = controller.remember(self.request(job))
        for replacement in (None, "0" * 64):
            changed = copy.deepcopy(good)
            if replacement is None:
                del changed["recipe"]["plugin_source_digest"]
            else:
                changed["recipe"]["plugin_source_digest"] = replacement
            controller.runner.save_override = (0, backend.encoded(changed))
            self.assert_error(lambda: controller.remember(self.request(job)), "retention_unconfirmed")


if __name__ == "__main__":
    unittest.main()
