"""Whole immutable release selection with the real starter and archived owners.

Controlled installer and recipe programs prove path custody, not Linux noexec
mount behavior or native runtime qualification.
"""
import json
import os
from pathlib import Path
import shutil
import threading
import unittest
from unittest.mock import patch

import test_start as controls


class ImmutableReleaseTests(unittest.TestCase):
    publish_release = controls.StarterTests.publish_release
    installer = controls.StarterTests.installer
    refresh = controls.StarterTests.refresh
    product = controls.StarterTests.product
    recipe_owner_fixture = controls.StarterTests.recipe_owner_fixture

    def setUp(self):
        controls.StarterTests.setUp(self)
        def writable():
            for directory, _, _ in os.walk(self.base):
                Path(directory).chmod(0o755)
        self.addCleanup(writable)

    def immutable(self, root):
        release = root / "library" / self.root_name
        shutil.copytree(self.release, release)
        for directory, _, files in os.walk(release):
            for name in files:
                path = Path(directory) / name
                path.chmod(0o555 if path.stat().st_mode & 0o111 else 0o444)
            Path(directory).chmod(0o555)
        release.parent.chmod(0o555)
        return release

    def selected_product(self):
        product, root = self.product()
        manifest, sha = product.verify_resources(root)
        release = self.immutable(root)
        manifest["library_release"] = release.relative_to(root).as_posix()
        return product, root, manifest, sha, release

    def test_real_recipe_and_bootstrap_use_immutable_release_with_mutable_state(self):
        self.recipe_owner_fixture()
        product, root, manifest, manifest_sha, release = self.selected_product()
        state = self.base / "state"
        parent = state / ".meerkat-connect/work"
        attempt = parent / "setup-immutable"
        attempt.mkdir(parents=True, mode=0o700)
        destination = attempt / "prepared"
        before = {p.relative_to(release): p.read_bytes() for p in release.rglob("*") if p.is_file()}
        config = product.prepare(root, destination, threading.Event(), realm_state_root=str(state), realm="work")
        self.assertFalse((destination / "distribution").exists())
        self.assertTrue((destination / "client/installed.json").is_file())
        self.assertEqual(config["mcpServers"]["toolkit-setup"]["args"][6], str(release / "setup/local_session.py"))
        recipe = json.loads((destination / "config/local-recipes.json").read_bytes())
        self.assertEqual(recipe["tool"]["path"], str(release / "selected/bin/toolkit-local-recipes"))
        self.assertEqual(recipe["database"], str(parent / "recipes.sqlite"))
        selected = json.loads((destination / "config/selected-install.json").read_bytes())
        for tool in (selected["installer"], selected["validators"]["python"], selected["package_inspector"]):
            self.assertTrue(Path(tool["path"]).is_relative_to(release))
        entered = json.loads((destination / "client/entered.json").read_bytes())["argv"]
        self.assertEqual(entered[entered.index("--bundle") + 1], str(release / "consumers"))
        snapshot, retained = controls.starter.host.release_snapshot(
            str(root / manifest["archive"]), product.sha256(root / manifest["archive"]), release, "macos-arm64")
        receipt = json.loads((destination / "config/prepared.json").read_bytes())
        helper, binding, _ = product.local_projection(controls.starter.host, destination, release, retained,
                                                     receipt["files"]["mcp.json"])
        with helper.session_owner(destination / "local-session", binding):
            pass
        context = product.retained_preparation(root, manifest, manifest_sha, controls.starter.host,
                                               parent, attempt.name)
        self.assertEqual(context.release, release)
        self.assertEqual(context.snapshot, snapshot)
        descriptor = destination / "product-mcp.json"
        prepared = destination / "product-prepared.json"
        request = self.base / "round.json"
        request.write_text(json.dumps({"prepared": str(destination), "realm_state_root": str(state),
            "realm": "work", "descriptor_sha256": product.sha256(descriptor),
            "preparation_sha256": product.sha256(prepared)}))
        projection = product.local_projection
        calls = []
        def observe(*args):
            self.assertEqual(args[2], release)
            owner, binding, descriptor = projection(*args)
            owner.offer_next_round = lambda *a, **kw: calls.append((a, kw))
            return owner, binding, descriptor
        with patch.object(product, "local_projection", side_effect=observe):
            product.local_round_owner(root, request, threading.Event(), 77)
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][0][1], destination / "local-session")
        self.assertEqual(before, {p.relative_to(release): p.read_bytes() for p in release.rglob("*") if p.is_file()})
        self.assertFalse(list(release.rglob("__pycache__")))
        # A new resource identity cannot relabel a retained preparation.
        with self.assertRaisesRegex(product.ResourceError, "retained_preparation_changed"):
            product.retained_preparation(root, manifest, "b" * 64, controls.starter.host, parent, attempt.name)

    def test_bad_explicit_selection_never_falls_back_to_mutable_extraction(self):
        product, root, manifest, _, release = self.selected_product()
        selected = manifest["library_release"]
        for invalid in (None, True, [], "", str(release), "../" + selected, "library/other", selected + "/"):
            with self.subTest(selection=invalid):
                manifest["library_release"] = invalid
                with self.assertRaises((product.ResourceError, ValueError)):
                    product.prepare(root, self.destination, threading.Event())
                self.assertFalse(self.destination.exists())

    def test_writable_linked_missing_or_changed_release_refuses_before_output(self):
        product, root, manifest, _, release = self.selected_product()
        notice = release / "licenses/notice.txt"
        for mode in (0o644, 0o666, 0o755):
            with self.subTest(mode=mode):
                notice.chmod(mode)
                with self.assertRaisesRegex(product.ResourceError, "immutable_library_member"):
                    product.prepare(root, self.destination, threading.Event())
                self.assertFalse(self.destination.exists())
        notice.chmod(0o644)
        notice.write_bytes(b"changed retained library")
        notice.chmod(0o444)
        with self.assertRaisesRegex(Exception, "extracted_release_changed"):
            product.prepare(root, self.destination, threading.Event())
        self.assertFalse(self.destination.exists())
        notice.parent.chmod(0o755)
        notice.unlink(); notice.symlink_to(self.release / "licenses/notice.txt")
        notice.parent.chmod(0o555)
        with self.assertRaisesRegex(product.ResourceError, "immutable_library_member"):
            product.prepare(root, self.destination, threading.Event())
        release.parent.chmod(0o755)
        release.rename(release.with_name("other"))
        release.parent.chmod(0o555)
        with self.assertRaises(FileNotFoundError):
            product.prepare(root, self.destination, threading.Event())
        self.assertFalse(self.destination.exists())

    def test_older_starter_without_immutable_keyword_is_not_silently_used(self):
        product, root, _, _, _ = self.selected_product()
        # A previously supported starter cannot silently ignore the selection.
        path = root / "starter/start.py"
        path.write_text(path.read_text().replace("def start(args, *, stopped=None, release_dir=None):",
                                                "def start(args, *, stopped=None):"))
        with self.assertRaisesRegex(TypeError, "release_dir"):
            product.prepare(root, self.destination, threading.Event())
        self.assertFalse(self.destination.exists())

    def test_starter_rejects_writable_selected_release_before_destination(self):
        root = self.base / "resources"; root.mkdir()
        release = self.immutable(root)
        release.chmod(0o755)
        with self.assertRaisesRegex(Exception, "immutable_library_is_writable"):
            controls.starter.start(self.options, release_dir=str(release))
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()
