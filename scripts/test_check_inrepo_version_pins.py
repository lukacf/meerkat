#!/usr/bin/env python3
"""Tests of scripts/check_inrepo_version_pins.py on synthetic repositories."""

from __future__ import annotations

import io
import json
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
