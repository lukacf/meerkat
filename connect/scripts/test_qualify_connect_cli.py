"""Exercise qualification failures without building or running Connect."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "qualify_connect_cli", Path(__file__).with_name("qualify-connect-cli.py")
)
QUALIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(QUALIFY)


class QualificationFailures(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="connect-qualification-control-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()

    def run_child(self, label, code, bound):
        real_popen = QUALIFY.subprocess.Popen

        def reap(child):
            if child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            child.wait(timeout=2)

        def entered(argv, **kwargs):
            child = real_popen(argv, **kwargs)
            if argv[0] == sys.executable:
                self.addCleanup(reap, child)
            return child

        # A regression in the owner must not leave this control's child alive.
        with patch.object(QUALIFY.subprocess, "Popen", side_effect=entered):
            return QUALIFY.run_command(
                self.root, label, [sys.executable, "-c", code], self.root, {}, bound
            )

    def source_tree(self):
        source = self.root / "source"
        for relative in (
            "Cargo.toml", "Cargo.lock", "native-candidate/Cargo.toml", "native-candidate/Cargo.lock",
            "scripts/repo-cargo", "scripts/qualify-connect-cli.py", "scripts/assemble-connect-product.py",
            "sdk/consumer-setup/product/prepare.py", "crates/meerkat-connect/src/lib.rs",
            "crates/toolkit-package/src/lib.rs", "crates/toolkit-store/src/lib.rs",
            "crates/toolkit-catalog/src/lib.rs", "crates/meerkat-toolkit/src/lib.rs",
            "crates/toolkit-mobkit/src/lib.rs",
        ):
            path = source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"INERT SOURCE INPUT - " + relative.encode() + b"\n")
        return source

    def test_consumed_build_selects_candidate_and_retains_toolkit_source_identity(self):
        source = self.source_tree()
        component = self.root / "inert-component"
        component.write_bytes(b"INERT - NEVER EXTRACT\n")
        destination = self.root / "qualification"

        def failed_build(output, label, argv, cwd, environment, bound):
            record = {"passed": False, "argv": list(map(str, argv))}
            QUALIFY.save(output / (label + ".json"), record)
            return record

        with patch.object(QUALIFY, "ROOT", source), \
             patch.object(QUALIFY, "__file__", str(source / "scripts/qualify-connect-cli.py")), \
             patch.object(QUALIFY.subprocess, "Popen") as spawned:
            before = QUALIFY.source_inputs()
            with patch.object(QUALIFY, "preflight", return_value=(
                    {}, {"component": component, "resources_sha256": "a" * 64}, {}, before)), \
                 patch.object(QUALIFY, "run_command", side_effect=failed_build) as entered:
                self.assertEqual(QUALIFY.qualify(destination), 1)
            entered.assert_called_once()
            args = entered.call_args.args
            self.assertEqual(args[1], "build")
            self.assertEqual(list(map(str, args[2])), [
                str(source / "scripts/repo-cargo"), "test", "--locked", "--offline",
                "--manifest-path", str(source / "native-candidate/Cargo.toml"),
                "-p", "meerkat-connect", "--test", "cli_paired_host", "--test", "packaged_setup",
                "--no-run", "--message-format=json",
            ])
            self.assertEqual(args[3], source)
            self.assertEqual(args[4]["MEERKAT_CONNECT_RESOURCES_SHA256"], "a" * 64)
            self.assertEqual(args[4]["CARGO_NET_OFFLINE"], "true")
            spawned.assert_not_called()
        output = destination / "evidence"
        receipt = json.loads((output / "result.json").read_text())
        self.assertEqual(receipt["cargo_manifest"], str(source / "native-candidate/Cargo.toml"))
        self.assertEqual(receipt["source_scope"],
                         "toolkit_inputs_only_external_native_identity_requires_lane_evidence")
        self.assertEqual(receipt["failure"], "resource-bound build did not pass")
        self.assertEqual(receipt["executions"], [])
        self.assertTrue(receipt["sources_unchanged"])
        self.assertEqual(json.loads((output / "source-inputs.json").read_text()), before)
        self.assertEqual(json.loads((output / "source-inputs-after.json").read_text()), before)

    def test_candidate_selection_and_local_dependency_changes_are_observed(self):
        source = self.source_tree()
        with patch.object(QUALIFY, "ROOT", source), \
             patch.object(QUALIFY, "__file__", str(source / "scripts/qualify-connect-cli.py")), \
             patch.object(QUALIFY.subprocess, "Popen") as entered:
            before = QUALIFY.source_inputs()
            for relative in (
                "native-candidate/Cargo.toml", "native-candidate/Cargo.lock",
                "crates/toolkit-catalog/src/lib.rs", "crates/meerkat-toolkit/src/lib.rs",
                "crates/toolkit-mobkit/src/lib.rs",
            ):
                with self.subTest(relative=relative):
                    self.assertIn(relative, before)
                    path = source / relative
                    original = path.read_bytes()
                    path.write_bytes(original + b"CHANGED\n")
                    after = QUALIFY.source_inputs()
                    self.assertNotEqual(after[relative]["sha256"], before[relative]["sha256"])
                    self.assertEqual({name for name in before if before[name] != after[name]}, {relative})
                    path.write_bytes(original)
            entered.assert_not_called()

    def test_missing_candidate_lock_refuses_before_resource_loading_or_build_entry(self):
        source = self.source_tree()
        (source / "native-candidate/Cargo.lock").unlink()
        destination = self.root / "qualification"
        with patch.object(QUALIFY, "ROOT", source), \
             patch.object(QUALIFY, "__file__", str(source / "scripts/qualify-connect-cli.py")), \
             patch.object(QUALIFY.platform, "system", return_value="Darwin"), \
             patch.object(QUALIFY.platform, "machine", return_value="arm64"), \
             patch.object(QUALIFY.platform, "mac_ver", return_value=("15.0", (), "")), \
             patch.object(QUALIFY.runpy, "run_path") as loaded, \
             patch.object(QUALIFY, "run_command") as entered, \
             patch.object(QUALIFY.subprocess, "Popen") as spawned:
            with self.assertRaises(FileNotFoundError):
                QUALIFY.qualify(destination)
            loaded.assert_not_called()
            entered.assert_not_called()
            spawned.assert_not_called()
        self.assertFalse(destination.exists())

    def test_existing_destination_and_wrong_platform_stop_before_build(self):
        sentinel = self.root / "sentinel"
        sentinel.write_bytes(b"leave existing work alone")
        with patch.object(QUALIFY.platform, "system", return_value="Darwin"), \
             patch.object(QUALIFY.platform, "machine", return_value="arm64"), \
             patch.object(QUALIFY.platform, "mac_ver", return_value=("15.0", (), "")), \
             patch.object(QUALIFY.subprocess, "Popen") as entered:
            with self.assertRaisesRegex(ValueError, "fresh canonical destination outside checkout required"):
                QUALIFY.qualify(self.root)
            entered.assert_not_called()
        self.assertEqual(list(self.root.iterdir()), [sentinel])
        self.assertEqual(sentinel.read_bytes(), b"leave existing work alone")
        fresh = self.root / "fresh"
        with patch.object(QUALIFY.platform, "system", return_value="Linux"), \
             patch.object(QUALIFY.subprocess, "Popen") as entered:
            with self.assertRaisesRegex(ValueError, "local Apple Silicon Mac required"):
                QUALIFY.qualify(fresh)
            entered.assert_not_called()
        self.assertFalse(fresh.exists())

    def test_missing_ambiguous_and_unsuccessful_cargo_artifacts_are_refused(self):
        messages = []
        for name, kind, source in (
            ("meerkat-connect", "bin", "src/main.rs"),
            ("cli_paired_host", "test", "tests/cli_paired_host.rs"),
            ("packaged_setup", "test", "tests/packaged_setup.rs"),
        ):
            binary = self.root / name
            binary.write_bytes(b"inert artifact-selection fixture")
            messages.append({
                "reason": "compiler-artifact", "executable": str(binary),
                "target": {"name": name, "kind": [kind],
                           "src_path": str(QUALIFY.ROOT / "crates/meerkat-connect" / source)},
                "profile": {"test": kind == "test"},
            })
        successful = {"reason": "build-finished", "success": True}
        output = self.root / "cargo.jsonl"
        for bad, reason in (
            (messages[:-1] + [successful], "missing successful Cargo artifacts"),
            (messages + [messages[0], successful], "ambiguous Cargo artifact"),
            (messages + [{"reason": "build-finished", "success": False}], "missing successful Cargo artifacts"),
        ):
            with self.subTest(messages=bad):
                output.write_text("".join(json.dumps(row) + "\n" for row in bad))
                with patch.object(QUALIFY.subprocess, "Popen") as entered:
                    with self.assertRaisesRegex(ValueError, reason):
                        QUALIFY.cargo_artifacts(output)
                    entered.assert_not_called()

    def test_timeout_reaps_the_disposable_child_and_cannot_pass(self):
        code = "import signal,time; signal.signal(signal.SIGINT, signal.SIG_IGN); time.sleep(60)"
        with patch.object(QUALIFY, "JOIN_BOUND", 0.25), \
             patch.object(QUALIFY, "KILL_BOUND", 2):
            record = self.run_child("timeout", code, 0.5)
        self.assertFalse(record["passed"])
        self.assertTrue(any("observation_deadline" in error for error in record["errors"]))
        self.assertTrue(record["operator_signals"])
        self.assertTrue(record["direct_child_reaped"])
        self.assertTrue(record["process_group_absent"])
        self.assertNotEqual(record["exit_code"], 0)
        self.assertEqual(record["remaining_processes"], [])
        with self.assertRaises(ProcessLookupError):
            os.killpg(record["pid"], 0)
        retained = json.loads((self.root / "timeout.json").read_text())
        self.assertEqual(retained, record)

    def test_observation_failure_still_reaps_child_and_cannot_pass(self):
        with patch.object(QUALIFY, "process_rows", side_effect=OSError("fixture observation failure")), \
             patch.object(QUALIFY, "JOIN_BOUND", 0.25), \
             patch.object(QUALIFY, "KILL_BOUND", 1):
            record = self.run_child("observation-failure", "import time; time.sleep(60)", 1)
        self.assertFalse(record["passed"])
        self.assertTrue(any(error == "process_observation:OSError" for error in record["errors"]))
        self.assertTrue(record["direct_child_reaped"])
        self.assertTrue(record["process_group_absent"])
        with self.assertRaises(ProcessLookupError):
            os.killpg(record["pid"], 0)


if __name__ == "__main__":
    unittest.main()
