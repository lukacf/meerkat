"""Meta Forge's bounded stop of its native work owners (owner_shutdown)."""
import asyncio
import logging
import os
from pathlib import Path
import signal
import sys
import time
from types import SimpleNamespace

import pytest

from launchkit import owner_shutdown, server
from launchkit.owner_shutdown import Owner, release_worker, stop_owners

SELF_STOPPING_WORKER = r'''
import os, signal, sys, time
from pathlib import Path
marker = Path(sys.argv[1])
signal.signal(signal.SIGTERM, lambda *_: marker.with_suffix(".signalled").write_text("SIGTERM"))
sys.stdin.buffer.read()
marker.with_suffix(".eof").write_text("owner lost")
time.sleep(float(sys.argv[2]))
marker.write_text("stopped by itself")
'''


def absent(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    return False


async def spawn_worker(marker, stop_seconds):
    return await asyncio.create_subprocess_exec(sys.executable, "-c", SELF_STOPPING_WORKER, str(marker),
        str(stop_seconds), stdin=asyncio.subprocess.PIPE, start_new_session=True)


async def eventually(predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while not predicate():
        assert time.monotonic() < deadline
        await asyncio.sleep(.02)


class Hung:
    """An owner whose stop keeps its custody through cancellation and never ends."""

    def __init__(self):
        self.cancelled = 0
        self.release = asyncio.Event()

    async def shutdown(self):
        while not self.release.is_set():
            try:
                await self.release.wait()
            except asyncio.CancelledError:
                self.cancelled += 1


async def test_owner_past_its_deadline_is_left_and_the_owners_after_it_still_stop(caplog):
    calls = []

    def owner(name, result=None):
        async def shutdown():
            calls.append(name)
            if result is not None:
                raise result
        return Owner(name, SimpleNamespace(shutdown=shutdown))

    hung = Hung()
    failure = RuntimeError("cleanup unconfirmed")
    caplog.set_level(logging.INFO, logger="uvicorn.error")
    started = time.monotonic()
    failures = await stop_owners([owner("first"), Owner("hung", hung), owner("failing", failure),
                                  owner("last")], deadline=.3)
    try:
        assert time.monotonic() - started < 2
        assert calls == ["first", "failing", "last"]
        assert [name for name, _ in failures] == ["hung", "failing"]
        assert isinstance(failures[0][1], TimeoutError) and failures[1][1] is failure
        # Cancelled once and never awaited again: the owner keeps its custody.
        await asyncio.sleep(.05)
        assert hung.cancelled == 1 and not hung.release.is_set()
        lines = [record.getMessage() for record in caplog.records if record.name == "uvicorn.error"]
        assert "Meta Forge: hung did not stop within 0.3 s" in lines
        assert "Meta Forge: stopping failing failed (RuntimeError)" in lines
        assert any(line.startswith("Meta Forge: stopped last in ") for line in lines)
        assert not any(line.startswith("Meta Forge: stopped hung") for line in lines)
    finally:
        hung.release.set()


async def test_only_the_blown_owners_own_groups_are_signalled_sigterm_then_sigkill(monkeypatch):
    stubborn = await asyncio.create_subprocess_exec(sys.executable, "-c",
        "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)",
        start_new_session=True)
    polite = await asyncio.create_subprocess_exec(sys.executable, "-c", "import time; time.sleep(60)",
        start_new_session=True)
    bystander = await asyncio.create_subprocess_exec(sys.executable, "-c", "import time; time.sleep(60)",
        start_new_session=True)
    real_killpg, sent = os.killpg, []

    def recording(group, number):
        if number:
            sent.append((group, number, time.monotonic()))
        return real_killpg(group, number)

    monkeypatch.setattr(owner_shutdown.os, "killpg", recording)
    await asyncio.sleep(.3)
    hung = Hung()

    async def done():
        pass

    try:
        failures = await stop_owners([
            Owner("finished", SimpleNamespace(shutdown=done), groups=lambda: [bystander.pid]),
            Owner("hung", hung, groups=lambda: [stubborn.pid, polite.pid, os.getpgrp(), 0, 1, -polite.pid,
                                                 polite.pid]),
        ], deadline=.2, grace=1.5)
        assert [name for name, _ in failures] == ["hung"]
        # SIGTERM first, to exactly the hung owner's own groups; Forge's own
        # group and ids that address no single group are never signalled.
        assert [(group, number) for group, number, _ in sent[:2]] == [
            (stubborn.pid, signal.SIGTERM), (polite.pid, signal.SIGTERM)]
        killed = [(group, at) for group, number, at in sent[2:] if number == signal.SIGKILL]
        assert all(number == signal.SIGKILL for _, number, _ in sent[2:])
        assert stubborn.pid in [group for group, _ in killed]
        assert {group for group, _ in killed} <= {stubborn.pid, polite.pid}
        # SIGKILL only once the grace has passed.
        assert all(at - sent[0][2] >= 1.45 for _, at in killed)
        assert await asyncio.wait_for(stubborn.wait(), 5) == -signal.SIGKILL
        assert await asyncio.wait_for(polite.wait(), 5) == -signal.SIGTERM
        assert bystander.returncode is None and not absent(bystander.pid)
    finally:
        hung.release.set()
        for process in (stubborn, polite, bystander):
            if process.returncode is None:
                real_killpg(process.pid, signal.SIGKILL)
                await process.wait()


async def test_self_stopping_workers_are_released_never_signalled_and_survive_collection(tmp_path, monkeypatch):
    released = await spawn_worker(tmp_path / "released", 2.0)
    control = await spawn_worker(tmp_path / "control", 2.0)
    pid, signals = released.pid, []

    def recorder(real):
        def send(target, number):
            if number and abs(target) == pid:
                signals.append((target, number))
            return real(target, number)
        return send

    monkeypatch.setattr(os, "kill", recorder(os.kill))
    monkeypatch.setattr(os, "killpg", recorder(os.killpg))
    hung = Hung()
    try:
        failures = await stop_owners([Owner("worker owner", hung, workers=lambda: [released])], deadline=.2)
        assert [name for name, _ in failures] == ["worker owner"]
        # Owner loss is the closed stdin; no signal ever reached the worker.
        await eventually(lambda: (tmp_path / "released.eof").exists())
        # A normal exit closes, or collects and so closes, this handle; asyncio's
        # kill in that close must not run for a released worker.
        released._transport.close()
        await asyncio.sleep(.1)
        assert not absent(pid) and signals == []
        await eventually(lambda: (tmp_path / "released").exists())
        assert (tmp_path / "released").read_text() == "stopped by itself"
        # The control handle was never released: asyncio's own close kills it,
        # which is the exit-time kill the release prevents.
        control.stdin.close()
        await eventually(lambda: (tmp_path / "control.eof").exists())
        control._transport.close()
        assert await asyncio.wait_for(control.wait(), 5) == -signal.SIGKILL
        assert not (tmp_path / "control").exists()
    finally:
        hung.release.set()
        if not absent(pid):
            await eventually(lambda: (tmp_path / "released").exists())
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass
        if control.returncode is None:
            control.kill()
            await control.wait()


async def test_release_guard_failure_keeps_asyncio_behaviour_and_only_warns(tmp_path, monkeypatch, caplog):
    worker = await spawn_worker(tmp_path / "unreleased", .2)
    monkeypatch.setattr(owner_shutdown, "RELEASE_VERSIONS", ())
    caplog.set_level(logging.INFO, logger="uvicorn.error")
    hung = Hung()
    try:
        failures = await stop_owners([Owner("worker owner", hung, workers=lambda: [worker])], deadline=.2)
        assert [name for name, _ in failures] == ["worker owner"]
        assert worker._transport.is_closing() is False
        lines = [record.getMessage() for record in caplog.records if record.name == "uvicorn.error"]
        assert ("Meta Forge: 1 worker owner worker(s) could not be released on this Python; "
                "Forge's exit may end them") in lines
        assert release_worker(SimpleNamespace(stdin=None, returncode=None)) is False
        assert await asyncio.wait_for(worker.wait(), 10) == 0
    finally:
        hung.release.set()
        if worker.returncode is None:
            worker.kill()
            await worker.wait()


def test_lifespan_names_self_stopping_workers_for_three_owners_and_groups_for_none(monkeypatch):
    rows = []

    async def recorded(owners):
        rows.extend(owners)
        return []

    services = {}
    for name in ["guide_operations", "visualization_operations", "visualization_reviews", "visual_review_runtime",
                 "remote_deployment", "gcp_provisioning", "gcp_preflight", "deployment", "rehearsal",
                 "implementation", "checker_setup", "toolkit_saved_requests", "architect"]:
        services[name] = SimpleNamespace(processes={"live": object()}, shutdown=None)
        monkeypatch.setattr(server, name, services[name])
    monkeypatch.setattr(server, "stop_owners", recorded)
    monkeypatch.setattr(server.provider_settings, "restore", lambda: None)

    async def reconciled():
        pass

    monkeypatch.setattr(services["visual_review_runtime"], "reconcile_terminated", reconciled, raising=False)

    async def run():
        async with server.lifespan(server.app):
            pass

    asyncio.run(run())
    assert [row.name for row in rows] == ["guide operations", "visualization operations", "visualization reviews",
        "visual critic runtime", "remote deployment", "GCP resource preparation", "GCP access checks",
        "deployment", "rehearsal", "implementation", "checker preparation", "saved Toolkit requests", "architect"]
    assert all(row.groups is None for row in rows)
    with_workers = {row.name: row.workers() for row in rows if row.workers is not None}
    assert set(with_workers) == {"deployment", "rehearsal", "implementation"}
    for name, attribute in (("deployment", "deployment"), ("rehearsal", "rehearsal"),
                            ("implementation", "implementation")):
        assert with_workers[name] == list(services[attribute].processes.values())


def test_slow_application_stop_survives_forge_exit_and_next_start_confirms_its_cleanup(tmp_path):
    import gc
    import io
    import json
    import socket
    import zipfile
    from datetime import datetime, timezone
    from launchkit import deployment
    from launchkit.deployment import DeploymentService, materialize, read_receipt, update_receipt
    from test_deployment import Guide, Service, submit
    from test_deployment_failed_cleanup import APP, ChildImplementation, wait_status

    # The application's own SIGTERM handling takes longer than Forge's deadline.
    slow_stop = APP.replace("    def stop(*_):\n", "    def stop(*_):\n        time.sleep(3)\n", 1)
    assert slow_stop != APP

    class SlowStopImplementation(ChildImplementation):
        async def package(self, identity, build_id):
            payload, slug = await super().package(identity, build_id)
            result = io.BytesIO()
            with zipfile.ZipFile(io.BytesIO(payload)) as old, zipfile.ZipFile(result, "w") as archive:
                for name in old.namelist():
                    archive.writestr(zipfile.ZipInfo(name), slow_stop if name == "app.py" else old.read(name))
            return result.getvalue(), slug

    guide = Guide()
    implementation = SlowStopImplementation(guide)
    implementation.digest = "a" * 64
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        implementation.definition["port"] = probe.getsockname()[1]
    implementation.application_configuration = {"fields": [
        {"key": "APP_READY", "label": "Fixture ready", "required": True, "secret": False}]}
    target = tmp_path / "application"

    async def install():
        held = Service(tmp_path / "server", guide, implementation)
        initial = await submit(held, target, settings={"OPENAI_API_KEY": "fixture-private", "APP_READY": "yes"})
        directory = held._directory(guide.identity)
        materialize(directory)
        await held.shutdown()
        update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None,
                       runtime_pid=None, process_live=False)
        interpreter = target / ".venv/bin/python"
        interpreter.parent.mkdir(parents=True)
        interpreter.symlink_to(sys.executable)
        return initial["deployment_id"], directory

    deployment_id, directory = asyncio.run(install())

    async def forge():
        # One server lifetime, ended like uvicorn's asyncio.run: every task
        # still pending is cancelled once the lifespan has returned.
        service = DeploymentService(tmp_path / "server", guide, implementation)
        await service.action(guide.identity, deployment_id, "restart")
        await wait_status(service, "running")
        worker = service.processes[deployment_id].pid
        started = time.monotonic()
        failures = await stop_owners([Owner("deployment", service,
            workers=lambda: list(service.processes.values()))], deadline=1)
        assert time.monotonic() - started < 3
        assert [name for name, _ in failures] == ["deployment"] and isinstance(failures[0][1], TimeoutError)
        # Forge's stop returned while the worker still stops its application.
        assert not absent(worker) and read_receipt(directory)["interactive_cleanup"] is None
        return worker, datetime.now(timezone.utc)

    worker, returned = asyncio.run(forge())
    # Interpreter exit collects the server's handles; that must not kill the worker.
    gc.collect()
    deadline = time.monotonic() + 30
    while True:
        cleanup = read_receipt(directory).get("interactive_cleanup") or {}
        if cleanup.get("supervisor_pid") == worker:
            break
        assert time.monotonic() < deadline, "the worker never recorded its own cleanup"
        time.sleep(.05)
    while True:
        try:
            if os.waitpid(worker, os.WNOHANG)[0] == worker:
                break
        except ChildProcessError:
            break
        assert time.monotonic() < deadline
        time.sleep(.05)
    assert absent(worker)
    # The worker recorded its own cleanup after Forge's stop returned.
    assert datetime.fromisoformat(cleanup["completed_at"]) > returned
    assert cleanup["boundary"] == "owned-process-groups-absent" and cleanup["worker_exit_confirmed"] is False
    children = json.loads((target / ".mobkit/test-child.json").read_text())
    assert all(absent(pid) for pid in children.values())

    async def next_start():
        cold = DeploymentService(tmp_path / "server", guide, implementation)
        return await cold.action(guide.identity, deployment_id, "stop")

    stopped = asyncio.run(next_start())
    confirmed = stopped["interactive_cleanup"]
    assert stopped["status"] == "stopped" and stopped["process_live"] is False
    assert confirmed["supervisor_pid"] == worker and confirmed["supervisor_absence_confirmed"] is True
    assert confirmed["supervisor_absence_scope"] == "cold-host-owned-native-teardown"
    assert deployment.interactive_cleanup_matches(stopped)
