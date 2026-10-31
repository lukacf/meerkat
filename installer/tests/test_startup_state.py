"""Original state selection controls; no native child or server import."""
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from launchkit import packaged_runtime, startup_state


class SavedStateStartupTests(unittest.TestCase):
    def setUp(self):
        self.directory = TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.base = Path(self.directory.name).resolve()
        self.original = self.base / "original"
        self.original.mkdir()
        self.default = self.base / ".studio"
        self.enterContext(patch.object(startup_state, "_selected", None))
        self.enterContext(patch.dict(os.environ, {}, clear=True))

    def test_selected_original_keeps_existing_databases_without_default(self):
        history = self.original / "guided-setups.sqlite3"
        history.write_bytes(b"retained fixture bytes")
        before = history.stat()
        startup_state.configure_saved_state_root(self.original)
        self.assertEqual(startup_state.saved_state_root(self.default), self.original)
        self.assertEqual((history.stat().st_ino, history.read_bytes()),
                         (before.st_ino, b"retained fixture bytes"))
        self.assertFalse(self.default.exists())

    def test_missing_original_refuses_before_package_or_server_and_creates_nothing(self):
        with patch.object(packaged_runtime, "open_selection") as opening, \
             patch.object(packaged_runtime.importlib, "import_module") as importing:
            with self.assertRaises(FileNotFoundError):
                packaged_runtime.create_app(saved_state_root=self.base / "missing")
        opening.assert_not_called()
        importing.assert_not_called()
        self.assertIsNone(startup_state._selected)
        self.assertEqual(list(self.base.iterdir()), [self.original])

    def test_directory_replacement_cannot_become_a_fresh_store(self):
        startup_state.configure_saved_state_root(self.original)
        self.original.rename(self.base / "retained")
        self.original.mkdir()
        with self.assertRaisesRegex(ValueError, "changed"):
            startup_state.saved_state_root(self.default)
        self.assertFalse(self.default.exists())

    def test_link_and_different_selection_are_refused(self):
        link = self.base / "link"
        link.symlink_to(self.original, target_is_directory=True)
        with self.assertRaises(ValueError):
            startup_state.configure_saved_state_root(link)
        startup_state.configure_saved_state_root(self.original)
        startup_state.configure_saved_state_root(self.original)
        other = self.base / "other"
        other.mkdir()
        with self.assertRaisesRegex(ValueError, "cannot be replaced"):
            startup_state.configure_saved_state_root(other)

    def test_environment_conflict_and_late_drift_cannot_redirect_state(self):
        for name in ("META_FORGE_HOME", "MOBKIT_LAUNCH_HOME"):
            with self.subTest(name=name), patch.dict(os.environ, {name: str(self.default)}):
                with self.assertRaisesRegex(ValueError, "environment"):
                    startup_state.configure_saved_state_root(self.original)
        startup_state.configure_saved_state_root(self.original)
        with patch.dict(os.environ, {"META_FORGE_HOME": str(self.default)}):
            with self.assertRaisesRegex(ValueError, "environment"):
                startup_state.saved_state_root(self.default)
        self.assertFalse(self.default.exists())

    def test_selection_after_server_import_is_refused(self):
        with patch.dict(sys.modules, {"launchkit.server": SimpleNamespace()}):
            with self.assertRaisesRegex(RuntimeError, "before importing"):
                startup_state.configure_saved_state_root(self.original)
        self.assertIsNone(startup_state._selected)

    def test_factory_joins_original_state_before_package_and_full_app_import(self):
        calls = []
        target, app = object(), object()

        def opening():
            calls.append(("package", startup_state.saved_state_root(self.default)))
            return target

        def importing(name):
            calls.append((name, startup_state.saved_state_root(self.default)))
            return SimpleNamespace(app=app, select_local_application_runtime=lambda selected, **kw:
                                   calls.append(("select", selected, kw)))

        with patch.object(packaged_runtime, "open_selection", opening), \
             patch.object(packaged_runtime, "selected_ordinary_gateway", return_value=None), \
             patch.object(packaged_runtime, "_application_model_custody", None), \
             patch.object(packaged_runtime.importlib, "import_module", importing):
            self.assertIs(packaged_runtime.create_app(saved_state_root=self.original), app)
        self.assertEqual(calls, [("package", self.original), ("launchkit.server", self.original),
                                 ("select", target, {})])

    def test_ordinary_environment_precedence_and_default_are_unchanged(self):
        self.assertEqual(startup_state.saved_state_root(self.default), self.default)
        with patch.dict(os.environ, {"MOBKIT_LAUNCH_HOME": str(self.original)}):
            self.assertEqual(startup_state.saved_state_root(self.default), self.original)
            with patch.dict(os.environ, {"META_FORGE_HOME": str(self.default)}):
                self.assertEqual(startup_state.saved_state_root(self.original), self.default)
        self.assertFalse(self.default.exists())


if __name__ == "__main__":
    unittest.main()
