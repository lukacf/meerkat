"""Pure filesystem controls for the native-selected retained realm root.

No package, native owner, installer, browser or authentication process is run.
"""
from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import threading
import unittest


SOURCE = Path(__file__).resolve().parents[2]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


product = load("retained_realm_product", SOURCE / "product/prepare.py")
host = load("retained_realm_host", SOURCE / "prepare_host.py")


class RetainedRealmRootTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="retained-realm-root-")
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve(strict=True)
        self.state = self.base / "physical-state"
        self.state.mkdir(mode=0o700)
        self.alias = self.base / "selected-state-alias"
        self.alias.symlink_to(self.state, target_is_directory=True)

    def request(self, state):
        return {"realm": "work", "realm_state_root": str(state),
                "retained_installations": {"action": "list", "cursor": None}}

    def page(self, request):
        return product.retained_page(None, None, None, host, request, threading.Event())

    def test_retained_listing_resolves_trusted_state_root_alias_without_mutation(self):
        parent = self.state / ".meerkat-connect" / "work"
        parent.parent.mkdir(mode=0o700)
        parent.mkdir(mode=0o700)
        # Real private, unprepared attempts are skipped without entering an
        # owner. The next cursor proves enumeration reached the selected realm.
        for name in ("setup-first", "setup-second"):
            (parent / name).mkdir(mode=0o700)
        marker = parent / "unrelated-note"
        marker.write_bytes(b"preserve original state")
        request = self.request(self.alias)
        original = {**request, "retained_installations": dict(request["retained_installations"])}
        # This is also the existing initial preparation/direct-readback rule.
        self.assertEqual(host.directory(request["realm_state_root"]), self.state)
        self.assertEqual(product.retained_parent(host, request), parent)
        self.assertEqual(self.page(request), self.page(self.request(self.state)))
        self.assertEqual(self.page(request), {
            "kind": "retained_installation_page", "candidates": [],
            "next_cursor": {"setup": "setup-second", "workspace": "", "after_job": ""},
            "unconfirmed": False,
        })
        self.assertEqual(request, original)
        self.assertTrue(self.alias.is_symlink())
        self.assertEqual(marker.read_bytes(), b"preserve original state")
        self.assertEqual({path.name for path in parent.iterdir()},
                         {"setup-first", "setup-second", "unrelated-note"})

    def test_missing_retained_realm_is_empty_for_alias_and_physical_root(self):
        for state in (self.alias, self.state):
            with self.subTest(state=state.name):
                self.assertIsNone(product.retained_parent(host, self.request(state)))
                self.assertEqual(self.page(self.request(state)), {
                    "kind": "retained_installation_page", "candidates": [],
                    "next_cursor": None, "unconfirmed": False,
                })
        self.assertEqual(list(self.state.iterdir()), [])

    def test_symlinked_product_or_realm_descendants_remain_refused(self):
        for component in ("product", "realm"):
            with self.subTest(component=component):
                state = self.base / (component + "-state")
                state.mkdir(mode=0o700)
                alias = self.base / (component + "-alias")
                alias.symlink_to(state, target_is_directory=True)
                foreign = self.base / (component + "-foreign")
                foreign.mkdir(mode=0o700)
                product_root = state / ".meerkat-connect"
                if component == "product":
                    (foreign / "work").mkdir(mode=0o700)
                    product_root.symlink_to(foreign, target_is_directory=True)
                else:
                    product_root.mkdir(mode=0o700)
                    (product_root / "work").symlink_to(foreign, target_is_directory=True)
                for selected in (alias, state):
                    with self.assertRaisesRegex(product.ResourceError, "retained_private_directory_required"):
                        product.retained_parent(host, self.request(selected))
                self.assertTrue((product_root if component == "product" else product_root / "work").is_symlink())

    def test_nonprivate_product_or_realm_descendants_remain_refused(self):
        product_root = self.state / ".meerkat-connect"
        parent = product_root / "work"
        product_root.mkdir(mode=0o700)
        parent.mkdir(mode=0o700)
        for exposed in (product_root, parent):
            with self.subTest(exposed=exposed.name):
                exposed.chmod(0o750)
                try:
                    for selected in (self.alias, self.state):
                        with self.assertRaisesRegex(product.ResourceError, "retained_private_directory_required"):
                            product.retained_parent(host, self.request(selected))
                finally:
                    exposed.chmod(0o700)
        self.assertEqual(product.retained_parent(host, self.request(self.alias)), parent)

    def test_relative_or_missing_selected_state_root_is_refused(self):
        with self.assertRaisesRegex(product.ResourceError, "invalid_retained_realm"):
            product.retained_parent(host, self.request(Path("relative-state")))
        with self.assertRaisesRegex(host.PreparationError, "invalid_directory"):
            product.retained_parent(host, self.request(self.base / "missing-state"))
        self.assertFalse((self.base / "missing-state").exists())


if __name__ == "__main__":
    unittest.main()
