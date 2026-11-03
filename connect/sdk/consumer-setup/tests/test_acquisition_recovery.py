"""Durable acquisition requests reconnect to current, non-executing readers."""
import asyncio
import copy
import json
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

import test_compose_acquisition as compose_tests
import test_node_acquisition as node_tests
import test_python_acquisition as python_tests
from test_selected_setup import adapter, backend


def inventory(root):
    return {str(path.relative_to(root)): (path.stat().st_mode, backend.digest(path.read_bytes()))
            for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


class AcquisitionRecoveryTests(unittest.TestCase):
    def package_controller(self, kind):
        case = python_tests.AcquisitionTests() if kind == "python" else node_tests.NodeAcquisitionTests()
        case.setUp()
        self.addCleanup(case.doCleanups)
        case.profile_path.write_bytes(backend.encoded(case.profile))
        validator = case.root / "validator"
        validator.write_bytes(b"inert validator; recovery must not execute\n")
        validator.chmod(0o700)
        selected = case.root / "selected.json"
        selected.write_bytes(backend.encoded({
            "schema_version": 1, "scope": backend.SELECTED_SCOPE, "staging_root": str(case.staging),
            "installer": {"path": str(case.installer), "sha256": backend.digest(case.installer.read_bytes())},
            "validators": {kind: {"path": str(validator), "sha256": backend.digest(validator.read_bytes())}}}))
        controller = backend.Controller(None, None, case.root / "setup", (), sys.executable,
            str(case.node) if kind == "node" else None, sys.executable if kind == "node" else None,
            selected_install_profile=selected, selected_install_profile_sha256=backend.digest(selected.read_bytes()),
            **{kind + "_acquisition_profile": case.profile_path,
               kind + "_acquisition_profile_sha256": backend.digest(case.profile_path.read_bytes())})
        self.addCleanup(controller.join)
        owner = getattr(controller, kind + "_acquisition")
        original_run = owner.run

        def run(request, directory, run_callable, check_stop):
            case.job = directory
            if kind == "python":
                with patch.object(owner, "fetch_retained", lambda row, checkpoint, *_: case.fetch(row, checkpoint)):
                    return original_run(request, directory, case.runner, check_stop)
            return original_run(request, directory, case.runner, check_stop)

        owner.run = run
        return case, controller, owner

    def publication(self, kind):
        if kind == "compose":
            case = compose_tests.ComposeAcquisitionTests()
            case.setUp()
            self.addCleanup(case.tearDown)
            controller = case.controller()
            request = case.request()
        else:
            case, controller, _ = self.package_controller(kind)
            request = case.request
        controller._acquire(kind, getattr(controller, kind + "_acquisition"), request)
        controller.join()
        self.assertEqual(controller.job["status"], "succeeded", controller.job)
        if kind == "compose":
            (controller.workspace / "acquisitions" / controller.job["id"] / "proposal/entered").unlink()
        return case, controller, request

    def recovered(self, original, suffix="recovered", **changes):
        args = dict(bundle=None, contents_sha256=None, workspace=original.workspace.parent / suffix,
                    families=(), python=sys.executable, node=None, npm_cli=None,
                    recover_workspace=original.workspace, recover_request=original.job["id"],
                    recover_request_sha256=original.job["recovery_request"]["sha256"])
        args.update(changes)
        controller = backend.Controller(**args)
        self.addCleanup(controller.join)
        return controller

    def test_fsync_and_exact_request_precede_first_effect_and_replay(self):
        for kind in ("python", "node"):
            with self.subTest(kind=kind):
                case, controller, owner = self.package_controller(kind)
                events = []
                sync_file, sync_directory, run = backend.sync_file, backend.sync_directory, owner.run

                def file_sync(root, name):
                    sync_file(root, name)
                    events.append((str(root), name))

                def directory_sync(path):
                    sync_directory(path)
                    events.append((str(path), None))

                def effect(request, directory, *args):
                    job = controller.job
                    anchor = job["recovery_request"]
                    record, _ = backend.load_acquisition_recovery(controller.workspace, job["id"], anchor["sha256"])
                    self.assertEqual(record["arguments"], request)
                    self.assertEqual(record["host"]["selected_profile_sha256"], controller.selected_binding["profile_sha256"])
                    self.assertEqual(set(record["host"]["assets"]), backend.ACQUISITION_ASSETS[kind])
                    self.assertIn((str(controller.workspace / "requests"), job["id"] + ".json"), events)
                    self.assertIn((str(controller.workspace.parent), None), events)
                    self.assertIn((str(directory), "request.json"), events)
                    return run(request, directory, *args)

                with patch.object(backend, "sync_file", file_sync), patch.object(backend, "sync_directory", directory_sync), patch.object(owner, "run", effect):
                    first = controller._acquire(kind, owner, case.request)["job"]
                    controller.join()
                self.assertEqual(controller.job["status"], "succeeded", controller.job)
                self.assertEqual(controller._acquire(kind, owner, case.request)["job"]["id"], first["id"])
                self.assertEqual(len(controller.acquisition_intents), 1)
                self.assertEqual(controller.reconcile({"job_id": first["id"]})["observation"]["status"], "matched")

    def test_sync_failure_prevents_effect_and_leaves_no_replay_authority(self):
        case, controller, owner = self.package_controller("python")
        with patch.object(backend, "sync_file", side_effect=OSError("private disk failure")), patch.object(owner, "run") as run:
            with self.assertRaises(OSError):
                controller.acquire_python(case.request)
            run.assert_not_called()
        self.assertFalse(controller.jobs)
        self.assertFalse(controller.acquisition_requests)
        self.assertFalse(controller.operation_lock.locked())

    def test_request_anchor_failure_blocks_new_acquisition_without_erasing_files(self):
        for stage in ("sync", "readback"):
            with self.subTest(stage=stage):
                case, controller, owner = self.package_controller("python")
                error = OSError("private request failure")
                real_sync, real_load = backend.sync_file, backend.load_acquisition_recovery

                def sync(root, name):
                    if root == controller.workspace / "requests" and stage == "sync":
                        raise error
                    return real_sync(root, name)

                def load(*args, **kwargs):
                    if stage == "readback":
                        raise error
                    return real_load(*args, **kwargs)

                with patch.object(backend, "sync_file", sync), patch.object(backend, "load_acquisition_recovery", load), patch.object(owner, "run") as run:
                    with self.assertRaises(OSError) as caught:
                        controller.acquire_python(case.request)
                    self.assertIs(caught.exception, error)
                    run.assert_not_called()
                    retained = inventory(controller.workspace)
                    self.assertEqual(len(list((controller.workspace / "requests").iterdir())), 1)
                    with self.assertRaisesRegex(backend.SetupError, "recovery_request_unavailable"):
                        controller.acquire_python(case.request)
                    run.assert_not_called()
                    self.assertEqual(inventory(controller.workspace), retained)
                self.assertFalse(controller.jobs)
                self.assertFalse(controller.acquisition_requests)
                self.assertFalse(controller.operation_lock.locked())
                self.assertFalse(controller.closing)

    def test_all_families_recover_without_original_external_tools_or_mutating_authority(self):
        for kind in ("python", "node", "compose"):
            with self.subTest(kind=kind):
                case, original, request = self.publication(kind)
                owner = getattr(original, kind + "_acquisition")
                owner.profile_path.unlink()
                if kind == "compose":
                    for name in ("runtime", "planner", "host_profile"):
                        Path(owner.host[name]["path"]).unlink()
                else:
                    (case.pip if kind == "python" else case.npm).unlink()
                before = inventory(original.workspace)
                recovered = self.recovered(original)
                with patch.object(subprocess, "Popen", side_effect=AssertionError("replayed effect")):
                    result = recovered.reconcile({"job_id": original.job["id"]})["observation"]
                self.assertEqual(result["status"], "matched", result)
                self.assertEqual(result["result"], original.job["result"])
                self.assertEqual(result["scope"], backend.ACQUISITION_READBACK)
                self.assertEqual(recovered.job["status"], "unconfirmed")
                self.assertNotIn("result", recovered.job)
                self.assertEqual(inventory(original.workspace), before)
                state = recovered.state()
                self.assertEqual(state["mode"], "recovery")
                summary = state[kind + "_acquisition"]
                self.assertTrue(summary["readback_only"])
                self.assertEqual(summary["workspace"], str(original.workspace))
                self.assertEqual(summary["host_profile_sha256"], owner.profile_sha)
                # History preserves exact correlation, not permission to repeat
                # the acquisition or recreate an install/retry validation handle.
                retained_history = {request["request_id"]: (
                    {"kind": kind, "arguments": copy.deepcopy(request)}, original.job["id"])}
                self.assertEqual(recovered.acquisition_requests, retained_history)
                self.assertTrue(recovered.job["recovered"])
                self.assertEqual(recovered.job["family"], "selected-" + kind)
                self.assertEqual(recovered.job["recovery_request"], original.job["recovery_request"])
                self.assertEqual(list(recovered.jobs), [original.job["id"]])
                retained_jobs = copy.deepcopy(recovered.jobs)
                recovered_files = inventory(recovered.workspace)
                self.assertFalse(recovered.acquisition_bindings)
                for family in ("python", "node", "compose", "mcpb"):
                    self.assertIsNone(getattr(recovered, family + "_acquisition"))
                self.assertIsNone(recovered.selected)
                self.assertEqual(recovered.validations, {})
                self.assertEqual(recovered.selected_retries, {})
                with patch.object(subprocess, "Popen", side_effect=AssertionError("replayed effect")):
                    for action, arguments in ((recovered.acquire_python, request), (recovered.acquire_node, request),
                            (recovered.acquire_compose, request), (recovered.stop, {"job_id": original.job["id"]}),
                            (recovered.selected_check, {"profile": "python", "staged_id": "chosen", "component": "server",
                                                        "package_sha256": "a" * 64, "selection_sha256": "b" * 64}),
                            (recovered.selected_install, {"validation_id": "a" * 32, "effect_sha256": "b" * 64}),
                            (recovered.selected_retry, {"job_id": original.job["id"]})):
                        with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
                            action(arguments)
                    readback = recovered.reconcile({"job_id": original.job["id"]})["observation"]
                    self.assertGreater(readback["revision"], result["revision"])
                    self.assertEqual(readback, {**result, "revision": readback["revision"]})
                self.assertEqual(recovered.jobs, retained_jobs)
                self.assertEqual(recovered.acquisition_requests, retained_history)
                self.assertFalse(recovered.acquisition_bindings)
                self.assertEqual(recovered.validations, {})
                self.assertEqual(recovered.selected_retries, {})
                self.assertEqual(inventory(original.workspace), before)
                self.assertEqual(inventory(recovered.workspace), recovered_files)

    def test_local_history_keeps_exact_acquisition_dedup_and_does_not_restore_retry_authority(self):
        case, original, request = self.publication("python")
        original.close_admission(); original.join()
        identifier = original.job["id"]
        selection = original.recovery_manifest({"job_ids": [identifier]})["text"].encode()
        selected = case.root / "selected.json"
        current = backend.Controller(None, None, case.root / "next-round", (), sys.executable, None, None,
            selected_install_profile=selected,
            selected_install_profile_sha256=backend.digest(selected.read_bytes()),
            python_acquisition_profile=case.profile_path,
            python_acquisition_profile_sha256=backend.digest(case.profile_path.read_bytes()),
            _retained_selections=(selection,))
        self.addCleanup(current.join)
        self.assertFalse(current.recovery_only)
        self.assertIsNotNone(current.python_acquisition)
        with patch.object(current.python_acquisition, "run", side_effect=AssertionError("historical effect replayed")):
            repeated = current.acquire_python(request)["job"]
            current.join()
        self.assertEqual(repeated["id"], identifier)
        self.assertEqual(repeated["status"], "unconfirmed")
        self.assertTrue(repeated["recovered"])
        with self.assertRaises(backend.SetupError) as changed:
            current.acquire_python({**request, "version": "2.0"})
        self.assertEqual(changed.exception.code, "acquisition_request_changed")
        self.assertEqual(list(current.jobs), [identifier])
        self.assertEqual(current.state()["python_acquisition"]["workspace"], str(current.workspace))
        self.assertEqual(current.reconcile({"job_id": identifier})["observation"]["status"], "matched")
        # Current profiles cannot convert an old matched observation into a
        # successful original job or an install/retry validation handle.
        self.assertEqual(current.jobs[identifier]["status"], "unconfirmed")
        self.assertEqual(current.validations, {})
        self.assertEqual(current.selected_retries, {})
        self.assertEqual(current.recovery_manifest({"job_ids": [identifier]})["text"].encode(), selection)

    def test_unused_fresh_inspection_capability_does_not_break_local_library_only_recovery(self):
        # Product descriptors always include public-PyPI and acquired inspection.
        # This exact host setup can have only a library request in its history.
        import test_local_session as local_tests
        fixture = local_tests.LocalRoundTests(methodName="runTest")
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        original, first, _ = fixture.first_install()
        case, unused, _ = self.package_controller("python")
        unused.close_admission(); unused.join()
        selected = case.root / "selected.json"
        with local_tests.local.session_owner(fixture.state_root, fixture.binding) as retained:
            recovered = local_tests.local.controller_factory(local_tests.backend, *retained)(
                fixture.bundle, fixture.sha, fixture.root / "unused", local_tests.backend.FAMILIES,
                sys.executable, sys.executable, sys.executable,
                runner_factory=local_tests.reconciliation.ReconciliationRunner,
                verifier=fixture.bundle / "install-library-consumer.py",
                verifier_sha256=fixture.manifest["files"]["install-library-consumer.py"]["sha256"],
                selected_install_profile=selected,
                selected_install_profile_sha256=backend.digest(selected.read_bytes()),
                python_acquisition_profile=case.profile_path,
                python_acquisition_profile_sha256=backend.digest(case.profile_path.read_bytes()),
                allow_acquired_inspection=True)
            fixture.controllers.append(recovered)
            self.assertTrue(recovered.recovery_only)
            self.assertIsNone(recovered.python_acquisition)
            self.assertFalse(recovered.allow_acquired_inspection)
            self.assertEqual(recovered.reconcile({"job_id": first["id"]})["observation"]["status"], "matched")

    def test_missing_or_changed_members_remain_recoverable_but_unconfirmed(self):
        for target in ("receipt", "worker", "artifact", "request"):
            with self.subTest(target=target):
                case, original, _ = self.publication("python")
                identifier = original.job["id"]
                paths = {"receipt": case.staging / identifier / "acquisition.json",
                         "artifact": case.staging / identifier / "artifact.whl",
                         "worker": original.workspace / "python-acquisition/worker.py",
                         "request": original.workspace / "acquisitions" / identifier / "request.json"}
                paths[target].unlink()
                recovered = self.recovered(original)
                before = inventory(original.workspace)
                observed = recovered.reconcile({"job_id": identifier})["observation"]
                self.assertEqual(observed["status"], "unconfirmed")
                self.assertNotIn("result", observed)
                self.assertEqual(before, inventory(original.workspace))
                self.assertEqual(recovered.job["error"], "historical_outcome_unknown")

    def test_lost_result_and_partial_publication_remain_historically_unknown(self):
        for published in (False, True):
            with self.subTest(published=published):
                case, original, owner = self.package_controller("python")
                run = owner.run

                def interrupted(*args):
                    if published:
                        run(*args)
                    raise backend.SetupError("operation_unconfirmed")

                with patch.object(owner, "run", interrupted):
                    original.acquire_python(case.request)
                    original.join()
                self.assertEqual(original.job["status"], "unconfirmed")
                self.assertNotIn("result", original.job)
                recovered = self.recovered(original)
                observed = recovered.reconcile({"job_id": original.job["id"]})["observation"]
                self.assertEqual(observed["status"], "matched" if published else "unconfirmed")
                self.assertEqual(recovered.job["status"], "unconfirmed")

    def test_wrong_profile_kind_or_digest_cannot_override_recovery_anchor(self):
        _, original, _ = self.publication("python")
        owner = original.python_acquisition
        recovered = self.recovered(original, python_acquisition_profile=owner.profile_path,
                                   python_acquisition_profile_sha256=owner.profile_sha)
        self.assertEqual(recovered.job["family"], "selected-python")
        for index, args in enumerate((
                {"node_acquisition_profile": owner.profile_path, "node_acquisition_profile_sha256": owner.profile_sha},
                {"python_acquisition_profile": owner.profile_path, "python_acquisition_profile_sha256": "0" * 64},
                {"recover_request_sha256": "0" * 64})):
            with self.subTest(args=args), self.assertRaises(backend.SetupError):
                self.recovered(original, suffix="refused" + str(index), **args)
            self.assertFalse((original.workspace.parent / ("refused" + str(index))).exists())

    def test_anchor_rechecked_after_reader_and_failed_observation_can_be_repeated(self):
        _, original, _ = self.publication("python")
        recovered = self.recovered(original)
        identifier = original.job["id"]
        path = original.workspace / "requests" / (identifier + ".json")
        raw = path.read_bytes()
        module = recovered.acquisition_modules["python"]
        reader = module.readback

        def changed(*args):
            result = reader(*args)
            path.write_bytes(raw + b" ")
            return result

        with patch.object(module, "readback", changed):
            self.assertEqual(recovered.reconcile({"job_id": identifier})["observation"]["status"], "unconfirmed")
        path.write_bytes(raw)
        observed = recovered.reconcile({"job_id": identifier})["observation"]
        self.assertEqual(observed["status"], "matched")
        self.assertEqual(observed["revision"], 2)
        self.assertEqual(recovered.job["status"], "unconfirmed")

    def test_reader_failure_and_cancellation_release_lock_without_raw_diagnostics(self):
        _, original, _ = self.publication("python")
        recovered = self.recovered(original)
        module = recovered.acquisition_modules["python"]
        for error in (ValueError("private diagnostic"), asyncio.CancelledError("private cancellation")):
            with patch.object(module, "readback", side_effect=error):
                if isinstance(error, asyncio.CancelledError):
                    with self.assertRaises(asyncio.CancelledError):
                        recovered.reconcile({"job_id": original.job["id"]})
                else:
                    recovered.reconcile({"job_id": original.job["id"]})
            self.assertFalse(recovered.operation_lock.locked())
            self.assertNotIn("private diagnostic", json.dumps(recovered.state()))
            self.assertNotIn("private cancellation", json.dumps(recovered.state()))
            self.assertEqual(recovered.state()["reconciliation"]["observation"]["status"], "unconfirmed")

    def test_concurrent_readback_is_exclusive_and_does_not_restore_effect_admission(self):
        _, original, _ = self.publication("python")
        recovered = self.recovered(original)
        entered, release = threading.Event(), threading.Event()
        module = recovered.acquisition_modules["python"]
        reader = module.readback

        def blocked(*args):
            entered.set()
            self.assertTrue(release.wait(5))
            return reader(*args)

        with patch.object(module, "readback", blocked):
            thread = threading.Thread(target=lambda: recovered.reconcile({"job_id": original.job["id"]}))
            thread.start()
            try:
                self.assertTrue(entered.wait(5))
                self.assertEqual(recovered.state()["reconciliation"]["observation"]["status"], "checking")
                with self.assertRaisesRegex(backend.SetupError, "operation_in_progress"):
                    recovered.reconcile({"job_id": original.job["id"]})
                with self.assertRaisesRegex(backend.SetupError, "recovery_read_only"):
                    recovered.acquire_python({})
            finally:
                release.set()
                thread.join(5)
        self.assertFalse(thread.is_alive())
        self.assertEqual(recovered.state()["reconciliation"]["observation"]["status"], "matched")

    def test_recovery_workspace_cannot_mutate_original_staging_tree(self):
        case, original, _ = self.publication("python")
        with self.assertRaisesRegex(backend.SetupError, "invalid_recovery_path"):
            self.recovered(original, workspace=case.staging / "recovery")
        self.assertFalse((case.staging / "recovery").exists())

    def test_fresh_process_real_mcp_lists_only_readback_tools(self):
        for kind in ("python", "node", "compose"):
            with self.subTest(kind=kind):
                _, original, _ = self.publication(kind)
                before = inventory(original.workspace)
                code = '''import asyncio,importlib.util,json,pathlib,subprocess,sys
from mcp import Client
p=pathlib.Path(sys.argv[1]); spec=importlib.util.spec_from_file_location("recovery_adapter",p)
m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
root=pathlib.Path(sys.argv[2]); identifier=sys.argv[3]
c=m.backend.Controller(None,None,root.parent/"fresh-process",(),sys.executable,None,None,
 recover_workspace=root,recover_request=identifier,recover_request_sha256=sys.argv[4])
def refuse(*a,**k): raise AssertionError("unexpected process execution")
subprocess.Popen=refuse
async def check():
 owner=m.SetupAdapter(c)
 try:
  async with Client(m.create_server(owner)) as client:
   names=sorted(t.name for t in (await client.list_tools()).tools)
   result=await client.call_tool("setup_reconcile",{"job_id":identifier})
   print(json.dumps({"tools":names,"error":result.is_error,"result":result.structured_content,"state":c.state()}))
 finally: await owner.aclose()
asyncio.run(check())
'''
                completed = subprocess.run([sys.executable, "-I", "-B", "-c", code, adapter.__file__,
                                            str(original.workspace), original.job["id"],
                                            original.job["recovery_request"]["sha256"]],
                                           capture_output=True, timeout=15)
                self.assertEqual(completed.returncode, 0, completed.stderr)
                result = json.loads(completed.stdout)
                self.assertEqual(result["tools"], ["setup_reconcile", "setup_recovery_manifest", "setup_state"])
                self.assertFalse(result["error"])
                self.assertEqual(result["result"]["observation"]["status"], "matched")
                self.assertEqual(result["state"]["job"]["status"], "unconfirmed")
                self.assertEqual(before, inventory(original.workspace))


if __name__ == "__main__":
    unittest.main()
