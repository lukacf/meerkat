#!/usr/bin/env python3
"""Exercise the documentation mirror renderer on a small fixture."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

TOOL = Path(__file__).resolve().with_name("render-docs-mirror.py")

PAGE = """---
title: "One"
description: "A page."
---

See [two](/guides/two) and [elsewhere](https://example.com/x) and <a href="/three">three</a>.

```sh
echo "[kept](/as-it-is)"
```
"""
RENDERED = PAGE.replace("](/guides/two)", "](/connect/guides/two)").replace('href="/three"', 'href="/connect/three"')
SITE_NAV = {"navigation": {"tabs": [{"tab": "Docs", "groups": [{"group": "Start", "pages": ["one", "guides/two"]}]}]}}
NAV = {"navigation": {"products": [{"product": "Core", "tabs": []}, {"product": "Connect", "tabs": []}]}}


def dump(value):
    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


class RenderDocsMirrorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        root = Path(self.temp.name)
        self.site, self.mirror, self.nav = root / "site", root / "mirror", root / "docs.json"
        (self.site / "guides").mkdir(parents=True)
        (self.site / "one.mdx").write_text(PAGE, encoding="utf-8")
        (self.site / "guides" / "two.mdx").write_text("# Two\n\n[one](/one)\n", encoding="utf-8")
        (self.site / "docs.json").write_text(dump(SITE_NAV), encoding="utf-8")
        self.nav.write_text(dump(NAV), encoding="utf-8")
        self.crates = root / "crates"
        for name in ("first", "second"):
            (self.crates / name).mkdir(parents=True)
            (self.crates / name / "Cargo.toml").write_text(
                f'[package]\nname = "{name}"\nversion = "1.2.3"\n\n[dependencies]\nother = {{ version = "9.9.9" }}\n', encoding="utf-8")

    def tearDown(self):
        self.temp.cleanup()

    def run_tool(self, *arguments):
        return subprocess.run(
            [sys.executable, "-B", str(TOOL), "--site", str(self.site), "--mirror", str(self.mirror), "--nav", str(self.nav),
             "--crates", str(self.crates), *arguments],
            stdin=subprocess.DEVNULL, capture_output=True, text=True, check=False,
        )

    def test_the_manifest_states_the_crates_version_and_no_source_commit(self):
        self.assertEqual(self.run_tool().returncode, 0)
        path = self.mirror / "_source.json"
        manifest = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(manifest["connect_source_version"], "1.2.3")
        stale = {"connect_source_commit": "0" * 40, "connect_source_tree": "1" * 40, **manifest, "connect_source_version": "0.0.1"}
        path.write_text(json.dumps(stale, indent=2) + "\n", encoding="utf-8")
        done = self.run_tool("--check")
        self.assertEqual(done.returncode, 1)
        self.assertIn("differs _source.json", done.stdout)
        self.assertEqual(self.run_tool().returncode, 0)
        after = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(after["connect_source_version"], "1.2.3")
        self.assertNotIn("connect_source_commit", after)
        self.assertNotIn("connect_source_tree", after)
        self.assertEqual(self.run_tool("--check").returncode, 0)

    def test_crates_that_state_different_versions_are_refused(self):
        (self.crates / "second" / "Cargo.toml").write_text('[package]\nname = "second"\nversion = "1.2.4"\n', encoding="utf-8")
        done = self.run_tool()
        self.assertEqual(done.returncode, 2)
        self.assertIn("different versions", done.stderr)
        self.assertFalse((self.mirror / "_source.json").exists())

    def test_renders_links_outside_fences_and_records_the_manifest(self):
        done = self.run_tool()
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual((self.mirror / "one.mdx").read_text(encoding="utf-8"), RENDERED)
        manifest = json.loads((self.mirror / "_source.json").read_text(encoding="utf-8"))
        self.assertEqual((manifest["pages"], manifest["namespaced_links"], manifest["fenced_blocks_preserved"]), (2, 3, 1))
        self.assertEqual([entry["path"] for entry in manifest["files"]], ["guides/two.mdx", "one.mdx"])
        self.assertRegex(manifest["source_docs_tree"], r"^[0-9a-f]{40}$")
        navigation = json.loads(self.nav.read_text(encoding="utf-8"))
        connect = navigation["navigation"]["products"][1]["tabs"][0]["groups"][0]["pages"]
        self.assertEqual(connect, ["connect/one", "connect/guides/two"])
        self.assertEqual(navigation["navigation"]["products"][0], {"product": "Core", "tabs": []})

    def test_check_passes_after_a_regeneration_and_changes_nothing(self):
        self.assertEqual(self.run_tool().returncode, 0)
        before = {path: path.read_bytes() for path in list(self.mirror.rglob("*")) + [self.nav] if path.is_file()}
        done = self.run_tool("--check")
        self.assertEqual(done.returncode, 0, done.stdout)
        self.assertIn("same 2, differ 0; other differences 0", done.stdout)
        self.assertEqual(before, {path: path.read_bytes() for path in before})

    def test_check_fails_on_a_changed_copy_and_writes_nothing(self):
        self.assertEqual(self.run_tool().returncode, 0)
        copy = self.mirror / "one.mdx"
        copy.write_text(RENDERED + "edited by hand\n", encoding="utf-8")
        done = self.run_tool("--check")
        self.assertEqual(done.returncode, 1)
        self.assertIn("differs one.mdx", done.stdout)
        self.assertEqual(copy.read_text(encoding="utf-8"), RENDERED + "edited by hand\n")

    def test_check_fails_on_a_missing_copy_a_stale_copy_and_a_changed_source(self):
        self.assertEqual(self.run_tool().returncode, 0)
        (self.mirror / "guides" / "two.mdx").unlink()
        self.assertIn("missing guides/two.mdx", self.run_tool("--check").stdout)
        self.assertEqual(self.run_tool().returncode, 0)
        (self.mirror / "gone.mdx").write_text("# Gone\n", encoding="utf-8")
        done = self.run_tool("--check")
        self.assertEqual(done.returncode, 1)
        self.assertIn("stale gone.mdx", done.stdout)
        self.assertEqual(self.run_tool().returncode, 0)
        self.assertFalse((self.mirror / "gone.mdx").exists())
        (self.site / "one.mdx").write_text(PAGE + "\nMore.\n", encoding="utf-8")
        done = self.run_tool("--check")
        self.assertEqual(done.returncode, 1)
        self.assertIn("differs one.mdx", done.stdout)
        self.assertIn("differs _source.json", done.stdout)

    def test_the_manifest_keeps_its_other_fields(self):
        self.assertEqual(self.run_tool().returncode, 0)
        path = self.mirror / "_source.json"
        manifest = json.loads(path.read_text(encoding="utf-8"))
        ordered = {"canonical_url": "https://example.com/connect", **manifest}
        path.write_text(json.dumps(ordered, indent=2) + "\n", encoding="utf-8")
        self.assertEqual(self.run_tool("--check").returncode, 0)
        (self.site / "guides" / "two.mdx").write_text("# Two\n", encoding="utf-8")
        self.assertEqual(self.run_tool().returncode, 0)
        after = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(after["canonical_url"], "https://example.com/connect")
        self.assertNotEqual(after["source_docs_tree"], manifest["source_docs_tree"])


if __name__ == "__main__":
    unittest.main()
