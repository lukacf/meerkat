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


class PublishedInputTests(GuardTest):
    """Published release inputs pin assets by digest: this tree's version or the release before it."""

    def setUp(self):
        super().setUp()
        self.repo.write("meta/CHANGELOG.md", "# Changelog\n\n## [Unreleased]\n\n## [1.4.2] - 2026-01-03\n\n"
                        "## [1.4.1] - 2026-01-02\n\n## [1.4.0] - 2026-01-01\n")
        self.repo.config["published_release_inputs"] = {
            "reason": "Published assets pinned by digest move after their release.",
            "paths": ["meta/tool/release/**"]}
        self.repo.save_config()

    def inputs(self, version, path="meta/tool/release/inputs.json"):
        url = (f"https://github.com/lukacf/meerkat/releases/download/meta-v{version}/"
               f"demo-gateway-{version}-x86_64.tar.gz")
        self.repo.write(path, json.dumps({"gateway": {"url": url, "sha256": HEX}}, indent=2) + "\n")

    def test_the_version_in_the_tree_passes(self):
        self.inputs("1.4.2")
        self.assertIn("1 published release input paths", self.assertClean())

    def test_the_release_before_it_passes(self):
        self.inputs("1.4.1")
        self.assertClean()

    def test_two_releases_back_is_refused(self):
        self.inputs("1.4.0")
        self.assertFindings("asset demo-gateway-1.4.0: pins meta 1.4.0, older than the release before its version "
                            "1.4.2 in this tree (a published release input names 1.4.2 or the release before it, "
                            "1.4.1)", "release lukacf/meerkat release meta-v1.4.0")

    def test_a_version_newer_than_the_tree_is_refused(self):
        self.inputs("1.4.3")
        self.assertFindings("asset demo-gateway-1.4.3: pins meta 1.4.3, above its version 1.4.2 in this tree")

    def test_the_release_before_it_outside_the_inputs_is_refused(self):
        self.inputs("1.4.1", path="meta/tool/other/inputs.json")
        self.assertFindings("asset demo-gateway-1.4.1: pins meta 1.4.1, below its version 1.4.2 in this tree")

    def test_a_requirement_follows_its_package_product(self):
        catalog = self.repo.write("meta/tool/release/catalog.json", json.dumps({"requires": "demo-meta==1.4.1"}))
        self.assertClean()
        catalog.write_text(json.dumps({"requires": "demo-meta==1.4.0"}), encoding="utf-8")
        self.assertFindings("requirement demo-meta==1.4.0: pins demo-meta 1.4.0, older than the release before")

    def test_a_product_of_its_own_reads_its_own_changelog(self):
        # demo-tool is the forge product inside meta/: its releases are in meta/tool/CHANGELOG.md.
        pins = self.repo.write("meta/tool/release/tool.json", json.dumps({"requires": "demo-tool==0.3.0"}))
        self.assertFindings("requirement demo-tool==0.3.0: pins demo-tool 0.3.0, below its version 0.3.1 in this "
                            "tree (a published release input names 0.3.1 or the release before it, and the "
                            "product's CHANGELOG.md has none)")
        self.repo.write("meta/tool/CHANGELOG.md", "## [0.3.1] - 2026-01-02\n\n## [0.3.0] - 2026-01-01\n")
        self.assertClean()
        pins.write_text(json.dumps({"requires": "demo-tool==0.2.9"}), encoding="utf-8")
        self.assertFindings("pins demo-tool 0.2.9, older than the release before its version 0.3.1")

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

    def test_a_bump_leaves_values_filled_in_after_a_release_one_release_behind(self):
        # Digests, a wheel hash and a recorded catalog are filled in after their release
        # is published: at a bump they name the release before the new version.
        self.repo.config["version_constants"] = {"meta/tool/release/runtime.py": {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.inputs("1.4.2")
        self.repo.write("meta/tool/release/runtime.py", f'VERSION = "1.4.2"\nWHEEL_SHA256 = "{HEX}"\n')
        self.assertClean()
        self.repo.write("meta/crates/demo-meta-rs/Cargo.toml", '[package]\nname = "demo-meta-rs"\nversion = "1.4.3"\n')
        self.repo.write("meta/sdk/package.json", json.dumps({"name": "@rkat/demo-meta", "version": "1.4.3"}))
        self.repo.write("meta/sdk/pyproject.toml", '[project]\nname = "demo-meta"\nversion = "1.4.3"\n')
        self.repo.write("meta/CHANGELOG.md", "## [Unreleased]\n\n## [1.4.3] - 2026-01-04\n\n## [1.4.2] - 2026-01-03\n\n"
                        "## [1.4.1] - 2026-01-02\n")
        self.assertClean()
        self.repo.write("meta/tool/release/runtime.py", f'VERSION = "1.4.1"\nWHEEL_SHA256 = "{HEX}"\n')
        self.assertFindings('constant VERSION = "1.4.1": pins demo-meta 1.4.1, older than the release before its '
                            'version 1.4.3 in this tree (a published release input names 1.4.3 or the release before '
                            'it, 1.4.2)')

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
        self.inputs("1.4.0")
        self.assertFindings("asset demo-gateway-1.4.0: pins meta 1.4.0, older than the release before")

    def test_a_published_path_with_a_match_that_applies_to_no_pin_is_stale(self):
        self.repo.config["published_release_inputs"]["paths"].append(
            {"path": "meta/tool/runtime.py", "match": "^VERSION = "})
        self.repo.config["version_constants"] = {"meta/tool/runtime.py": {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.inputs("1.4.2")
        self.repo.write("meta/tool/runtime.py", 'VERSION = "1.4.1"\n')
        self.assertClean()
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
        # release inputs. Two releases past 0.9.10, the temporary entry for Forge's product
        # inputs covers the ordinary gateway at 0.9.10, and a Guide row fallen back to it is refused.
        entries = [entry for entry in self.configured()["allow"]
                   if entry["path"].startswith("meta/installer/release/")]
        self.assertTrue(entries)
        self.repo = Repo(self.repo.root, version="0.9.12")
        self.repo.write("meta/CHANGELOG.md", "## [0.9.12] - 2026-01-03\n\n## [0.9.11] - 2026-01-02\n\n"
                        "## [0.9.10] - 2026-01-01\n")
        self.repo.config["published_release_inputs"] = {
            "reason": "Published assets pinned by digest move after their release.",
            "paths": ["meta/installer/release/**"]}
        self.repo.config["allow"] = entries
        self.repo.save_config()
        url = ("https://github.com/lukacf/meerkat/releases/download/meta-v0.9.10/"
               "demo-gateway-0.9.10-x86_64-unknown-linux-gnu.tar.gz")
        inputs = {"targets": {"linux-x86_64": {"files": {
            "guide/rpc_gateway": {"url": url, "sha256": HEX},
            "ordinary/rpc_gateway": {"url": url, "sha256": HEX}}}}}
        self.repo.write("meta/installer/release/forge-release-inputs.json", json.dumps(inputs, indent=2) + "\n")
        err = self.assertFindings("meta/installer/release/forge-release-inputs.json:6: release lukacf/meerkat "
                                  "release meta-v0.9.10: pins meta 0.9.10, older than the release before")
        self.assertNotIn("forge-release-inputs.json:10:", err)
        self.assertNotIn("covers nothing", err)

    def test_the_agent_team_runtime_follows_the_published_rule_on_its_version_line_only(self):
        # The agent-team runtime's VERSION sits beside its wheel's SHA-256 and may lag a bump
        # by one release; any other pin in that file names the version in this tree.
        runtime = "meta/installer/launchkit/public_meta_runtime.py"
        configured = [path for path in self.configured()["published_release_inputs"]["paths"]
                      if (path if isinstance(path, str) else path["path"]) == runtime]
        self.assertEqual(len(configured), 1)
        for changelog in ("CHANGELOG.md", "meta/CHANGELOG.md"):
            self.repo.write(changelog, "## [1.4.2] - 2026-01-03\n\n## [1.4.1] - 2026-01-02\n")
        self.repo.config["published_release_inputs"] = {
            "reason": "Published assets pinned by digest move after their release.", "paths": configured}
        self.repo.config["version_constants"] = {runtime: {"VERSION": "pypi:demo-meta"}}
        self.repo.save_config()
        self.repo.write(runtime, f'VERSION = "1.4.1"\nSDK_WHEEL_SHA256 = "{HEX}"\n')
        self.assertClean()
        self.repo.write(runtime, f'VERSION = "1.4.1"\nSDK_WHEEL_SHA256 = "{HEX}"\nCORE_REQUIREMENT = "=1.4.1"\n')
        self.assertFindings(f'{runtime}:3: constant CORE_REQUIREMENT = "=1.4.1": pins core 1.4.1, below its version '
                            '1.4.2 in this tree\n')


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
