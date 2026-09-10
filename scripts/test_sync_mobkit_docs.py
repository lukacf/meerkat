#!/usr/bin/env python3
"""Unit tests for the MobKit documentation snapshot transformer."""

from __future__ import annotations

import importlib.util
import json
import subprocess
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

    def test_root_links_preserve_code_fences(self) -> None:
        for fence in ("```", "````", "~~~"):
            with self.subTest(fence=fence):
                example = f'{fence}md\n[example](/quickstart)\n{fence}\n'
                self.assertEqual(sync.rewrite_root_links(example), example)

    def test_legacy_anchor_correction_requires_an_unambiguous_existing_heading(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            docs = Path(temp)
            (docs / "concepts").mkdir()
            roster = docs / "concepts" / "roster.mdx"
            heading = "## Profiles are templates; members are declared\n"
            legacy = "/concepts/roster#profiles-are-templates-members-are-declared"
            canonical = "/concepts/roster#profiles-are-templates%3B-members-are-declared"
            roster.write_text(heading, encoding="utf-8")
            corrections = sync.released_anchor_corrections(docs)
            self.assertEqual(corrections, {legacy: canonical})
            source = (
                f"[Profiles are templates]({legacy})\n"
                f'<Card href="{legacy}" />\n'
            )
            self.assertEqual(
                sync.normalize_released_anchors(source, corrections),
                source.replace(legacy, canonical),
            )

            for text in (
                "",
                f"```md\n{heading}```\n",
                f"~~~md\n{heading}~~~\n",
                heading * 2,
                heading + "## Profiles are templates members are declared\n",
            ):
                with self.subTest(text=text):
                    roster.write_text(text, encoding="utf-8")
                    corrections = sync.released_anchor_corrections(docs)
                    self.assertEqual(corrections, {})
                    self.assertEqual(
                        sync.normalize_released_anchors(source, corrections), source
                    )
            roster.unlink()
            self.assertEqual(sync.released_anchor_corrections(docs), {})

    def test_legacy_anchor_correction_preserves_unrelated_links_and_examples(self) -> None:
        legacy = "/concepts/roster#profiles-are-templates-members-are-declared"
        corrections = {
            legacy: "/concepts/roster#profiles-are-templates%3B-members-are-declared"
        }
        source = (
            f"[external](https://example.com{legacy})\n"
            f'[external](//example.com{legacy})\n'
            "[valid](/concepts/roster#profiles-are-templates%3B-members-are-declared)\n"
            "[valid](/concepts/roster#runtime-modes)\n"
            "[unknown](/concepts/roster#unknown-fragment)\n"
            "[different page](/guides/roster#profiles-are-templates-members-are-declared)\n"
            f"Literal URL: {legacy}\n"
        )
        for fence in ("```", "````", "~~~"):
            source += f'{fence}md\n[example]({legacy})\n<Card href="{legacy}" />\n{fence}\n'
        self.assertEqual(sync.normalize_released_anchors(source, corrections), source)

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

    def test_version_stamp_names_the_documented_version_and_its_release_ref(self) -> None:
        stamp = sync.version_stamp("1.2.3", "v1.2.3", "a" * 40)
        self.assertIn("This page documents MobKit v1.2.3", stamp)
        self.assertIn("[v1.2.3](https://github.com/lukacf/meerkat-mobkit/tree/v1.2.3)", stamp)
        self.assertTrue(stamp.startswith("*") and stamp.endswith("*"))
        self.assertNotIn("\n", stamp)

    def test_version_stamp_falls_back_to_the_commit_without_a_release_ref(self) -> None:
        commit = "44f1c4ef3c6ae45079c54fac760bc703604d3f0b"
        stamp = sync.version_stamp("1.2.3", None, commit)
        self.assertIn("This page documents MobKit v1.2.3", stamp)
        self.assertIn(
            f"commit [44f1c4ef3c6a](https://github.com/lukacf/meerkat-mobkit/commit/{commit})",
            stamp,
        )

    def test_stamp_page_inserts_the_stamp_as_the_first_body_line(self) -> None:
        source = '---\ntitle: "Quickstart"\ndescription: "Boot"\nicon: "rocket"\n---\n\n## Install\n\nText.\n'
        rendered = sync.stamp_page(source, "*STAMP*", "quickstart")
        self.assertEqual(
            rendered,
            '---\ntitle: "Quickstart"\ndescription: "Boot"\nicon: "rocket"\n---\n\n*STAMP*\n\n## Install\n\nText.\n',
        )

    def test_stamp_page_requires_frontmatter(self) -> None:
        with self.assertRaisesRegex(SystemExit, "lacks frontmatter"):
            sync.stamp_page("## No frontmatter\n", "*STAMP*", "guides/x")
        with self.assertRaisesRegex(SystemExit, "unclosed frontmatter"):
            sync.stamp_page('---\ntitle: "x"\n', "*STAMP*", "guides/x")

    def test_snapshot_stamps_every_page_with_the_manifest_version(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "meerkat-mobkit"
            docs = source / "docs"
            (docs / "guides").mkdir(parents=True)
            (docs / "api").mkdir()
            (docs / "concepts").mkdir()
            (source / "Cargo.toml").write_text(
                '[workspace]\n\n[workspace.package]\nversion = "1.2.3"\n',
                encoding="utf-8",
            )
            (docs / "docs.json").write_text(
                json.dumps(
                    {
                        "navigation": {
                            "tabs": [
                                {
                                    "tab": "Documentation",
                                    "groups": [
                                        {
                                            "group": "Start",
                                            "pages": [
                                                "introduction", "guides/deploy",
                                                "quickstart", "api/rpc", "concepts/roster",
                                            ],
                                        }
                                    ],
                                }
                            ]
                        }
                    }
                ),
                encoding="utf-8",
            )
            (docs / "introduction.mdx").write_text(
                '---\ntitle: "Introduction"\ndescription: "Intro"\n---\n\nSee [deploy](/guides/deploy).\n',
                encoding="utf-8",
            )
            (docs / "guides" / "deploy.mdx").write_text(
                '---\ntitle: "Deploy"\ndescription: "Deploy"\nicon: "ship"\n---\n## Steps\n',
                encoding="utf-8",
            )
            legacy = "/concepts/roster#profiles-are-templates-members-are-declared"
            for page_id in ("quickstart", "api/rpc"):
                (docs / f"{page_id}.mdx").write_text(
                    '---\ntitle: "Page"\ndescription: "Page"\nicon: "book"\n---\n\n'
                    f"[Profiles are templates]({legacy})\n"
                    "[Runtime modes](/concepts/roster#runtime-modes)\n",
                    encoding="utf-8",
                )
            (docs / "concepts" / "roster.mdx").write_text(
                '---\ntitle: "Roster"\ndescription: "Roster"\nicon: "users"\n---\n\n'
                "## Profiles are templates; members are declared\n\n## Runtime modes\n",
                encoding="utf-8",
            )
            git = ["git", "-C", str(source)]
            subprocess.run(["git", "init", "-q", str(source)], check=True)
            subprocess.run([*git, "add", "."], check=True)
            subprocess.run(
                [
                    *git,
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "-q",
                    "-m",
                    "docs",
                ],
                check=True,
            )
            subprocess.run([*git, "tag", "v1.2.3"], check=True)

            destination = Path(temp) / "mobkit"
            sync.build_snapshot(source, destination, source_ref=None, require_clean=True)

            manifest = json.loads((destination / "_source.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["source_version"], "1.2.3")
            self.assertEqual(manifest["source_ref"], "v1.2.3")
            self.assertEqual(manifest["source_commit"], sync.git_output(source, "rev-parse", "HEAD"))
            self.assertIs(manifest["source_docs_dirty"], False)
            self.assertEqual(manifest["content_sha256"], sync.validate.tree_digest(destination))
            expected_stamp = sync.version_stamp("1.2.3", "v1.2.3", manifest["source_commit"])
            pages = sorted(destination.rglob("*.mdx"))
            self.assertEqual(len(pages), 5)
            for page in pages:
                rendered = page.read_text(encoding="utf-8")
                frontmatter_end = rendered.index("\n---\n", 4) + len("\n---\n")
                body = rendered[frontmatter_end:]
                self.assertTrue(
                    body.startswith(f"\n{expected_stamp}\n\n"),
                    f"{page.name} does not open with the version stamp:\n{body[:200]}",
                )
            introduction = (destination / "introduction.mdx").read_text(encoding="utf-8")
            self.assertIn("[deploy](/mobkit/guides/deploy)", introduction)
            self.assertIn("This page documents MobKit v1.2.3", introduction)
            self.assertIn('icon: "boxes-stacked"', introduction)
            for page_id in ("quickstart", "api/rpc"):
                rendered = (destination / f"{page_id}.mdx").read_text(encoding="utf-8")
                self.assertIn(
                    "/mobkit/concepts/roster#profiles-are-templates%3B-members-are-declared",
                    rendered,
                )
                self.assertIn("/mobkit/concepts/roster#runtime-modes", rendered)
                self.assertIn(legacy, (docs / f"{page_id}.mdx").read_text(encoding="utf-8"))
            self.assertEqual(sync.git_output(source, "status", "--porcelain"), "")

            (docs / "quickstart.mdx").write_text("dirty", encoding="utf-8")
            with self.assertRaisesRegex(SystemExit, "uncommitted changes"):
                sync.build_snapshot(
                    source, Path(temp) / "dirty", source_ref="v1.2.3", require_clean=True
                )


if __name__ == "__main__":
    unittest.main()
