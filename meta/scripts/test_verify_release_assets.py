#!/usr/bin/env python3
"""Regression tests for the Meta release-asset manifest contract."""

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("verify-release-assets.py")
SPEC = importlib.util.spec_from_file_location("verify_release_assets", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
release_assets = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release_assets)


class ReleaseAssetContractTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.source = self.root / "downloaded"
        self.source.mkdir()
        self.output = self.root / "release-assets"

    def tearDown(self):
        self.temp.cleanup()

    def write_complete_source(self):
        for index, name in enumerate(release_assets.expected_archive_names("0.8.0")):
            artifact_dir = self.source / f"matrix-{index}"
            artifact_dir.mkdir()
            (artifact_dir / name).write_bytes(f"artifact-{index}".encode())

    def test_expected_inventory_includes_the_repair_binary_on_all_targets(self):
        # Release-blocker regression (v0.8.14 attempt 1): meta-repair
        # archives were built but rejected as unexpected because the
        # inventory constant was never taught about the new binary. The
        # repair tool ships as a release asset by explicit consumer
        # requirement - building a maintenance tool from source inside a
        # repair window is how deployment defects are born.
        names = release_assets.expected_archive_names("0.8.0")
        for target, ext in release_assets.TARGET_ARCHIVES:
            self.assertIn(f"meta-repair-0.8.0-{target}.{ext}", names)

    def test_prepare_emits_exact_flat_manifest_and_verified_checksums(self):
        self.write_complete_source()

        release_assets.prepare_assets(self.source, self.output, "v0.8.0")

        release_assets.verify_assets(self.output, "v0.8.0")
        self.assertEqual(
            sorted(path.name for path in self.output.iterdir()),
            sorted(
                release_assets.expected_archive_names("0.8.0")
                + [release_assets.CHECKSUMS_NAME, release_assets.MANIFEST_NAME]
            ),
        )
        self.assertTrue(all(path.parent == self.output for path in self.output.iterdir()))

    def test_prepare_rejects_missing_matrix_archive(self):
        self.write_complete_source()
        next(self.source.rglob("*.zip")).unlink()

        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "missing"):
            release_assets.prepare_assets(self.source, self.output, "v0.8.0")

    def test_prepare_rejects_duplicate_basename_before_flattening(self):
        self.write_complete_source()
        archive = next(self.source.rglob("*.tar.gz"))
        duplicate_dir = self.source / "duplicate"
        duplicate_dir.mkdir()
        (duplicate_dir / archive.name).write_bytes(b"different")

        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "duplicate"):
            release_assets.prepare_assets(self.source, self.output, "v0.8.0")

    def test_verify_rejects_tampered_archive(self):
        self.write_complete_source()
        release_assets.prepare_assets(self.source, self.output, "v0.8.0")
        next(self.output.glob("*.tar.gz")).write_bytes(b"tampered")

        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "checksum mismatch"):
            release_assets.verify_assets(self.output, "v0.8.0")


    def write_tier_source(self, tier, folder):
        folder.mkdir()
        for index, name in enumerate(release_assets.expected_archive_names("0.9.2", tier)):
            (folder / name).write_bytes(f"{tier}-{index}".encode())

    def test_tiers_partition_the_targets_and_keep_their_own_metadata(self):
        tiers = release_assets.TIER_TARGETS
        self.assertEqual(tiers["core"], ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
                                         "aarch64-apple-darwin"))
        self.assertEqual(tiers["trailing"], ("x86_64-apple-darwin", "x86_64-pc-windows-msvc"))
        self.assertFalse(set(tiers["core"]) & set(tiers["trailing"]))
        self.assertEqual(sorted(tiers["core"] + tiers["trailing"]),
                         sorted(target for target, _ in release_assets.TARGET_ARCHIVES))
        # The core keeps the schema 1 file names; the trailing tier never
        # writes a file the core published.
        self.assertEqual(release_assets.metadata_names("core"), release_assets.metadata_names())
        self.assertFalse(set(release_assets.metadata_names("core"))
                         & set(release_assets.metadata_names("trailing")))
        self.assertEqual(sorted(release_assets.expected_archive_names("0.9.2", "core")
                                + release_assets.expected_archive_names("0.9.2", "trailing")),
                         release_assets.expected_archive_names("0.9.2"))

    def test_each_tier_prepares_and_verifies_alone_and_together(self):
        self.write_tier_source("core", self.root / "core-src")
        self.write_tier_source("trailing", self.root / "trailing-src")
        core, trailing, both = self.root / "core", self.root / "trailing", self.root / "both"
        release_assets.prepare_assets(self.root / "core-src", core, "v0.9.2", "core")
        release_assets.prepare_assets(self.root / "trailing-src", trailing, "v0.9.2", "trailing")
        index = __import__("json").loads((core / "index.json").read_text())
        self.assertEqual((index["tier"], index["checksums"]), ("core", "checksums.sha256"))
        self.assertEqual(index["artifacts"], release_assets.expected_archive_names("0.9.2", "core"))
        release_assets.verify_assets(core, "v0.9.2", ("core",))
        release_assets.verify_tier_assets(trailing, "v0.9.2", "trailing")
        both.mkdir()
        for folder in (core, trailing):
            for path in folder.iterdir():
                (both / path.name).write_bytes(path.read_bytes())
        release_assets.verify_assets(both, "v0.9.2", ("core", "trailing"))
        # The core set alone must not carry trailing files, and schema 1
        # still demands every archive with untiered metadata.
        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "unexpected"):
            release_assets.verify_assets(both, "v0.9.2", ("core",))
        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "missing"):
            release_assets.verify_assets(core, "v0.9.2")
        # Trailing metadata alone (no trailing archives) is not part of a core set.
        core_plus_metadata = self.root / "core-plus-metadata"
        core_plus_metadata.mkdir()
        for path in core.iterdir():
            (core_plus_metadata / path.name).write_bytes(path.read_bytes())
        for name in release_assets.metadata_names("trailing"):
            (core_plus_metadata / name).write_bytes((trailing / name).read_bytes())
        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "release file set mismatch"):
            release_assets.verify_assets(core_plus_metadata, "v0.9.2", ("core",))
        next(both.glob("*.zip")).write_bytes(b"tampered")
        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "checksum mismatch"):
            release_assets.verify_assets(both, "v0.9.2", ("core", "trailing"))

    def test_tier_lists_start_with_core_and_name_known_tiers_once(self):
        self.write_tier_source("core", self.root / "core-src")
        release_assets.prepare_assets(self.root / "core-src", self.output, "v0.9.2", "core")
        for tiers in ((), ("trailing",), ("trailing", "core"), ("core", "core"), ("core", "other")):
            with self.subTest(tiers=tiers):
                with self.assertRaisesRegex(release_assets.ReleaseAssetError, "invalid release tiers"):
                    release_assets.verify_assets(self.output, "v0.9.2", tiers)

    def test_schema_1_metadata_has_no_tier_and_schema_2_metadata_names_its_own(self):
        self.write_complete_source()
        release_assets.prepare_assets(self.source, self.output, "v0.8.0")
        index = __import__("json").loads((self.output / "index.json").read_text())
        self.assertNotIn("tier", index)
        self.assertEqual(len(index["artifacts"]), 15)
        # A core index presented as schema 1 metadata (or the reverse) fails.
        self.write_tier_source("core", self.root / "core-src")
        core = self.root / "core"
        release_assets.prepare_assets(self.root / "core-src", core, "v0.9.2", "core")
        document = __import__("json").loads((core / "index.json").read_text())
        document.pop("tier")
        (core / "index.json").write_text(__import__("json").dumps(document))
        with self.assertRaisesRegex(release_assets.ReleaseAssetError, "does not match"):
            release_assets.verify_assets(core, "v0.9.2", ("core",))

    def run_verify_cli(self, folder, *tiers, tag="v0.9.2"):
        import subprocess, sys
        argv = [sys.executable, str(SCRIPT), "verify", "--assets-dir", str(folder), "--release-tag", tag]
        for tier in tiers:
            argv += ["--tier", tier]
        return subprocess.run(argv, capture_output=True, text=True)

    def test_the_verify_cli_defaults_to_every_tier_present(self):
        self.write_tier_source("core", self.root / "core-src")
        self.write_tier_source("trailing", self.root / "trailing-src")
        core, trailing, both = self.root / "core", self.root / "trailing", self.root / "both"
        release_assets.prepare_assets(self.root / "core-src", core, "v0.9.2", "core")
        release_assets.prepare_assets(self.root / "trailing-src", trailing, "v0.9.2", "trailing")
        both.mkdir()
        for folder in (core, trailing):
            for path in folder.iterdir():
                (both / path.name).write_bytes(path.read_bytes())
        self.assertEqual(self.run_verify_cli(core).returncode, 0)
        result = self.run_verify_cli(both)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("(core, trailing)", result.stdout)
        # Without a tier the default still refuses a trailing archive whose
        # trailing index is absent.
        (both / "index-trailing.json").unlink()
        self.assertNotEqual(self.run_verify_cli(both).returncode, 0)
        # Schema 1 sets keep verifying with no flag at all.
        self.write_complete_source()
        release_assets.prepare_assets(self.source, self.output, "v0.8.0")
        result = self.run_verify_cli(self.output, tag="v0.8.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("verified 15 flat release archives for v0.8.0\n", result.stdout)

if __name__ == "__main__":
    unittest.main()
