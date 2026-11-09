"""Disposable source transactions and real owned child groups, without native providers."""
import asyncio
import json
import os
import sys
from types import MethodType
import uuid

import pytest

from launchkit import deployment, deployment_worker, deployment_updates as updates
from launchkit.deployment import DeploymentService, read_receipt, update_receipt
from launchkit.projects import DraftConflict
from test_deployment import Service
from test_deployment_failed_cleanup import real_fixture, absent


async def transaction_fixture(tmp_path, monkeypatch, mode, *, incomplete=False, changed=False):
    service, directory, target, initial = await real_fixture(tmp_path)
    service.implementation.identity = str(uuid.uuid4())
    service.implementation.digest = "b" * 64
    service.implementation.extra_files = {"application/feature.py": "UPDATED = True\n"}
    staged = await service.stage_update(service.guide.identity, initial["deployment_id"],
        service.guide.state["version"], service.implementation.identity)
    identity = staged["update"]["update_id"]
    if mode != "update":
        updates.activate(directory, identity)
        _, root, current = updates.transaction(directory, identity)
        updates.checkpoint(directory, root, current, runtime_admission_possible=True)
        updates.committed(directory, identity)
        if mode == "recover-current":
            _, root, current = updates.transaction(directory, identity)
            updates.checkpoint(directory, root, current, status="recovery-needed")
    ready, stop = asyncio.Event(), asyncio.Event()
    real_spawn = asyncio.create_subprocess_exec
    children = []

    async def run(root, actual_mode, *, update_selection=None):
        assert actual_mode == mode
        supervisor = deployment_worker.Supervisor(root)
        if mode == "update":
            updates.activate(root, identity)
        elif mode == "rollback":
            updates.restore(root, identity, restore_data=False)
        else:
            updates.retry_candidate(root, identity)
        supervisor.child = await real_spawn(sys.executable, "-c", "import time; time.sleep(60)",
            start_new_session=True)
        children.append(supervisor.child.pid)
        _, transaction_root, current = updates.transaction(root, identity)
        updates.checkpoint(root, transaction_root, current, runtime_admission_possible=True)
        updates.committed(root, identity, restored=mode == "rollback")
        update_receipt(root, status="running", phase="run", runtime_pid=supervisor.child.pid,
            process_live=True, startup_attempt_id="fixture-current-source")
        ready.set()
        try:
            await stop.wait()
        finally:
            await supervisor.terminate()
        if incomplete:
            supervisor.cleanup_observed = False
        if changed:
            update_receipt(root, update_action={"action": "rollback", "update_id": str(uuid.uuid4())})
        update_receipt(root, status="stopped", runtime_pid=None, process_live=False)
        return 0

    class Input:
        def write(self, value):
            assert json.loads(value)["action"] == "stop"
            stop.set()

        async def drain(self):
            pass

        def close(self):
            stop.set()

    class Wrapper:
        pid = os.getpid()  # This fixture runs the fixed wrapper in this test process.
        returncode = None
        stdin = Input()

        def __init__(self, root, actual_mode, lifecycle, *selection):
            self.execution = asyncio.create_task(deployment.interactive_worker(root, actual_mode, lifecycle, *selection))

        async def wait(self):
            try:
                result = await asyncio.shield(self.execution)
            except ValueError:
                self.returncode = 1
                return 1
            self.returncode = result or 0
            return self.returncode

    async def spawn(*args, **kwargs):
        assert args[1] == "-c" and "interactive_worker" in args[2]
        assert kwargs["start_new_session"] is True
        return Wrapper(*args[3:])

    monkeypatch.setattr(deployment_worker, "run", run)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    service._run = MethodType(DeploymentService._run, service)
    action = "apply-update" if mode == "update" else mode
    admitted = await service.action(service.guide.identity, initial["deployment_id"], action, identity)
    await asyncio.wait_for(ready.wait(), 5)
    return service, directory, target, initial, admitted, children


@pytest.mark.parametrize("mode", ["update", "rollback", "recover-current"])
@pytest.mark.parametrize("cancel_owner", [False, True])
async def test_transaction_teardown_binds_final_source_and_allows_ordinary_restart(tmp_path, monkeypatch,
        mode, cancel_owner):
    service, directory, target, initial, admitted, children = await transaction_fixture(tmp_path, monkeypatch, mode)
    task = service.tasks[initial["deployment_id"]]
    cold = None
    try:
        if cancel_owner:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
        else:
            await service.action(service.guide.identity, initial["deployment_id"], "stop")
            await asyncio.wait_for(task, 5)
        stopped = read_receipt(directory)
        assert stopped["status"] == ("interrupted" if cancel_owner else "stopped")
        assert deployment.interactive_cleanup_matches(stopped)
        assert stopped["interactive_cleanup"]["lifecycle_id"] == admitted["interactive_lifecycle_id"]
        assert stopped["interactive_cleanup"]["build_id"] == stopped["build_id"]
        assert stopped["interactive_cleanup"]["package_sha256"] == stopped["package_sha256"]
        assert stopped["build_id"] == (initial["build_id"] if mode == "rollback" else service.implementation.identity)
        assert all(absent(pid) for pid in children)
        assert (target / "data/records.json").read_text() == '{"kept":"domain record"}'
        assert (target / ".mobkit/state/native.json").read_text() == '{"kept":"native record"}'
        before = {name: (target / name).read_bytes() for name in ("app.py", ".env", "application/__init__.py")}
        real_observe = os.kill

        def observe(pid, number):
            # Only wrapper execution is in-process in this fixture. Native-like
            # child-group teardown above used the actual captured OS handles.
            if pid == os.getpid() and number == 0:
                raise ProcessLookupError("fixture wrapper returned")
            return real_observe(pid, number)

        monkeypatch.setattr(deployment.os, "kill", observe)
        cold = Service(service.root.parent, service.guide, service.implementation)
        assert not cold.tasks and not cold.processes and not cold.worker_admissions
        restarted = await cold.action(service.guide.identity, initial["deployment_id"], "restart")
        assert restarted["status"] == "queued"
        assert restarted["interactive_lifecycle_id"] != stopped["interactive_lifecycle_id"]
        assert restarted["interactive_cleanup"] is None
        assert all((target / name).read_bytes() == content for name, content in before.items())
    finally:
        if cold is not None:
            await cold.shutdown()
        await service.shutdown()


@pytest.mark.parametrize("defect", ["incomplete", "changed"])
async def test_transaction_never_promotes_unconfirmed_or_different_teardown(tmp_path, monkeypatch, defect):
    service, directory, _, initial, _, children = await transaction_fixture(tmp_path, monkeypatch, "update",
        incomplete=defect == "incomplete", changed=defect == "changed")
    task = service.tasks[initial["deployment_id"]]
    try:
        await service.action(service.guide.identity, initial["deployment_id"], "stop")
        await asyncio.wait_for(task, 5)
        assert all(absent(pid) for pid in children)
        assert read_receipt(directory)["status"] == "interrupted"
        assert read_receipt(directory)["interactive_cleanup"] is None
        with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
            await service.action(service.guide.identity, initial["deployment_id"], "restart")
    finally:
        await service.shutdown()


@pytest.mark.parametrize("mode,tamper", [
    ("update", None), ("rollback", None), ("recover-current", None),
    ("update", "journal"), ("update", "package"),
])
async def test_running_transaction_keeps_exact_cleanup_source_when_next_update_is_staged(
        tmp_path, monkeypatch, mode, tamper):
    service, directory, target, initial, admitted, children = await transaction_fixture(
        tmp_path, monkeypatch, mode)
    task = service.tasks[initial["deployment_id"]]
    original = read_receipt(directory)
    original_id = original["update"]["update_id"]
    original_root = updates.update_directory(directory, original_id)
    source = {key: original[key] for key in ("build_id", "guide_version", "blueprint_hash", "package_sha256")}
    before = {name: (target / name).read_bytes() for name in
        ("app.py", ".env", "application/__init__.py", "data/records.json", ".mobkit/state/native.json")}
    try:
        assert all(not absent(pid) for pid in children)
        service.implementation.identity = str(uuid.uuid4())
        service.implementation.digest = "c" * 64
        service.implementation.extra_files = {"application/feature.py": "NEXT_UPDATE = True\n"}
        staged = await service.stage_update(service.guide.identity, initial["deployment_id"],
            service.guide.state["version"], service.implementation.identity)
        selected = staged["update"]
        assert selected["update_id"] != original_id
        assert staged["update_action"] == original["update_action"]
        assert all(staged[key] == value for key, value in source.items())
        with pytest.raises(DraftConflict, match="different update is now selected"):
            updates.transaction(directory, original_id)
        if tamper == "journal":
            update = json.loads((original_root / "update.json").read_text())
            updates.write_private(original_root / "update.json", {**update, "source_digest": "d" * 64})
        elif tamper == "package":
            (original_root / "candidate.zip").write_bytes(b"changed retained package")
        journal = (original_root / "update.json").read_bytes()

        await service.action(service.guide.identity, initial["deployment_id"], "stop")
        await asyncio.wait_for(task, 5)
        stopped = read_receipt(directory)
        assert all(absent(pid) for pid in children)
        assert all((target / name).read_bytes() == value for name, value in before.items())
        assert stopped["update"] == selected
        assert (original_root / "update.json").read_bytes() == journal
        if tamper:
            assert stopped["status"] == "interrupted"
            assert stopped["interactive_cleanup"] is None
        else:
            assert stopped["status"] == "stopped"
            assert deployment.interactive_cleanup_matches(stopped)
            cleanup = stopped["interactive_cleanup"]
            assert cleanup["lifecycle_id"] == admitted["interactive_lifecycle_id"]
            assert all(cleanup[key] == value for key, value in source.items())
            assert cleanup["worker_exit_confirmed"] is True
        with pytest.raises(DraftConflict, match="different update is now selected"):
            updates.transaction(directory, original_id)
    finally:
        await service.shutdown()


async def test_existing_interrupted_transaction_cannot_gain_retroactive_cleanup(tmp_path):
    service, directory, _, initial = await real_fixture(tmp_path)
    before = update_receipt(directory, status="interrupted", interactive_lifecycle_id=None,
        interactive_cleanup=None, runtime_pid=None, process_live=False)
    saved = (directory / "receipt.json").read_bytes()
    for action in ("stop", "restart"):
        with pytest.raises(DraftConflict, match="cleanup is unconfirmed"):
            await service.action(service.guide.identity, initial["deployment_id"], action)
    assert (directory / "receipt.json").read_bytes() == saved
    assert read_receipt(directory)["build_id"] == before["build_id"]
