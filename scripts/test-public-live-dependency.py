#!/usr/bin/env python3
"""Regression tests for the public Live dependency gate."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location(
    "public_live_dependency", Path(__file__).with_name("check-public-live-dependency.py")
)
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class PublicLiveDependencyTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "meerkat-openai").mkdir()
        (self.root / "MODULE.bazel").write_text("")
        (self.root / "Cargo.toml").write_text(
            '[workspace.dependencies]\noai-rt-rs = { version = "=0.5.0" }\n'
        )
        (self.root / "Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "oai-rt-rs"\nversion = "0.5.0"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            f'checksum = "{CHECKER.CHECKSUM}"\n'
        )
        (self.root / "meerkat-openai/Cargo.toml").write_text(
            '[target.\'cfg(not(target_arch = "wasm32"))\'.dependencies]\n'
            'oai-rt-rs = { workspace = true, optional = true }\n'
            '[features]\nlive = ["dep:oai-rt-rs"]\n'
            'experimental-gpt-live = ["dep:oai-rt-rs", "oai-rt-rs/experimental-gpt-live"]\n'
        )
        (self.root / "MODULE.bazel.lock").write_text(json.dumps({
            "moduleExtensions": {"crate": {"general": {"generatedRepoSpecs": {
                "crates__oai-rt-rs-0.5.0": {"attributes": {
                    "sha256": CHECKER.CHECKSUM,
                    "urls": ["https://static.crates.io/crates/oai-rt-rs/0.5.0/download"]
                }}
            }}}}
        }))

    def change(self, file, old, new):
        path = self.root / file
        path.write_text(path.read_text().replace(old, new))

    def test_registry_pin_passes(self):
        self.assertEqual(CHECKER.check(self.root), [])

    def test_old_pin_rejects(self):
        self.change("Cargo.toml", "=0.5.0", "=0.4.1")
        self.assertTrue(CHECKER.check(self.root))

    def test_cargo_checksum_rejects(self):
        self.change("Cargo.lock", CHECKER.CHECKSUM, "0" * 64)
        self.assertTrue(CHECKER.check(self.root))

    def test_bazel_checksum_rejects(self):
        self.change("MODULE.bazel.lock", CHECKER.CHECKSUM, "0" * 64)
        self.assertTrue(CHECKER.check(self.root))

    def test_bazel_nonregistry_archive_rejects(self):
        self.change("MODULE.bazel.lock", "https://static.crates.io", "https://not-the-registry.invalid")
        self.assertTrue(CHECKER.check(self.root))

    def test_root_private_feature_rejects(self):
        self.change("Cargo.toml", '"=0.5.0"', '"=0.5.0", features = ["experimental-gpt-live"]')
        self.assertTrue(CHECKER.check(self.root))

    def test_transitive_private_feature_rejects(self):
        self.change("meerkat-openai/Cargo.toml", 'live = ["dep:oai-rt-rs"]', 'live = ["experimental-gpt-live"]')
        self.assertTrue(CHECKER.check(self.root))

    def test_root_path_override_rejects(self):
        self.change("Cargo.toml", '"=0.5.0"', '"=0.5.0", path = "private"')
        self.assertTrue(CHECKER.check(self.root))

    def test_patch_rejects(self):
        with (self.root / "Cargo.toml").open("a") as file:
            file.write('[patch.crates-io]\noai-rt-rs = { path = "private" }\n')
        self.assertTrue(CHECKER.check(self.root))

    def test_private_feature_removal_rejects(self):
        self.change("meerkat-openai/Cargo.toml", ', "oai-rt-rs/experimental-gpt-live"', "")
        self.assertTrue(CHECKER.check(self.root))

    def test_bazel_override_rejects(self):
        (self.root / "MODULE.bazel").write_text('crate.annotation(crate = "oai-rt-rs")')
        self.assertTrue(CHECKER.check(self.root))

    def test_renamed_direct_private_dependency_rejects(self):
        self.change(
            "meerkat-openai/Cargo.toml", "[features]",
            'voice_protocol = { package = "oai-rt-rs", version = "=0.5.0", features = ["experimental-gpt-live"] }\n[features]',
        )
        self.assertTrue(CHECKER.check(self.root))

    def add_workspace_alias(self):
        with (self.root / "Cargo.toml").open("a") as file:
            file.write('voice_protocol = { package = "oai-rt-rs", version = "=0.5.0" }\n')
        self.change(
            "meerkat-openai/Cargo.toml", "[features]",
            'voice_protocol = { workspace = true, optional = true }\n[features]',
        )

    def test_renamed_inherited_public_dependency_passes(self):
        self.add_workspace_alias()
        self.assertEqual(CHECKER.check(self.root), [])

    def test_renamed_inherited_private_feature_rejects(self):
        self.add_workspace_alias()
        self.change("meerkat-openai/Cargo.toml", 'live = ["dep:oai-rt-rs"]',
                    'live = ["voice_protocol?/experimental-gpt-live"]')
        self.assertTrue(CHECKER.check(self.root))

    def test_renamed_workspace_private_feature_rejects(self):
        self.add_workspace_alias()
        self.change("Cargo.toml", 'package = "oai-rt-rs", version = "=0.5.0"',
                    'package = "oai-rt-rs", version = "=0.5.0", features = ["experimental-gpt-live"]')
        self.assertTrue(CHECKER.check(self.root))


if __name__ == "__main__":
    unittest.main()
