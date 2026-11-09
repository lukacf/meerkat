"""Maintained data must reproduce the canonical manual and package metadata.

This source packaging test neither installs a connector nor checks any account
or native host.
"""
import importlib.util
import hashlib
import json
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


class ProfileGuideDataTests(unittest.TestCase):
    def test_checked_in_sdk_data_and_standalone_stage_have_one_source(self):
        stage = load("profile_guide_stage_test", ROOT / "sdk/agno/stage.py")
        composer = load("profile_guide_composer_test", ROOT / "sdk/agno/compose_profile_guides.py")
        package = ROOT / "sdk/python/src/meerkat_toolkit/adapters"
        manual = stage.project_guide((ROOT / "sdk/python/AGNO-MCP.md").read_text()).encode()
        selections = json.loads((ROOT / "sdk/agno/profile-guide-selections.json").read_bytes())
        profiles, fields = composer.package_metadata(package)
        expected = composer.compose(manual, selections, profiles, fields)
        encoded = (json.dumps(expected, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()
        self.assertEqual((package / "CONFIGURATION-GUIDES.json").read_bytes(), encoded)
        self.assertEqual((package / "PROFILE-GUIDE.md").read_bytes(), manual)
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "component"
            manifest = stage.stage(destination)
            staged = destination / "src/meerkat_toolkit_agno"
            self.assertEqual((staged / "CONFIGURATION-GUIDES.json").read_bytes(), encoded)
            self.assertEqual((staged / "PROFILE-GUIDE.md").read_bytes(), manual)
            self.assertEqual(manifest["configuration_guides"]["sha256"], composer.sha256(encoded))
            self.assertEqual(manifest["configuration_guides"]["full_guide_sha256"], composer.sha256(manual))
            # The native consumer is Salesforce-only; its real handler and
            # rejection of another profile are covered by the existing
            # test_native_connector_consumer.py controls. This checks the
            # exact source publication, not a provider login or execution.
            entry = staged / "agno_mcp.py"
            digest = hashlib.sha256(entry.read_bytes()).hexdigest()
            self.assertEqual(manifest["native_connector"], {
                "schema_version": 1,
                "module": "meerkat_toolkit_agno.agno_mcp",
                "mode": "serve-native",
                "configuration_argument": "--config",
                "profiles": ["salesforce"],
                "member": "meerkat_toolkit_agno/agno_mcp.py",
                "member_sha256": digest,
            })
            self.assertEqual(digest, hashlib.sha256((package / "agno_mcp.py").read_bytes()).hexdigest())
            self.assertEqual(digest, manifest["source_mapping"]["src/meerkat_toolkit_agno/agno_mcp.py"]["sha256"])
            self.assertEqual(json.loads((staged / "PACKAGE-SOURCES.json").read_bytes()), manifest)


if __name__ == "__main__":
    unittest.main()
