"""Real preparation with controlled installer processes, plus cancellation."""
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import threading
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

import test_prepare_host as preparation_tests

SOURCE, sha = preparation_tests.SOURCE, preparation_tests.sha


SPEC = importlib.util.spec_from_file_location("bootstrap_host", SOURCE / "bootstrap_host.py")
bootstrap = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bootstrap)

INSTALLER = r'''
import argparse,hashlib,json,os,pathlib,sys
p=argparse.ArgumentParser()
for key in ('bundle','contents-sha256','family','python','destination'):
    p.add_argument('--'+key,required=True)
p.add_argument('--verify-existing',action='store_true')
a=p.parse_args()
d=pathlib.Path(a.destination)
mode=MODE
if not a.verify_existing:
    d.mkdir(mode=0o700)
    (d/'entered.json').write_text(json.dumps({'argv':sys.argv[1:],'environment':dict(os.environ)}))
    if mode=='partial':
        (d/'partial').write_text('keep this failed installation')
        sys.exit(2)
    (d/'runtime/bin').mkdir(parents=True)
    (d/'runtime/bin/python').symlink_to(a.python)
    r={'schema_version':1,'scope':'observed_software_installation_only','family':a.family,
       'destination':str(d),'contents_sha256':a.contents_sha256,
       'command':[str(d/'runtime/bin/python'),'-I','-m','meerkat_toolkit.adapters.services_cli']}
    raw=(json.dumps(r,sort_keys=True,separators=(',',':'))+'\n').encode()
    (d/'installed.json').write_bytes(raw)
    if mode=='changed-release':
        (pathlib.Path(a.bundle).parent/'licenses/notice.txt').write_text('changed')
    if mode=='changed-pin':
        pathlib.Path(CHANGED_PIN).write_text('changed runtime')
    if mode=='receipt':
        r['family']='agno'
    if mode=='malformed':
        print('not JSON');sys.exit(0)
    print(json.dumps(r))
else:
    (d/'verify-entered').write_text('readback')
    if mode=='verify-fails':
        sys.exit(2)
    raw=(d/'installed.json').read_bytes()
    r=json.loads(raw)
    v={'schema_version':1,'scope':'retained_payload_and_launch_binding_only','verified':True,
       'family':a.family,'destination':str(d),'contents_sha256':a.contents_sha256,
       'configuration_sha256':None,'command':r['command'],'selected_runtime_path':a.python,
       'historical_receipt':{'sha256':hashlib.sha256(raw).hexdigest()}}
    if mode=='verify-binding':
        v['historical_receipt']['sha256']='0'*64
    if mode=='changed-interpreter':
        (d/'runtime/bin/python').unlink()
        (d/'runtime/bin/python').symlink_to('/bin/sh')
    print(json.dumps(v))
'''


class BootstrapTests(unittest.TestCase):
    publish_release = preparation_tests.PreparationTests.publish_release
    selected_node = preparation_tests.PreparationTests.selected_node

    def setUp(self):
        preparation_tests.PreparationTests.setUp(self)
        self.args.client_directory = str(self.base / "new offline client")
        self.args.timeout_seconds = 30
        self.args.families = "agno,services"
        self.args.node = self.args.node_sha256 = None
        self.args.npm_cli = self.args.npm_cli_sha256 = None
        self.args.python = str(Path(sys.executable).resolve())
        self.args.python_sha256 = sha(Path(self.args.python).read_bytes())
        # These inputs belong to the preparer, not the bootstrap entry point.
        del self.args.mcp_python, self.args.mcp_python_sha256
        self.files["setup/prepare_host.py"] = ((SOURCE / "prepare_host.py").read_bytes(), 0o644)
        self.installer()

    def installer(self, mode="ok", changed_pin=""):
        code = "MODE=" + repr(mode) + "\nCHANGED_PIN=" + repr(changed_pin) + "\n" + INSTALLER
        self.files["consumers/install-library-consumer.py"] = (code.encode(), 0o644)
        self.publish_release()

    @property
    def client(self):
        return Path(self.args.client_directory)

    def refused(self, code):
        with self.assertRaisesRegex(Exception, code):
            bootstrap.bootstrap(self.args)
        self.assertFalse(Path(self.args.output).exists())
        self.assertFalse(Path(self.args.workspace).exists())

    def test_installer_readback_then_real_preparation_and_no_secret_inheritance(self):
        with patch.dict(os.environ, {"BOOTSTRAP_PRIVATE_TEST": "must not reach child"}):
            result = bootstrap.bootstrap(self.args)
        self.assertEqual(result["status"], "prepared")
        self.assertEqual(result["client"]["receipt_sha256"], sha((self.client / "installed.json").read_bytes()))
        entered = json.loads((self.client / "entered.json").read_text())
        self.assertEqual(entered["argv"], ["--bundle", str(self.release / "consumers"),
            "--contents-sha256", sha(self.files["consumers/CONTENTS.json"][0]),
            "--family", "services", "--python", self.args.python, "--destination", str(self.client)])
        self.assertNotIn("BOOTSTRAP_PRIVATE_TEST", entered["environment"])
        self.assertEqual(entered["environment"]["PIP_NO_INDEX"], "1")
        self.assertTrue((self.client / "verify-entered").is_file())
        output = Path(self.args.output)
        prepared = json.loads((output / "prepared.json").read_bytes())
        self.assertEqual(prepared["runtimes"]["mcp_python"]["path"], str(self.client / "runtime/bin/python"))
        self.assertEqual(result["host"]["receipt_sha256"], sha((output / "prepared.json").read_bytes()))
        self.assertIn(str(self.client / "runtime/bin/python"), json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"])
        self.assertFalse(Path(self.args.workspace).exists())
        self.assertFalse(list(self.base.glob(".toolkit-bootstrap-*")))
        self.assertFalse(list(self.release.rglob("__pycache__")))

    def test_invalid_selection_is_refused_before_installer(self):
        self.args.families = "unknown-family"
        self.refused("invalid_library_families")
        self.assertFalse(self.client.exists())

    def test_missing_required_runtime_is_refused_before_installer(self):
        self.args.families = "activepieces"
        self.refused("activepieces_runtime_required")
        self.assertFalse(self.client.exists())

    def test_invalid_scope_is_refused_before_installer(self):
        self.args.allow_public_pypi = True
        self.refused("explicit_pypi_scope_required")
        self.assertFalse(self.client.exists())

    def test_actual_output_workspace_overlap_is_not_hidden_by_preview(self):
        self.args.workspace = self.args.output
        self.refused("overlapping_directories")
        self.assertFalse(self.client.exists())

    def test_case_and_unicode_destination_aliases_refuse_before_installer(self):
        for first, second in (("NewConfig", "newconfig"), ("caf\u00e9", "cafe\u0301")):
            with self.subTest(first=first):
                self.args.output = str(self.base / first)
                self.args.workspace = str(self.base / second)
                self.refused("overlapping_directories")
                self.assertFalse(self.client.exists())

    def test_client_case_alias_refuses_before_installer(self):
        self.args.client_directory = str(self.base / "NewClient")
        self.args.workspace = str(self.base / "newclient")
        self.refused("overlapping_directories")
        self.assertFalse(self.client.exists())

    def test_case_alias_policy_applies_to_whole_parent_path(self):
        self.assertTrue(bootstrap.overlaps(self.base / "Different/Root", self.base / "different/root/child"))

    def test_client_overlap_staging_or_output_refuses(self):
        for value in (str(self.staging / "client"), self.args.output):
            self.args.client_directory = value
            self.refused("overlapping_directories")
            self.assertFalse(self.client.exists())

    def test_recipe_database_cannot_overlap_actual_output_or_client(self):
        for value in (self.args.output, self.args.client_directory):
            self.args.recipes_database = value
            self.refused("recipes_database_overlaps_host_paths")
            self.assertFalse(self.client.exists())

    def test_existing_client_is_never_reused_or_deleted(self):
        self.client.mkdir()
        original = self.client / "partial"
        original.write_text("preserve")
        self.refused("output_or_workspace_exists")
        self.assertEqual(original.read_text(), "preserve")

    def test_wrong_archive_and_python_hashes_refuse_before_installation(self):
        for key in ("release_sha256", "python_sha256"):
            with self.subTest(key=key):
                saved = getattr(self.args, key)
                setattr(self.args, key, "0" * 64)
                self.refused("changed")
                self.assertFalse(self.client.exists())
                setattr(self.args, key, saved)

    def test_changed_extracted_release_refuses_before_installation(self):
        (self.release / "licenses/notice.txt").write_text("changed")
        self.refused("extracted_release_changed")
        self.assertFalse(self.client.exists())

    def test_partial_installation_stays_unused_and_is_not_retried(self):
        self.installer("partial")
        self.refused("owner_refused")
        self.assertEqual((self.client / "partial").read_text(), "keep this failed installation")
        self.assertFalse((self.client / "verify-entered").exists())
        self.refused("output_or_workspace_exists")

    def test_mutated_release_after_install_refuses_readback_and_publication(self):
        self.installer("changed-release")
        self.refused("extracted_release_changed")
        self.assertTrue((self.client / "installed.json").exists())
        self.assertFalse((self.client / "verify-entered").exists())

    def test_mutated_optional_runtime_refuses_after_install(self):
        self.args.families = "activepieces"
        self.args.node, self.args.node_sha256 = str(self.runtime), sha(self.runtime.read_bytes())
        self.args.npm_cli, self.args.npm_cli_sha256 = str(self.pip), sha(self.pip.read_bytes())
        self.installer("changed-pin", str(self.runtime))
        self.refused("host_input_changed")
        self.assertFalse((self.client / "verify-entered").exists())

    def test_false_or_malformed_receipt_refuses(self):
        self.installer("receipt")
        self.refused("installation_receipt_mismatch")
        self.assertFalse((self.client / "verify-entered").exists())

    def test_malformed_owner_stdout_refuses(self):
        self.installer("malformed")
        self.refused("invalid_owner_result")
        self.assertTrue((self.client / "installed.json").exists())

    def test_readback_failure_preserves_client_and_prevents_publication(self):
        self.installer("verify-fails")
        self.refused("owner_refused")
        self.assertTrue((self.client / "verify-entered").exists())

    def test_wrong_readback_binding_refuses(self):
        self.installer("verify-binding")
        self.refused("verification_binding_mismatch")

    def test_changed_interpreter_after_readback_refuses(self):
        self.installer("changed-interpreter")
        self.refused("host_input_changed")

    def test_cancel_between_installation_and_readback_preserves_client(self):
        stopped = threading.Event()
        run = bootstrap.run_owner
        def stop_after_install(*args):
            value = run(*args)
            stopped.set()
            return value
        with patch.object(bootstrap, "run_owner", side_effect=stop_after_install):
            with self.assertRaisesRegex(bootstrap.BootstrapError, "bootstrap_cancelled"):
                bootstrap.bootstrap(self.args, stopped=stopped)
        self.assertTrue((self.client / "installed.json").exists())
        self.assertFalse((self.client / "verify-entered").exists())
        self.assertFalse(Path(self.args.output).exists())

    def test_already_cancelled_starts_nothing(self):
        stopped = threading.Event()
        stopped.set()
        with self.assertRaisesRegex(bootstrap.BootstrapError, "bootstrap_cancelled"):
            bootstrap.bootstrap(self.args, stopped=stopped)
        self.assertFalse(self.client.exists())

    def test_cancellation_after_final_publication_reports_unconfirmed_and_keeps_outputs(self):
        stopped = threading.Event()
        load = bootstrap.host.owner_module
        def load_owner(*args):
            owner = load(*args)
            prepare = owner.prepare
            calls = 0
            def cancelling_prepare(selection):
                nonlocal calls
                result = prepare(selection)
                calls += 1
                if calls == 2:
                    stopped.set()
                return result
            owner.prepare = cancelling_prepare
            return owner
        with patch.object(bootstrap.host, "owner_module", side_effect=load_owner):
            with self.assertRaisesRegex(bootstrap.BootstrapError, "bootstrap_cancelled"):
                bootstrap.bootstrap(self.args, stopped=stopped)
        self.assertTrue((self.client / "installed.json").exists())
        self.assertTrue((Path(self.args.output) / "prepared.json").exists())
        self.assertTrue((Path(self.args.output) / "mcp.json").exists())
        self.assertFalse(Path(self.args.workspace).exists())

    def test_timeout_validation(self):
        for value in (0, 901, True, 1.5):
            self.args.timeout_seconds = value
            self.refused("invalid_timeout")
        self.assertFalse(self.client.exists())

    def test_caller_cannot_replace_generated_mcp_interpreter(self):
        self.args.mcp_python = self.args.python
        self.refused("mcp_runtime_is_generated")
        self.assertFalse(self.client.exists())

    def test_cli_delegates_existing_scope_and_refuses_invalid_scope_before_install(self):
        arguments = []
        for name, value in vars(self.args).items():
            if value is None or value is False:
                continue
            arguments.append("--" + name.replace("_", "-"))
            if value is not True:
                arguments.append(str(value))
        with redirect_stdout(io.StringIO()) as output:
            self.assertEqual(bootstrap.main(arguments), 0)
        self.assertEqual(json.loads(output.getvalue())["host"]["directory"], self.args.output)

    def test_cli_preserves_preparation_diagnostic(self):
        self.args.families = "invalid"
        arguments = []
        for name, value in vars(self.args).items():
            if value is None or value is False:
                continue
            arguments.extend(["--" + name.replace("_", "-"), str(value)])
        with redirect_stderr(io.StringIO()) as error, self.assertRaises(SystemExit) as raised:
            bootstrap.main(arguments)
        self.assertEqual(raised.exception.code, 1)
        self.assertIn("invalid_library_families", error.getvalue())
        self.assertFalse(self.client.exists())

    def test_cli_node_search_retains_separate_acquisition_permission(self):
        preparation_tests.PreparationTests.npm_acquisition(self)
        self.args.allow_public_npm = False
        self.args.enable_node_search = True
        self.args.selected_npm_cli = self.args.selected_npm_cli_sha256 = None
        arguments = []
        for name, value in vars(self.args).items():
            if value is None or value is False:
                continue
            arguments.append("--" + name.replace("_", "-"))
            if value is not True:
                arguments.append(str(value))
        with redirect_stdout(io.StringIO()):
            self.assertEqual(bootstrap.main(arguments), 0)
        output = Path(self.args.output)
        self.assertIs(json.loads((output / "node-acquisition.json").read_bytes())["allow_acquisition"], False)
        argv = json.loads((output / "mcp.json").read_bytes())["mcpServers"]["toolkit-setup"]["args"]
        self.assertIn("--allow-node-search", argv)
        self.assertNotIn("--selected-npm-cli", argv)

    def test_bounded_child_timeout_and_output(self):
        stopped = threading.Event()
        for script, code in (("import time;time.sleep(60)", "bootstrap_timeout"),
                             ("print('x'*10000)", "owner_output_limit")):
            with self.subTest(code=code), patch.object(bootstrap, "OUTPUT_LIMIT", 4096):
                with self.assertRaisesRegex(bootstrap.BootstrapError, code):
                    bootstrap.run_owner([self.args.python, "-I", "-c", script], self.base,
                                        time.monotonic() + 0.3, stopped)

    def test_cancellation_allows_installer_to_settle_its_own_child(self):
        entered, settled = self.base / "child-started", self.base / "child-settled"
        script = '''import pathlib,signal,subprocess,sys,time
child=subprocess.Popen([sys.executable,'-I','-c','import time;time.sleep(60)'],start_new_session=True)
pathlib.Path(sys.argv[1]).write_text(str(child.pid))
def stop(*_):
    child.terminate();child.wait(timeout=2)
    pathlib.Path(sys.argv[2]).write_text('settled')
    sys.exit(2)
signal.signal(signal.SIGTERM,stop)
while True:time.sleep(.02)
'''
        stopped = threading.Event()
        def request_stop():
            deadline = time.monotonic() + 3
            while not entered.exists() and time.monotonic() < deadline:
                time.sleep(.01)
            stopped.set()
        thread = threading.Thread(target=request_stop)
        thread.start()
        try:
            with self.assertRaisesRegex(bootstrap.BootstrapError, "bootstrap_cancelled"):
                bootstrap.run_owner([self.args.python, "-I", "-c", script, str(entered), str(settled)],
                                    self.base, time.monotonic() + 5, stopped)
        finally:
            thread.join(timeout=4)
        self.assertEqual(settled.read_text(), "settled")
        with self.assertRaises(ProcessLookupError):
            os.kill(int(entered.read_text()), 0)


if __name__ == "__main__":
    unittest.main()
