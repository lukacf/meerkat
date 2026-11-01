"""Consumer cancellation with real threads and the host-supplied client parser.

Exchange/close are explicit fixtures. No credential, socket, native worker,
installed launcher or state custody is asserted by these consumer tests.
"""
import asyncio
import copy
import json
import pickle
import threading
from dataclasses import replace

import pytest

from launchkit import guide_runtime, implementation, rehearsal
from launchkit.blueprint import starter_blueprint
from launchkit.remote_protected_worker import RemoteProtectedWorker

RestrictedWorkerClient = pytest.importorskip("workspace_worker_client").RestrictedWorkerClient

pytestmark = pytest.mark.asyncio


class RootExchange:
    def __init__(self):
        # Retain the actual delivered parser; replace only OS exchange/close.
        self.client = object.__new__(RestrictedWorkerClient)
        self.client.exchange = self.exchange
        self.client.close = self.close
        self.client.revoke = lambda: self.events.append("revoke")
        self.client.failed = self.client.closed = False
        self.events = []
        self.calls = []
        self.started = False
        self.exit_code = None
        self.retirement = "open"
        self.failure = None
        self.block = None
        self.entered = threading.Event()
        self.release = threading.Event()
        self.fail_action = self.bad_action = None
        self.exit_on_start = False

    def exchange(self, value, *, timeout):
        # Exercise the delivered request-specific deadline, including stop's
        # longer bound. The original common 60-second limit rejected Forge.
        self.client._validate_timeout(value, timeout)
        action = value["action"]
        self.calls.append((action, value["payload"], timeout, threading.get_ident()))
        self.events.append(action + "-entered")
        if self.client.failed:
            raise RuntimeError("fixture poisoned channel")
        if action == self.block:
            self.entered.set()
            assert self.release.wait(5), "fixture release was not provided"
        if action == self.fail_action:
            self.client.failed = True
            raise TimeoutError("fixture exchange lost")
        if action == "start":
            self.started = True
            if self.exit_on_start:
                self.exit_code = 0
        if action == "stop" and self.retirement == "open":
            self.retirement = "joined"
            if self.started:
                self.exit_code = 0
        result = dict(version=1, started=self.started, exit_code=self.exit_code,
                      retirement=self.retirement, failure=self.failure)
        if action == "send":
            result["pipe_bytes"] = 17
        if action == self.bad_action:
            result["unknown"] = "not accepted"
        self.events.append(action + "-returned")
        return result

    def close(self):
        self.events.append("close")
        self.client.closed = True


async def entered(root):
    async with asyncio.timeout(2):
        while not root.entered.is_set():
            await asyncio.sleep(.005)


async def test_exit_is_observed_separately_from_joined_retirement():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.start()
    root.exit_code = 9
    assert worker.returncode is None
    assert await worker.wait() == 9
    assert not root.client.closed and root.retirement == "open"
    await worker.stop()
    assert root.retirement == "joined" and root.client.closed
    assert all(call[3] != threading.get_ident() for call in root.calls)


async def test_cancelled_start_retains_thread_and_stop_joins_before_close():
    root = RootExchange()
    root.block = "start"
    worker = RemoteProtectedWorker(root.client)
    starting = asyncio.create_task(worker.start())
    await entered(root)
    starting.cancel()
    with pytest.raises(asyncio.CancelledError):
        await starting
    stopping = asyncio.create_task(worker.stop())
    await asyncio.sleep(.01)
    assert not stopping.done() and not root.client.closed
    assert [c[0] for c in root.calls] == ["start"]
    root.release.set()
    await stopping
    assert root.events == ["start-entered", "start-returned", "stop-entered", "stop-returned", "close"]


async def test_cancelled_send_keeps_original_write_and_never_resends():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.start()
    root.block = "send"
    sending = asyncio.create_task(worker.send({"kind": "end", "id": "original"}))
    await entered(root)
    sending.cancel()
    with pytest.raises(asyncio.CancelledError):
        await sending
    stopping = asyncio.create_task(worker.stop())
    await asyncio.sleep(.01)
    assert not root.client.closed
    root.release.set()
    await stopping
    assert [c[0] for c in root.calls] == ["start", "send", "stop"]
    assert root.events.index("send-returned") < root.events.index("stop-entered") < root.events.index("close")


async def test_cancelled_queued_command_is_not_admitted_later():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.start()
    root.block = "status"
    waiting = asyncio.create_task(worker.wait())
    await entered(root)
    sending = asyncio.create_task(worker.send({"kind": "continue", "id": "cancelled"}))
    await asyncio.sleep(.01)
    sending.cancel()
    with pytest.raises(asyncio.CancelledError):
        await sending
    root.exit_code = 0
    root.release.set()
    await waiting
    await worker.stop()
    assert [c[0] for c in root.calls] == ["start", "status", "stop"]


async def test_queued_send_freezes_input_and_serializes_with_status():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.start()
    root.block = "status"
    waiting = asyncio.create_task(worker.wait())
    await entered(root)
    command = {"kind": "continue", "body": {"text": "original"}}
    sending = asyncio.create_task(worker.send(command))
    await asyncio.sleep(.01)
    command["body"]["text"] = "changed"
    assert [c[0] for c in root.calls] == ["start", "status"]
    root.release.set()
    assert await sending == 17
    assert next(c[1] for c in root.calls if c[0] == "send")["body"]["text"] == "original"
    waiting.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiting
    await worker.stop()


async def test_cancelled_stop_observer_does_not_close_before_original_join():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.start()
    root.block = "stop"
    first = asyncio.create_task(worker.stop())
    await entered(root)
    first.cancel()
    with pytest.raises(asyncio.CancelledError):
        await first
    assert not root.client.closed
    root.release.set()
    await worker.stop()
    assert [c[0] for c in root.calls].count("stop") == 1
    assert root.client.closed


@pytest.mark.parametrize("retirement", ["pending", "failed"])
async def test_exit_and_local_close_cannot_confirm_remote_cleanup(retirement):
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.start()
    root.exit_code = 0
    root.retirement = retirement
    assert await worker.wait() == 0
    with pytest.raises(ExceptionGroup, match="stop failed"):
        await worker.stop()
    assert root.client.closed
    with pytest.raises(ExceptionGroup):
        await worker.stop()
    assert [c[0] for c in root.calls].count("stop") == 1


async def test_uncertain_send_poison_is_not_hidden_by_local_close():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.start()
    root.fail_action = "send"
    with pytest.raises(TimeoutError):
        await worker.send({"kind": "end"})
    with pytest.raises(ExceptionGroup, match="stop failed"):
        await worker.stop()
    assert root.client.closed and root.retirement == "open"
    assert [c[0] for c in root.calls] == ["start", "send", "stop"]


async def test_actual_client_rejects_malformed_root_observation_and_poison_is_retained():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    root.bad_action = "start"
    with pytest.raises(Exception, match="Invalid original worker observation"):
        await worker.start()
    assert root.client.failed and "revoke" in root.events
    with pytest.raises(ExceptionGroup):
        await worker.stop()


async def test_owner_refuses_reuse_copies_replay_and_pid_transfer(monkeypatch):
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    with pytest.raises(TypeError):
        RemoteProtectedWorker({"fd": 42})
    for duplicate in (lambda: RemoteProtectedWorker(root.client), lambda: replace(worker),
                      lambda: copy.copy(worker), lambda: pickle.dumps(worker)):
        with pytest.raises((TypeError, RuntimeError)):
            duplicate()
    await worker.start()
    with pytest.raises(RuntimeError, match="replayed"):
        await worker.start()
    with monkeypatch.context() as patch:
        patch.setattr("launchkit.remote_protected_worker.os.getpid", lambda: -1)
        with pytest.raises(RuntimeError, match="original process"):
            await worker.stop()
    await worker.stop()


async def test_stop_before_start_releases_original_unstarted_owner():
    root = RootExchange()
    worker = RemoteProtectedWorker(root.client)
    await worker.stop()
    assert [c[0] for c in root.calls] == ["stop"]
    assert root.client.closed
    with pytest.raises(RuntimeError):
        await worker.start()


@pytest.mark.parametrize("kind", ["implementation", "rehearsal"])
@pytest.mark.parametrize("failed_join", [False, True])
async def test_services_route_remote_owner_and_retain_it_until_join(tmp_path, monkeypatch, kind, failed_join):
    root = RootExchange()
    root.exit_on_start = True
    root.retirement = "failed" if failed_join else "open"
    module = implementation if kind == "implementation" else rehearsal
    constructor = module.ImplementationService if kind == "implementation" else module.RehearsalService
    def prepare(**selection):
        assert selection == {"kind": kind, "directory": tmp_path}
        return root.client
    service = constructor(tmp_path, object(), worker_preparer=prepare)
    def forbidden(*args, **kwargs):
        pytest.fail("remote selection fell back to legacy worker machinery")
    monkeypatch.setattr(guide_runtime, "capture_worker_runtime", forbidden)
    monkeypatch.setattr(module, "spawn_owned_worker", forbidden)
    monkeypatch.setattr(module, "stop_owned_worker", forbidden)
    (tmp_path / "input.json").write_text(json.dumps({"blueprint": starter_blueprint().model_dump(mode="json")}))
    (tmp_path / "receipt.json").write_text(json.dumps({"status": "running", "worker_stop_confirmed": False}))
    task = asyncio.create_task(service._run(tmp_path))
    service.tasks[tmp_path.name] = task
    if failed_join:
        with pytest.raises(ExceptionGroup):
            await task
        assert type(service.processes[tmp_path.name]) is RemoteProtectedWorker
        assert implementation.read_receipt(tmp_path)["worker_stop_confirmed"] is False
    else:
        await task
        assert not service.processes
        assert implementation.read_receipt(tmp_path)["worker_stop_confirmed"] is True
    assert not (tmp_path / "worker.log").exists()
    assert not (tmp_path / "forge-agent-runtime.json").exists()
