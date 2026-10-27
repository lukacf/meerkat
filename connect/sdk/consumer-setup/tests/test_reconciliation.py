"""Reconciliation composition checks; fixtures do not prove package payloads."""
from __future__ import annotations

import copy
import http.client
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("setup_fixtures", ROOT / "tests/test_backend.py")
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)
backend = fixtures.backend
CANARY = "PRIVATE_VERIFIER_DIAGNOSTIC"


class ReconciliationRunner(fixtures.FakeRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.transform = lambda value: value
        self.verify_block = None
        self.verify_entered = threading.Event()

    def run(self, argv, capture=False):
        if "--verify-existing" not in argv:
            return super().run(argv, capture=capture)
        self.calls.append((list(argv), capture))
        self.verify_entered.set()
        if self.verify_block:
            assert self.verify_block.wait(5)
        arg = lambda flag: argv[argv.index(flag) + 1]
        destination = Path(arg("--destination"))
        raw = (destination / "ready/installed.json").read_bytes()
        receipt = backend.decode(raw)
        # Only a plumbing fixture. Real archive/payload checks belong to the
        # supplied canonical verifier and the parent's installed-artifact trial.
        value = {"schema_version": 1, "scope": backend.READBACK_SCOPE, "verified": True,
                 "family": arg("--family"), "destination": str(destination),
                 "contents_sha256": arg("--contents-sha256"),
                 "configuration_sha256": arg("--config-sha256"), "command": receipt["command"],
                 "selected_runtime_path": arg("--node") if "--node" in argv else arg("--python"),
                 "historical_receipt": {"sha256": backend.digest(raw), "recorded_scope": receipt["scope"]},
                 "retained_payloads": {}, "excluded": [CANARY], "custody": CANARY}
        return 0, backend.encoded(self.transform(value))


class ReconciliationMixin(fixtures.FixtureMixin):
    def controller(self, **changes):
        changes.setdefault("verifier", self.bundle / "install-library-consumer.py")
        changes.setdefault("verifier_sha256", self.manifest["files"]["install-library-consumer.py"]["sha256"])
        changes.setdefault("runner_factory", ReconciliationRunner)
        return super().controller(**changes)

    def installed(self, lost=False):
        controller = self.prepared()
        request = self.checked(controller)
        controller.runner.nonzero_after_publish = lost
        job = controller.install(request)["job"]
        controller.join()
        return controller, request, job["id"]


class ReconciliationTests(ReconciliationMixin, unittest.TestCase):
    def test_lost_response_matches_without_rewriting_history_or_installing_again(self):
        controller, request, job_id = self.installed(lost=True)
        historical = copy.deepcopy(controller.jobs[job_id])
        self.assertEqual(historical["status"], "unconfirmed")
        before = len(controller.runner.calls)
        observed = controller.dispatch("/api/reconcile", {"job_id": job_id})["observation"]
        self.assertEqual(observed["status"], "matched")
        self.assertEqual(controller.jobs[job_id], historical)
        self.assertEqual(controller.install(request)["job"], historical)
        calls = controller.runner.calls[before:]
        self.assertEqual(len(calls), 1)
        argv, captured = calls[0]
        self.assertTrue(captured)
        self.assertIn("--verify-existing", argv)
        self.assertEqual(argv[argv.index("--config-sha256") + 1], request["configuration_sha256"])
        self.assertEqual(argv[3], str(controller.verifier))
        self.assertNotIn(CANARY, json.dumps(observed))
        self.assertEqual(controller.state()["reconciliation"]["observation"], observed)
        observed["result"]["mcp_configuration"].clear()
        self.assertTrue(controller.state()["reconciliation"]["observation"]["result"]["mcp_configuration"])

    def metadata_recovery(self, runner=ReconciliationRunner):
        original, request, job_id = self.installed()
        original.close_admission()
        anchor = original.jobs[job_id]["recovery_request"]
        recovered = self.controller(recover_workspace=original.workspace,
            recover_request=job_id, recover_request_sha256=anchor["sha256"], runner_factory=runner)
        return original, recovered, request, job_id

    def test_retained_metadata_is_opt_in_and_missing_credential_projection_stays_unknown(self):
        original, recovered, request, job_id = self.metadata_recovery()
        history = copy.deepcopy(recovered.jobs[job_id])
        plain = recovered.reconcile({"job_id": job_id})["observation"]
        self.assertNotIn("setup_metadata", plain)
        self.assertEqual(plain["excluded"], backend.READBACK_EXCLUSIONS)
        self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))
        before = len(recovered.runner.calls)
        observed = recovered.reconcile({"job_id": job_id}, retained_setup_metadata=True)["observation"]
        self.assertEqual(observed["status"], "matched")
        metadata = observed["setup_metadata"]
        self.assertIn("Separate config-help/check-config metadata observation", observed["excluded"][3])
        self.assertIn("No installed configuration validator", plain["excluded"][3])
        self.assertEqual(metadata["selection"]["operations"], request["operations"])
        self.assertEqual(metadata["selection"]["values"], {})
        self.assertEqual(metadata["configuration_check"], {}, "omission is not a no-secret declaration")
        self.assertNotIn("authentication", metadata["help"])
        self.assertNotIn("tool", metadata["help"]["operations"][0], "never infer actual tool names")
        self.assertNotIn(request["values"]["host"], json.dumps(metadata))
        self.assertNotIn("CONFIGURATION_VALUE_MUST_NOT_ESCAPE", json.dumps(observed))
        calls = recovered.runner.calls[before:]
        self.assertEqual(len(calls), 4)
        self.assertIn("--verify-existing", calls[0][0])
        self.assertIn("config-help", calls[1][0])
        self.assertIn("check-config", calls[2][0])
        self.assertIn("--verify-existing", calls[3][0])
        self.assertEqual(recovered.jobs[job_id], history)
        self.assertEqual(history["error"], "historical_outcome_unknown")
        self.assertEqual(sum("--destination" in argv and "--config" in argv and "--verify-existing" not in argv
            for argv, _ in original.runner.calls), 1)

    def test_retained_metadata_changes_refuse_without_overwriting_history_or_poisoning_next_read(self):
        original, recovered, _, job_id = self.metadata_recovery()
        history = copy.deepcopy(recovered.jobs[job_id])
        receipt = recovered.install_intents[job_id]["destination"] / "ready/installed.json"
        saved = receipt.read_bytes()
        original_run = recovered.runner.run
        def change_after_parser(argv, **kwargs):
            result = original_run(argv, **kwargs)
            if "check-config" in argv:
                receipt.write_bytes(saved + b" ")
            return result
        with patch.object(recovered.runner, "run", side_effect=change_after_parser):
            observed = recovered.reconcile({"job_id": job_id}, retained_setup_metadata=True)["observation"]
        self.assertEqual(observed["status"], "unconfirmed")
        self.assertNotIn("setup_metadata", observed)
        self.assertNotIn("result", observed)
        self.assertEqual(recovered.jobs[job_id], history)
        receipt.write_bytes(saved)
        current = recovered.reconcile({"job_id": job_id}, retained_setup_metadata=True)["observation"]
        self.assertEqual(current["status"], "matched")
        self.assertEqual(recovered.jobs[job_id], history)

    def test_retained_metadata_rejects_changed_credential_reference_projection(self):
        original = self.prepared("services")
        selection = {"family": "services", "profile": "slack", "operations": ["auth_test"],
                     "values": {"token_env": "TEST_TOKEN", "timeout_seconds": 10}}
        checked = original.check(selection)
        job = original.install({**selection, "validation_id": checked["validation_id"],
            "configuration_sha256": checked["configuration_sha256"]})["job"]
        original.join()
        original.close_admission()
        recovered = self.controller(recover_workspace=original.workspace, recover_request=job["id"],
            recover_request_sha256=job["recovery_request"]["sha256"])
        run = recovered.runner.run
        def changed_reference(argv, **kwargs):
            code, raw = run(argv, **kwargs)
            if "check-config" in argv:
                value = backend.decode(raw)
                value["credential_environment_references"] = [
                    {"field_path": ["config", "token_env"], "environment_variable": "OTHER_TOKEN"}]
                raw = backend.encoded(value)
            return code, raw
        with patch.object(recovered.runner, "run", side_effect=changed_reference):
            observed = recovered.reconcile({"job_id": job["id"]}, retained_setup_metadata=True)["observation"]
        self.assertEqual(observed["status"], "unconfirmed")
        self.assertNotIn("setup_metadata", observed)
        self.assertNotIn("OTHER_TOKEN", json.dumps(observed))
        self.assertEqual(recovered.jobs[job["id"]]["error"], "historical_outcome_unknown")

    def test_retained_metadata_stop_before_entry_and_entered_parser_are_settled_by_existing_owner(self):
        class BlockingMetadata(ReconciliationRunner):
            def __init__(inner, workspace):
                super().__init__(workspace)
                inner.metadata_entered = threading.Event()
                inner.metadata_release = threading.Event()
            def run(inner, argv, **kwargs):
                if "config-help" in argv:
                    inner.metadata_entered.set()
                    if not inner.metadata_release.wait(5):
                        raise AssertionError("metadata fixture did not release")
                return super().run(argv, **kwargs)
        _, recovered, _, job_id = self.metadata_recovery(BlockingMetadata)
        stop = threading.Event()
        stop.set()
        recovered._job_context.control = {"requested": stop, "spawn_lock": threading.Lock()}
        try:
            self.assertEqual(recovered.reconcile({"job_id": job_id}, retained_setup_metadata=True)
                ["observation"]["status"], "unconfirmed")
            self.assertEqual(recovered.runner.calls, [])
        finally:
            del recovered._job_context.control
        stop.clear()
        result = []
        def read():
            recovered._job_context.control = {"requested": stop, "spawn_lock": threading.Lock()}
            try:
                result.append(recovered.reconcile({"job_id": job_id}, retained_setup_metadata=True))
            finally:
                del recovered._job_context.control
        thread = threading.Thread(target=read)
        thread.start()
        try:
            self.assertTrue(recovered.runner.metadata_entered.wait(5))
            stop.set()
            self.assertTrue(thread.is_alive(), "the entered parser remains owned until it returns")
            self.error("operation_in_progress", lambda: recovered.reconcile({"job_id": job_id}))
        finally:
            recovered.runner.metadata_release.set()
            thread.join(5)
        self.assertFalse(thread.is_alive())
        self.assertEqual(result[0]["observation"]["status"], "unconfirmed")
        self.assertNotIn("setup_metadata", result[0]["observation"])
        self.assertEqual(len(recovered.runner.calls), 2)
        self.assertIn("--verify-existing", recovered.runner.calls[0][0])
        self.assertIn("config-help", recovered.runner.calls[1][0])
        self.assertEqual(recovered.jobs[job_id]["error"], "historical_outcome_unknown")

    def test_explicit_verifier_required_and_startup_freezes_exact_bytes(self):
        controller = self.controller(verifier=None, verifier_sha256=None)
        self.assertFalse(controller.state()["reconciliation"]["available"])
        controller.prepare({"family": "agno"}); controller.join()
        job = controller.install(self.checked(controller))["job"]; controller.join()
        self.error("reconciliation_unavailable", lambda: controller.reconcile({"job_id": job["id"]}))
        for changes, code in [({"verifier_sha256": "0" * 64}, "verifier_digest_mismatch"),
                              ({"verifier": None}, "invalid_verifier"),
                              ({"verifier": Path("relative.py")}, "invalid_verifier")]:
            self.error(code, lambda: self.controller(**changes))
        selected = self.controller()
        (self.bundle / "install-library-consumer.py").write_text("changed original")
        self.assertEqual(selected.verifier.read_bytes(), b"# inert fixture\n")

    def test_public_host_refuses_incomplete_verifier_selection_without_workspace(self):
        workspace = self.root / "invalid-startup"
        command = [sys.executable, "-I", "-B", str(ROOT / "backend.py"),
                   "--bundle", str(self.bundle), "--contents-sha256", self.sha,
                   "--workspace", str(workspace), "--families", "agno", "--python", sys.executable,
                   "--verifier", str(self.bundle / "install-library-consumer.py")]
        result = subprocess.run(command, env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"},
                                capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, b"")
        self.assertNotIn(str(self.bundle).encode(), result.stderr)
        self.assertIn(b"invalid_verifier", result.stderr)
        self.assertFalse(workspace.exists())

    def test_only_own_settled_install_job_can_select_a_readback(self):
        controller = self.prepared()
        prepare_job = controller.job["id"]
        for request in ({"job_id": prepare_job}, {"job_id": "0" * 32}, {"job_id": []}):
            self.error("unknown_installation", lambda: controller.reconcile(request))
        self.error("invalid_request", lambda: controller.reconcile({"job_id": prepare_job, "destination": "/tmp/other"}))
        controller.runner.block = threading.Event()
        job = controller.install(self.checked(controller))["job"]
        self.error("operation_in_progress", lambda: controller.reconcile({"job_id": job["id"]}))
        controller.runner.block.set(); controller.join()
        other = self.controller()
        self.error("unknown_installation", lambda: other.reconcile({"job_id": job["id"]}))
        self.assertFalse(other.runner.calls)

    def test_old_job_observation_cannot_replace_current_job_or_its_observation(self):
        controller, _, first = self.installed()
        request = self.checked(controller)
        second = controller.install(request)["job"]["id"]; controller.join()
        current = copy.deepcopy(controller.job)
        old = controller.reconcile({"job_id": first})["observation"]
        self.assertEqual(old["status"], "matched")
        self.assertEqual(controller.job, current)
        self.assertIsNone(controller.state()["reconciliation"]["observation"])
        new = controller.reconcile({"job_id": second})["observation"]
        self.assertEqual(controller.state()["reconciliation"]["observation"], new)
        self.assertNotEqual(old["result"]["mcp_path"], new["result"]["mcp_path"])

    def test_mismatched_or_malformed_proof_never_promotes_output(self):
        controller, _, job_id = self.installed(lost=True)
        for change in (lambda p: None, lambda p: dict(p, schema_version=True),
                       lambda p: dict(p, verified=1), lambda p: dict(p, retained_payloads=None),
                       lambda p: dict(p, family="services"), lambda p: dict(p, command=[CANARY]),
                       lambda p: dict(p, configuration_sha256="0" * 64),
                       lambda p: dict(p, historical_receipt={"sha256": "0" * 64, "recorded_scope": "observed_software_installation_only"}),
                       lambda p: dict(p, selected_runtime_path=CANARY)):
            controller.runner.transform = change
            observation = controller.reconcile({"job_id": job_id})["observation"]
            self.assertEqual(observation["status"], "unconfirmed")
            self.assertNotIn("result", observation)
            self.assertNotIn(CANARY, json.dumps(observation))

    def test_changed_original_config_or_verifier_refuses_before_child(self):
        for target in ("configuration", "verifier", "bundle"):
            controller, _, job_id = self.installed()
            intent = controller.install_intents[job_id]
            path = (intent["configuration"]["path"] if target == "configuration" else
                    controller.verifier if target == "verifier" else controller.bundle / "CONTENTS.json")
            path.write_text(CANARY)
            before = len(controller.runner.calls)
            self.assertEqual(controller.reconcile({"job_id": job_id})["observation"]["status"], "unconfirmed")
            self.assertEqual(len(controller.runner.calls), before)

    def test_receipt_mutation_during_readback_and_missing_publication_refuse(self):
        controller, _, job_id = self.installed()
        path = controller.install_intents[job_id]["destination"] / "ready/installed.json"
        def mutate(proof):
            path.write_bytes(path.read_bytes() + b" ")
            return proof
        controller.runner.transform = mutate
        self.assertEqual(controller.reconcile({"job_id": job_id})["observation"]["status"], "unconfirmed")
        path.unlink()
        before = len(controller.runner.calls)
        self.assertEqual(controller.reconcile({"job_id": job_id})["observation"]["status"], "unconfirmed")
        self.assertEqual(len(controller.runner.calls), before)

    def test_active_readback_blocks_effects_and_publishes_only_its_observation(self):
        controller, _, job_id = self.installed()
        controller.runner.verify_block = threading.Event()
        thread = threading.Thread(target=lambda: controller.reconcile({"job_id": job_id}))
        thread.start()
        try:
            self.assertTrue(controller.runner.verify_entered.wait(2))
            self.assertEqual(controller.state()["reconciliation"]["observation"]["status"], "checking")
            self.error("operation_in_progress", lambda: controller.reconcile({"job_id": job_id}))
            self.error("operation_in_progress", lambda: controller.check(self.request()))
            controller.close_admission()
            self.error("host_closing", lambda: controller.reconcile({"job_id": job_id}))
        finally:
            controller.runner.verify_block.set(); thread.join(3)
        self.assertFalse(thread.is_alive())
        self.assertEqual(controller.state()["reconciliation"]["observation"]["status"], "matched")

    def test_http_reconciliation_requires_session_and_returns_separate_observation(self):
        controller, _, job_id = self.installed(lost=True)
        server = backend.Server(controller)
        thread = threading.Thread(target=server.serve_forever); thread.start()
        def call(body, token):
            connection = http.client.HTTPConnection(*server.server_address, timeout=3)
            try:
                connection.request("POST", "/api/reconcile", backend.encoded(body), headers={
                    "Origin": server.origin, "X-Toolkit-Session": token, "Content-Type": "application/json"})
                response = connection.getresponse()
                return response.status, json.loads(response.read())
            finally:
                connection.close()
        try:
            self.assertEqual(call({"job_id": job_id}, "wrong")[0], 403)
            status, result = call({"job_id": job_id}, server.capability)
            self.assertEqual(status, 200); self.assertEqual(result["observation"]["status"], "matched")
            self.assertEqual(controller.job["status"], "unconfirmed")
            self.assertEqual(call({"job_id": job_id, "path": CANARY}, server.capability), (400, {"error": "invalid_request"}))
        finally:
            server.shutdown(); server.server_close(); thread.join(3)

    def test_actual_child_deadline_output_and_inherited_pipe_do_not_reinstall(self):
        # Entire probe has a second external deadline and own process group so
        # a regression in Runner cannot wedge this test or leave child workers.
        for mode in ("sleep", "pipe", "overflow", "invalid", "nonzero"):
            path = self.root / mode; path.mkdir()
            process = subprocess.Popen([sys.executable, "-I", "-B", str(Path(__file__).resolve()),
                "--child-probe", str(path), mode], env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"},
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
            try:
                stdout, stderr = process.communicate(timeout=8)
                self.assertEqual(process.returncode, 0, stderr)
                result = json.loads(stdout)
                self.assertEqual(result["observation"]["status"], "unconfirmed")
                self.assertEqual(result["new_installs"], 0)
                self.assertLess(result["seconds"], 3)
                self.assertNotIn(CANARY, stdout.decode() + stderr.decode())
                pids = [p.read_text() for p in path.glob("*.pid")]
                if pids:
                    check = subprocess.run(["/bin/ps", "-p", ",".join(pids), "-o", "stat="],
                        env={"PATH": "/usr/bin:/bin"}, capture_output=True, timeout=2)
                    self.assertIn(check.returncode, (0, 1))
                    self.assertEqual([line for line in check.stdout.decode().split() if not line.startswith("Z")], [])
            finally:
                try: os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError: pass
                process.wait()
                for pidfile in path.glob("*.pid"):
                    try: os.kill(int(pidfile.read_text()), signal.SIGKILL)
                    except ProcessLookupError: pass


def child_probe(path, mode):
    fixture = ReconciliationTests("runTest"); fixture.setUp()
    try:
        controller, _, job_id = fixture.installed(lost=True)
        fake = controller.runner
        code = "import os, pathlib, subprocess, sys, time; p=pathlib.Path(sys.argv[1]); (p/'direct.pid').write_text(str(os.getpid())); print('" + CANARY + "', flush=True); print('" + CANARY + "', file=sys.stderr, flush=True); "
        if mode == "pipe":
            code += "c=subprocess.Popen([sys.executable,'-I','-B','-c','import time; time.sleep(300)']); (p/'descendant.pid').write_text(str(c.pid))"
        elif mode == "sleep": code += "time.sleep(300)"
        elif mode == "overflow": code += "print('x' * (3*1024*1024), flush=True)"
        elif mode == "nonzero": code += "sys.exit(2)"
        original = fake.run
        def run(argv, capture=False):
            if "--verify-existing" in argv:
                return backend.Runner(controller.workspace).run([sys.executable, "-I", "-B", "-c", code, str(path)], capture=capture)
            return original(argv, capture)
        fake.run = run
        before = len(fake.calls); start = time.monotonic()
        with patch.object(backend, "INSPECTION_TIMEOUT", 0.25):
            result = controller.reconcile({"job_id": job_id})
        print(json.dumps({**result, "new_installs": len(fake.calls) - before, "seconds": time.monotonic() - start}))
    finally:
        fixture.tearDown()


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--child-probe":
        child_probe(Path(sys.argv[2]), sys.argv[3])
    else:
        unittest.main()
