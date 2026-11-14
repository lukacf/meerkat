#!/usr/bin/env python3
"""Tests of scripts/check_inrepo_version_pins.py on synthetic repositories."""

from __future__ import annotations

import io
import json
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

import check_inrepo_version_pins as pins  # noqa: E402

HEX = "a" * 64


def products(core="", meta="meta", connect="connect"):
    return {"schema_version": 1, "products": [
        {"name": "core", "root": core, "tag_prefix": "v"},
        {"name": "meta", "root": meta, "tag_prefix": "meta-v"},
        {"name": "connect", "root": connect, "tag_prefix": "connect-v"},
    ]}


class Repo:
    """A synthetic tree of three products at version 1.4.2, staged in a git index."""

    def __init__(self, root: Path, version="1.4.2"):
        self.root = root
        self.version = version
        self.config = {"schema_version": 1, "former_names": {"pypi:old-meta": "pypi:demo-meta"},
                       "release_tags": {"tool-v": "pypi:demo-tool"},
                       "release_assets": {"demo-gateway": "product:meta", "demo-cli": "product:core"},
                       "products": {"forge": "pypi:demo-tool"}, "allow": []}
        self.write("Cargo.toml", f"""\
            [workspace]
            members = ["crates/*", "meta/crates/*", "connect/crates/*"]

            [workspace.package]
            version = "{version}"

            [workspace.dependencies]
            demo-core = {{ version = "={version}", path = "crates/demo-core" }}
            demo-meta-rs = {{ version = "={version}", path = "meta/crates/demo-meta-rs" }}
            demo-old = {{ package = "demo-meta-rs", version = "={version}", path = "meta/crates/demo-meta-rs" }}
            """)
        self.write("crates/demo-core/Cargo.toml", '[package]\nname = "demo-core"\nversion.workspace = true\n')
        self.write("meta/crates/demo-meta-rs/Cargo.toml", f'[package]\nname = "demo-meta-rs"\nversion = "{version}"\n')
        self.write("connect/crates/demo-connect/Cargo.toml",
                   f'[package]\nname = "demo-connect"\nversion = "{version}"\n')
        self.write("ci/products.json", json.dumps(products()))
        self.write("sdk/package.json", json.dumps({"name": "@rkat/demo", "version": version}))
        self.write("meta/sdk/package.json", json.dumps({"name": "@rkat/demo-meta", "version": version}))
        self.write("meta/sdk/pyproject.toml", f'[project]\nname = "demo-meta"\nversion = "{version}"\n')
        self.write("meta/tool/pyproject.toml", '[project]\nname = "demo-tool"\nversion = "0.3.1"\n')
        self.save_config()

    def write(self, path: str, text: str) -> Path:
        full = self.root / path
        full.parent.mkdir(parents=True, exist_ok=True)
        full.write_text(textwrap.dedent(text), encoding="utf-8")
        return full

    def save_config(self) -> None:
        self.write(pins.CONFIG, json.dumps(self.config, indent=2))

    def allow(self, **entry) -> None:
        self.config["allow"].append(entry)
        self.save_config()

    def run(self, write=False):
        subprocess.run(["git", "-C", str(self.root), "add", "-A"], check=True)
        out, err = io.StringIO(), io.StringIO()
        code = pins.run(self.root, self.root / pins.CONFIG, write, out=out, err=err)
        return code, out.getvalue(), err.getvalue()


class GuardTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="inrepo pins ")
        root = Path(self.directory.name)
        subprocess.run(["git", "-c", "init.defaultBranch=main", "init", "-q", str(root)], check=True)
        self.repo = Repo(root)

    def tearDown(self):
        self.directory.cleanup()

    def assertClean(self):
        code, out, err = self.repo.run()
        self.assertEqual(code, 0, err)
        return out

    def assertFindings(self, *needles, code=1):
        result, _, err = self.repo.run()
        self.assertEqual(result, code, err)
        for needle in needles:
            self.assertIn(needle, err)
        return err

    def assertNoWarning(self):
        """The check passes and reports nothing at all."""
        code, out, err = self.repo.run()
        self.assertEqual(code, 0, err)
        self.assertEqual(err, "")
        return out

    def assertWarning(self, *needles):
        """The check passes, and everything it reports is a warning line."""
        code, out, err = self.repo.run()
        self.assertEqual(code, 0, err)
        self.assertIn("every in-repo pin names its version", out)
        lines = err.splitlines()
        self.assertTrue(lines, "the check printed no warning")
        for line in lines:
            self.assertTrue(line.startswith("check_inrepo_version_pins: WARNING: "), line)
        for needle in needles:
            self.assertIn(needle, err)
        return err


class InventoryTests(GuardTest):
    def test_versions_come_from_the_tree(self):
        subprocess.run(["git", "-C", str(self.repo.root), "add", "-A"], check=True)
        inventory = pins.Inventory(self.repo.root, self.repo.config, pins.tracked_files(self.repo.root))
        self.assertEqual(inventory.get("cargo", "demo-core").version, "1.4.2")
        self.assertEqual(inventory.get("cargo", "demo-old").version, "1.4.2")
        self.assertEqual(inventory.get("npm", "@rkat/demo-meta").version, "1.4.2")
        self.assertEqual(inventory.get("pypi", "Demo_Meta").version, "1.4.2")
        self.assertEqual(inventory.get("pypi", "old-meta").version, "1.4.2")
        self.assertEqual(inventory.expected(("product", "meta")), ("meta", "1.4.2"))
        self.assertEqual(inventory.expected(("product", "forge")), ("demo-tool", "0.3.1"))
        self.assertEqual(inventory.tag("meta-v1.4.1"), (("product", "meta"), "1.4.1"))
        self.assertEqual(inventory.tag("tool-v0.3.0"), (("pypi", "demo-tool"), "0.3.0"))
        self.assertIsNone(inventory.tag("mirror-v1.0.0"))

    def test_a_product_with_two_crate_versions_is_an_error(self):
        self.repo.write("meta/crates/demo-extra/Cargo.toml", '[package]\nname = "demo-extra"\nversion = "1.4.1"\n')
        self.assertFindings("product meta has no single crate version", code=2)

    def test_an_unknown_configured_target_is_an_error(self):
        self.repo.config["release_assets"]["demo-x"] = "pypi:not-here"
        self.repo.save_config()
        self.assertFindings("not an in-repo package", code=2)


class ShapeTests(GuardTest):
    def test_a_clean_tree_passes(self):
        self.assertIn("every in-repo pin names its version", self.assertClean())

    def test_npm_manifest_pins(self):
        self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "devDependencies": {"@rkat/demo": "1.4.1", "@rkat/demo-meta": "file:../../meta/sdk", "other": "1.0.0"},
            "peerDependencies": {"@rkat/demo": ">=1.4.1 <2.0.0", "@rkat/demo-meta": "*"}}, indent=2))
        err = self.assertFindings("devDependencies @rkat/demo@1.4.1: pins @rkat/demo 1.4.1, below its version 1.4.2",
                                  "peerDependencies @rkat/demo@>=1.4.1 <2.0.0",
                                  "peerDependencies @rkat/demo-meta@*: admits @rkat/demo-meta below 1.4.2")
        self.assertNotIn("other", err)
        self.assertNotIn("file:", err)

    def test_ranges_at_the_version_in_the_tree_pass(self):
        self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "devDependencies": {"@rkat/demo": "file:../../sdk"},
            "peerDependencies": {"@rkat/demo": ">=1.4.2 <2.0.0", "@rkat/demo-meta": "^1.4.2 || ^2.0.0"}}))
        self.assertClean()

    def test_range_admission(self):
        # A range admits a version when one of its alternatives has no bound that shuts it
        # out: no ceiling at or below it, no strict floor at it, no floor above it.
        for spec, admits in (
                (">=1.4.2", True), ("^1.4.2 || ^2.0.0", True), ("1.4.2", True), ("=1.4.2", True), ("==1.4.2", True),
                ("~=1.4.2", True), ("~1.4.2", True), (">=1.4.2, <2", True), (">=1.4.2,<2,!=1.4.5", True),
                ("1.4.2 - 2.0.0", True), ("<=1.4.2", True), (">1.4.1", True), ("npm:@rkat/demo2@^1.4.2", True),
                (">=1.4.2 <1.4.2 || >=1.4.2 <2.0.0", True), ("==1.4.*", True),
                (">1.4.2", False), (">=1.4.3", False), ("1.4.3", False), ("=1.4.3", False), ("==1.4.3", False),
                ("~=1.4.3", False), ("^1.4.3", False), ("~1.4.3", False), (">=1.4.3, <2", False),
                ("1.4.3 - 2.0.0", False), ("1.4.0 - 1.4.1", False), ("<1.4.2", False), (">=1.4.2 <1.4.2", False),
                (">=1.4.2 <1.4.2 || >=1.4.3", False), (">=1.4.2,!=1.4.2", False), (">=1.4.0,!=1.4.*", False),
                ("npm:@rkat/demo2@^1.4.3", False)):
            with self.subTest(spec=spec):
                self.assertEqual(pins.range_admits(spec, "1.4.2"), admits)

    def test_a_lower_bound_that_shuts_out_the_version_here_is_refused(self):
        # Every in-repo consumer admits the version this tree builds. A strict floor at
        # it, a floor above it and an exact pin above it shut it out, in every ecosystem.
        self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "dependencies": {"@rkat/demo": "1.4.3"},
            "devDependencies": {"@rkat/demo": "^1.4.3"},
            "peerDependencies": {"@rkat/demo": ">1.4.2", "@rkat/demo-meta": ">=1.4.3 <2.0.0"},
            "optionalDependencies": {"@rkat/demo-meta": ">=1.4.2 <1.4.2 || >=1.4.3"}}, indent=2))
        self.repo.write("connect/py/pyproject.toml", '[project]\nname = "demo-py"\nversion = "1.4.2"\n'
                        '[project.optional-dependencies]\nstrict = ["demo-meta>1.4.2"]\n'
                        'floor = ["demo-meta>=1.4.3,<2"]\n'
                        'exact = ["demo-meta==1.4.3"]\ncompatible = ["demo-meta~=1.4.3"]\n'
                        'unequal = ["demo-meta>=1.4.2,!=1.4.2"]\n')
        self.repo.write("docs/install.md", '```toml\ndemo-core = "=1.4.3"\ndemo-core = ">1.4.2"\n'
                        'demo-meta-rs = { version = ">=1.4.3, <2", features = ["x"] }\ndemo-core = "1.4.3"\n```\n'
                        'npm install @rkat/demo@1.4.3 @rkat/demo-meta@^1.4.3\n')
        self.assertFindings(
            "dependencies @rkat/demo@1.4.3: pins @rkat/demo 1.4.3, above its version 1.4.2 in this tree",
            "devDependencies @rkat/demo@^1.4.3: pins @rkat/demo 1.4.3, above its version 1.4.2",
            "peerDependencies @rkat/demo@>1.4.2: admits no @rkat/demo 1.4.2, its version in this tree",
            "peerDependencies @rkat/demo-meta@>=1.4.3 <2.0.0: pins @rkat/demo-meta 1.4.3, above its version 1.4.2",
            "optionalDependencies @rkat/demo-meta@>=1.4.2 <1.4.2 || >=1.4.3: admits no @rkat/demo-meta 1.4.2",
            "pyproject.toml:5: requirement demo-meta>1.4.2: admits no demo-meta 1.4.2, its version in this tree",
            "pyproject.toml:6: requirement demo-meta>=1.4.3,<2: pins demo-meta 1.4.3, above its version 1.4.2",
            "pyproject.toml:7: requirement demo-meta==1.4.3: pins demo-meta 1.4.3, above its version 1.4.2",
            "pyproject.toml:8: requirement demo-meta~=1.4.3: pins demo-meta 1.4.3, above its version 1.4.2",
            "pyproject.toml:9: requirement demo-meta>=1.4.2,!=1.4.2: admits no demo-meta 1.4.2",
            'docs/install.md:2: cargo demo-core = "=1.4.3": pins demo-core 1.4.3, above its version 1.4.2',
            'docs/install.md:3: cargo demo-core = ">1.4.2": admits no demo-core 1.4.2, its version in this tree',
            'docs/install.md:4: cargo demo-meta-rs = { version = ">=1.4.3, <2", features = ["x"] }: pins '
            "demo-meta-rs 1.4.3, above its version 1.4.2",
            'docs/install.md:5: cargo demo-core = "1.4.3": pins demo-core 1.4.3, above its version 1.4.2',
            "docs/install.md:7: npm-spec @rkat/demo@1.4.3: pins @rkat/demo 1.4.3, above its version 1.4.2",
            "docs/install.md:7: npm-spec @rkat/demo-meta@^1.4.3: pins @rkat/demo-meta 1.4.3, above its version 1.4.2")

    def test_bounds_that_admit_the_version_here_pass(self):
        self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "dependencies": {"@rkat/demo": "1.4.2"},
            "devDependencies": {"@rkat/demo": "~1.4.2"},
            "peerDependencies": {"@rkat/demo": "1.4.2 - 2.0.0", "@rkat/demo-meta": ">=1.4.2 <2.0.0 || >=3.0.0"}}))
        self.repo.write("connect/py/pyproject.toml", '[project]\nname = "demo-py"\nversion = "1.4.2"\n'
                        '[project.optional-dependencies]\nexact = ["demo-meta==1.4.2"]\n'
                        'compatible = ["demo-meta~=1.4.2"]\nrange = ["demo-meta>=1.4.2,<2,!=1.4.5"]\n')
        self.repo.write("docs/install.md", '```toml\ndemo-core = "=1.4.2"\ndemo-core = "1.4.2"\n'
                        'demo-meta-rs = { version = ">=1.4.2, <2" }\ndemo-core = "^1.4.2"\n```\n'
                        'npm install @rkat/demo@1.4.2 @rkat/demo-meta@^1.4.2\n')
        self.assertClean()

    def test_lock_entries(self):
        self.repo.write("connect/sdk/package-lock.json", json.dumps({"lockfileVersion": 3, "packages": {
            "": {"devDependencies": {"@rkat/demo": "1.4.1"}},
            "node_modules/@rkat/demo": {"version": "1.4.1", "resolved": "https://registry.example/demo.tgz"},
            "node_modules/@rkat/demo-meta": {"resolved": "../../meta/sdk", "link": True}}}, indent=2))
        err = self.assertFindings("locked @rkat/demo@1.4.1 (node_modules/@rkat/demo)")
        self.assertNotIn("demo-meta", err)

    def test_requirements(self):
        self.repo.write("meta/tool/requirements.txt", f"demo-meta==1.4.1 --hash=sha256:{HEX}\nother==1.0\n")
        self.repo.write("connect/py/pyproject.toml",
                        '[project]\nname = "demo-py"\nversion = "1.4.2"\n'
                        '[project.optional-dependencies]\nhost = ["demo_meta[x]>=1.4.0,<2", "old-meta==0.8.4"]\n')
        self.assertFindings("requirements.txt:1: requirement demo-meta==1.4.1",
                            "requirement demo_meta[x]>=1.4.0,<2: pins demo-meta 1.4.0",
                            "requirement old-meta==0.8.4: pins old-meta 0.8.4")

    def test_cargo_lines_outside_the_workspace_table(self):
        self.repo.write("docs/install.md", '```toml\ndemo-core = "=1.4.0"\ndemo-meta-rs = { version = "1.4", '
                        'features = ["x"] }\ndemo-core = { path = "../crates/demo-core" }\n```\n')
        self.assertFindings('docs/install.md:2: cargo demo-core = "=1.4.0"',
                            "docs/install.md:3: cargo demo-meta-rs = { version = \"1.4\"")

    def test_release_urls_and_selections(self):
        self.repo.write("docs/get.md", textwrap.dedent("""\
            https://github.com/lukacf/meerkat/releases/download/connect-v1.4.1/demo-connect.tar.gz
            https://github.com/lukacf/meerkat/releases/tag/v1.4.2
            https://github.com/lukacf/meerkat/releases/download/mirror-v1.0.0/tool.jar
            https://github.com/lukacf/meerkat-meta/releases/download/v{VERSION}
            https://github.com/lukacf/meerkat-meta/commit/0123456789abcdef
            """))
        self.repo.write("release/inputs.json", json.dumps({"core": {"repository": "lukacf/meerkat", "tag": "v1.3.0"}}))
        err = self.assertFindings("docs/get.md:1: release lukacf/meerkat release connect-v1.4.1: pins connect 1.4.1",
                                  "docs/get.md:4: lukacf/meerkat-meta release v{VERSION}: a release of the retired",
                                  "release/inputs.json:1: release lukacf/meerkat v1.3.0: pins core 1.3.0")
        self.assertNotIn("docs/get.md:2", err)
        self.assertNotIn("mirror", err)
        self.assertNotIn("meerkat-meta/commit", err)
        self.assertNotIn("docs/get.md:5", err)

    def test_asset_names_and_tracked_artifacts(self):
        self.repo.write("docs/assets.md", f"demo-gateway-1.4.1-x86_64.tar.gz {HEX}\ndemo_meta-1.4.2-py3-none-any.whl\n")
        self.repo.write("vendor/demo_meta-1.3.0-py3-none-any.whl", "wheel")
        self.repo.write("docs/tarball.md", "rkat-demo-meta-1.4.0.tgz\n")
        err = self.assertFindings("docs/assets.md:1: asset demo-gateway-1.4.1: pins meta 1.4.1",
                                  "vendor/demo_meta-1.3.0-py3-none-any.whl: tracked-artifact demo_meta-1.3.0",
                                  "docs/tarball.md:1: asset rkat-demo-meta-1.4.0: pins @rkat/demo-meta 1.4.0")
        self.assertNotIn("demo_meta-1.4.2", err)

    def test_product_constants(self):
        self.repo.write("meta/tool/pins.py", 'META_VERSION = "1.4.1"\nCORE_REQUIREMENT = "=1.4.0"\n'
                        'NODE_VERSION = "22.0.0"\nCONTRACT_VERSION = "0.1.0"\nFORGE_VERSION = "0.3.0"\n')
        self.repo.write("meta/tool/notes.md", 'META_VERSION = "1.4.1"\n')
        err = self.assertFindings('pins.py:1: constant META_VERSION = "1.4.1": pins meta 1.4.1',
                                  'pins.py:2: constant CORE_REQUIREMENT = "=1.4.0": pins core 1.4.0',
                                  'pins.py:5: constant FORGE_VERSION = "0.3.0": pins demo-tool 0.3.0')
        self.assertNotIn("NODE_VERSION", err)
        self.assertNotIn("CONTRACT_VERSION", err)
        self.assertNotIn("notes.md", err)

    def test_history_comments_are_not_pins(self):
        self.repo.write("crates/demo-core/src/lib.rs", "// demo-core 1.3.0 refused this; 1.4.0 accepts it.\n")
        self.repo.write("docs/history.md", "Since demo-meta 1.2.0 the gateway restarts.\n")
        self.assertClean()

    def test_a_known_issue_link_to_a_release_page_is_recorded_history(self):
        # A page that tells the reader of a later version what an earlier release's page
        # says keeps pointing at that page, whatever the version of the tree.
        page = "https://github.com/lukacf/meerkat/releases/tag"
        self.repo.write("docs/notes.md", textwrap.dedent(f"""\
            Fixed here; see the [known issue on the 1.4.1 release page]({page}/v1.4.1) for earlier releases.
            Details: [known issue on the Meta 1.4.0 release page]({page}/meta-v1.4.0).
            Both the [known issue on the Connect 1.3.9 release page]({page}/connect-v1.3.9) and the
            [known issue on the Core 1.2.0 release page]({page}/v1.2.0) describe it.
            """))
        self.assertClean()

    def test_only_the_known_issue_form_is_recorded_history(self):
        # The link text and the target together make the form: other words, another
        # target, or a release URL outside such a link are pins as before.
        releases = "https://github.com/lukacf/meerkat/releases"
        known = "known issue on the 1.4.1 release page"
        lines = [
            f"Install [Demo Core 1.4.1]({releases}/tag/v1.4.1) first.",
            f"Download the archives from the [release page]({releases}/tag/connect-v1.4.1).",
            f"See the [release notes]({releases}/tag/meta-v1.4.1).",
            f"See the [1.4.1 release page]({releases}/tag/v1.4.1).",
            f"See the [known issues on the 1.4.1 release page]({releases}/tag/v1.4.1).",
            f"See the [{known}]({releases}/tag/v1.4.0).",
            f"See the [{known}]({releases}/tag/meta-v1.4.1).",
            f"See the [known issue on the Meta 1.4.1 release page]({releases}/tag/v1.4.1).",
            f"See the [known issue on the meta 1.4.1 release page]({releases}/tag/meta-v1.4.1).",
            f"See the [{known}]({releases}/tag/v1.4.1/assets).",
            f"See the [{known}]({releases}/download/v1.4.1/checksums.sha256).",
            f"See the {known}: {releases}/tag/v1.4.1",
            f"See the [known issue on the Tool 0.3.0 release page]({releases}/tag/tool-v0.3.0).",
            "See the [known issue on the 1.0.0 release page]"
            "(https://github.com/lukacf/meerkat-meta/releases/tag/v1.0.0).",
        ]
        self.repo.write("docs/get.md", "\n".join(lines) + "\n")
        err = self.assertFindings()
        for number in range(1, len(lines) + 1):
            self.assertEqual(err.count(f"docs/get.md:{number}: "), 1, f"line {number}: {lines[number - 1]}\n{err}")
        self.assertIn(f"{len(lines)} pin(s) below", err)

    def test_a_known_issue_link_hides_nothing_beside_it(self):
        # The link alone is history: an install command, a download URL and an archive
        # name on the same line or the next one are still read.
        releases = "https://github.com/lukacf/meerkat/releases"
        link = f"[known issue on the 1.4.1 release page]({releases}/tag/v1.4.1)"
        self.repo.write("docs/upgrade.md", textwrap.dedent(f"""\
            See the {link}, then run `pip install demo-meta==1.4.1`.
            See the {link}, then run `npm install @rkat/demo@1.4.1`.
            See the {link}, then fetch {releases}/download/v1.4.1/checksums.sha256
            See the {link} and unpack demo-cli-1.4.1-linux-x86_64.tar.gz
            See the {link}.
            """))
        self.repo.write("docs/install.md", 'pip install demo-meta==1.4.0\ndemo-core = "=1.4.1"\n')
        err = self.assertFindings("docs/upgrade.md:1: requirement demo-meta==1.4.1: pins demo-meta 1.4.1",
                                  "docs/upgrade.md:2: npm-spec @rkat/demo@1.4.1: pins @rkat/demo 1.4.1",
                                  "docs/upgrade.md:3: release lukacf/meerkat release v1.4.1: pins core 1.4.1",
                                  "docs/upgrade.md:4: asset demo-cli-1.4.1: pins core 1.4.1",
                                  "docs/install.md:1: requirement demo-meta==1.4.0",
                                  'docs/install.md:2: cargo demo-core = "=1.4.1"')
        self.assertNotIn("docs/upgrade.md:5", err)
        self.assertIn("6 pin(s) below", err)


class AllowlistTests(GuardTest):
    def setUp(self):
        super().setUp()
        self.repo.write("docs/old.md", "demo-meta==1.4.1\nhttps://github.com/lukacf/meerkat-connect/releases/download/v1.0.0/x\n")

    def test_a_whole_file_entry_skips_the_file(self):
        self.repo.allow(path="docs/*.md", reason="Recorded history of earlier releases.")
        self.assertClean()

    def test_a_match_entry_covers_only_its_pins(self):
        self.repo.allow(path="docs/old.md", match="^demo-meta==1\\.4\\.1$", reason="The notes of release 1.4.1.")
        err = self.assertFindings("lukacf/meerkat-connect release v1.0.0")
        self.assertNotIn("demo-meta==1.4.1", err)

    def test_an_entry_that_covers_nothing_is_stale(self):
        self.repo.allow(path="docs/old.md", reason="Recorded history of earlier releases.")
        self.repo.allow(path="docs/gone.md", reason="A file that no longer exists here.")
        self.repo.allow(path="docs/old.md", match="demo-meta==1\\.0\\.0", reason="A pin that was already moved.",
                        removed_by="the change that moves the pin to the version in the tree")
        err = self.assertFindings("allow entry 1 docs/gone.md covers nothing", "allow entry 2 docs/old.md")
        self.assertIn("2 stale allow entries", err)

    def test_a_temporary_entry_is_counted(self):
        self.repo.allow(path="docs/old.md", reason="Pins another change removes.",
                        removed_by="the change that moves the pins to the version in the tree")
        self.assertIn("1 allow entries, 1 temporary", self.assertClean())

    def test_entries_need_a_reason_and_known_keys(self):
        self.repo.allow(path="docs/old.md", reason="history")
        self.assertFindings("needs its reason", code=2)
        self.repo.config["allow"] = [{"path": "docs/old.md", "reason": "Recorded history of releases.", "until": "x"}]
        self.repo.save_config()
        self.assertFindings("unknown keys", code=2)

    def test_a_context_entry_covers_only_the_pins_in_that_text(self):
        # Two lines pin the same release; the entry names the words that stand beside one of them.
        url = "https://github.com/lukacf/meerkat/releases/tag/v1.4.1"
        self.repo.write("docs/pair.md", f"Use [Demo Core 1.4.1]({url}), the release the\n"
                        "add-on 1.4.1 was made with.\n\n"
                        f"Install [Demo Core 1.4.1]({url}) first.\n")
        self.repo.allow(path="docs/pair.md", match=r"^lukacf/meerkat release v1\.4\.1$",
                        context=r"\[Demo Core 1\.4\.1\]\([^)]*/tag/v1\.4\.1\), "
                                r"the release the add-on 1\.4\.1 was made with",
                        reason="The page of the add-on names the release it was made with.")
        err = self.assertFindings("docs/pair.md:4: release lukacf/meerkat release v1.4.1", "docs/old.md:1:")
        self.assertNotIn("docs/pair.md:1:", err)
        self.assertNotIn("covers nothing", err)
        # An entry of context alone is not a whole-file entry: the file is still read.
        self.repo.config["allow"][-1].pop("match")
        self.repo.save_config()
        err = self.assertFindings("docs/pair.md:4: release lukacf/meerkat release v1.4.1")
        self.assertNotIn("docs/pair.md:1:", err)
        self.repo.allow(path="docs/pair.md", context="(", reason="Not a regular expression at all.")
        self.assertFindings(code=2)

    def test_a_context_entry_covers_only_the_pin_inside_its_text(self):
        # The entry names a sentence. A pin that only stands near that sentence (on its
        # line, or on the line before it) is not the sentence's pin and is still reported.
        releases = "https://github.com/lukacf/meerkat/releases"
        held = f"Use [Demo Core 1.4.1]({releases}/tag/v1.4.1), the release the add-on 1.4.1 was made with"
        entry = {"path": "docs/pair.md", "match": r"^lukacf/meerkat release v1\.4\.1$",
                 "context": r"\[Demo Core 1\.4\.1\]\([^)]*/tag/v1\.4\.1\), "
                            r"the release the add-on 1\.4\.1 was made with",
                 "reason": "The page of the add-on names the release it was made with."}
        for label, page, line in (
                ("a download address on the sentence's line",
                 f"{held}; fetch {releases}/download/v1.4.1/sums.txt\n", 1),
                ("a second link on the sentence's line", f"{held}, or see the [page]({releases}/tag/v1.4.1).\n", 1),
                ("a link on the line before the sentence", f"Get it from {releases}/tag/v1.4.1 first.\n{held}.\n", 1)):
            with self.subTest(label):
                self.repo.config["allow"] = [entry]
                self.repo.save_config()
                self.repo.write("docs/pair.md", page)
                err = self.assertFindings(f"docs/pair.md:{line}: release lukacf/meerkat release v1.4.1")
                self.assertEqual(err.count("docs/pair.md:"), 1, err)
                self.assertNotIn("covers nothing", err)
        # The sentence alone is covered, and so is one whose words begin on the line before the pin.
        self.repo.write("docs/pair.md", f"{held}.\n")
        self.repo.write("docs/old.md", "")
        self.assertClean()
        self.repo.config["allow"] = [dict(entry, match=r"^demo-meta==1\.4\.1$",
                                          context=r"the add-on requires `demo-meta==1\.4\.1`\. Nothing newer")]
        self.repo.save_config()
        self.repo.write("docs/pair.md",
                        "Until then the add-on\nrequires `demo-meta==1.4.1`. Nothing\nnewer works with it.\n")
        self.assertClean()

    def test_a_context_entry_covers_no_pin_read_from_a_structure(self):
        # A dependency read from a manifest's structure has no place in a line of prose.
        self.repo.write("docs/old.md", "")
        self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "dependencies": {"@rkat/demo": "1.4.1"}}, indent=2) + "\n")
        self.repo.allow(path="connect/sdk/package.json", context="@rkat/demo",
                        reason="A context names prose, not a manifest.")
        err = self.assertFindings("connect/sdk/package.json:5: dependencies @rkat/demo@1.4.1")
        self.assertIn("covers nothing", err)


class HoldTests(GuardTest):
    """An entry with `at` holds its pins for one release of the package they pin.

    It waits while that package is below the version, covers its pins at the version, and
    is stale after it: the next bump stops on the pins and on the entry, and the hold is
    decided again.
    """

    URL = "https://github.com/lukacf/meerkat/releases/tag/v1.4.2"
    PLAIN = f"Install [Demo Core 1.4.2]({URL})\nfor the agent.\n"
    HELD = f"Install [Demo Core 1.4.2]({URL}), the Core release\nConnect 1.4.2 was released with, for the agent.\n"
    FINDING = "connect/README.md:1: release lukacf/meerkat release v1.4.2: pins core 1.4.2, below its version"
    STALE = "allow entry 0 connect/README.md (^lukacf/meerkat release v1\\.4\\.2$) [at 1.4.3] covers nothing; delete it"

    def tree(self, version, page, **changes):
        """The tree at `version` with the page and the hold for the release 1.4.3."""
        self.repo = Repo(self.repo.root, version=version)
        self.repo.write("connect/README.md", page)
        entry = {"path": "connect/README.md", "match": r"^lukacf/meerkat release v1\.4\.2$",
                 "context": r"\[Demo Core 1\.4\.2\]\([^)]*/tag/v1\.4\.2\), "
                            r"the Core release Connect 1\.4\.2 was released with\b",
                 "at": "1.4.3",
                 "reason": "The page describes the Connect release that was made with Core 1.4.2."}
        entry.update(changes)
        self.repo.allow(**{name: value for name, value in entry.items() if value is not None})

    def test_a_hold_waits_below_its_version(self):
        # The entry lands before the bump it is for: the pin is no finding yet, with or
        # without the words, and the entry is not stale.
        for page in (self.PLAIN, self.HELD):
            with self.subTest(page=page):
                self.tree("1.4.2", page)
                self.assertIn("1 allow entries", self.assertClean())

    def test_a_hold_covers_its_pin_at_its_version(self):
        self.tree("1.4.3", self.HELD)
        self.assertClean()

    def test_a_hold_needs_the_words_beside_the_link(self):
        self.tree("1.4.3", self.PLAIN)
        err = self.assertFindings(f"{self.FINDING} 1.4.3", self.STALE)
        self.assertIn("1 pin(s) below", err)
        self.assertIn("1 stale allow entry.", err)

    def test_a_hold_needs_the_link_the_words_name(self):
        # The words still say 1.4.2 and the link moved on: nothing is pinned below the
        # tree, and the entry that holds the old link covers nothing.
        self.tree("1.4.3", self.HELD.replace("releases/tag/v1.4.2", "releases/tag/v1.4.3"))
        err = self.assertFindings(self.STALE)
        self.assertIn("0 pin(s) below", err)

    def test_a_hold_ends_with_its_version(self):
        self.tree("1.4.4", self.HELD)
        err = self.assertFindings(f"{self.FINDING} 1.4.4", self.STALE)
        self.assertIn("1 pin(s) below", err)
        self.assertIn("1 stale allow entry.", err)

    def test_a_hold_that_waits_on_no_pin_is_stale(self):
        self.tree("1.4.2", "Install Demo Core first.\n")
        self.assertFindings(self.STALE)

    def test_a_hold_follows_the_package_its_pin_names(self):
        # Meta moves past the hold's version while Core stays below it: the hold still
        # waits on its Core pin, and it covers no pin on Meta.
        self.tree("1.4.2", self.HELD + "See https://github.com/lukacf/meerkat/releases/tag/meta-v1.4.2\n",
                  match=r"release (meta-)?v1\.4\.2$", context=None)
        self.repo.write("meta/crates/demo-meta-rs/Cargo.toml", '[package]\nname = "demo-meta-rs"\nversion = "1.4.4"\n')
        self.repo.write("meta/sdk/package.json", json.dumps({"name": "@rkat/demo-meta", "version": "1.4.4"}))
        self.repo.write("meta/sdk/pyproject.toml", '[project]\nname = "demo-meta"\nversion = "1.4.4"\n')
        err = self.assertFindings("connect/README.md:3: release lukacf/meerkat release meta-v1.4.2: pins meta 1.4.2")
        self.assertIn("1 pin(s) below", err)
        self.assertIn("0 stale allow entries", err)

    def test_a_hold_on_a_requirement_reads_the_sentence_in_its_paragraph(self):
        # The words that make the sentence a statement about a published release stand
        # lines before the pin: the context is read in the pin's paragraph.
        page = ("Until Connect 1.4.3 is published, use Connect 1.4.2. An environment\n"
                "with the `host` extra of Connect 1.4.2's `demo-kit` cannot\n"
                "take `demo-meta` 1.4.3 until Connect 1.4.3 is published: the extra\n"
                "requires `demo-meta==1.4.2`. In that pairing:\n\n- nothing else changes.\n")
        hold = {"match": r"^demo-meta==1\.4\.2$",
                "context": r"Connect 1\.4\.2's `demo-kit` cannot take `demo-meta` 1\.4\.3 until Connect 1\.4\.3 is "
                           r"published: the extra requires `demo-meta==1\.4\.2`\."}
        stale = "allow entry 0 connect/README.md (^demo-meta==1\\.4\\.2$) [at 1.4.3] covers nothing; delete it"
        self.tree("1.4.2", page, **hold)
        self.assertClean()
        self.tree("1.4.3", page, **hold)
        self.assertClean()
        # A bare install command on the same page is not the sentence.
        self.tree("1.4.3", page + "\nRun `pip install demo-meta==1.4.2`.\n", **hold)
        err = self.assertFindings("connect/README.md:8: requirement demo-meta==1.4.2: pins demo-meta 1.4.2")
        self.assertIn("1 pin(s) below", err)
        self.assertIn("0 stale allow entries", err)
        # The same sentence with another pin is not the held one.
        self.tree("1.4.3", page.replace("==1.4.2", "==1.4.1"), **hold)
        err = self.assertFindings("connect/README.md:4: requirement demo-meta==1.4.1: pins demo-meta 1.4.1", stale)
        self.assertIn("1 pin(s) below", err)
        # Other words around the pin are not the held sentence.
        self.tree("1.4.3", page.replace("the extra\nrequires", "the extra\nneeds"), **hold)
        self.assertFindings("connect/README.md:4: requirement demo-meta==1.4.2: pins demo-meta 1.4.2", stale)
        # The next version ends the hold.
        self.tree("1.4.4", page, **hold)
        err = self.assertFindings("connect/README.md:4: requirement demo-meta==1.4.2: pins demo-meta 1.4.2", stale)
        self.assertIn("1 pin(s) below", err)
        self.assertIn("1 stale allow entry.", err)

    def test_a_hold_names_a_version_and_the_pins_it_holds(self):
        for changes, needle in (({"at": "next"}, "at must be a version"),
                                ({"at": 143}, "at must be a version"),
                                ({"match": None, "context": None}, "at needs a match or a context"),
                                ({"key": "^files/"}, "at takes no key")):
            with self.subTest(changes=changes):
                self.tree("1.4.2", self.HELD, **changes)
                self.assertFindings(needle, code=2)

    def test_write_leaves_a_held_pin_in_place(self):
        manifest = '[project]\nname = "demo-py"\nversion = "1.4.2"\ndependencies = ["demo-meta==1.4.2"]\n'
        self.tree("1.4.3", self.HELD)
        self.repo.write("connect/py/pyproject.toml", manifest)
        self.repo.allow(path="connect/py/pyproject.toml", match=r"^demo-meta==1\.4\.2$", at="1.4.3",
                        reason="The package requires the Meta release it was built with.")
        code, out, err = self.repo.run(write=True)
        self.assertEqual(code, 0, err)
        self.assertEqual((self.repo.root / "connect/py/pyproject.toml").read_text(), manifest)
        self.assertNotIn("moved", out)


class KeyEntryTests(GuardTest):
    """In a JSON file an entry with a key covers the pins of the values under the key paths it matches."""

    URL = "https://github.com/lukacf/meerkat/releases/download/meta-v1.4.1/demo-gateway-1.4.1-x86_64.tar.gz"

    def setUp(self):
        super().setUp()
        # Two rows of one file pin the same release: line 6 under guide/, line 9 under product/.
        self.repo.write("meta/release/inputs.json", json.dumps({"targets": [{"files": {
            "guide/gateway": {"url": self.URL}, "product/gateway": {"url": self.URL}}}]}, indent=2) + "\n")

    def test_key_paths_name_nested_keys_and_array_items(self):
        text = '{\n  "a": [\n    {"b": 1},\n    "c"\n  ],\n  "d": {\n    "e/f": true\n  }\n}\n'
        self.assertEqual(pins.json_value_lines(text), {1: "", 2: "a", 3: "a/0", 4: "a/1", 6: "d", 7: "d/e/f"})
        for broken in ('{"a": }', '{"a": 1} x', '{"a" 1}'):
            with self.assertRaises(ValueError):
                pins.json_value_lines(broken)

    def test_a_key_entry_covers_the_values_under_its_keys(self):
        self.repo.allow(path="meta/release/*.json", key="^targets/[0-9]+/files/product/",
                        reason="The product rows of the release inputs.")
        err = self.assertFindings("meta/release/inputs.json:6: release lukacf/meerkat release meta-v1.4.1",
                                  "meta/release/inputs.json:6: asset demo-gateway-1.4.1")
        self.assertNotIn("inputs.json:9:", err)
        self.assertNotIn("covers nothing", err)

    def test_a_key_entry_with_a_match_covers_the_pins_both_select(self):
        self.repo.allow(path="meta/release/*.json", key="files/product/", match="^demo-gateway-",
                        reason="The product gateway archive of the release inputs.")
        err = self.assertFindings("inputs.json:6: release lukacf/meerkat release meta-v1.4.1",
                                  "inputs.json:6: asset demo-gateway-1.4.1",
                                  "inputs.json:9: release lukacf/meerkat release meta-v1.4.1")
        self.assertNotIn("inputs.json:9: asset", err)
        self.repo.config["allow"][0]["match"] = "^nothing$"
        self.repo.save_config()
        self.assertFindings("inputs.json:9: asset demo-gateway-1.4.1", "allow entry 0 meta/release/*.json (^nothing$) "
                            "[key files/product/] covers nothing")

    def test_a_key_entry_covers_no_file_but_json(self):
        self.repo.write("meta/release/notes.md", f"{self.URL}\n")
        self.repo.allow(path="meta/release/*", key="files/", reason="The rows of the release inputs.")
        err = self.assertFindings("meta/release/notes.md:1: release lukacf/meerkat release meta-v1.4.1")
        self.assertNotIn("inputs.json", err)

    def test_a_key_entry_needs_its_file_to_be_json(self):
        self.repo.write("meta/release/broken.json", f'{{"url": "{self.URL}",}}\n')
        self.repo.allow(path="meta/release/*.json", key="files/", reason="The rows of the release inputs.")
        self.assertFindings("meta/release/broken.json cannot be read as JSON for an entry with a key", code=2)


class WriteTests(GuardTest):
    def test_write_moves_manifest_pins_only(self):
        manifest = self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "peerDependencies": {"@rkat/demo": ">=1.4.1 <2.0.0"}}, indent=2) + "\n")
        project = self.repo.write("connect/py/pyproject.toml", '[project]\nname = "demo-py"\nversion = "1.4.2"\n'
                                  '[project.optional-dependencies]\nhost = ["demo-meta==1.4.1"]\n')
        self.repo.write("meta/tool/pins.py", 'META_VERSION = "1.4.1"\n')
        self.repo.write("docs/install.md", 'demo-meta==1.4.0\ndemo-core = "=1.4.0"\n')
        self.repo.write("meta/tool/requirements.txt", f"demo-meta==1.4.1 --hash=sha256:{HEX}\n")
        code, out, err = self.repo.run(write=True)
        self.assertEqual(code, 1, err)
        self.assertIn('"@rkat/demo": ">=1.4.2 <2.0.0"', manifest.read_text())
        self.assertIn('host = ["demo-meta==1.4.2"]', project.read_text())
        self.assertIn("moved in-repo pins in connect/sdk/package.json", out)
        self.assertIn("moved in-repo pins in connect/py/pyproject.toml", out)
        # Code, prose and a requirement held by its hash stay findings for a person or a stamper.
        self.assertEqual((self.repo.root / "meta/tool/pins.py").read_text(), 'META_VERSION = "1.4.1"\n')
        self.assertEqual((self.repo.root / "docs/install.md").read_text(), 'demo-meta==1.4.0\ndemo-core = "=1.4.0"\n')
        self.assertIn('constant META_VERSION = "1.4.1"', err)
        self.assertIn("docs/install.md:1: requirement demo-meta==1.4.0", err)
        self.assertIn("requirements.txt:1: requirement demo-meta==1.4.1", err)
        self.assertNotIn("package.json", err)
        self.assertNotIn("pyproject.toml", err)

    def test_write_leaves_allowed_pins_in_place(self):
        # An allowed pin (an upgrade fixture, or a pin another change removes) is not
        # this check's to move: --write moves only the pins it would report.
        project = self.repo.write("meta/tool/pyproject.toml", '[project]\nname = "demo-tool"\nversion = "0.3.1"\n'
                                  'dependencies = ["demo-meta==1.4.0"]\n[project.optional-dependencies]\n'
                                  'test = ["old-meta==0.8.4", "demo-meta==1.4.1"]\n')
        self.repo.allow(path="meta/tool/pyproject.toml", match="^demo-meta==1\\.4\\.0$",
                        reason="The release another change moves.",
                        removed_by="the change that runs the tool on the version in the tree")
        self.repo.allow(path="meta/tool/pyproject.toml", match="^old-meta==",
                        reason="The former name an upgrade test installs.")
        code, out, err = self.repo.run(write=True)
        self.assertEqual(code, 0, err)
        self.assertIn("moved in-repo pins in meta/tool/pyproject.toml", out)
        self.assertEqual(project.read_text(), '[project]\nname = "demo-tool"\nversion = "0.3.1"\n'
                         'dependencies = ["demo-meta==1.4.0"]\n[project.optional-dependencies]\n'
                         'test = ["old-meta==0.8.4", "demo-meta==1.4.2"]\n')

    def test_a_version_bump_with_write_leaves_the_check_clean(self):
        # The release bumps move the packages' own versions, then run --write: every
        # plain-text pin on the bumped packages follows in the same commit.
        self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "devDependencies": {"@rkat/demo": "file:../../sdk"},
            "peerDependencies": {"@rkat/demo": ">=1.4.2 <2.0.0", "@rkat/demo-meta": ">=1.4.2 <2.0.0"}}, indent=2))
        self.repo.write("connect/py/pyproject.toml", '[project]\nname = "demo-py"\nversion = "1.4.2"\n'
                        '[project.optional-dependencies]\nhost = ["demo-meta==1.4.2"]\n')
        self.assertClean()
        self.repo.write("sdk/package.json", json.dumps({"name": "@rkat/demo", "version": "1.4.3"}))
        self.repo.write("meta/sdk/package.json", json.dumps({"name": "@rkat/demo-meta", "version": "1.4.3"}))
        self.repo.write("meta/sdk/pyproject.toml", '[project]\nname = "demo-meta"\nversion = "1.4.3"\n')
        self.assertFindings("pins @rkat/demo 1.4.2, below its version 1.4.3", "pins demo-meta 1.4.2")
        code, _, err = self.repo.run(write=True)
        self.assertEqual(code, 0, err)
        self.assertIn("demo-meta==1.4.3", (self.repo.root / "connect/py/pyproject.toml").read_text())

    def test_a_paired_bump_to_the_next_minor_on_the_zero_line_moves_ceilings(self):
        # On the 0.x line a minor release may break, so a range ends below the next minor.
        # A paired bump to 0.10.0 moves each range's floor and ceiling together: the check
        # is green after the bump with nothing left to edit.
        directory = tempfile.TemporaryDirectory(prefix="inrepo pins zero line ")
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        subprocess.run(["git", "-c", "init.defaultBranch=main", "init", "-q", str(root)], check=True)
        repo = Repo(root, version="0.9.11")
        peers = {"@rkat/demo": ">=0.9.11 <0.10.0", "@rkat/demo-meta": ">=0.9.11 <0.10.0"}
        manifest = repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "0.9.11",
            "devDependencies": {"@rkat/demo": "file:../../sdk"}, "peerDependencies": peers}, indent=2) + "\n")
        lock = repo.write("connect/sdk/package-lock.json", json.dumps({"lockfileVersion": 3, "packages": {"": {
            "name": "@rkat/demo-connect", "version": "0.9.11", "peerDependencies": peers}}}, indent=2) + "\n")
        project = repo.write("connect/py/pyproject.toml", '[project]\nname = "demo-py"\nversion = "0.9.11"\n'
                             '[project.optional-dependencies]\nhost = ["demo-meta>=0.9.11,<0.10"]\n')
        code, _, err = repo.run()
        self.assertEqual(code, 0, err)
        # The paired bump: every product's own version moves to 0.10.0.
        Repo(root, version="0.10.0")
        for path in (manifest, lock, project):
            path.write_text(path.read_text().replace('"version": "0.9.11"', '"version": "0.10.0"')
                            .replace('version = "0.9.11"', 'version = "0.10.0"'))
        code, _, err = repo.run(write=True)
        self.assertEqual(code, 0, err)
        for path in (manifest, lock):
            self.assertIn('"@rkat/demo": ">=0.10.0 <0.11.0"', path.read_text())
            self.assertIn('"@rkat/demo-meta": ">=0.10.0 <0.11.0"', path.read_text())
        self.assertIn('host = ["demo-meta>=0.10.0,<0.11.0"]', project.read_text())
        code, _, err = repo.run()
        self.assertEqual(code, 0, err)

    def test_a_major_bump_moves_ceilings_to_the_next_major(self):
        manifest = self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "peerDependencies": {"@rkat/demo": ">=1.4.2 <2.0.0"}}, indent=2) + "\n")
        self.assertClean()
        Repo(self.repo.root, version="2.0.0")
        code, _, err = self.repo.run(write=True)
        self.assertEqual(code, 0, err)
        self.assertIn('"@rkat/demo": ">=2.0.0 <3.0.0"', manifest.read_text())

    def test_a_range_that_admits_no_version_here_is_refused_and_moved(self):
        manifest = self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "peerDependencies": {"@rkat/demo": ">=1.4.2 <1.4.2"}}, indent=2) + "\n")
        self.assertFindings("peerDependencies @rkat/demo@>=1.4.2 <1.4.2: admits no @rkat/demo 1.4.2, its version in "
                            "this tree")
        code, _, err = self.repo.run(write=True)
        self.assertEqual(code, 0, err)
        self.assertIn('"@rkat/demo": ">=1.4.2 <2.0.0"', manifest.read_text())

    def test_write_moves_a_lower_bound_that_shuts_out_the_version_here(self):
        # A floor or an exact pin above the version in the tree moves down to it, and a
        # strict floor at it becomes inclusive, as a floor below it moves up.
        manifest = self.repo.write("connect/sdk/package.json", json.dumps({
            "name": "@rkat/demo-connect", "version": "1.4.2",
            "dependencies": {"@rkat/demo": "1.4.3"},
            "peerDependencies": {"@rkat/demo": ">1.4.2", "@rkat/demo-meta": ">=1.4.3 <2.0.0"},
            "optionalDependencies": {"@rkat/demo-meta": ">1.4.3"}}, indent=2) + "\n")
        project = self.repo.write("connect/py/pyproject.toml", '[project]\nname = "demo-py"\nversion = "1.4.2"\n'
                                  '[project.optional-dependencies]\nstrict = ["demo-meta>1.4.2"]\n'
                                  'exact = ["demo-meta==1.4.3"]\n')
        cargo = self.repo.write("examples/demo/Cargo.toml", '[package]\nname = "demo-example"\nversion = "0.0.0"\n'
                                '[dependencies]\ndemo-core = "=1.4.3"\ndemo-meta-rs = { version = ">1.4.2" }\n')
        code, out, err = self.repo.run(write=True)
        self.assertEqual(code, 0, err)
        for path in ("connect/sdk/package.json", "connect/py/pyproject.toml", "examples/demo/Cargo.toml"):
            self.assertIn(f"moved in-repo pins in {path}", out)
        moved = manifest.read_text()
        self.assertIn('"@rkat/demo": "1.4.2"', moved)
        self.assertIn('"@rkat/demo": ">=1.4.2"', moved)
        self.assertIn('"@rkat/demo-meta": ">=1.4.2 <2.0.0"', moved)
        self.assertIn('"@rkat/demo-meta": ">=1.4.2"', moved)
        self.assertIn('strict = ["demo-meta>=1.4.2"]\nexact = ["demo-meta==1.4.2"]\n', project.read_text())
        self.assertIn('demo-core = "=1.4.2"\ndemo-meta-rs = { version = ">=1.4.2" }\n', cargo.read_text())
        self.assertClean()


class PublishedInputTests(GuardTest):
    """Published release inputs record one published release of a product.

    They all name that release, it is never newer than the tree, and one older
    than the tree is a warning: such inputs move only after a publication, so
    a version bump must not fail on them.
    """

    def setUp(self):
        super().setUp()
        self.repo.config["published_release_inputs"] = {
            "reason": "Published assets pinned by digest move after their release.",
            "paths": ["meta/tool/release/**"]}
        self.repo.save_config()

    def inputs(self, version, path="meta/tool/release/inputs.json"):
        url = (f"https://github.com/lukacf/meerkat/releases/download/meta-v{version}/"
               f"demo-gateway-{version}-x86_64.tar.gz")
        self.repo.write(path, json.dumps({"gateway": {"url": url, "sha256": HEX}}, indent=2) + "\n")

    def runtime(self, version, path="meta/tool/release/runtime.py"):
        """A version constant among the inputs: the release they select."""
        self.repo.config["version_constants"] = {path: {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.repo.write(path, f'VERSION = "{version}"\nWHEEL_SHA256 = "{HEX}"\n')

    def bump_meta(self, version):
        self.repo.write("meta/crates/demo-meta-rs/Cargo.toml", f'[package]\nname = "demo-meta-rs"\nversion = "{version}"\n')
        self.repo.write("meta/sdk/package.json", json.dumps({"name": "@rkat/demo-meta", "version": version}))
        self.repo.write("meta/sdk/pyproject.toml", f'[project]\nname = "demo-meta"\nversion = "{version}"\n')

    def test_the_version_in_the_tree_passes(self):
        self.inputs("1.4.2")
        self.runtime("1.4.2")
        self.assertIn("1 published release input paths", self.assertNoWarning())

    def test_an_older_release_is_a_warning_and_the_check_passes(self):
        # One release back, two back, and a release far behind: none fails the check.
        for version in ("1.4.1", "1.4.0", "1.2.0"):
            with self.subTest(version=version):
                self.inputs(version)
                self.runtime(version)
                self.assertWarning(f"WARNING: 3 published release input pin(s) name meta {version}, the release the "
                                   "inputs select (the version constant at meta/tool/release/runtime.py:1), older "
                                   "than 1.4.2 in this tree")

    def test_an_input_that_names_another_release_than_the_selected_one_is_refused(self):
        # The version constant selects 1.4.1; a file that names any other release, the
        # tree's own version included, is a refresh that moved one file and not the rest.
        self.runtime("1.4.1")
        for version in ("1.4.2", "1.4.0"):
            with self.subTest(version=version):
                self.inputs(version)
                err = self.assertFindings(
                    f"meta/tool/release/inputs.json:3: release lukacf/meerkat release meta-v{version}: pins meta "
                    f"{version}, not 1.4.1, the release of meta the published release inputs select (the version "
                    "constant at meta/tool/release/runtime.py:1)",
                    f"meta/tool/release/inputs.json:3: asset demo-gateway-{version}: pins meta {version}, not 1.4.1, "
                    "the release of meta the published release inputs select")
                self.assertNotIn("runtime.py:1: constant", err)
        self.inputs("1.4.1")
        self.assertWarning("WARNING: 3 published release input pin(s) name meta 1.4.1")

    def test_inputs_without_a_version_constant_agree_on_the_newest_release_they_name(self):
        self.inputs("1.4.1", path="meta/tool/release/a.json")
        self.inputs("1.4.0", path="meta/tool/release/b.json")
        err = self.assertFindings(
            "meta/tool/release/b.json:3: asset demo-gateway-1.4.0: pins meta 1.4.0, not 1.4.1, the release of meta "
            "the published release inputs select (the newest release they name, at meta/tool/release/a.json:3)")
        self.assertNotIn("a.json:3: asset", err)
        self.inputs("1.4.1", path="meta/tool/release/b.json")
        self.assertWarning("WARNING: 4 published release input pin(s) name meta 1.4.1, the release the inputs "
                           "select (the newest release they name, at meta/tool/release/a.json:3), older than 1.4.2")

    def test_a_version_newer_than_the_tree_is_refused(self):
        self.inputs("1.4.3")
        self.assertFindings("asset demo-gateway-1.4.3: pins meta 1.4.3, above its version 1.4.2 in this tree")
        # Also when every input agrees on it: no selection makes an unpublished release a published one.
        self.runtime("1.4.3")
        err = self.assertFindings(
            'constant VERSION = "1.4.3": pins demo-meta 1.4.3, above its version 1.4.2 in this tree',
            "asset demo-gateway-1.4.3: pins meta 1.4.3, above its version 1.4.2 in this tree")
        self.assertNotIn("WARNING", err)

    def test_the_release_before_it_outside_the_inputs_is_refused(self):
        self.inputs("1.4.1", path="meta/tool/other/inputs.json")
        self.assertFindings("asset demo-gateway-1.4.1: pins meta 1.4.1, below its version 1.4.2 in this tree")

    def test_a_pin_beside_the_inputs_follows_the_ordinary_rule(self):
        # The class is the configured paths and matches, nothing wider: a pin in another file
        # of the same directory, and a pin on another line of a file listed with a match,
        # are refused below the version in the tree, also while the inputs lag behind it.
        self.repo.config["published_release_inputs"]["paths"] = [
            {"path": "meta/tool/launch/runtime.py", "match": "^VERSION = "}, "meta/tool/launch/catalog_*.json"]
        self.runtime("1.4.1", path="meta/tool/launch/runtime.py")
        self.repo.write("meta/tool/launch/catalog_1_4_1.json", json.dumps({"requires": "demo-meta==1.4.1"}))
        beside = self.repo.write("meta/tool/launch/other.json", json.dumps({"requires": "demo-meta==1.4.1"}))
        self.assertFindings("meta/tool/launch/other.json:1: requirement demo-meta==1.4.1: pins demo-meta 1.4.1, "
                            "below its version 1.4.2 in this tree\n")
        beside.unlink()
        self.repo.write("meta/tool/launch/runtime.py",
                        f'VERSION = "1.4.1"\nWHEEL_SHA256 = "{HEX}"\nCORE_REQUIREMENT = "=1.4.1"\n')
        self.assertFindings('meta/tool/launch/runtime.py:3: constant CORE_REQUIREMENT = "=1.4.1": pins core '
                            '1.4.1, below its version 1.4.2 in this tree\n')

    def test_a_requirement_counts_with_its_package_product(self):
        self.inputs("1.4.1")
        catalog = self.repo.write("meta/tool/release/catalog.json", json.dumps({"requires": "demo-meta==1.4.1"}))
        self.assertWarning("WARNING: 3 published release input pin(s) name meta 1.4.1")
        catalog.write_text(json.dumps({"requires": "demo-meta==1.4.0"}), encoding="utf-8")
        self.assertFindings("requirement demo-meta==1.4.0: pins demo-meta 1.4.0, not 1.4.1, the release of meta the "
                            "published release inputs select")

    def test_a_product_of_its_own_selects_its_own_release(self):
        # demo-tool is the forge product inside meta/: its inputs name its release, not Meta's.
        self.inputs("1.4.1")
        self.repo.write("meta/tool/release/tool.json", json.dumps({"requires": "demo-tool==0.3.0"}))
        self.assertWarning("WARNING: 2 published release input pin(s) name meta 1.4.1",
                           "WARNING: 1 published release input pin(s) name forge 0.3.0",
                           "older than 0.3.1 in this tree")

    def test_an_input_that_names_no_single_release_is_refused(self):
        self.inputs("1.4.2")
        self.repo.write("meta/tool/release/bundle/package.json", json.dumps(
            {"name": "@rkat/demo-bundle", "peerDependencies": {"@rkat/demo-meta": "*"}}, indent=2))
        self.assertFindings("peerDependencies @rkat/demo-meta@*: pins @rkat/demo-meta an open range")

    def test_an_allowed_pin_is_left_out_of_the_judgment(self):
        # An allow entry covers an input that names another release, with its reason, as before:
        # that pin neither disagrees with the selection nor counts in the warning.
        self.inputs("1.4.1")
        self.inputs("1.4.0", path="meta/tool/release/product.json")
        self.repo.allow(path="meta/tool/release/**", match=r"1\.4\.0", key="^gateway/",
                        reason="A product row that another change moves.", removed_by="the change that moves it")
        err = self.assertWarning("WARNING: 2 published release input pin(s) name meta 1.4.1")
        self.assertNotIn("covers nothing", err)
        self.inputs("1.4.1", path="meta/tool/release/product.json")
        self.assertFindings("meta/tool/release/** (1\\.4\\.0) [key ^gateway/] covers nothing; delete it")

    def test_an_input_path_that_matches_nothing_is_stale(self):
        self.repo.config["published_release_inputs"]["paths"].append("meta/gone/**")
        self.repo.save_config()
        self.inputs("1.4.2")
        self.assertFindings("published release input meta/gone/** matches no tracked file; delete it")

    def test_the_inputs_need_a_reason(self):
        self.repo.config["published_release_inputs"]["reason"] = "digests"
        self.repo.save_config()
        self.inputs("1.4.2")
        self.assertFindings("published_release_inputs needs its reason", code=2)

    def test_a_bump_does_not_fail_on_inputs_that_move_after_a_publication(self):
        # Digests, a wheel hash and a recorded catalog are filled in after their release is
        # published: a bump, and the bump after it, leave them behind and still pass.
        self.inputs("1.4.2")
        self.runtime("1.4.2")
        self.assertNoWarning()
        for bumped in ("1.4.3", "1.4.4"):
            with self.subTest(bumped=bumped):
                self.bump_meta(bumped)
                self.assertWarning("WARNING: 3 published release input pin(s) name meta 1.4.2, the release the "
                                   "inputs select (the version constant at meta/tool/release/runtime.py:1), older "
                                   f"than {bumped} in this tree")
        # The refresh after the publication moves every input: one left behind is refused.
        self.runtime("1.4.4")
        self.assertFindings("asset demo-gateway-1.4.2: pins meta 1.4.2, not 1.4.4, the release of meta the "
                            "published release inputs select")
        self.inputs("1.4.4")
        self.assertNoWarning()

    def test_a_bare_version_constant_is_a_pin_only_where_configured(self):
        self.inputs("1.4.2")
        self.repo.write("meta/tool/runtime.py", 'VERSION = "1.0.0"\n')
        self.assertClean()
        self.repo.config["version_constants"] = {"meta/tool/runtime.py": {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.assertFindings('meta/tool/runtime.py:1: constant VERSION = "1.0.0": pins demo-meta 1.0.0, below its '
                            'version 1.4.2')

    def test_a_configured_version_constant_that_is_gone_is_stale(self):
        self.inputs("1.4.2")
        self.repo.config["version_constants"] = {"meta/tool/runtime.py": {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.repo.write("meta/tool/runtime.py", 'RELEASE = "1.4.2"\n')
        self.assertFindings("version constant VERSION of meta/tool/runtime.py is not in that file; delete it")

    def test_a_published_input_is_read_under_a_whole_file_entry(self):
        self.repo.allow(path="meta/tool/**", reason="Fixtures and tests of the tool.")
        self.inputs("1.4.3")
        self.assertFindings("asset demo-gateway-1.4.3: pins meta 1.4.3, above its version 1.4.2 in this tree")
        self.inputs("1.4.0")
        self.assertWarning("WARNING: 2 published release input pin(s) name meta 1.4.0")

    def test_a_published_path_with_a_match_that_applies_to_no_pin_is_stale(self):
        self.repo.config["published_release_inputs"]["paths"].append(
            {"path": "meta/tool/runtime.py", "match": "^VERSION = "})
        self.repo.config["version_constants"] = {"meta/tool/runtime.py": {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.inputs("1.4.1")
        self.repo.write("meta/tool/runtime.py", 'VERSION = "1.4.1"\n')
        self.assertWarning("WARNING: 3 published release input pin(s) name meta 1.4.1, the release the inputs "
                           "select (the version constant at meta/tool/runtime.py:1)")
        self.repo.config["published_release_inputs"]["paths"][1]["match"] = "^RELEASE = "
        self.repo.save_config()
        self.assertFindings('meta/tool/runtime.py:1: constant VERSION = "1.4.1": pins demo-meta 1.4.1, below its '
                            'version 1.4.2 in this tree\n',
                            "published release input meta/tool/runtime.py (^RELEASE = ) applies to no pin")

    def test_a_published_path_object_needs_a_match(self):
        self.repo.config["published_release_inputs"]["paths"] = [{"path": "meta/tool/release/**"}]
        self.repo.save_config()
        self.inputs("1.4.2")
        self.assertFindings("published_release_inputs paths must be path patterns or {path, match} objects", code=2)


class BumpTests(unittest.TestCase):
    """The version bumps move the pins this check reads, in the same commit."""

    def test_the_release_hook_moves_and_stages_pins_before_its_parity_check(self):
        hook = (SCRIPTS / "release-hook.sh").read_text()
        write = '"$PYTHON" "$ROOT/scripts/check_inrepo_version_pins.py" --root "$ROOT" --write'
        self.assertEqual(hook.count(write), 1)
        self.assertLess(hook.index('"$ROOT/scripts/bump-sdk-versions.sh" "$VERSION"'), hook.index(write))
        self.assertLess(hook.index(write), hook.index('"$ROOT/scripts/verify-version-parity.sh"'))
        stage = 'git add -- "${MOVED_PIN_FILES[@]/#/$ROOT/}"'
        self.assertEqual(hook.count(stage), 1)
        self.assertLess(hook.index(stage), hook.index('echo "$VERSION" > "$SENTINEL"'))

    def test_the_parity_check_runs_this_check(self):
        parity = (SCRIPTS / "verify-version-parity.sh").read_text()
        self.assertEqual(parity.count('"$ROOT/scripts/check_inrepo_version_pins.py" --root "$ROOT"'), 1)

    def test_the_meta_sdk_bump_moves_the_pins_on_meta_packages(self):
        bump = (SCRIPTS.parent / "meta/scripts/bump-sdk-versions.sh").read_text()
        self.assertEqual(bump.count('"$TOPLEVEL/scripts/check_inrepo_version_pins.py" --root "$TOPLEVEL" --write'), 1)

    def test_the_docs_stamper_leaves_the_attestation_example_a_release_archive_name(self):
        # The deploying guide verifies an rkat release archive, named
        # rkat-<version>-<target>.tar.gz; stamping the docs at a bump keeps it one.
        root = SCRIPTS.parent
        documents = ("scripts/stamp-docs-contract-version.sh", "docs/guides/deploying.mdx", "README.md",
                     ".claude/skills/meerkat-platform/SKILL.md")
        for version in ("0.9.12", "0.10.0"):
            with tempfile.TemporaryDirectory(prefix="attestation example ") as directory:
                scratch = Path(directory)
                for path in documents:
                    (scratch / path).parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(root / path, scratch / path)
                subprocess.run(["bash", str(scratch / documents[0]), version], check=True, capture_output=True)
                stamped = (scratch / "docs/guides/deploying.mdx").read_text(encoding="utf-8")
                names = re.findall(r"gh attestation verify (rkat-\S+)", stamped)
                self.assertTrue(names, "the deploying guide verifies a release archive")
                for name in names:
                    self.assertIn(name, ("rkat-<version>-x86_64-unknown-linux-gnu.tar.gz",
                                         f"rkat-{version}-x86_64-unknown-linux-gnu.tar.gz"), f"stamped for {version}")


class ConfiguredEntryTests(GuardTest):
    """This repository's own entries cover what their reasons name, applied to synthetic trees."""

    @staticmethod
    def configured():
        return json.loads((SCRIPTS.parent / pins.CONFIG).read_text(encoding="utf-8"))

    def test_the_forge_product_input_entry_leaves_a_guide_row_to_the_check(self):
        # The ordinary gateway and the Guide's gateway have the same URL shape in Forge's
        # release inputs. The temporary entry for Forge's product inputs covers the ordinary
        # gateway at 0.9.10; a Guide row fallen back to it names another release than the
        # one the inputs select, and is refused.
        entries = [entry for entry in self.configured()["allow"]
                   if entry["path"].startswith("meta/installer/release/")]
        self.assertTrue(entries)
        self.repo = Repo(self.repo.root, version="0.9.12")
        self.repo.config["published_release_inputs"] = {
            "reason": "Published assets pinned by digest move after their release.",
            "paths": ["meta/installer/release/**"]}
        self.repo.config["allow"] = entries
        self.repo.save_config()

        def url(version):
            return (f"https://github.com/lukacf/meerkat/releases/download/meta-v{version}/"
                    f"demo-gateway-{version}-x86_64-unknown-linux-gnu.tar.gz")

        inputs = {"targets": {"linux-x86_64": {"files": {
            "guide/rpc_gateway": {"url": url("0.9.10"), "sha256": HEX},
            "ordinary/rpc_gateway": {"url": url("0.9.10"), "sha256": HEX},
            "guide/gateway": {"url": url("0.9.11"), "sha256": HEX}}}}}
        self.repo.write("meta/installer/release/forge-release-inputs.json", json.dumps(inputs, indent=2) + "\n")
        err = self.assertFindings("meta/installer/release/forge-release-inputs.json:6: release lukacf/meerkat "
                                  "release meta-v0.9.10: pins meta 0.9.10, not 0.9.11, the release of meta the "
                                  "published release inputs select")
        self.assertNotIn("forge-release-inputs.json:10:", err)
        self.assertNotIn("forge-release-inputs.json:14: release", err)
        self.assertNotIn("covers nothing", err)

    def test_the_agent_team_runtime_follows_the_published_rule_on_its_version_line_only(self):
        # The agent-team runtime's VERSION sits beside its wheel's SHA-256 and moves after a
        # publication; any other pin in that file names the version in this tree.
        runtime = "meta/installer/launchkit/public_meta_runtime.py"
        configured = [path for path in self.configured()["published_release_inputs"]["paths"]
                      if (path if isinstance(path, str) else path["path"]) == runtime]
        self.assertEqual(len(configured), 1)
        self.repo.config["published_release_inputs"] = {
            "reason": "Published assets pinned by digest move after their release.", "paths": configured}
        self.repo.config["version_constants"] = {runtime: {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.repo.write(runtime, f'VERSION = "1.4.1"\nSDK_WHEEL_SHA256 = "{HEX}"\n')
        self.assertWarning(f"WARNING: 1 published release input pin(s) name meta 1.4.1, the release the inputs "
                           f"select (the version constant at {runtime}:1), older than 1.4.2 in this tree")
        self.repo.write(runtime, f'VERSION = "1.4.1"\nSDK_WHEEL_SHA256 = "{HEX}"\nCORE_REQUIREMENT = "=1.4.1"\n')
        self.assertFindings(f'{runtime}:3: constant CORE_REQUIREMENT = "=1.4.1": pins core 1.4.1, below its version '
                            '1.4.2 in this tree\n')

    def test_a_pin_beside_this_repository_s_published_inputs_is_still_refused(self):
        # This repository's own published release input paths, on a synthetic tree whose
        # inputs lag one release: a pin in a file beside them that no listed path names
        # follows the ordinary rule and fails the check.
        published = self.configured()["published_release_inputs"]
        runtime = "meta/installer/launchkit/public_meta_runtime.py"
        self.repo.config["published_release_inputs"] = published
        self.repo.config["version_constants"] = {runtime: {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        url = "https://github.com/lukacf/meerkat/releases/download/meta-v1.4.1/demo-gateway-1.4.1-x86_64.tar.gz"
        self.repo.write(runtime, f'VERSION = "1.4.1"\nSDK_WHEEL_SHA256 = "{HEX}"\n')
        self.repo.write("meta/installer/launchkit/meta_model_catalog_1_4_1.json",
                        json.dumps({"requires": "demo-meta==1.4.1"}))
        self.repo.write("meta/installer/release/inputs/stable-inputs.json", json.dumps({"url": url, "sha256": HEX}))
        self.repo.write("meta/installer/tests/fixtures/meta_release_1_4_1/checksums.sha256",
                        f"{HEX}  demo-gateway-1.4.1-x86_64.tar.gz\n")
        for beside in ("meta/installer/launchkit/other_catalog.json", "meta/installer/launchkit/notes.txt",
                       "meta/installer/inputs.json", "meta/installer/tests/fixtures/other_release/SOURCE.json"):
            with self.subTest(beside=beside):
                added = self.repo.write(beside, json.dumps({"requires": "demo-meta==1.4.1"}))
                err = self.assertFindings(f"{beside}:1: requirement demo-meta==1.4.1: pins demo-meta 1.4.1, below "
                                          "its version 1.4.2 in this tree\n")
                self.assertNotIn(" not 1.4.1", err)
                added.unlink()


class CommandTests(unittest.TestCase):
    def test_the_command_reports_its_exit_codes(self):
        with tempfile.TemporaryDirectory(prefix="inrepo pins cli ") as directory:
            root = Path(directory)
            subprocess.run(["git", "-c", "init.defaultBranch=main", "init", "-q", str(root)], check=True)
            repo = Repo(root)
            subprocess.run(["git", "-C", str(root), "add", "-A"], check=True)
            command = [sys.executable, "-I", str(SCRIPTS / "check_inrepo_version_pins.py"), "--root", str(root)]
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 0)
            repo.write("docs/pin.md", "demo-meta==1.4.0\n")
            subprocess.run(["git", "-C", str(root), "add", "-A"], check=True)
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 1)
            repo.write(pins.CONFIG, "{")
            self.assertEqual(subprocess.run(command, capture_output=True).returncode, 2)


if __name__ == "__main__":
    unittest.main()
