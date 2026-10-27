"""Owned harmless process fixtures; no native runtime or service-manager proof."""
import asyncio
from copy import deepcopy
import os
from pathlib import Path
import plistlib
import signal
import sys
import uuid
from types import SimpleNamespace

import pytest

from launchkit import deployment, deployment_worker, deployment_service
from launchkit.deployment_deadlines import (UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS,
    MAX_REPORTED_GATEWAY_SHUTDOWN_HORIZON_MS, EARLIER_PINNED_GATEWAY_SHUTDOWN_HORIZON_MS,
    APPLICATION_STOP_TIMEOUT, NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT, APPLICATION_ADAPTER_STOP_MARGIN,
    PROCESS_FORCE_REAP_TIMEOUT, WORKER_STOP_TIMEOUT, USER_SERVICE_STOP_TIMEOUT,
    SERVICE_STOP_COMMAND_TIMEOUT, LEGACY_USER_SERVICE_STOP_TIMEOUT, shutdown_budget)
from launchkit.projects import DraftConflict
from test_deployment import Guide, Implementation, Service, submit
from test_gateway_shutdown_horizon import gateway_horizon_ms


def test_every_horizon_has_ordered_enclosing_deadlines():
    unreported = -(-UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS // 1000)
    assert unreported + NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT < APPLICATION_STOP_TIMEOUT
    assert APPLICATION_ADAPTER_STOP_MARGIN == 10
    assert APPLICATION_STOP_TIMEOUT + PROCESS_FORCE_REAP_TIMEOUT < WORKER_STOP_TIMEOUT
    assert WORKER_STOP_TIMEOUT < USER_SERVICE_STOP_TIMEOUT < SERVICE_STOP_COMMAND_TIMEOUT
    assert deployment_worker.STOP_TIMEOUT == APPLICATION_STOP_TIMEOUT
    assert shutdown_budget() == shutdown_budget(UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS)
    for horizon_ms in (1, 999, EARLIER_PINNED_GATEWAY_SHUTDOWN_HORIZON_MS, 432_000,
                       MAX_REPORTED_GATEWAY_SHUTDOWN_HORIZON_MS):
        budget = shutdown_budget(horizon_ms)
        # A partial second rounds up: no window ends inside the horizon.
        assert budget["application"] * 1000 >= horizon_ms + (
            NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT + APPLICATION_ADAPTER_STOP_MARGIN) * 1000
        assert budget["application"] + budget["force_reap"] < budget["worker"] < budget["service"] < budget["command"]
    # The unit an earlier Forge wrote from its pinned 347-second horizon.
    assert shutdown_budget(EARLIER_PINNED_GATEWAY_SHUTDOWN_HORIZON_MS)["service"] == 397


async def test_application_adapter_time_is_outside_full_installed_transport_shutdown(monkeypatch):
    # The installed transport can spend its full negotiated horizon, then TERM
    # and KILL waits. Console close happens before runtime shutdown; this real
    # application boundary therefore needs time beyond the transport's maximum.
    from meerkat_mobkit import _transport
    transport_maximum = (UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS // 1000
        + _transport._PROCESS_TERMINATE_GRACE_SECONDS
        + _transport._PROCESS_KILL_GRACE_SECONDS)
    assert transport_maximum == UNREPORTED_GATEWAY_SHUTDOWN_HORIZON_MS // 1000 + NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT
    process = SimpleNamespace(pid=778, returncode=None, terminate=lambda: None)
    async def wait(): return process.returncode
    process.wait = wait
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    supervisor.child = process
    required_seconds = transport_maximum + 5  # Actual generated Console close bound.
    signals = []
    async def model_wait(awaitable, timeout):
        if process.returncode is None and timeout < required_seconds:
            awaitable.close()
            raise asyncio.TimeoutError
        if process.returncode is None:
            process.returncode = 0
        return await awaitable
    monkeypatch.setattr(deployment_worker.asyncio, "wait_for", model_wait)
    def owned_signal(pid, sig):
        signals.append((pid, sig))
        if process.returncode is None: process.returncode = -sig
    monkeypatch.setattr(deployment_worker.os, "killpg", owned_signal)
    await supervisor.terminate()
    assert process.returncode == 0
    assert required_seconds < deployment_worker.STOP_TIMEOUT
    assert signals == [(778, signal.SIGTERM), (778, signal.SIGKILL)]


async def test_delayed_graceful_application_cleanup_survives_original_short_window(tmp_path, monkeypatch):
    marker = tmp_path / "cleanup-ack"
    script = ("import signal,sys,time\nfrom pathlib import Path\n"
        "def stop(*_):\n time.sleep(.08)\n Path(sys.argv[1]).write_text('cleanup completed')\n raise SystemExit(0)\n"
        "signal.signal(signal.SIGTERM,stop)\nprint('ready',flush=True)\n"
        "while True: time.sleep(1)\n")
    process = await asyncio.create_subprocess_exec(sys.executable, "-c", script, str(marker),
        stdout=asyncio.subprocess.PIPE, start_new_session=True)
    assert await process.stdout.readline() == b"ready\n"
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    supervisor.child = process
    actual_wait = asyncio.wait_for
    observed = []
    async def bounded_wait(awaitable, timeout):
        observed.append(timeout)
        # 30 seconds would map to .015 and preempt this .08-second cleanup.
        return await actual_wait(awaitable, timeout * .0005 if timeout == APPLICATION_STOP_TIMEOUT else timeout)
    monkeypatch.setattr(deployment_worker.asyncio, "wait_for", bounded_wait)
    try:
        await supervisor.terminate()
        assert process.returncode == 0 and marker.read_text() == "cleanup completed"
        assert observed == [APPLICATION_STOP_TIMEOUT, PROCESS_FORCE_REAP_TIMEOUT]
        await deployment.require_owned_group_absent(process)
    finally:
        if process.returncode is None:
            os.killpg(process.pid, signal.SIGKILL)
            await process.wait()


async def test_uncooperative_application_is_force_reaped_only_in_its_owned_group():
    process = await asyncio.create_subprocess_exec(sys.executable, "-c",
        "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print('ready',flush=True); time.sleep(60)",
        stdout=asyncio.subprocess.PIPE, start_new_session=True)
    assert await process.stdout.readline() == b"ready\n"
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    supervisor.child = process
    try:
        await asyncio.wait_for(supervisor.terminate(timeout=.02), 2)
        assert process.returncode == -signal.SIGKILL
        await deployment.require_owned_group_absent(process)
    finally:
        if process.returncode is None:
            os.killpg(process.pid, signal.SIGKILL)
            await process.wait()


async def test_force_reap_timeout_never_reports_cleanup(monkeypatch):
    process = SimpleNamespace(pid=777, returncode=None, terminate=lambda: None)
    async def never_exit():
        await asyncio.Event().wait()
    process.wait = never_exit
    supervisor = deployment_worker.Supervisor.__new__(deployment_worker.Supervisor)
    supervisor.child = process
    signals = []
    monkeypatch.setattr(deployment_worker.os, "killpg", lambda pid, sig: signals.append((pid, sig)))
    monkeypatch.setattr(deployment_worker, "PROCESS_FORCE_REAP_TIMEOUT", .01)
    with pytest.raises(RuntimeError, match="did not confirm exit"):
        await supervisor.terminate(timeout=.01)
    assert signals == [(777, signal.SIGTERM), (777, signal.SIGTERM), (777, signal.SIGKILL)]


@pytest.mark.parametrize("recorded", [None, 432_000], ids=["unreported", "reported-432s"])
@pytest.mark.parametrize("force", [False, True])
async def test_host_waits_for_owned_worker_then_bounds_exact_force_reap(tmp_path, monkeypatch, force, recorded):
    guide = Guide()
    held = Service(tmp_path / "server", guide, Implementation(guide))
    initial = await submit(held, tmp_path / "application")
    directory = held._directory(guide.identity)
    await held.shutdown()
    if recorded is not None:
        # The worker recorded the horizon its application reported at readiness.
        deployment.update_receipt(directory, gateway_shutdown_horizon_ms=recorded)
    # The worker's own application stop, its force reap, then five seconds.
    # Without a report the worker assumes this repository's gateway horizon.
    worker_window = ((gateway_horizon_ms() if recorded is None else recorded) // 1000
        + NATIVE_TRANSPORT_FORCE_REAP_TIMEOUT + APPLICATION_ADAPTER_STOP_MARGIN + PROCESS_FORCE_REAP_TIMEOUT + 5)
    closed, exited = asyncio.Event(), asyncio.Event()
    admitted = asyncio.Event()
    signals = []
    class Input:
        def close(self):
            closed.set()
            if not force:
                asyncio.get_running_loop().call_later(.06, exited.set)
    class Process:
        pid = 999
        returncode = None
        stdin = Input()
        async def wait(self):
            await exited.wait()
            self.returncode = -signal.SIGKILL if force else 0
            return self.returncode
    process = Process()
    async def spawn(*args, **kwargs):
        admitted.set()
        return process
    def killpg(pid, sig):
        assert pid == process.pid and sig == signal.SIGKILL
        signals.append((pid, sig))
        exited.set()
    actual_wait = asyncio.wait_for
    timeouts = []
    async def bounded_wait(awaitable, timeout):
        timeouts.append(timeout)
        return await actual_wait(awaitable, .03 if force else .15)
    monkeypatch.setattr(deployment.asyncio, "create_subprocess_exec", spawn)
    monkeypatch.setattr(deployment.asyncio, "wait_for", bounded_wait)
    monkeypatch.setattr(deployment.os, "killpg", killpg)
    fresh = deployment.DeploymentService(held.root.parent, guide, held.implementation)
    task = asyncio.create_task(fresh._run(directory, "install"))
    await admitted.wait()
    # Wait until the recovered child is owned before cancelling its controller.
    while initial["deployment_id"] not in fresh.processes:
        await asyncio.sleep(0)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert closed.is_set() and process.returncode is not None
    assert timeouts == ([worker_window, PROCESS_FORCE_REAP_TIMEOUT] if force else [worker_window])
    assert signals == ([(999, signal.SIGKILL)] if force else [])
    assert not fresh.processes
    assert deployment.read_receipt(directory).get("interactive_cleanup") is None


@pytest.mark.parametrize("platform", ["darwin", "linux"])
async def test_legacy_recorded_unit_keeps_its_original_exact_budget(tmp_path, monkeypatch, platform):
    monkeypatch.setattr(deployment_service, "sys", SimpleNamespace(platform=platform, executable=sys.executable))
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    Path.home().mkdir()
    guide = Guide()
    async def capability():
        return {"available": True, "manager": "launchd" if platform == "darwin" else "systemd-user"}
    held = Service(tmp_path / "server", guide, Implementation(guide),
                   service_manager=SimpleNamespace(capability=capability))
    initial = await submit(held, tmp_path / "application", execution_mode="background")
    directory = held._directory(guide.identity)
    deployment.materialize(directory)
    await held.shutdown()
    plan = deployment_service.prepare_service(directory, deployment.read_receipt(directory))
    legacy = deepcopy(plan)
    legacy.pop("stop_timeout_seconds")
    content = deployment_service.unit_bytes(legacy)
    unit = Path(plan["unit_path"])
    unit.write_bytes(content)
    legacy["unit_sha256"] = deployment_service.digest(content)
    deployment.update_receipt(directory, service=legacy)
    assert deployment_service.verify_service(directory, deployment.read_receipt(directory)) == legacy
    assert unit.read_bytes() == content
    if platform == "darwin":
        assert plistlib.loads(content)["ExitTimeOut"] == LEGACY_USER_SERVICE_STOP_TIMEOUT
    else:
        assert b"TimeoutStopSec=45\n" in content
    forged = {**legacy, "stop_timeout_seconds": USER_SERVICE_STOP_TIMEOUT}
    with pytest.raises(DraftConflict, match="definition changed"):
        deployment_service.verify_service(directory, {**deployment.read_receipt(directory), "service": forged})
    with pytest.raises(DraftConflict, match="budget is unsupported"):
        deployment_service.verify_service(directory, {**deployment.read_receipt(directory),
            "service": {**legacy, "stop_timeout_seconds": True}})
    assert unit.read_bytes() == content


@pytest.mark.parametrize("platform", ["darwin", "linux"])
async def test_manager_stop_command_outlives_new_unit_budget_without_lengthening_queries(tmp_path, platform):
    class Manager(deployment_service.UserServiceManager):
        def __init__(self): self.calls = []
        async def observe(self, service): return {"loaded": True, "pid": 123, "state": "running"}
        async def command(self, arguments, **kwargs):
            self.calls.append((arguments, kwargs.get("timeout")))
            return 0, ""
    service = deployment_service.service_plan(str(uuid.uuid4()), tmp_path,
        platform=platform, home=tmp_path)
    manager = Manager()
    await manager.stop(service)
    if platform == "darwin":
        assert [timeout for _, timeout in manager.calls] == [None, SERVICE_STOP_COMMAND_TIMEOUT]
        assert manager.calls[-1][0][1] == "bootout"
    else:
        assert [timeout for _, timeout in manager.calls] == [SERVICE_STOP_COMMAND_TIMEOUT]
        assert manager.calls[-1][0][2:4] == ["disable", "--now"]


async def test_explicit_stop_command_timeout_preserves_actual_delayed_command_completion(monkeypatch):
    monkeypatch.setattr(deployment_service, "SERVICE_TIMEOUT", .01)
    code, output = await deployment_service.UserServiceManager().command(
        [sys.executable, "-I", "-c", "import time; time.sleep(.06); print('stopped')"], timeout=1)
    assert code == 0 and output == "stopped\n"
