"""Canonical archive graph checks shared by acquisition and selected installation."""
import copy
import unittest
from unittest.mock import patch

import test_selected_node_install as fixtures


MODULE = fixtures.MODULE


class NpmClosureTests(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.SelectedNodeTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def test_graph_without_entrypoint_preserves_exact_nested_archives_and_inputs(self):
        files = self.fixture.files
        original = copy.deepcopy(files)
        graph = MODULE.npm_archive_closure(files)
        self.assertEqual(graph["component_path"], "node_modules/@outside/server")
        self.assertEqual(graph["rows"]["node_modules/nested"][1]["version"], "1.0.0")
        self.assertEqual(graph["rows"]["node_modules/@outside/server/node_modules/nested"][1]["version"], "2.0.0")
        self.assertNotIn("entrypoint", graph)
        self.assertEqual(files, original)
        selected = MODULE.selected_npm_closure(files, self.fixture.plan)
        self.assertEqual(selected, {**graph, "entrypoint": "dist/main.js"})

    def test_acquisition_does_not_choose_or_require_a_javascript_entrypoint(self):
        MODULE.npm_archive_closure(self.fixture.files)
        for entry in ("missing.js", "package.json", "../escape.js"):
            with self.subTest(entry=entry), self.assertRaises(ValueError):
                MODULE.selected_npm_closure(self.fixture.files, {"entrypoint": {"executable": entry}})

    def test_graph_rejects_lock_archive_disagreement_before_entrypoint(self):
        lock = copy.deepcopy(self.fixture.lock)
        lock["packages"]["node_modules/nested"]["version"] = "9.0.0"
        files = {**self.fixture.files, "closure/package-lock": MODULE.encoded(lock)}
        for validator in (lambda: MODULE.npm_archive_closure(files),
                          lambda: MODULE.selected_npm_closure(files, self.fixture.plan)):
            with self.assertRaisesRegex(ValueError, "name/version differs"):
                validator()

    def test_acquisition_uses_same_realized_graph_budget(self):
        with patch.object(MODULE, "MAX_NPM_MEMBERS", 7):
            with self.assertRaisesRegex(ValueError, "excessive npm closure"):
                MODULE.npm_archive_closure(self.fixture.files)


if __name__ == "__main__":
    unittest.main()
