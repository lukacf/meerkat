"""Forge parent integration with explicit receiver/process/control fixtures.

No native runtime, credential drop, installed host or process containment is
exercised here. King owns those separate controls and the allocation itself.
"""
import asyncio
import copy
from dataclasses import replace
import json
import pickle
from pathlib import Path
import sys
from types import SimpleNamespace

import pytest

from launchkit import guide_runtime, implementation, protected_worker, rehearsal
from launchkit.blueprint import starter_blueprint
from launchkit.protected_worker import ProtectedWorker

pytestmark = pytest.mark.asyncio


class Child:
    def __init__(self):
        self.code = None

    def poll(self):
        return self.code


class Control:
    def __init__(self, preparation):
        self.preparation = preparation
        self.started = self.closing = False
        self.task = None
        self.commands = []
        self.writing = asyncio.Event()
        self.release = asyncio.Event()
        self.release.set()

    async def send(self, command):
        if self.task is not None and not self.task.done():
            raise RuntimeError("fixture busy writer")
        async def write():
            self.writing.set()
            await self.release.wait()
            self.commands.append(command)
            return 1
        self.task = asyncio.create_task(write())
        return await asyncio.shield(self.task)

    async def finish(self):
        self.closing = True
        if self.task is not None:
            await asyncio.shield(self.task)
        self.preparation.events.append("eof")
        self.preparation.spawn.child.code = 0


class Preparation:
    def __init__(self):
        self.request = self.original_request = None
        self.spawn = self.original_spawn = None
        self.spawn_task = None
        self.entered = asyncio.Event()
        self.spawn_release = asyncio.Event()
        self.spawn_release.set()
        self.close_entered = asyncio.Event()
        self.close_release = asyncio.Event()
        self.close_release.set()
        self.closed = self.fail_close = self.fail_activate = self.exit_on_activate = False
        self.events = []
        self.native_close_entered = asyncio.Event()
        self.native_close_release = asyncio.Event()
        self.native_close_release.set()
        self.fail_native_close = False
        self.gateway = SimpleNamespace(closed=False, original_receiver_preparation=self,
                                       receiver_preparation=self)
        state = SimpleNamespace(gateway=self.gateway, path=None,
                                joined=lambda: self.gateway.closed and self.closed)
        self.gateway.state_owner = self.gateway.original_state_owner = state
        async def close_gateway():
            self.native_close_entered.set()
            await self.native_close_release.wait()
            if self.fail_native_close:
                raise RuntimeError("fixture native/state cleanup remains unconfirmed")
            self.events.append("gateway-close")
            self.gateway.closed = True
        self.gateway.close = close_gateway

    def current(self):
        return not self.closed

    def select_worker(self, *, kind, directory):
        if self.request is not None:
            raise RuntimeError("fixture original request already selected")
        self.request = self.original_request = SimpleNamespace(kind=kind, directory=directory)
        suffix = ".mobkit/state" if kind == "implementation" else ".meta/state"
        self.gateway.original_state_owner.path = str(Path(directory) / "native" / suffix)
        self.events.append("select")
        return self.request

    async def spawn_held(self):
        async def spawn():
            child, control = Child(), Control(self)
            async def activate():
                self.events.append("activate")
                if self.fail_activate:
                    raise RuntimeError("fixture activation failed after child admission")
                control.started = True
                if self.exit_on_activate:
                    child.code = 0
            self.spawn = self.original_spawn = SimpleNamespace(child=child,
                process=SimpleNamespace(child=child), control=control,
                original_control=control, activate=activate)
            self.entered.set()
            await self.spawn_release.wait()
            return self.spawn
        assert self.spawn_task is None
        self.spawn_task = asyncio.create_task(spawn())
        return await asyncio.shield(self.spawn_task)

    async def close(self):
        self.close_entered.set()
        if self.spawn_task is not None:
            await asyncio.shield(self.spawn_task)
        await self.close_release.wait()
        if self.fail_close:
            raise RuntimeError("fixture original cleanup remains unconfirmed")
        self.events.append("close")
        if self.spawn is not None:
            self.spawn.child.code = 0
        self.closed = True

    def borrower_joined(self):
        return self.closed


@pytest.fixture
def preparation(monkeypatch):
    # Exact-type fixture injection is local to these tests. Production imports
    # the delivered King's OriginalReceiverPreparation, never this substitute.
    monkeypatch.setitem(sys.modules, "workspace_receiver_preparation",
                        SimpleNamespace(OriginalReceiverPreparation=Preparation))
    return Preparation()


@pytest.fixture(params=["implementation", "rehearsal"])
def service(request, preparation, tmp_path, monkeypatch):
    kind = request.param
    module = implementation if kind == "implementation" else rehearsal
    constructor = module.ImplementationService if kind == "implementation" else module.RehearsalService
    def prepare(**selection):
        assert selection == {"kind": kind, "directory": tmp_path}
        return preparation
    service = constructor(tmp_path, object(), worker_preparer=prepare)
    def forbidden(*args, **kwargs):
        pytest.fail("protected worker used legacy factory export, environment, spawn or stop")
    monkeypatch.setattr(guide_runtime, "capture_worker_runtime", forbidden)
    monkeypatch.setattr(module, "spawn_owned_worker", forbidden)
    monkeypatch.setattr(module, "stop_owned_worker", forbidden)
    if kind == "rehearsal":
        monkeypatch.setattr(module, "worker_environment", forbidden)
    (tmp_path / "input.json").write_text(json.dumps({"blueprint": starter_blueprint().model_dump(mode="json")}))
    (tmp_path / "receipt.json").write_text(json.dumps({"status": "running", "worker_stop_confirmed": False}))
    return service, tmp_path


def launch(service, directory):
    task = asyncio.create_task(service._run(directory))
    service.tasks[directory.name] = task
    return task


async def test_both_services_use_original_receiver_without_export_or_legacy_spawn(service, preparation):
    owner, directory = service
    preparation.exit_on_activate = True
    await launch(owner, directory)
    assert preparation.events == ["select", "activate", "close", "gateway-close"]
    assert implementation.read_receipt(directory)["worker_stop_confirmed"] is True
    assert not owner.processes
    assert not (directory / "forge-agent-runtime.json").exists()
    assert not (directory / "worker.log").exists(), "King owns the protected output drain"


async def test_cancelled_spawn_keeps_preparation_until_original_child_and_cleanup_join(service, preparation):
    owner, directory = service
    preparation.spawn_release.clear()
    preparation.close_release.clear()
    task = launch(owner, directory)
    await preparation.entered.wait()
    retained = owner.processes[directory.name]
    assert type(retained) is ProtectedWorker and retained.preparation is preparation
    task.cancel()
    await preparation.close_entered.wait()
    assert owner.processes[directory.name] is retained
    assert not implementation.read_receipt(directory)["worker_stop_confirmed"]
    assert not task.done()
    preparation.spawn_release.set()
    preparation.close_release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert preparation.closed and not owner.processes
    assert implementation.read_receipt(directory)["worker_stop_confirmed"]


async def test_failed_activation_still_retires_original_admitted_child(service, preparation):
    owner, directory = service
    preparation.fail_activate = True
    await launch(owner, directory)
    assert preparation.closed and preparation.spawn.child.code == 0
    assert implementation.read_receipt(directory)["status"] == "failed"
    assert not owner.processes


async def test_invalid_host_preparation_cannot_fall_back_to_legacy_spawn(service, preparation):
    owner, directory = service
    owner._worker_preparer = lambda **selection: None
    await launch(owner, directory)
    assert implementation.read_receipt(directory)["status"] == "failed"
    assert preparation.events == [] and not owner.processes
    assert not (directory / "forge-agent-runtime.json").exists()
    assert not (directory / "worker.log").exists()


async def test_failed_cleanup_retains_live_owner_and_cannot_claim_stop_from_exit(service, preparation):
    owner, directory = service
    preparation.exit_on_activate = preparation.fail_close = True
    with pytest.raises(ExceptionGroup, match="Protected worker stop failed"):
        await launch(owner, directory)
    retained = owner.processes[directory.name]
    assert retained.preparation is preparation and retained.returncode == 0
    assert not implementation.read_receipt(directory)["worker_stop_confirmed"]
    assert not preparation.closed


async def test_rehearsal_cancelled_command_is_uncertain_and_writer_joins_before_eof(preparation, tmp_path):
    worker = ProtectedWorker(preparation, "rehearsal", str(tmp_path))
    await worker.start()
    control = preparation.spawn.control
    control.release.clear()
    owner = rehearsal.RehearsalService(tmp_path, object())
    owner.tasks[tmp_path.name] = asyncio.current_task()
    owner.processes[tmp_path.name] = worker
    command = {"kind": "continue", "id": "fixture-command", "text": "fixture"}
    task = asyncio.create_task(owner._command(tmp_path, command))
    await control.writing.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert json.loads((tmp_path / "control.json").read_text())["status"] == "uncertain"
    assert not control.task.done() and owner.processes[tmp_path.name] is worker
    stopping = asyncio.create_task(worker.stop())
    await asyncio.sleep(.02)
    assert not stopping.done() and "eof" not in preparation.events
    control.release.set()
    await stopping
    assert control.commands == [command]
    assert preparation.events[-3:] == ["eof", "close", "gateway-close"]


async def test_parent_copy_replay_and_pid_change_refuse_before_process_operation(preparation, tmp_path, monkeypatch):
    worker = ProtectedWorker(preparation, "implementation", str(tmp_path))
    for duplicate in (copy.copy, copy.deepcopy, pickle.dumps):
        with pytest.raises(TypeError):
            duplicate(worker)
    with pytest.raises(RuntimeError, match="already selected"):
        replace(worker)
    monkeypatch.setattr(protected_worker.os, "getpid", lambda: worker._pid + 1)
    with pytest.raises(RuntimeError, match="original parent"):
        await worker.start()
    assert preparation.spawn is None


async def test_replaced_process_cannot_be_waited_or_used_as_cleanup_evidence(preparation, tmp_path):
    worker = ProtectedWorker(preparation, "implementation", str(tmp_path))
    await worker.start()
    child = preparation.spawn.child
    preparation.spawn.child = Child()
    with pytest.raises(RuntimeError, match="association changed"):
        await worker.wait()
    with pytest.raises(ExceptionGroup):
        await worker.stop()
    # The original owner was asked to close even when observation refused.
    assert preparation.close_entered.is_set() and worker._child is child


async def test_stop_observer_cancellation_keeps_same_original_cleanup_task(preparation, tmp_path):
    worker = ProtectedWorker(preparation, "implementation", str(tmp_path))
    await worker.start()
    preparation.close_release.clear()
    stopping = asyncio.create_task(worker.stop())
    await preparation.close_entered.wait()
    original = worker._stop_task
    stopping.cancel()
    with pytest.raises(asyncio.CancelledError):
        await stopping
    assert not original.done()
    preparation.close_release.set()
    await worker.stop()
    assert worker._stop_task is original and preparation.closed


async def test_cancelled_original_write_still_joins_preparation_and_preserves_failure(preparation, tmp_path):
    worker = ProtectedWorker(preparation, "implementation", str(tmp_path))
    await worker.start()
    async def failed_finish():
        raise asyncio.CancelledError("fixture original writer cancellation")
    preparation.spawn.control.finish = failed_finish
    with pytest.raises(BaseExceptionGroup) as raised:
        await worker.stop()
    assert any(isinstance(error, asyncio.CancelledError) for error in raised.value.exceptions)
    assert preparation.closed and preparation.close_entered.is_set()


async def test_launcher_selection_is_atomic_original_and_precedes_admission(tmp_path):
    build = implementation.ImplementationService(tmp_path, object())
    trial = rehearsal.RehearsalService(tmp_path, object())
    def preparer(**selection):
        raise AssertionError("selection alone must not allocate or spawn")
    trial.tasks["fixture-busy"] = object()
    with pytest.raises(RuntimeError, match="before any worker admission"):
        protected_worker.select_worker_preparer(preparer, build, trial)
    assert build._worker_preparer is None and trial._worker_preparer is None
    trial.tasks.clear()
    protected_worker.select_worker_preparer(preparer, build, trial)
    trial.tasks["fixture-active"] = object()
    protected_worker.select_worker_preparer(preparer, build, trial)
    with pytest.raises(RuntimeError, match="cannot be replaced"):
        protected_worker.select_worker_preparer(lambda **selection: None, build, trial)
    assert build._worker_preparer is preparer and trial._worker_preparer is preparer


async def test_parent_retains_owner_after_receiver_exit_until_native_and_state_join(service, preparation):
    owner, directory = service
    preparation.exit_on_activate = True
    preparation.native_close_release.clear()
    task = launch(owner, directory)
    await preparation.native_close_entered.wait()
    retained = owner.processes[directory.name]
    assert preparation.closed and not preparation.gateway.closed
    assert not implementation.read_receipt(directory)["worker_stop_confirmed"]
    task.cancel()
    await asyncio.sleep(.02)
    assert not task.done() and owner.processes[directory.name] is retained
    preparation.native_close_release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert preparation.gateway.original_state_owner.joined()
    assert not owner.processes


async def test_native_state_join_failure_is_not_hidden_by_reaped_receiver(service, preparation):
    owner, directory = service
    preparation.exit_on_activate = preparation.fail_native_close = True
    with pytest.raises(ExceptionGroup):
        await launch(owner, directory)
    assert preparation.closed and preparation.spawn.child.code == 0
    assert not preparation.gateway.closed
    assert owner.processes[directory.name].preparation is preparation
    assert not implementation.read_receipt(directory)["worker_stop_confirmed"]


@pytest.mark.parametrize("state_path", [None, "/different/state"])
async def test_worker_state_cannot_be_substituted_or_made_ephemeral(preparation, tmp_path, state_path):
    worker = ProtectedWorker(preparation, "implementation", str(tmp_path))
    preparation.gateway.original_state_owner.path = state_path
    with pytest.raises(RuntimeError, match="exact persistent state"):
        await worker.start()
    assert preparation.spawn is None
    with pytest.raises(ExceptionGroup):
        await worker.stop()
    assert preparation.closed and preparation.gateway.closed


async def test_legacy_unselected_gateway_is_not_borrowed_or_retired(preparation, tmp_path):
    preparation.gateway.state_owner = preparation.gateway.original_state_owner = None
    with pytest.raises(TypeError, match="own explicitly selected"):
        ProtectedWorker(preparation, "implementation", str(tmp_path))
    assert preparation.events == []
    assert not preparation.closed and not preparation.gateway.closed


async def test_closed_gateway_without_original_state_join_cannot_confirm_cleanup(preparation, tmp_path):
    worker = ProtectedWorker(preparation, "implementation", str(tmp_path))
    await worker.start()
    preparation.gateway.original_state_owner.joined = lambda: False
    with pytest.raises(ExceptionGroup, match="Protected worker stop failed"):
        await worker.stop()
    assert preparation.gateway.closed and preparation.closed
