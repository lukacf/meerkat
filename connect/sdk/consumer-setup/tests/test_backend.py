"""Controller boundary tests. Package installation is a separate real-host check."""
import copy
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / "backend.py"
spec = importlib.util.spec_from_file_location("consumer_setup_backend", SOURCE)
backend = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backend)


def profiles(family):
    if family == "services":
        return {"profiles": [{"profile": "slack", "operations": [
            {"operation": "auth_test", "tool": "slack_auth_test", "effect": "read"},
            {"operation": "read_interval", "tool": "slack_read_interval", "effect": "read"}]}]}
    if family == "activepieces":
        return {"profiles": [{"profile": "test-write", "operations": ["test_write"], "effect": "write"}]}
    return {"profiles": [{"profile": "searxng", "operations": ["search_web"]}]}


def help_source(family):
    row = copy.deepcopy(profiles(family)["profiles"][0])
    row.update(prerequisites=[{"kind": "host_deployment", "instruction": "Fixture prerequisite"}],
               unresolved_details=["Fixture unresolved detail"], scope="Field requirements only")
    if family == "services":
        row["config_fields"] = {
            "token_env": {"kind": "string", "credential_reference": True},
            "timeout_seconds": {"kind": "integer", "minimum": 1, "maximum": 60},
            "max_interval_requests": {"kind": "integer", "only_when_operation_selected": "read_interval"}}
    elif family == "activepieces":
        row["fields"] = [{"name": "profile", "kind": "string", "required": True},
                         {"name": "token_env", "kind": "string", "required": True, "credential_reference": True},
                         {"name": "timeout_ms", "kind": "integer", "required": False}]
    else:
        row["config_fields"] = [{"name": "host", "kind": "string", "required": True},
                                {"name": "timeout", "kind": "integer", "required": True},
                                {"name": "optional_flag", "kind": "boolean", "required": False}]
    return row


class FakeRunner:
    """Deterministic package-owner fixture; never executes an installer."""
    def __init__(self, workspace):
        self.workspace = workspace
        self.calls = []
        self.invalid = False
        self.block = None
        self.entered = threading.Event()
        self.bad_receipt = False
        self.nonzero_after_publish = False

    def run(self, argv, capture=False):
        self.calls.append((list(argv), capture))
        if "--destination" in argv:
            def argument(flag):
                return argv[argv.index(flag) + 1]
            family, destination = argument("--family"), Path(argument("--destination"))
            self.entered.set()
            if self.block:
                self.block.wait(5)
            destination.mkdir()
            if family == "activepieces":
                command = [argument("--node"), str(destination / "node_modules/@rkat/toolkit-activepieces/dist/mcp-cli.js")]
            else:
                module = "meerkat_toolkit_agno.agno_mcp" if family == "agno" else "meerkat_toolkit_services.adapters.services_mcp"
                command = [str(destination / "runtime/bin/python"), "-I", "-m", module]
            receipt = {"family": family, "destination": str(destination),
                       "scope": "observed_software_installation_only",
                       "contents_sha256": argument("--contents-sha256"), "command": command}
            if "--config" in argv:
                data = Path(argument("--config")).read_bytes()
                config = destination / "configuration/selected.json"
                config.parent.mkdir()
                config.write_bytes(data)
                command += ["serve", "--config", str(config)]
                receipt["configuration"] = {"path": str(config), "sha256": backend.digest(data),
                                            "server_name": argument("--server-name"), "validation": "configuration_only"}
                mcp = {"mcpServers": {argument("--server-name"): {"command": command[0], "args": command[1:]}}}
                receipt["mcp_configuration"] = mcp
                ready = destination / "ready"
                ready.mkdir()
                (ready / "mcp.json").write_bytes(backend.encoded(mcp))
                if self.bad_receipt:
                    receipt["family"] = "unexpected"
                (ready / "installed.json").write_bytes(backend.encoded(receipt))
            else:
                (destination / "installed.json").write_bytes(backend.encoded(receipt))
            return (2 if self.nonzero_after_publish else 0), b"untrusted child output"
        family = "activepieces" if any("activepieces" in arg for arg in argv) else "services" if any("services" in arg for arg in argv) else "agno"
        if "profiles" in argv:
            return 0, backend.encoded(profiles(family))
        if "config-help" in argv:
            return 0, backend.encoded(help_source(family))
        if "check-config" in argv:
            configuration = backend.decode(Path(argv[argv.index("--config") + 1]).read_bytes())
            return (2 if self.invalid else 0), backend.encoded({"profile": configuration["profile"],
                "valid": True, "scope": "configuration_only", "diagnostic": "CONFIGURATION_VALUE_MUST_NOT_ESCAPE"})
        raise AssertionError(argv)


class FixtureMixin:
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.bundle = self.root / "source"
        self.bundle.mkdir()
        files = {"install-library-consumer.py": b"# inert fixture\n"}
        for family in backend.FAMILIES:
            files[family + "/PROFILE-GUIDE.md"] = ("Exact " + family + " guide\n").encode()
        for name, data in files.items():
            path = self.bundle / name
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(data)
        self.manifest = {"files": {name: {"sha256": backend.digest(data)} for name, data in files.items()}}
        self.write_manifest()
        self.controllers = []
        self.attempts = 0

    def tearDown(self):
        for controller in self.controllers:
            if controller.runner.block:
                controller.runner.block.set()
            controller.join()
        self.temp.cleanup()

    def write_manifest(self):
        raw = backend.encoded(self.manifest)
        (self.bundle / "CONTENTS.json").write_bytes(raw)
        self.sha = backend.digest(raw)

    def controller(self, **changes):
        args = dict(bundle=self.bundle, contents_sha256=self.sha,
                    workspace=self.root / ("work" + str(self.attempts)),
                    families=backend.FAMILIES, python=sys.executable,
                    node=sys.executable, npm_cli=sys.executable, runner_factory=FakeRunner)
        args.update(changes)
        self.attempts += 1
        result = backend.Controller(**args)
        self.controllers.append(result)
        return result

    def prepared(self, family="agno"):
        controller = self.controller()
        controller.prepare({"family": family})
        controller.join()
        self.assertEqual(controller.state()["job"]["status"], "succeeded")
        return controller

    def request(self):
        return {"family": "agno", "profile": "searxng", "operations": ["search_web"],
                "values": {"host": "https://127.0.0.1:1234", "timeout": 10}}

    def checked(self, controller):
        request = self.request()
        validation = controller.check(request)
        return dict(request, validation_id=validation["validation_id"], configuration_sha256=validation["configuration_sha256"])

    def error(self, code, call):
        with self.assertRaises(backend.SetupError) as caught:
            call()
        self.assertEqual(caught.exception.code, code)


def validation_deadline_probe(root: Path, mode: str, action: str):
    """Run only in an externally supervised child, never in the test process."""
    fixture = ControllerTests("runTest")
    fixture.setUp()
    try:
        controller = fixture.prepared()
        request = fixture.checked(controller) if action == "install" else fixture.request()
        child_code = (
            "import os, pathlib, subprocess, sys, time; "
            "root = pathlib.Path(sys.argv[1]); "
            "(root / 'validator.pid').write_text(str(os.getpid())); "
            "print('CONFIGURATION_VALUE_MUST_NOT_ESCAPE', flush=True); "
        )
        if mode == "hung":
            child_code += "time.sleep(300)"
        else:
            child_code += (
                "child = subprocess.Popen([sys.executable, '-I', '-B', '-c', "
                "'import time; time.sleep(300)']); "
                "(root / 'descendant.pid').write_text(str(child.pid))"
            )

        class ValidatorRunner(FakeRunner):
            def run(self, argv, capture=False):
                if "check-config" in argv:
                    self.calls.append((list(argv), capture))
                    return backend.Runner(self.workspace).run(
                        [sys.executable, "-I", "-B", "-c", child_code, str(root)],
                        capture=capture)
                return super().run(argv, capture=capture)

        controller.runner = ValidatorRunner(controller.workspace)
        error = None
        started = time.monotonic()
        with patch.object(backend, "INSPECTION_TIMEOUT", 0.3):
            if action == "install":
                controller.install(request)
                controller.join()
            else:
                try:
                    controller.check(request)
                except backend.SetupError as caught:
                    error = caught.code
        unlocked = controller.operation_lock.acquire(blocking=False)
        if unlocked:
            controller.operation_lock.release()
        observed = {
            "action": action, "mode": mode, "elapsed": time.monotonic() - started,
            "error": error, "validations": len(controller.validations),
            "job": controller.state()["job"], "operation_lock_released": unlocked,
            "installer_calls": sum("--destination" in argv for argv, _ in controller.runner.calls),
            "validator_capture": [capture for argv, capture in controller.runner.calls if "check-config" in argv],
            "installation_entries": list(path.name for path in (controller.workspace / "installations").iterdir()),
        }
        (root / "observed.json").write_bytes(backend.encoded(observed))
    finally:
        fixture.tearDown()


class ControllerTests(FixtureMixin, unittest.TestCase):
    def deadline_case(self, mode, action):
        root = self.root / (mode + "-" + action)
        root.mkdir()
        env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TMPDIR": str(root),
               "PYTHONNOUSERSITE": "1", "PYTHONDONTWRITEBYTECODE": "1"}
        code = (
            "import importlib.util, pathlib, sys; "
            "spec = importlib.util.spec_from_file_location('deadline_test', sys.argv[1]); "
            "module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module); "
            "module.validation_deadline_probe(pathlib.Path(sys.argv[2]), sys.argv[3], sys.argv[4])"
        )
        process = subprocess.Popen(
            [sys.executable, "-I", "-B", "-c", code, str(Path(__file__).resolve()), str(root), mode, action],
            cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, start_new_session=True)
        expired = False
        running_owned = []
        try:
            try:
                stdout, stderr = process.communicate(timeout=4)
            except subprocess.TimeoutExpired:
                expired = True
            if not expired:
                pids = [path.read_text() for path in (root / "validator.pid", root / "descendant.pid") if path.exists()]
                status = subprocess.run(
                    ["/bin/ps", "-p", ",".join(pids), "-o", "stat="],
                    env=env, stdin=subprocess.DEVNULL, capture_output=True, timeout=1)
                self.assertIn(status.returncode, (0, 1))
                running_owned = [value for value in status.stdout.decode("ascii").split() if not value.startswith("Z")]
        finally:
            # Runner creates a separate session. The parent supervisor owns
            # cleanup even if the predecessor blocks or accepts a leaked child.
            marker = root / "validator.pid"
            if marker.exists():
                try:
                    os.killpg(int(marker.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
            stdout, stderr = process.communicate(timeout=2)
        self.assertFalse(expired, "parser_check exceeded the external 4-second test deadline")
        self.assertEqual(process.returncode, 0, stderr.decode("utf-8", "replace"))
        self.assertEqual(stdout, b"")
        self.assertEqual(stderr, b"")
        self.assertEqual(running_owned, [], "owned validator processes still running before supervisor cleanup")
        observed = json.loads((root / "observed.json").read_bytes())
        self.assertLess(observed["elapsed"], 3)
        self.assertTrue(observed["operation_lock_released"])
        self.assertEqual(observed["installer_calls"], 0)
        self.assertEqual(observed["installation_entries"], [])
        self.assertEqual(observed["validator_capture"], [True])
        self.assertNotIn("CONFIGURATION_VALUE", json.dumps(observed))
        if action == "check":
            self.assertEqual(observed["error"], "inspection_unavailable")
            self.assertEqual(observed["validations"], 0)
        else:
            self.assertEqual(observed["job"]["status"], "unconfirmed")
            self.assertEqual(observed["job"]["error"], "operation_unconfirmed")
            self.assertNotIn("result", observed["job"])

    def test_hung_validator_is_bounded_before_check_or_install_publication(self):
        for action in ("check", "install"):
            with self.subTest(action=action):
                self.deadline_case("hung", action)

    def test_exited_validator_with_stdout_descendant_is_bounded(self):
        for action in ("check", "install"):
            with self.subTest(action=action):
                self.deadline_case("descendant", action)

    def test_validator_output_is_discarded_and_installer_stays_uncaptured(self):
        controller = self.prepared()
        request = self.checked(controller)
        controller.install(request)
        controller.join()
        calls = controller.runner.calls
        self.assertEqual([capture for argv, capture in calls if "check-config" in argv], [True, True])
        self.assertEqual([capture for argv, capture in calls if "--destination" in argv], [False, False])
        self.assertEqual(controller.state()["job"]["status"], "succeeded")
        self.assertNotIn("CONFIGURATION_VALUE", json.dumps(controller.state()))

    def test_exact_bundle_pin_and_mutation_refuse_before_execution(self):
        self.error("bundle_digest_mismatch", lambda: self.controller(contents_sha256="0" * 64))
        (self.bundle / "install-library-consumer.py").write_bytes(b"changed")
        self.error("bundle_file_mismatch", self.controller)

    def test_bundle_descendant_symlink_and_path_escape_refused(self):
        installer = self.bundle / "install-library-consumer.py"
        installer.unlink()
        installer.symlink_to(SOURCE)
        with self.assertRaises(OSError):
            self.controller()
        self.manifest["files"]["../outside"] = {"sha256": "0" * 64}
        self.write_manifest()
        self.error("invalid_bundle", self.controller)

    def test_existing_workspace_and_missing_guide_refused(self):
        self.error("workspace_must_be_new", lambda: self.controller(workspace=self.bundle))
        self.manifest["files"].pop("services/PROFILE-GUIDE.md")
        self.write_manifest()
        self.error("invalid_bundle", self.controller)

    def test_source_changes_after_freeze_do_not_change_inspection_inputs(self):
        controller = self.controller()
        (self.bundle / "install-library-consumer.py").write_bytes(b"changed")
        controller.prepare({"family": "agno"})
        controller.join()
        self.assertEqual(controller.state()["job"]["status"], "succeeded")
        self.assertEqual((controller.bundle / "install-library-consumer.py").read_bytes(), b"# inert fixture\n")

    def test_frozen_installer_rechecked_before_each_execution(self):
        controller = self.controller()
        (controller.bundle / "install-library-consumer.py").write_bytes(b"changed")
        controller.prepare({"family": "agno"})
        controller.join()
        self.assertEqual(controller.state()["job"]["status"], "unconfirmed")
        self.assertEqual(controller.runner.calls, [])

    def test_explicit_prepare_single_effect_and_detached_state(self):
        controller = self.controller()
        self.assertFalse(controller.runner.calls)
        self.assertTrue(all(row["profiles"] == [] for row in controller.state()["families"]))
        controller.runner.block = threading.Event()
        first = controller.prepare({"family": "agno"})
        self.assertTrue(controller.runner.entered.wait(2))
        self.error("operation_in_progress", lambda: controller.prepare({"family": "services"}))
        controller.runner.block.set()
        controller.join()
        state = controller.state()
        state["families"][0]["profiles"].clear()
        self.assertTrue(controller.state()["families"][0]["profiles"])
        self.assertEqual(first["job"]["status"], "running")

    def test_help_preserves_effects_and_source_conditional_fields(self):
        controller = self.prepared("services")
        help_value = controller.help({"family": "services", "profile": "slack"})
        self.assertEqual(help_value["guide_text"], "Exact services guide\n")
        conditional = next(row for row in help_value["fields"] if row["name"] == "max_interval_requests")
        self.assertEqual(conditional["only_when_operation_selected"], "read_interval")
        request = {"family": "services", "profile": "slack", "operations": ["auth_test"],
                   "values": {"token_env": "SLACK_TOKEN", "timeout_seconds": 10, "max_interval_requests": 2}}
        self.error("invalid_configuration", lambda: controller.check(request))
        request["operations"] = ["read_interval"]
        self.assertEqual(controller.check(request)["scope"], "configuration_only")
        self.assertEqual(backend.normalize_help("activepieces", "test-write", help_source("activepieces"), "")["operations"][0]["effect"], "write")
        self.assertEqual(backend.normalize_help("agno", "searxng", help_source("agno"), "")["operations"][0]["effect"], "unspecified")

    def test_profile_operations_and_primitive_boundaries(self):
        controller = self.prepared()
        for change in ({"family": "other"}, {"profile": "other"}, {"operations": []},
                       {"operations": ["search_web", "search_web"]}, {"operations": ["write"]},
                       {"values": {"host": "x", "timeout": True}},
                       {"values": {"host": "x", "timeout": {"value": 1}}},
                       {"values": {"host": "x", "timeout": 10, "arbitrary": "x"}},
                       {"values": {"host": "x\nsecret", "timeout": 10}}):
            with self.subTest(change=change), self.assertRaises(backend.SetupError):
                controller.check(dict(self.request(), **change))

    def test_credential_reference_name_only_and_ap_config_shape(self):
        controller = self.prepared("activepieces")
        request = {"family": "activepieces", "profile": "test-write", "operations": ["test_write"], "values": {"token_env": "Bearer real-value"}}
        self.error("environment_reference_name_required", lambda: controller.check(request))
        request["values"]["token_env"] = "TEST_TOKEN"
        result = controller.check(request)
        stored = controller.validations[result["validation_id"]]["path"]
        self.assertEqual(json.loads(stored.read_bytes()), {"profile": "test-write", "token_env": "TEST_TOKEN"})

    def test_parser_is_final_validator_and_errors_do_not_echo_output(self):
        controller = self.prepared()
        controller.runner.invalid = True
        self.error("invalid_configuration", lambda: controller.check(self.request()))
        self.assertEqual(controller.validations, {})
        self.assertNotIn("CONFIGURATION_VALUE", json.dumps(controller.state()))

    def test_install_requires_exact_validated_selection(self):
        controller = self.prepared()
        request = self.checked(controller)
        changed = copy.deepcopy(request)
        changed["values"]["timeout"] = 11
        self.error("validation_changed", lambda: controller.install(changed))
        changed = dict(request, configuration_sha256="0" * 64)
        self.error("validation_changed", lambda: controller.install(changed))
        self.assertEqual(sum("--destination" in argv for argv, _ in controller.runner.calls), 1)

    def test_revalidation_failure_or_file_change_never_enters_installer(self):
        for mutate in (False, True):
            with self.subTest(mutate=mutate):
                controller = self.prepared()
                request = self.checked(controller)
                if mutate:
                    controller.validations[request["validation_id"]]["path"].write_bytes(b"{}")
                else:
                    controller.runner.invalid = True
                if mutate:
                    self.error("configuration_changed", lambda: controller.install(request))
                    self.assertFalse(controller.install_intents)
                else:
                    controller.install(request)
                    controller.join()
                    self.assertEqual(controller.state()["job"]["status"], "unconfirmed")
                self.assertEqual(sum("--destination" in argv for argv, _ in controller.runner.calls), 1)

    def test_duplicate_install_joins_same_effect_and_reads_actual_receipt(self):
        controller = self.prepared()
        request = self.checked(controller)
        controller.runner.block = threading.Event()
        first = controller.install(request)
        second = controller.install(copy.deepcopy(request))
        self.assertEqual(first["job"]["id"], second["job"]["id"])
        controller.runner.block.set()
        controller.join()
        job = controller.state()["job"]
        self.assertEqual(job["status"], "succeeded")
        self.assertEqual(job["result"]["mcp_configuration"], json.loads(Path(job["result"]["mcp_path"]).read_bytes()))
        self.assertEqual(sum("--destination" in argv for argv, _ in controller.runner.calls), 2)
        self.assertEqual(controller.install(request)["job"]["id"], first["job"]["id"])

    def test_receipt_mismatch_and_interrupted_publication_remain_unconfirmed(self):
        for flag in ("bad_receipt", "nonzero_after_publish"):
            controller = self.prepared()
            request = self.checked(controller)
            setattr(controller.runner, flag, True)
            controller.install(request)
            controller.join()
            job = controller.state()["job"]
            self.assertEqual(job["status"], "unconfirmed")
            self.assertNotIn("result", job)
            self.assertTrue(list((controller.workspace / "installations").glob("*/ready/mcp.json")))

    def test_help_guide_mutation_refuses_and_normalization_is_detached(self):
        controller = self.prepared()
        help_value = controller.help({"family": "agno", "profile": "searxng"})
        help_value["fields"].clear()
        self.assertTrue(controller.help({"family": "agno", "profile": "searxng"})["fields"])
        (controller.bundle / "agno/PROFILE-GUIDE.md").write_text("changed")
        self.error("frozen_bundle_changed", lambda: controller.help({"family": "agno", "profile": "searxng"}))

    def test_runner_uses_minimal_environment_and_no_shell(self):
        workspace = self.root / "runner"
        workspace.mkdir()
        (workspace / "tmp").mkdir()
        runner = backend.Runner(workspace)
        with patch.dict(os.environ, {"SECRET_TOKEN": "MUST_NOT_INHERIT", "HOME": "/private-home", "CODEX_HOME": "/private-codex"}):
            code, data = runner.run([sys.executable, "-I", "-c", "import json,os;print(json.dumps(dict(os.environ)))"], capture=True)
        self.assertEqual(code, 0)
        observed = json.loads(data)
        for key in ("SECRET_TOKEN", "HOME", "CODEX_HOME"):
            self.assertNotIn(key, observed)
        self.assertEqual(observed["TMPDIR"], str(workspace / "tmp"))
        with patch.object(backend.subprocess, "Popen") as popen:
            popen.return_value.wait.return_value = 0
            runner.run(["/trusted/runtime", "; not a shell"])
            self.assertFalse(popen.call_args.kwargs["shell"])
            self.assertTrue(popen.call_args.kwargs["start_new_session"])

    def test_bounded_inert_child_output(self):
        workspace = self.root / "runner"
        workspace.mkdir()
        (workspace / "tmp").mkdir()
        runner = backend.Runner(workspace)
        self.error("inspection_unavailable", lambda: runner.run([sys.executable, "-I", "-c", "print('x'*2200000)"], capture=True))

    def test_inert_descendant_retaining_stdout_has_complete_deadline(self):
        workspace = self.root / "runner"
        workspace.mkdir()
        (workspace / "tmp").mkdir()
        runner = backend.Runner(workspace)
        # The direct child exits normally. Its descendant holds the pipe and
        # must not prevent either bounded capture or host shutdown.
        code = "import subprocess,sys;subprocess.Popen([sys.executable,'-I','-c','import time;time.sleep(300)'])"
        started = time.monotonic()
        with patch.object(backend, "INSPECTION_TIMEOUT", 0.3):
            self.error("inspection_unavailable", lambda: runner.run([sys.executable, "-I", "-c", code], capture=True))
        self.assertLess(time.monotonic() - started, 3)

    def test_shutdown_stops_new_admission_but_joins_existing_work(self):
        controller = self.prepared()
        request = self.checked(controller)
        controller.runner.block = threading.Event()
        controller.install(request)
        controller.close_admission()
        self.error("host_closing", lambda: controller.install(request))
        self.error("host_closing", lambda: controller.check(self.request()))
        self.error("host_closing", lambda: controller.prepare({"family": "services"}))
        controller.runner.block.set()
        controller.join()
        self.assertEqual(controller.state()["job"]["status"], "succeeded")


class HTTPTests(FixtureMixin, unittest.TestCase):
    def setUp(self):
        FixtureMixin.setUp(self)
        self.controller_instance = self.controller()
        assets = self.root / "assets"
        assets.mkdir()
        (assets / "index.html").write_text("<!doctype html><title>Fixture</title>")
        self.server = backend.Server(self.controller_instance, assets=assets)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
        FixtureMixin.tearDown(self)

    def exchange(self, method="POST", path="/api/state", body=b"{}", **changes):
        connection = http.client.HTTPConnection(self.server.host, timeout=3)
        headers = {"Host": self.server.host, "Origin": self.server.origin,
                   "X-Toolkit-Session": self.server.capability, "Content-Type": "application/json"}
        for name, value in changes.items():
            if value is None:
                headers.pop(name, None)
            else:
                headers[name] = value
        connection.request(method, path, body, headers)
        response = connection.getresponse()
        result = response.status, dict(response.getheaders()), response.read()
        connection.close()
        return result

    def test_http_same_origin_capability_and_static_allowlist(self):
        self.assertEqual(self.exchange()[0], 200)
        for change in ({"Host": "localhost:123"}, {"Origin": "https://evil.example"},
                       {"Origin": None}, {"X-Toolkit-Session": "wrong"},
                       {"Sec-Fetch-Site": "cross-site"}):
            self.assertEqual(self.exchange(**change)[0], 403)
        status, headers, _ = self.exchange("GET", "/", Origin=None)
        self.assertEqual(status, 200)
        self.assertEqual(headers["Cache-Control"], "no-store")
        self.assertEqual(self.exchange("GET", "/", Origin=None, **{"Sec-Fetch-Site": "cross-site"})[0], 200)
        self.assertNotIn("Access-Control-Allow-Origin", headers)
        for path in ("/api/state", "/../backend.py", "/start.html", "/?secret=x"):
            self.assertEqual(self.exchange("GET", path, Origin=None)[0], 404)
        self.assertEqual(self.exchange("OPTIONS")[0], 501)

    def test_http_size_duplicate_json_and_unknown_keys(self):
        for body in (b'{"x":1,"x":2}', b'{"value":NaN}', b'[]', b'{"path":"/arbitrary"}'):
            self.assertEqual(self.exchange(body=body)[0], 400)
        self.assertEqual(self.exchange(body=b"x" * (backend.MAX_BODY + 1))[0], 413)
        self.assertEqual(self.exchange(**{"Content-Type": "text/plain"})[0], 400)
        self.assertEqual(self.exchange(path="/api/state?x=1")[0], 404)

    def test_http_duplicate_host_transfer_encoding_and_error_redaction(self):
        raw = ("POST /api/state HTTP/1.1\r\nHost: " + self.server.host + "\r\nHost: evil\r\n"
               "Origin: " + self.server.origin + "\r\nX-Toolkit-Session: " + self.server.capability +
               "\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").encode()
        with socket.create_connection(self.server.server_address, timeout=3) as client:
            client.sendall(raw)
            self.assertIn(b"403", client.recv(4096))
        self.assertEqual(self.exchange(**{"Transfer-Encoding": "chunked"})[0], 400)
        secret = "PRIVATE_CONFIG_SENTINEL"
        status, _, data = self.exchange(path="/api/help", body=backend.encoded({"family": secret, "profile": secret}))
        self.assertEqual(status, 400)
        self.assertNotIn(secret.encode(), data)

    def test_http_disconnect_does_not_cancel_admitted_prepare(self):
        controller = self.controller_instance
        controller.runner.block = threading.Event()
        body = backend.encoded({"family": "agno"})
        raw = ("POST /api/prepare HTTP/1.1\r\nHost: " + self.server.host + "\r\nOrigin: " + self.server.origin +
               "\r\nX-Toolkit-Session: " + self.server.capability + "\r\nContent-Type: application/json\r\nContent-Length: " + str(len(body)) + "\r\n\r\n").encode() + body
        client = socket.create_connection(self.server.server_address, timeout=3)
        client.sendall(raw)
        self.assertTrue(controller.runner.entered.wait(2))
        client.close()
        controller.runner.block.set()
        controller.join()
        self.assertEqual(controller.state()["job"]["status"], "succeeded")


if __name__ == "__main__":
    unittest.main()
