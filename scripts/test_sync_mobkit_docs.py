#!/usr/bin/env python3
"""Unit tests for the MobKit documentation snapshot transformer."""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("sync-mobkit-docs.py")
SPEC = importlib.util.spec_from_file_location("sync_mobkit_docs", SCRIPT)
assert SPEC and SPEC.loader
sync = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sync)


class SyncMobKitDocsTests(unittest.TestCase):
    def test_namespaces_root_relative_links_and_assets(self) -> None:
        source = '<Card href="/quickstart" />\n<img src="/images/a.png" />\n'
        rendered = sync.rewrite_root_links(source)
        self.assertIn('href="/mobkit/quickstart"', rendered)
        self.assertIn('src="/mobkit/images/a.png"', rendered)

    def test_release_snapshot_normalizes_required_icons(self) -> None:
        source = '---\ntitle: "Introduction"\ndescription: "Intro"\n---\n'
        rendered = sync.ensure_page_icon(source, "introduction")
        self.assertIn('icon: "boxes-stacked"', rendered)
        self.assertEqual(sync.ensure_page_icon(rendered, "introduction"), rendered)

    def test_reads_workspace_package_version(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            cargo_toml = Path(temp) / "Cargo.toml"
            cargo_toml.write_text(
                '[workspace]\n\n[workspace.package]\nversion = "1.2.3"\n',
                encoding="utf-8",
            )
            self.assertEqual(sync.workspace_version(cargo_toml), "1.2.3")


if __name__ == "__main__":
    unittest.main()
