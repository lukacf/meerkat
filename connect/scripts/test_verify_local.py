#!/usr/bin/env python3
"""Exercise verifier receipts and selection with a fake Cargo wrapper only."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("verify-local")
FAKE_WRAPPER = r'''
import json
from pathlib import Path
import sys
root = Path(__file__).resolve().parents[1]
control = json.loads((root / 'control.json').read_text())
args = sys.argv[1:]
with (root / 'calls.jsonl').open('a') as output:
    output.write(json.dumps(args) + '\n')
if args == ['--print-env']:
    print('CARGO_HOME=' + control['cargo_home'])
    print('CARGO_TARGET_DIR=' + control['target_directory'])
    sys.exit(0)
if args[0] == 'metadata':
    metadata_count = sum(json.loads(line)[0] == 'metadata'
                         for line in (root / 'calls.jsonl').read_text().splitlines())
    if control.get('metadata_failure') or (control.get('metadata_after_failure') and metadata_count > 1):
        print('fixture: missing native prerequisite', file=sys.stderr)
        sys.exit(101)
    if control.get('invalid_metadata'):
        print('not json')
    else:
        manifest = args[args.index('--manifest-path') + 1]
        metadata = control['metadata'][manifest]
        if metadata_count > 1 and 'extra_dependency_after' in control:
            metadata['packages'].append(control['extra_dependency_after'])
        print(json.dumps(metadata))
    sys.exit(0)
if args[0] == control.get('mutate_on'):
    Path(control['mutate_path']).write_text(control['mutate_contents'])
if args[0] == control.get('fail_on'):
    print('fixture command failed', file=sys.stderr)
    sys.exit(9)
print('Fake wrapper: no compilation performed.')
'''


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class VerifyLocalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="verify-local-tests-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / "toolkit with spaces"
        (self.root / "scripts").mkdir(parents=True)
        (self.root / "docs/spec").mkdir(parents=True)
        shutil.copy2(SCRIPT, self.root / "scripts/verify-local")
        self.write(self.root / "scripts/repo-cargo", "#!" + sys.executable + "\n" + FAKE_WRAPPER)
        (self.root / "scripts/repo-cargo").chmod(0o755)
        self.write(self.root / "AGENTS.md", "Fixture instructions.\n")
        self.write(self.root / "docs/spec/meerkat-toolkit-capability-acquisition.md", "Fixture spec.\n")
        self.write(self.root / "Cargo.toml", "[workspace]\nmembers=[]\n")
        self.write(self.root / "Cargo.lock", "version = 4\n")
        self.write(self.root / ".cargo/config.toml", "[build]\njobs=1\n")
        self.cargo_home = self.base / "cargo-home"
        self.write(self.cargo_home / "config.toml", "[net]\noffline=true\n")
        self.target = self.root / "crates/alpha/target"
        self.write(self.target / "generated", "Ignored target output.\n")
        self.alpha = self.package(self.root / "crates/alpha", "toolkit-package")
        self.beta = self.package(self.root / "crates/beta", "toolkit-store")
        self.native = self.package(self.root / "crates/native", "native")
        self.external_root = self.base / "external owners"
        self.write(self.external_root / "Cargo.toml", "[workspace]\nmembers=[]\n")
        self.write(self.external_root / "Cargo.lock", "version = 4\n")
        self.write(self.external_root / "rust-toolchain.toml", '[toolchain]\nchannel="stable"\n')
        self.write(self.external_root / ".cargo/config.toml", "[build]\njobs=2\n")
        self.external = self.package(self.external_root / "crates/runtime", "runtime")
        self.external_asset = Path(self.external["manifest_path"]).parent / "assets/catalog.json"
        self.write(self.external_asset, '{"version":1}\n')
        self.write(self.external_root / "unrelated/private-file", "Must not inventory an upstream monorepo.\n")
        self.candidate = self.root / "native-candidate/Cargo.toml"
        self.write(self.candidate, "[workspace]\nmembers=[]\n")
        self.write(self.candidate.with_name("Cargo.lock"), "version = 4\n")
        registry = {"id": "registry:dependency", "name": "dependency", "source": "registry+fixture",
                    "manifest_path": str(self.base / "not-installed/Cargo.toml"), "targets": []}
        self.control = {
            "cargo_home": str(self.cargo_home), "target_directory": str(self.target),
            "metadata": {
                str(self.root / "Cargo.toml"): self.metadata(self.root, [self.alpha, self.beta], [registry]),
                str(self.candidate): self.metadata(self.candidate.parent, [self.native], [self.external, registry]),
            },
        }

    @staticmethod
    def write(path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def package(self, base, name):
        self.write(base / "Cargo.toml", f'[package]\nname="{name}"\nversion="0.1.0"\n')
        self.write(base / "src/lib.rs", "pub fn fixture() {}\n")
        self.write(base / "tests/fixture.rs", "#[test] fn fixture() {}\n")
        self.write(base / "build.rs", "fn main() {}\n")
        return {"id": "path:" + str(base), "name": name, "source": None,
                "manifest_path": str(base / "Cargo.toml"),
                "targets": [{"src_path": str(base / "src/lib.rs")}, {"src_path": str(base / "build.rs")}]}

    def metadata(self, workspace, members, dependencies):
        return {"workspace_root": str(workspace), "workspace_members": [p["id"] for p in members],
                "packages": [*members, *dependencies], "target_directory": str(self.target)}

    def invoke(self, *args):
        (self.root / "control.json").write_text(json.dumps(self.control))
        result = subprocess.run([sys.executable, str(self.root / "scripts/verify-local"), *args],
                                cwd=self.root, capture_output=True, text=True, timeout=20,
                                env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
        receipts = list((self.root / "docs/verification").glob("*/run.json"))
        self.assertEqual(len(receipts), 1, result.stdout + result.stderr)
        receipt = json.loads(receipts[0].read_text())
        calls_path = self.root / "calls.jsonl"
        calls = [json.loads(line) for line in calls_path.read_text().splitlines()] if calls_path.exists() else []
        self.assertEqual(result.returncode == 0, receipt["result"] == "passed", receipt)
        for command in receipt["commands"]:
            for key in ("output", "stderr"):
                self.assertEqual(command[key + "_sha256"], sha(receipts[0].parent / command[key]))
        return result, receipt, calls

    def test_default_uses_actual_members_all_features_and_local_sources_only(self):
        result, receipt, calls = self.invoke()
        self.assertEqual(result.returncode, 0, receipt)
        self.assertEqual([p["name"] for p in receipt["workspace"]["members"]], ["toolkit-package", "toolkit-store"])
        self.assertEqual(receipt["workspace_kind"], "portable_package_store")
        self.assertIn("separate native candidate is not validated", receipt["scope"])
        self.assertEqual([c[0] for c in calls], ["--print-env", "metadata", "fmt", "test", "clippy", "metadata"])
        for call in calls[1:]:
            self.assertEqual(call[call.index("--manifest-path") + 1], str(self.root / "Cargo.toml"))
            if call[0] != "fmt":
                self.assertIn("--all-features", call)
                self.assertIn("--locked", call)
        fmt = next(c for c in calls if c[0] == "fmt")
        self.assertEqual(fmt[3:], ["-p", "toolkit-package", "-p", "toolkit-store", "--", "--check"])
        sources = receipt["source_sha256"]
        self.assertIn("crates/alpha/tests/fixture.rs", sources)
        self.assertIn("crates/alpha/build.rs", sources)
        self.assertIn(".cargo/config.toml", sources)
        self.assertIn(str(self.cargo_home / "config.toml"), sources)
        self.assertNotIn("crates/alpha/target/generated", sources)
        self.assertFalse(any("native" in name or "external owners" in name for name in sources))
        self.assertTrue(receipt["source_unchanged"])

    def test_candidate_records_actual_external_owner_and_assets(self):
        result, receipt, calls = self.invoke("--manifest-path", "native-candidate/Cargo.toml")
        self.assertEqual(result.returncode, 0, receipt)
        self.assertEqual([p["name"] for p in receipt["workspace"]["members"]], ["native"])
        self.assertEqual(receipt["workspace_kind"], "native_candidate")
        self.assertIn("not a released native integration", receipt["scope"])
        self.assertNotIn("native-host", receipt["scope"])
        self.assertEqual([p["name"] for p in receipt["workspace"]["local_packages"]], ["native", "runtime"])
        sources = receipt["source_sha256"]
        for path in (self.external_asset, self.external_root / "Cargo.toml", self.external_root / "Cargo.lock",
                     self.external_root / "rust-toolchain.toml", self.external_root / ".cargo/config.toml"):
            self.assertEqual(sources[str(path)], sha(path))
        self.assertFalse(any("private-file" in name for name in sources))
        self.assertIn("native-candidate/Cargo.lock", sources)
        self.assertTrue(all(str(self.candidate) in c for c in calls[1:]))

    def test_unknown_member_selection_gets_no_portable_or_native_label(self):
        self.control["metadata"][str(self.root / "Cargo.toml")]["packages"][0]["name"] = "different-package"
        result, receipt, _ = self.invoke()
        self.assertEqual(result.returncode, 0, receipt)
        self.assertEqual(receipt["workspace_kind"], "other_selected_workspace")
        self.assertIn("different-package", receipt["scope"])

    def test_missing_candidate_records_failure_before_cargo(self):
        result, receipt, calls = self.invoke("--manifest-path", "missing/Cargo.toml")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(calls, [])
        self.assertEqual(receipt["failure"]["type"], "FileNotFoundError")

    def test_metadata_failure_is_retained_without_compilation(self):
        self.control["metadata_failure"] = True
        result, receipt, calls = self.invoke("--manifest-path", str(self.candidate))
        self.assertEqual(result.returncode, 1)
        self.assertEqual([c[0] for c in calls], ["--print-env", "metadata"])
        self.assertEqual(receipt["commands"][-1]["exit_code"], 101)
        self.assertNotIn("source_sha256", receipt)

    def test_invalid_metadata_is_retained_without_compilation(self):
        self.control["invalid_metadata"] = True
        result, receipt, calls = self.invoke()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(receipt["failure"]["type"], "JSONDecodeError")
        self.assertEqual([c[0] for c in calls], ["--print-env", "metadata"])

    def test_missing_local_source_or_required_lock_cannot_pass(self):
        for relative in ("Cargo.lock", "crates/alpha/src/lib.rs", "crates/alpha/Cargo.toml"):
            with self.subTest(relative=relative):
                path = self.root / relative
                original = path.read_bytes()
                path.unlink()
                result, receipt, calls = self.invoke()
                self.assertEqual(result.returncode, 1)
                self.assertEqual([c[0] for c in calls], ["--print-env", "metadata"])
                self.assertEqual(receipt["failure"]["type"], "FileNotFoundError")
                path.write_bytes(original)
                shutil.rmtree(self.root / "docs/verification")
                (self.root / "calls.jsonl").unlink()

    def test_fmt_failure_prevents_test_and_clippy_but_checks_identity(self):
        self.control["fail_on"] = "fmt"
        result, receipt, calls = self.invoke()
        self.assertEqual(result.returncode, 1)
        self.assertEqual([c[0] for c in calls], ["--print-env", "metadata", "fmt", "metadata"])
        self.assertTrue(receipt["source_unchanged"])
        self.assertIn("fmt exited 9", receipt["failure"]["message"])

    def test_failed_final_metadata_cannot_produce_a_pass(self):
        self.control["metadata_after_failure"] = True
        result, receipt, calls = self.invoke()
        self.assertEqual(result.returncode, 1)
        self.assertEqual([c[0] for c in calls], ["--print-env", "metadata", "fmt", "test", "clippy", "metadata"])
        self.assertEqual(receipt["commands"][-1]["exit_code"], 101)
        self.assertNotIn("source_unchanged", receipt)
        self.assertIn("metadata-after exited 101", receipt["failure"]["message"])

    def test_changed_local_dependency_set_fails_final_identity(self):
        self.control["extra_dependency_after"] = self.external
        result, receipt, _ = self.invoke()
        self.assertEqual(result.returncode, 1)
        self.assertFalse(receipt["source_unchanged"])
        self.assertNotIn(str(self.external_asset), receipt["source_sha256"])
        self.assertIn(str(self.external_asset), receipt["source_after_sha256"])
        self.assertNotEqual(receipt["workspace"], receipt["workspace_after"])

    def test_external_asset_mutation_fails_final_identity(self):
        self.control.update(mutate_on="test", mutate_path=str(self.external_asset), mutate_contents='{"version":2}\n')
        result, receipt, _ = self.invoke("--manifest-path", str(self.candidate))
        self.assertEqual(result.returncode, 1)
        self.assertFalse(receipt["source_unchanged"])
        self.assertNotEqual(receipt["source_sha256"][str(self.external_asset)], receipt["source_after_sha256"][str(self.external_asset)])

    def test_generated_target_mutation_does_not_change_source_identity(self):
        self.control.update(mutate_on="test", mutate_path=str(self.target / "generated"), mutate_contents="New build output.\n")
        result, receipt, _ = self.invoke()
        self.assertEqual(result.returncode, 0, receipt)
        self.assertTrue(receipt["source_unchanged"])

    def test_symlinked_source_fails_without_silently_omitting_it(self):
        (self.root / "crates/alpha/assets").symlink_to(self.external_asset.parent, target_is_directory=True)
        result, receipt, calls = self.invoke()
        self.assertEqual(result.returncode, 1)
        self.assertEqual([c[0] for c in calls], ["--print-env", "metadata"])
        self.assertIn("symlinked source directory", receipt["failure"]["message"])

    def test_missing_wrapper_still_writes_failed_receipt(self):
        (self.root / "scripts/repo-cargo").unlink()
        result, receipt, calls = self.invoke()
        self.assertEqual(result.returncode, 1)
        self.assertEqual(calls, [])
        self.assertEqual(receipt["failure"]["type"], "FileNotFoundError")
        self.assertEqual(receipt["commands"][0]["argv"], ["./scripts/repo-cargo", "--print-env"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
