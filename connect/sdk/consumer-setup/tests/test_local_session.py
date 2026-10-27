"""Real retained setup requests and local leases; no native admission claims."""
from __future__ import annotations

import importlib.util
import json
import os
import select
import threading
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


local = load("local_setup_test", ROOT / "local_session.py")
fixtures = load("local_setup_fixtures", ROOT / "tests/test_backend.py")
backend = fixtures.backend
reconciliation = load("local_setup_reconciliation", ROOT / "tests/test_reconciliation.py")
reconciliation.backend = backend
reconciliation.fixtures.backend = backend


class LocalSessionTests(fixtures.FixtureMixin, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.state_root = self.root / "product-session"
        self.binding = {"configuration_sha256": "a" * 64, "helper_sha256": "b" * 64}

    def create(self, root, record, reopened, runner=fixtures.FakeRunner):
        result = local.controller_factory(backend, root, record, reopened)(
            self.bundle, self.sha, self.root / "unused", backend.FAMILIES,
            sys.executable, sys.executable, sys.executable,
            runner_factory=runner, verifier=self.bundle / "install-library-consumer.py",
            verifier_sha256=self.manifest["files"]["install-library-consumer.py"]["sha256"])
        self.controllers.append(result)
        return result

    def test_remote_recipe_selection_preserves_exact_pinned_arguments(self):
        helper = self.root / "local_session.py"
        helper.write_bytes(b"retained helper fixture")
        configuration = self.root / "mcp.json"
        base = ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1", str(Path(sys.executable).resolve()),
                "-I", "-B", str(helper.with_name("mcp_adapter.py")), "--workspace", str(self.root / "work")]
        remote = ["--local-recipes-profile", str(self.root / "local-recipes.json"),
                  "--local-recipes-profile-sha256", "b" * 64, "--allow-remote-recipes"]
        def select(arguments):
            configuration.write_bytes(local.encoded({"mcpServers": {"toolkit-setup": {
                "command": "/usr/bin/env", "args": arguments}}}))
            return local.prepared_configuration(configuration, local.digest(configuration.read_bytes()),
                                                helper, local.digest(helper.read_bytes()))
        self.assertEqual(select(base + remote)["args"], base + remote)
        self.assertEqual(select(base)["args"], base, "legacy selection is unchanged")
        for incomplete in (["--allow-remote-recipes"], remote[:2] + remote[-1:], remote[2:]):
            with self.assertRaisesRegex(local.LocalSetupError, "pinned_recipe_profile"):
                select(base + incomplete)
        for forbidden in ("--recover-workspace", "--prior-request", "--compose-profile", "--browser-port"):
            with self.assertRaisesRegex(local.LocalSetupError, "fresh_local_selection"):
                select(base + remote + [forbidden, str(self.root / "unselected")])

    def test_checkpoint_precedes_effect_and_restart_uses_existing_readback_owner(self):
        test = self

        class CheckpointRunner(reconciliation.ReconciliationRunner):
            def run(self, argv, **kwargs):
                if "--config" in argv and "--destination" in argv:
                    value = json.loads((test.state_root / "recovery.json").read_bytes())
                    test.assertEqual(value["workspace"], str(self.workspace))
                    test.assertGreaterEqual(len(value["requests"]), 1)
                    for entry in value["requests"]:
                        raw = (self.workspace / "requests" / (entry["id"] + ".json")).read_bytes()
                        test.assertEqual(local.digest(raw), entry["sha256"])
                return super().run(argv, **kwargs)

        with local.session_owner(self.state_root, self.binding) as selected:
            original = self.create(*selected, runner=CheckpointRunner)
            original.prepare({"family": "agno"}); original.join()
            ids = []
            for _ in range(2):
                ids.append(original.install(self.checked(original))["job"]["id"])
                original.join()
            saved = (self.state_root / "recovery.json").read_bytes()
            self.assertEqual([item["id"] for item in json.loads(saved)["requests"]], ids)
            original.close_admission()
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected, runner=reconciliation.ReconciliationRunner)
            self.assertTrue(selected[2])
            self.assertEqual(recovered.state()["mode"], "recovery")
            self.assertEqual(list(recovered.jobs), ids)
            self.assertEqual(recovered.runner.calls, [])
            self.error("recovery_read_only", lambda: recovered.prepare({"family": "agno"}))
            self.assertEqual(recovered.reconcile({"job_id": ids[0]})["observation"]["status"], "matched")
            self.assertTrue(all("--verify-existing" in argv for argv, _ in recovered.runner.calls))
            self.assertEqual((self.state_root / "recovery.json").read_bytes(), saved)

    def test_failed_publication_preserves_anchor_closes_admission_and_never_runs_worker(self):
        controller = self.prepared()
        request = self.checked(controller)
        before = len(controller.runner.calls)
        controller.recovery_selection_writer = lambda _: (_ for _ in ()).throw(OSError("private detail"))
        job = controller.install(request)["job"]
        self.assertEqual(job["status"], "unconfirmed")
        self.assertEqual(job["error"], "recovery_selection_unavailable")
        self.assertEqual(len(controller.runner.calls), before)
        raw = (controller.workspace / "requests" / (job["id"] + ".json")).read_bytes()
        self.assertEqual(local.digest(raw), job["recovery_request"]["sha256"])
        self.assertEqual(controller.recovery_manifest({"job_ids": [job["id"]]})["scope"], "retained_setup_requests_only")
        self.error("host_closing", lambda: controller.install(request))
        self.assertTrue(controller.operation_lock.acquire(blocking=False))
        controller.operation_lock.release()

    def test_request_retention_failure_fences_later_effects_and_preserves_readback(self):
        for stage in ("request_sync", "request_readback", "correlation", "registration", "control_registration", "interrupt"):
            with self.subTest(stage=stage):
                root = self.root / ("retention-" + stage)
                with local.session_owner(root, self.binding) as selected:
                    original = self.create(*selected, runner=reconciliation.ReconciliationRunner)
                    original.prepare({"family": "agno"}); original.join()
                    first = original.install(self.checked(original))["job"]; original.join()
                    request = self.checked(original)
                    checkpoint = (root / "recovery.json").read_bytes()
                    calls = len(original.runner.calls)
                    error = KeyboardInterrupt() if stage == "interrupt" else OSError("private reservation failure")
                    real_sync, real_read = backend.sync_file, backend.read_file

                    def sync(directory, name):
                        if directory == original.workspace / "requests" and stage in ("request_sync", "interrupt"):
                            raise error
                        return real_sync(directory, name)

                    def read(directory, name, limit=backend.MAX_JSON):
                        if directory == original.workspace / "requests" and stage == "request_readback":
                            raise error
                        return real_read(directory, name, limit)

                    class FailedRegistration(dict):
                        def __setitem__(self, key, value):
                            raise error

                    if stage == "correlation":
                        original.install_intents = FailedRegistration(original.install_intents)
                    if stage == "registration":
                        original.jobs = FailedRegistration(original.jobs)
                    if stage == "control_registration":
                        original._job_controls = FailedRegistration(original._job_controls)
                    with patch.object(backend, "sync_file", sync), patch.object(backend, "read_file", read):
                        with self.assertRaises(type(error)) as caught:
                            original.install(request)
                    self.assertIs(caught.exception, error)
                    self.assertEqual(len(original.runner.calls), calls)
                    self.assertEqual((root / "recovery.json").read_bytes(), checkpoint)
                    self.assertEqual(len(list((original.workspace / "requests").iterdir())), 2)
                    retained = {p: p.read_bytes() for p in original.workspace.rglob("*") if p.is_file()}
                    for call in (lambda: original.install(request),
                                 lambda: original.prepare({"family": "services"}),
                                 lambda: original.retry_prepare({"job_id": first["id"]}),
                                 lambda: original.acquire_python({}),
                                 lambda: original.compose_action("prepare", {})):
                        self.error("recovery_request_unavailable", call)
                    self.assertEqual(len(original.runner.calls), calls)
                    self.assertFalse(original.operation_lock.locked())
                    self.assertFalse(original.closing)
                    self.assertFalse(original.recovery_only)
                    # Only the previously complete canonical job is read back.
                    self.assertEqual(original.reconcile({"job_id": first["id"]})["observation"]["status"], "matched")
                    self.assertTrue(all("--verify-existing" in argv for argv, _ in original.runner.calls[calls:]))
                    if stage == "control_registration":
                        identifier = original.validations[request["validation_id"]]["job_id"]
                        self.assertEqual(original.jobs[identifier]["status"], "unconfirmed")
                        self.assertEqual(original.jobs[identifier]["error"], "recovery_request_unavailable")
                        self.assertEqual(original.reconcile({"job_id": identifier})["observation"]["status"], "unconfirmed")
                    self.assertEqual({p: p.read_bytes() for p in retained}, retained)
                    self.assertEqual((root / "recovery.json").read_bytes(), checkpoint)
                before = set(root.iterdir())
                with local.session_owner(root, self.binding) as selected:
                    with self.assertRaisesRegex(local.LocalSetupError, "recovery_selection_incomplete"):
                        self.create(*selected)
                self.assertEqual(set(root.iterdir()), before)
                self.assertEqual({p: p.read_bytes() for p in retained}, retained)

    def test_interrupt_during_checkpoint_cannot_enter_worker_or_lose_anchor(self):
        controller = self.prepared()
        request = self.checked(controller)
        before = len(controller.runner.calls)
        controller.recovery_selection_writer = lambda _: (_ for _ in ()).throw(KeyboardInterrupt())
        with self.assertRaises(KeyboardInterrupt):
            controller.install(request)
        self.assertEqual(len(controller.runner.calls), before)
        self.assertTrue(controller.closing)
        self.assertEqual(controller.job["error"], "recovery_selection_unavailable")
        self.assertIn(controller.job["id"], controller.install_intents)
        self.assertTrue(controller.operation_lock.acquire(blocking=False))
        controller.operation_lock.release()

    def test_full_selection_refuses_before_reservation_without_dropping_entries(self):
        controller = self.prepared()
        controller.recovery_selection_writer = lambda _: self.fail("must not publish a partial selection")
        for i in range(256):
            identifier = format(i, "032x")
            controller.jobs[identifier] = {"recovery_request": {"id": identifier, "sha256": "a" * 64}}
        before = dict(controller.jobs)
        self.error("recovery_selection_full", lambda: controller.start_job(
            "install", "agno", lambda: self.fail("worker entered"),
            before_start=lambda _: self.fail("request reserved")))
        self.assertEqual(controller.jobs, before)

    def test_empty_interrupted_session_requires_product_review_and_fresh_session_is_explicit(self):
        with local.session_owner(self.state_root, self.binding) as selected:
            original = self.create(*selected)
            original.close_admission()
        with local.session_owner(self.state_root, self.binding) as selected:
            with self.assertRaisesRegex(local.LocalSetupError, "previous_session_needs_review"):
                self.create(*selected)
        # A product-selected new root is an explicit fresh setup, not a replay.
        with local.session_owner(self.root / "next-product-session", self.binding) as selected:
            current = self.create(*selected)
            self.assertFalse(selected[2])
            self.assertFalse(current.recovery_only)
            self.assertNotEqual(current.workspace, original.workspace)
            self.assertTrue(original.workspace.exists())

    def test_selection_change_and_symlink_cannot_reuse_another_local_session(self):
        with local.session_owner(self.state_root, self.binding):
            pass
        with self.assertRaisesRegex(local.LocalSetupError, "session_changed"):
            with local.session_owner(self.state_root, {**self.binding, "configuration_sha256": "c" * 64}):
                self.fail("changed selection admitted")
        alias = self.root / "alias"
        alias.symlink_to(self.state_root)
        with self.assertRaisesRegex(local.LocalSetupError, "private_directory_required"):
            with local.session_owner(alias, self.binding):
                self.fail("symlink root admitted")

    def test_missing_session_record_with_retained_work_cannot_start_another_attempt(self):
        with local.session_owner(self.state_root, self.binding) as selected:
            original = self.create(*selected)
            original.prepare({"family": "agno"}); original.join()
            job = original.install(self.checked(original))["job"]; original.join()
            original.close_admission()
        (self.state_root / "session.json").unlink()
        before = {p: p.read_bytes() for p in self.state_root.rglob("*") if p.is_file()}
        directories = set(self.state_root.iterdir())
        with self.assertRaisesRegex(local.LocalSetupError, "previous_session_needs_review"):
            with local.session_owner(self.state_root, self.binding):
                self.fail("missing record selected new work")
        self.assertEqual(set(self.state_root.iterdir()), directories)
        self.assertEqual({p: p.read_bytes() for p in before}, before)
        self.assertTrue((original.workspace / "requests" / (job["id"] + ".json")).is_file())

    def test_adapter_and_backend_are_verified_before_either_import(self):
        entry = self.root / "entry"
        entry.mkdir()
        helper = entry / "local_session.py"
        adapter = entry / "mcp_adapter.py"
        backend_path = entry / "backend.py"
        adapter.write_bytes((ROOT / "mcp_adapter.py").read_bytes())
        backend_path.write_bytes(b"value = 'retained'\n")
        adapter_sha = local.digest(adapter.read_bytes())
        backend_sha = local.digest(backend_path.read_bytes())
        for changed in (adapter, backend_path):
            saved = changed.read_bytes()
            changed.write_bytes(saved + b"# changed\n")
            with patch.object(local, "retained_module") as imported:
                with self.assertRaisesRegex(local.LocalSetupError, "selection_changed"):
                    local.retained_adapter(helper, adapter_sha, backend_sha)
                imported.assert_not_called()
            changed.write_bytes(saved)
        load_retained = local.retained_module

        def after_verified_read(name, path, raw, namespace=None):
            backend_path.write_bytes(b"raise AssertionError('disk backend was reopened')\n")
            adapter.write_bytes(b"raise AssertionError('disk adapter was reopened')\n")
            return load_retained(name, path, raw, namespace)

        with patch.object(local, "retained_module", side_effect=after_verified_read):
            loaded = local.retained_adapter(helper, adapter_sha, backend_sha)
        self.assertEqual(loaded.backend.value, "retained")

    def test_real_process_lease_refuses_competing_helper_and_releases_after_close(self):
        script = """import importlib.util, pathlib, sys
spec = importlib.util.spec_from_file_location('selected', sys.argv[1])
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
try:
    with m.session_owner(pathlib.Path(sys.argv[2]), m.decode(sys.argv[3].encode())):
        print('entered')
except m.LocalSetupError as error:
    print(str(error)); sys.exit(7)
"""
        command = [sys.executable, "-I", "-B", "-c", script, str(ROOT / "local_session.py"),
                   str(self.state_root), local.encoded(self.binding).decode()]
        with local.session_owner(self.state_root, self.binding):
            refused = subprocess.run(command, capture_output=True, text=True, timeout=10)
            self.assertEqual(refused.returncode, 7, refused.stderr)
            self.assertEqual(refused.stdout.strip(), "local_setup_already_open")
        accepted = subprocess.run(command, capture_output=True, text=True, timeout=10)
        self.assertEqual(accepted.returncode, 0, accepted.stderr)
        self.assertEqual(accepted.stdout.strip(), "entered")

    def test_changed_anchor_cannot_reopen_or_create_another_workspace(self):
        with local.session_owner(self.state_root, self.binding) as selected:
            original = self.create(*selected)
            original.prepare({"family": "agno"}); original.join()
            job = original.install(self.checked(original))["job"]; original.join()
            original.close_admission()
        path = original.workspace / "requests" / (job["id"] + ".json")
        path.write_bytes(path.read_bytes() + b" ")
        before = set(self.state_root.iterdir())
        with local.session_owner(self.state_root, self.binding) as selected:
            self.error("recovery_request_changed", lambda: self.create(*selected))
        self.assertEqual(set(self.state_root.iterdir()), before)

    def test_sync_failure_before_replace_does_not_change_previous_selection(self):
        self.state_root.mkdir(mode=0o700)
        path = self.state_root / "recovery.json"
        path.write_bytes(b"retained previous selection")
        with patch.object(local, "sync_directory", side_effect=OSError("sync refused")):
            with self.assertRaises(OSError):
                local.replace(path, b"new selection")
        self.assertEqual(path.read_bytes(), b"retained previous selection")
        self.assertEqual(list(self.state_root.iterdir()), [path])

    def test_failed_later_publication_cannot_silently_reopen_older_partial_history(self):
        with local.session_owner(self.state_root, self.binding) as selected:
            original = self.create(*selected)
            original.prepare({"family": "agno"}); original.join()
            first = original.install(self.checked(original))["job"]; original.join()
            saved = (self.state_root / "recovery.json").read_bytes()
            original.recovery_selection_writer = lambda _: (_ for _ in ()).throw(OSError("full disk"))
            failed = original.install(self.checked(original))["job"]
            self.assertNotEqual(first["id"], failed["id"])
            self.assertEqual(failed["error"], "recovery_selection_unavailable")
            self.assertEqual((self.state_root / "recovery.json").read_bytes(), saved)
        with local.session_owner(self.state_root, self.binding) as selected:
            with self.assertRaisesRegex(local.LocalSetupError, "recovery_selection_incomplete"):
                self.create(*selected)


class LiveRoundOffer:
    """Actual private pipe/socket rendezvous; no native actor or provider call."""
    def __init__(self, test, root, binding):
        self.test, self.root, self.binding = test, root, binding
        self.errors = []
        self.input_read, input_write = os.pipe()
        output_read, self.output_write = os.pipe()
        self.cancel_read, self.cancel_write = os.pipe()
        self.input = os.fdopen(input_write, "wb", buffering=0)
        self.output = os.fdopen(output_read, "rb", buffering=0)
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        try:
            with os.fdopen(self.input_read, "rb", buffering=0) as source, os.fdopen(self.output_write, "wb", buffering=0) as sink:
                local.offer_next_round(backend, self.root, self.binding, source, sink, self.cancel_read)
        except BaseException as error:
            self.errors.append(error)

    def receive(self):
        self.test.assertTrue(select.select([self.output], [], [], 5)[0], "round owner did not answer")
        return local.read_frame(self.output)

    def __enter__(self):
        self.thread.start()
        self.test.assertEqual(self.receive(), {"ready": True})
        return self

    def finish(self):
        self.input.write(local.encoded({"settle": True}))
        self.test.assertEqual(self.receive(), {"consumed": True})
        self.test.assertEqual(self.receive(), {"settled": True})
        self.thread.join(5)
        self.test.assertFalse(self.thread.is_alive())
        self.test.assertEqual(self.errors, [])

    def __exit__(self, *_):
        self.input.close()
        try:
            os.write(self.cancel_write, b"x")
            self.thread.join(5)
            self.test.assertFalse(self.thread.is_alive(), "live round owner was not joined")
        finally:
            self.output.close()
            os.close(self.cancel_read); os.close(self.cancel_write)


class LocalRoundTests(fixtures.FixtureMixin, unittest.TestCase):
    create = LocalSessionTests.create

    def setUp(self):
        fixtures.FixtureMixin.setUp(self)
        self.state_root = self.root / "product-session"
        self.binding = {"configuration_sha256": "a" * 64, "helper_sha256": "b" * 64}
    def first_install(self):
        with local.session_owner(self.state_root, self.binding) as selected:
            owner = self.create(*selected, runner=reconciliation.ReconciliationRunner)
            owner.prepare({"family": "agno"}); owner.join()
            job = owner.install(self.checked(owner))["job"]; owner.join()
            expected = owner.reconcile({"job_id": job["id"]})["observation"]["result"]
            owner.close_admission(); owner.join()
        return owner, job, expected

    def test_live_empty_first_round_and_repeated_empty_rounds_can_continue(self):
        with local.session_owner(self.state_root, self.binding) as selected:
            current = self.create(*selected)
            self.assertEqual(current.jobs, {})
            self.assertEqual(current.runner.calls, [])
            current.close_admission(); current.join()
        workspaces = [current.workspace]
        for _ in range(2):
            offer = LiveRoundOffer(self, self.state_root, self.binding)
            try:
                offer.thread.start()
                self.assertEqual(offer.receive(), {"ready": True})
                with local.session_owner(self.state_root, self.binding) as selected:
                    current = self.create(*selected)
                    self.assertFalse(current.recovery_only)
                    self.assertEqual(current.jobs, {})
                    self.assertEqual(current.runner.calls, [])
                    self.assertNotIn(current.workspace, workspaces)
                    offer.finish()
                    current.close_admission(); current.join()
                    workspaces.append(current.workspace)
            finally:
                offer.__exit__()
        self.assertTrue(all(path.is_dir() for path in workspaces))
        with local.session_owner(self.state_root, self.binding) as selected:
            with self.assertRaisesRegex(local.LocalSetupError, "previous_session_needs_review"):
                self.create(*selected)

    def test_first_install_after_empty_round_survives_next_round_and_saved_readback(self):
        with local.session_owner(self.state_root, self.binding) as selected:
            empty = self.create(*selected)
            empty.close_admission(); empty.join()
        with LiveRoundOffer(self, self.state_root, self.binding) as offer:
            with local.session_owner(self.state_root, self.binding) as selected:
                installed = self.create(*selected, runner=reconciliation.ReconciliationRunner)
                offer.finish()
                installed.prepare({"family": "agno"}); installed.join()
                first = installed.install(self.checked(installed))["job"]; installed.join()
                self.assertEqual(installed.jobs[first["id"]]["status"], "succeeded")
                expected = installed.reconcile({"job_id": first["id"]})["observation"]["result"]
                checkpoint = (self.state_root / "recovery.json").read_bytes()
                self.assertEqual(json.loads(checkpoint)["workspace"], str(installed.workspace))
                installed.close_admission(); installed.join()
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected, runner=reconciliation.ReconciliationRunner)
            self.assertTrue(recovered.recovery_only)
            self.assertEqual(recovered.reconcile({"job_id": first["id"]})["observation"]["result"], expected)
            self.error("recovery_read_only", lambda: recovered.prepare({"family": "services"}))
            recovered.close_admission(); recovered.join()
        with LiveRoundOffer(self, self.state_root, self.binding) as offer:
            with local.session_owner(self.state_root, self.binding) as selected:
                current = self.create(*selected, runner=reconciliation.ReconciliationRunner)
                offer.finish()
                self.assertFalse(current.recovery_only)
                self.assertEqual(current.reconcile({"job_id": first["id"]})["observation"]["result"], expected)
                current.close_admission(); current.join()
        record, _ = local.read_session_record(self.state_root, self.binding)
        self.assertEqual(record["prior"], [{"workspace": installed.workspace.name,
            "recovery_sha256": local.digest(checkpoint)}])
        self.assertEqual((installed.workspace / "local-recovery.json").read_bytes(), checkpoint)
        self.assertTrue(empty.workspace.is_dir())
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected, runner=reconciliation.ReconciliationRunner)
            self.assertTrue(recovered.recovery_only)
            self.assertEqual(recovered.reconcile({"job_id": first["id"]})["observation"]["result"], expected)
            self.error("recovery_read_only", lambda: recovered.prepare({"family": "services"}))

    def test_empty_rounds_preserve_prior_installation_readback_without_duplicate_history(self):
        original, first, expected = self.first_install()
        checkpoint = (self.state_root / "recovery.json").read_bytes()
        for _ in range(2):
            with LiveRoundOffer(self, self.state_root, self.binding) as offer:
                with local.session_owner(self.state_root, self.binding) as selected:
                    current = self.create(*selected, runner=reconciliation.ReconciliationRunner)
                    self.assertFalse(current.recovery_only)
                    self.assertEqual(list(current.jobs), [first["id"]])
                    self.assertEqual(current.runner.calls, [])
                    offer.finish()
                    current.close_admission(); current.join()
            record, _ = local.read_session_record(self.state_root, self.binding)
            self.assertEqual(record["prior"], [{"workspace": original.workspace.name,
                "recovery_sha256": local.digest(checkpoint)}])
        self.assertEqual((original.workspace / "local-recovery.json").read_bytes(), checkpoint)
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected, runner=reconciliation.ReconciliationRunner)
            self.assertTrue(recovered.recovery_only)
            self.assertEqual(recovered.reconcile({"job_id": first["id"]})["observation"]["result"], expected)
            self.error("recovery_read_only", lambda: recovered.prepare({"family": "services"}))

    def test_empty_round_missing_changed_receipt_or_uncheckpointed_request_refuses_offer(self):
        for change in ("missing", "changed", "retained_request"):
            with self.subTest(change=change):
                self.state_root = self.root / ("empty-" + change)
                with local.session_owner(self.state_root, self.binding) as selected:
                    owner = self.create(*selected)
                    owner.close_admission(); owner.join()
                empty = owner.workspace / "local-empty.json"
                if change == "missing":
                    empty.unlink()
                elif change == "changed":
                    empty.write_bytes(empty.read_bytes() + b" ")
                else:
                    (owner.workspace / "requests/orphan.json").write_bytes(b"retained request")
                before = {p: p.read_bytes() for p in self.state_root.rglob("*") if p.is_file()}
                with patch.object(local.socket, "socket", side_effect=AssertionError("offer must not publish")):
                    with self.assertRaisesRegex(local.LocalSetupError,
                            "previous_session_needs_review|recovery_selection_incomplete"):
                        local.offer_next_round(backend, self.state_root, self.binding, None, None, -1)
                self.assertFalse((self.state_root / "round-handoff.json").exists())
                self.assertEqual({p: p.read_bytes() for p in before}, before)

    def test_initial_empty_receipt_failure_closes_controller_before_admission(self):
        actual_controller, actual_write = backend.Controller, local.write_new
        owners = []
        def create(*args, **kwargs):
            owner = actual_controller(*args, **kwargs)
            owners.append(owner)
            return owner
        def write(path, raw):
            if path.name == "local-empty.json":
                raise OSError("empty receipt refused")
            return actual_write(path, raw)
        with local.session_owner(self.state_root, self.binding) as selected:
            with patch.object(backend, "Controller", create), patch.object(local, "write_new", write):
                with self.assertRaisesRegex(OSError, "empty receipt refused"):
                    self.create(*selected)
        self.assertEqual(len(owners), 1)
        self.assertTrue(owners[0].closing)
        self.assertEqual(owners[0].runner.calls, [])
        self.assertFalse((owners[0].workspace / "local-empty.json").exists())

    def test_live_second_service_preserves_first_exact_readback_and_restart_is_read_only(self):
        original, first, expected = self.first_install()
        retained = {path: path.read_bytes() for path in original.workspace.rglob("*") if path.is_file()}
        prior_checkpoint = (self.state_root / "recovery.json").read_bytes()
        with LiveRoundOffer(self, self.state_root, self.binding) as offer:
            with local.session_owner(self.state_root, self.binding) as selected:
                current = self.create(*selected, runner=reconciliation.ReconciliationRunner)
                self.assertFalse(current.recovery_only)
                self.assertNotEqual(original.workspace, current.workspace)
                self.assertEqual(current.job_detail({"job_id": first["id"]})["job"]["status"], "unconfirmed")
                self.assertEqual(current.reconcile({"job_id": first["id"]})["observation"]["result"], expected)
                current.prepare({"family": "services"}); current.join()
                request = {"family": "services", "profile": "slack", "operations": ["auth_test"],
                           "values": {"token_env": "UNRESOLVED_FIXTURE_REFERENCE", "timeout_seconds": 3}}
                validation = current.check(request)
                second = current.install({**request, "validation_id": validation["validation_id"],
                                          "configuration_sha256": validation["configuration_sha256"]})["job"]
                current.join()
                self.assertEqual(current.jobs[second["id"]]["status"], "succeeded")
                rows = current.job_summaries({"before": None})["jobs"]
                self.assertTrue(next(row for row in rows if row["id"] == first["id"])["recovered"])
                self.assertNotIn("recovered", next(row for row in rows if row["id"] == second["id"]))
                checkpoint = (current.workspace / "local-recovery.json").read_bytes()
                self.assertEqual([entry["id"] for entry in json.loads(checkpoint)["requests"]], [second["id"]])
                offer.finish()
                current.close_admission(); current.join()
        self.assertEqual((original.workspace / "local-recovery.json").read_bytes(), prior_checkpoint)
        self.assertEqual({path: path.read_bytes() for path in retained}, retained)
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected, runner=reconciliation.ReconciliationRunner)
            self.assertTrue(recovered.recovery_only)
            self.error("recovery_read_only", lambda: recovered.prepare({"family": "services"}))
            self.assertEqual(recovered.reconcile({"job_id": first["id"]})["observation"]["result"], expected)
            self.assertEqual(recovered.reconcile({"job_id": second["id"]})["observation"]["status"], "matched")

    def test_host_exit_and_cancellation_before_consumption_leave_original_recovery_only(self):
        _, first, _ = self.first_install()
        original = (self.state_root / "session.json").read_bytes()
        for action in ("host_exit", "cancel"):
            with self.subTest(action=action):
                with LiveRoundOffer(self, self.state_root, self.binding) as offer:
                    if action == "host_exit":
                        offer.input.close()
                    else:
                        os.write(offer.cancel_write, b"x")
                    offer.thread.join(5)
                    self.assertFalse(offer.thread.is_alive())
                    self.assertEqual(len(offer.errors), 1)
                self.assertFalse((self.state_root / "round-handoff.json").exists())
                self.assertEqual((self.state_root / "session.json").read_bytes(), original)
                with local.session_owner(self.state_root, self.binding) as selected:
                    recovered = self.create(*selected)
                    self.assertTrue(recovered.recovery_only)
                    self.assertIn(first["id"], recovered.jobs)

    def test_changed_binding_and_duplicate_factory_consumption_never_create_another_round(self):
        self.first_install()
        with LiveRoundOffer(self, self.state_root, self.binding) as offer:
            with self.assertRaisesRegex(local.LocalSetupError, "local_setup_session_changed"):
                with local.session_owner(self.state_root, {**self.binding, "helper_sha256": "0" * 64}):
                    self.fail("changed descriptor entered")
            with local.session_owner(self.state_root, self.binding) as selected:
                factory = local.controller_factory(backend, *selected)
                args = (self.bundle, self.sha, self.root / "unused", backend.FAMILIES,
                        sys.executable, sys.executable, sys.executable)
                kwargs = {"runner_factory": reconciliation.ReconciliationRunner,
                    "verifier": self.bundle / "install-library-consumer.py",
                    "verifier_sha256": self.manifest["files"]["install-library-consumer.py"]["sha256"]}
                owner = factory(*args, **kwargs); self.controllers.append(owner)
                with self.assertRaisesRegex(local.LocalSetupError, "already_consumed"):
                    factory(*args, **kwargs)
                offer.finish()
                owner.close_admission(); owner.join()
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected)
            self.assertTrue(recovered.recovery_only, "an acknowledged empty round still cannot reauthorize a restart")

    def test_interrupted_round_publication_retains_old_anchors_without_setup_replay(self):
        for stage in ("before_replace", "after_replace", "lost_ack"):
            with self.subTest(stage=stage):
                self.state_root = self.root / ("interrupted-" + stage)
                original, first, expected = self.first_install()
                prior = (self.state_root / "session.json").read_bytes()
                real_replace, real_send = local.replace, local.send_frame
                def replace(path, raw):
                    if path == self.state_root / "session.json":
                        if stage == "after_replace":
                            real_replace(path, raw)
                        raise OSError("interrupted publication")
                    return real_replace(path, raw)
                def send(stream, value):
                    if stage == "lost_ack" and value == {"accepted": True}:
                        raise OSError("lost acknowledgement")
                    return real_send(stream, value)
                with LiveRoundOffer(self, self.state_root, self.binding) as offer:
                    with local.session_owner(self.state_root, self.binding) as selected:
                        with patch.object(local, "replace", real_replace if stage == "lost_ack" else replace), patch.object(local, "send_frame", send):
                            with self.assertRaises((OSError, local.LocalSetupError)):
                                self.create(*selected, runner=reconciliation.ReconciliationRunner)
                    offer.thread.join(5)
                    self.assertFalse(offer.thread.is_alive())
                if stage == "before_replace":
                    self.assertEqual((self.state_root / "session.json").read_bytes(), prior)
                with local.session_owner(self.state_root, self.binding) as selected:
                    recovered = self.create(*selected, runner=reconciliation.ReconciliationRunner)
                    self.assertTrue(recovered.recovery_only)
                    self.error("recovery_read_only", lambda: recovered.prepare({"family": "agno"}))
                    self.assertEqual(recovered.reconcile({"job_id": first["id"]})["observation"]["result"], expected)

    def test_dead_locator_does_not_authorize_saved_descriptor_or_block_a_new_preparation(self):
        _, first, _ = self.first_install()
        # A killed resource owner cannot clean up its private locator. Its
        # missing socket is evidence of no live owner, never install permission.
        locator = self.state_root / "round-handoff.json"
        local.write_new(locator, local.encoded({"schema_version": 1,
            "socket": str(self.root / "missing-private-owner" / "owner.sock")}))
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected)
            self.assertTrue(recovered.recovery_only)
            self.assertIn(first["id"], recovered.jobs)
        with local.session_owner(self.root / "new-invocation", self.binding) as selected:
            fresh = self.create(*selected)
            self.assertFalse(fresh.recovery_only)
            self.assertEqual(fresh.jobs, {})
        self.assertTrue(locator.exists(), "restart must not silently repurpose old owner custody")

    def test_aggregate_request_limit_refuses_before_effect_and_all_retained_jobs_still_reopen(self):
        self.first_install()
        # Preserve the existing per-workspace 128-validation ceiling. Actual
        # successful requests cross three rounds: 1 + 128 + 127 = 256.
        for count in (128, 127):
            with LiveRoundOffer(self, self.state_root, self.binding) as offer:
                with local.session_owner(self.state_root, self.binding) as selected:
                    current = self.create(*selected)
                    offer.finish()
                    current.prepare({"family": "agno"}); current.join()
                    for _ in range(count):
                        job = current.install(self.checked(current))["job"]; current.join()
                        self.assertEqual(current.jobs[job["id"]]["status"], "succeeded")
                    if count == 127:
                        self.assertEqual(sum("recovery_request" in row for row in current.jobs.values()), 256)
                        checked = self.checked(current)
                        before = len(current.runner.calls)
                        self.error("recovery_selection_full", lambda: current.install(checked))
                        self.assertEqual(len(current.runner.calls), before)
                    current.close_admission(); current.join()
        with local.session_owner(self.state_root, self.binding) as selected:
            recovered = self.create(*selected)
            self.assertTrue(recovered.recovery_only)
            self.assertEqual(len(recovered.jobs), 256)
            self.assertEqual(len(recovered.job_summaries({"before": None})["jobs"]), 50)

    def test_request_retained_before_failed_current_checkpoint_is_not_an_empty_round(self):
        self.first_install()
        with LiveRoundOffer(self, self.state_root, self.binding) as offer:
            with local.session_owner(self.state_root, self.binding) as selected:
                current = self.create(*selected)
                offer.finish()
                current.prepare({"family": "agno"}); current.join()
                current.recovery_selection_writer = lambda _: (_ for _ in ()).throw(OSError("full disk"))
                failed = current.install(self.checked(current))["job"]
                self.assertEqual(failed["error"], "recovery_selection_unavailable")
                self.assertFalse((current.workspace / "local-recovery.json").exists())
                self.assertEqual(current.jobs[failed["id"]]["status"], "unconfirmed")
        with local.session_owner(self.state_root, self.binding) as selected:
            with self.assertRaisesRegex(local.LocalSetupError, "recovery_selection_incomplete"):
                self.create(*selected)


if __name__ == "__main__":
    unittest.main()
