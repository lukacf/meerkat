#!/usr/bin/env python3
"""Unit tests for the MobKit documentation snapshot transformer."""

from __future__ import annotations

import importlib.util
import json
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

    def test_release_navigation_replaces_retired_pages_at_the_product_boundary(self) -> None:
        source_config = {
            "navigation": {
                "tabs": [
                    {
                        "tab": "Documentation",
                        "groups": [
                            {
                                "group": "Getting started",
                                "pages": ["introduction", "quickstart"],
                            }
                        ],
                    }
                ]
            }
        }
        site_config = {
            "navigation": {
                "products": [
                    {"product": "Meerkat", "tabs": []},
                    {
                        "product": "MobKit",
                        "description": "preserved",
                        "tabs": [
                            {
                                "tab": "Plans",
                                "groups": [
                                    {
                                        "group": "Plans",
                                        "pages": [
                                            "mobkit/plans/storage-unification-plan"
                                        ],
                                    }
                                ],
                            }
                        ],
                    },
                ]
            }
        }

        rendered = sync.site_config_for_source(source_config, site_config)

        mobkit = rendered["navigation"]["products"][1]
        self.assertEqual(mobkit["description"], "preserved")
        self.assertEqual(
            mobkit["tabs"][0]["groups"][0]["pages"],
            ["mobkit/introduction", "mobkit/quickstart"],
        )
        self.assertNotIn("storage-unification-plan", json.dumps(rendered))
        self.assertEqual(site_config["navigation"]["products"][1]["tabs"][0]["tab"], "Plans")

    def test_release_navigation_requires_one_mobkit_product(self) -> None:
        source_config = {"navigation": {"tabs": [{"tab": "Docs", "groups": []}]}}
        with self.assertRaisesRegex(SystemExit, "exactly one MobKit product"):
            sync.site_config_for_source(
                source_config,
                {"navigation": {"products": []}},
            )


if __name__ == "__main__":
    unittest.main()
