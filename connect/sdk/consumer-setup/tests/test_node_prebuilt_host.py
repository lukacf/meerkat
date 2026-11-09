"""Optional native payload policy uses normal preparation and exact runtime pins."""
import json
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

import test_prepare_host as preparation
import test_bootstrap_host as boot
import test_start as start_tests

host, SOURCE, sha = preparation.host, preparation.SOURCE, preparation.sha
INSTALLER = SOURCE.parents[1] / "scripts/install-library-consumer.py"


def enable(case, *, policy=False):
    preparation.PreparationTests.npm_acquisition(case)
    for name in ("node_prebuilt_acquisition.py", "mcpb_acquisition.py"):
        case.files["setup/" + name] = ((SOURCE / name).read_bytes(), 0o644)
    case.files["selected/bin/install-library-consumer.py"] = (INSTALLER.read_bytes(), 0o755)
    case.publish_release()
    target = {"node_sha256": case.args.selected_node_sha256, "node_version": "v24.19.0",
              "node_abi": "137", "platform": "darwin", "arch": "arm64", "libc": None}
    value = {"schema_version": 1, "scope": "trusted_public_node_prebuilt_resources" if policy else
             "trusted_public_node_prebuilt_acquisition", "target": target,
             "allowed_origins": ["https://github.com"],
             "allowed_redirect_origins": ["https://release-assets.githubusercontent.com"]}
    if not policy:
        value["python"] = {"path": case.args.python, "sha256": case.args.python_sha256}
    path = case.base / "explicit-prebuilt.json"
    path.write_text(json.dumps(value, indent=2) + "\n")
    key = "node_prebuilt_policy" if policy else "node_prebuilt_profile"
    setattr(case.args, key, str(path))
    setattr(case.args, key + "_sha256", sha(path.read_bytes()))
    return path, value


class PrebuiltHostTests(unittest.TestCase):
    setUp = preparation.PreparationTests.setUp
    publish_release = preparation.PreparationTests.publish_release
    selected_node = preparation.PreparationTests.selected_node
    refuse = preparation.PreparationTests.refuse

    def test_exact_profile_prepares_without_execution_and_legacy_defaults_remain_absent(self):
        self.selected_node()
        host.prepare(self.args)
        old = Path(self.args.output)
        self.assertNotIn("node-prebuilt.json", json.loads((old / "prepared.json").read_bytes())["files"])
        self.args.output = str(self.base / "enabled-output")
        path, value = enable(self)
        original = path.read_bytes()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("preparation must not execute")):
            result = host.prepare(self.args)
        output = Path(result["directory"])
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual((output / "node-prebuilt.json").read_bytes(), original)
        self.assertEqual(receipt["runtimes"]["node_prebuilt_profile"]["sha256"], sha(original))
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertEqual(argv[argv.index("--node-prebuilt-profile") + 1], str(output / "node-prebuilt.json"))
        self.assertEqual(argv[argv.index("--node-prebuilt-profile-sha256") + 1], sha(original))
        self.assertEqual(path.read_bytes(), original)
        self.assertFalse(Path(self.args.workspace).exists())

    def test_resource_policy_derives_actual_python_and_preserves_original_digest(self):
        path, policy = enable(self, policy=True)
        original = path.read_bytes()
        with patch.object(subprocess, "Popen", side_effect=AssertionError("no process")):
            host.prepare(self.args)
        output = Path(self.args.output)
        profile = json.loads((output / "node-prebuilt.json").read_bytes())
        self.assertEqual(profile, {**policy, "scope": "trusted_public_node_prebuilt_acquisition",
            "python": {"path": self.args.python, "sha256": self.args.python_sha256}})
        receipt = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(receipt["runtimes"]["node_prebuilt_policy"]["sha256"], sha(original))
        self.assertEqual(receipt["files"]["node-prebuilt.json"], sha((output / "node-prebuilt.json").read_bytes()))
        self.assertNotIn("node_prebuilt_profile", receipt["runtimes"])
        self.assertEqual(path.read_bytes(), original)

    def test_policy_target_mismatch_and_changed_input_refuse_before_publish(self):
        path, value = enable(self, policy=True)
        value["target"]["node_sha256"] = "0" * 64
        path.write_text(json.dumps(value))
        self.args.node_prebuilt_policy_sha256 = sha(path.read_bytes())
        with self.assertRaisesRegex(Exception, "invalid_node_prebuilt_acquisition_profile"):
            host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())
        value["target"]["node_sha256"] = self.args.selected_node_sha256
        path.write_text(json.dumps(value))
        self.args.node_prebuilt_policy_sha256 = sha(path.read_bytes())
        original = host.recheck_pins
        def changed(pins):
            path.write_bytes(path.read_bytes() + b" ")
            original(pins)
        with patch.object(host, "recheck_pins", side_effect=changed):
            with self.assertRaises(Exception):
                host.prepare(self.args)
        self.assertFalse(Path(self.args.output).exists())

    def test_partial_ambiguous_or_unselected_capability_refuses(self):
        path, _ = enable(self, policy=True)
        expected = self.args.node_prebuilt_policy_sha256
        self.args.node_prebuilt_policy_sha256 = None
        self.refuse("node_prebuilt_policy_pair_required")
        self.args.node_prebuilt_policy_sha256 = expected
        self.args.node_prebuilt_profile = str(path)
        self.refuse("ambiguous_node_prebuilt_selection")
        self.args.node_prebuilt_profile = None
        self.args.allow_public_npm = False
        self.refuse("node_prebuilt_requires_node_acquisition")

    def test_missing_new_module_refuses_only_when_explicitly_selected(self):
        enable(self)
        del self.files[host.NODE_PREBUILT_ACQUISITION]
        (self.release / host.NODE_PREBUILT_ACQUISITION).unlink()
        self.publish_release()
        self.refuse("node_prebuilt_not_in_release")
        self.args.node_prebuilt_profile = self.args.node_prebuilt_profile_sha256 = None
        host.prepare(self.args)


class PrebuiltStarterTests(unittest.TestCase):
    setUp = start_tests.StarterTests.setUp
    installer = start_tests.StarterTests.installer
    publish_release = start_tests.StarterTests.publish_release
    refresh = start_tests.StarterTests.refresh
    selected_node = preparation.PreparationTests.selected_node

    def test_normal_starter_bootstrap_projects_explicit_policy_and_pins_selected_runtime(self):
        path, policy = enable(self, policy=True)
        self.refresh()
        for key in ("selected_node", "selected_npm_cli", "npm_archive"):
            setattr(self.options, key, getattr(self.args, key))
        self.options.allow_public_npm = True
        self.options.node_prebuilt_policy = str(path)
        self.options.node_prebuilt_policy_sha256 = sha(path.read_bytes())
        result = start_tests.starter.start(self.options)
        self.assertEqual(result["status"], "prepared")
        receipt = json.loads((self.destination / "config/prepared.json").read_bytes())
        derived = json.loads((self.destination / "config/node-prebuilt.json").read_bytes())
        self.assertEqual(derived["target"], policy["target"])
        self.assertEqual(derived["python"]["path"], self.args.python)
        self.assertEqual(receipt["runtimes"]["node_prebuilt_policy"]["sha256"], sha(path.read_bytes()))
        self.assertEqual(receipt["runtimes"]["selected_node"]["sha256"], policy["target"]["node_sha256"])
        self.assertFalse((self.destination / "session").exists())

    product = start_tests.StarterTests.product
    npm_product = start_tests.StarterTests.npm_product

    def test_product_policy_reaches_actual_starter_bootstrap_and_preparer(self):
        for name in ("node_prebuilt_acquisition.py", "mcpb_acquisition.py", "public_github.py"):
            self.files["setup/" + name] = ((SOURCE / name).read_bytes(), 0o644)
        self.files["selected/bin/install-library-consumer.py"] = (INSTALLER.read_bytes(), 0o755)
        product, root, group = self.npm_product()
        policy = {"schema_version": 1, "scope": "trusted_public_node_prebuilt_resources",
            "target": {"node_sha256": sha((root / group["node"]).read_bytes()),
                "node_version": "v24.19.0", "node_abi": "137", "platform": "darwin",
                "arch": "arm64", "libc": None},
            "allowed_origins": ["https://github.com"],
            "allowed_redirect_origins": ["https://release-assets.githubusercontent.com"]}
        path = root / "node-prebuilt-policy.json"
        path.write_bytes(host.encoded(policy))
        product.verify_resources.return_value[0]["entries"] = [
            {"path": path.name, "kind": "file", "size": path.stat().st_size,
                "sha256": sha(path.read_bytes())}]
        result = product.prepare(root, self.destination, threading.Event())
        self.assertIsInstance(result, dict)
        receipt = json.loads((self.destination / "config/prepared.json").read_bytes())
        actual = json.loads((self.destination / "config/node-prebuilt.json").read_bytes())
        self.assertEqual(actual["target"], policy["target"])
        self.assertEqual(actual["python"]["path"], str(Path(sys.executable).resolve()))
        self.assertEqual(receipt["runtimes"]["node_prebuilt_policy"]["sha256"], sha(path.read_bytes()))
        self.assertEqual(receipt["runtimes"]["selected_node"]["sha256"], policy["target"]["node_sha256"])
        self.assertTrue((self.destination / "product-mcp.json").is_file())
        self.assertFalse((self.destination / "session").exists())


if __name__ == "__main__":
    unittest.main()
