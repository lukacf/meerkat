"""Offline implementation ownership and stop controls, without native jobs."""
import asyncio
from contextvars import copy_context
from copy import deepcopy
import os
from pathlib import Path
import signal
import threading
from types import SimpleNamespace
import uuid

import pytest

from launchkit import implementation as service_module
from launchkit import implementation_worker as worker
from launchkit import owned_worker
from launchkit.blueprint import starter_blueprint
from launchkit.implementation import ImplementationService, read_receipt, update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates.storage_setup import write_private
from test_meta_guide_runtime import selected as selected_guide_runtime

pytestmark = pytest.mark.asyncio


class Guide:
    def __init__(self):
        self.identity = str(uuid.uuid4())
        self.state = {"id": self.identity, "version": 3,
                      "proposal": starter_blueprint().model_dump(mode="json"),
                      "brief": {"mission": "Synthetic offline build"}, "messages": []}

    async def get(self, identity):
        assert identity == self.identity
        return deepcopy(self.state)


@pytest.fixture
def build(tmp_path, selected_guide_runtime):
    guide = Guide()
    service = ImplementationService(tmp_path, guide)
    identity = str(uuid.uuid4())
    directory = service._directory(guide.identity, identity)
    directory.mkdir(parents=True)
    state = guide.state
    frozen = {"blueprint": state["proposal"], "brief": state["brief"],
              "messages": [], "guide_id": guide.identity, "guide_version": 3}
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", {
        "guide_id": guide.identity, "guide_version": 3, "build_id": identity,
        "blueprint_hash": fingerprint(state["proposal"]), "brief_hash": fingerprint(state["brief"]),
        "status": "queued", "progress": [], "files": [], "checks": [], "unresolved": [],
        "request": "", "ready_to_package": False, "native_console_url": None,
        "stop_requested": False, "cleanup_pending": True,
        "cleanup_in_progress": False, "worker_stop_confirmed": False})
    write_private(directory.parent / "latest.json", {"build_id": identity})
    ImplementationWorkspace(directory / "workspace", state["proposal"]).seed()
    return service, directory


def own_task(service, directory, coroutine):
    task = asyncio.create_task(coroutine)
    service.tasks[directory.name] = task
    task.add_done_callback(lambda done: service._task_finished(directory, done))
    return task


class Child:
    """Caller-bound synthetic handle; stop helper behavior is covered separately."""
    pid = 7777777
    returncode = None

    def __init__(self):
        self.waiting = asyncio.Event()

    async def wait(self):
        await self.waiting.wait()
        return self.returncode


async def test_stop_is_prompt_idempotent_and_keeps_owned_cleanup_until_confirmed(build, monkeypatch):
    service, directory = build
    process, entered, draining, release = Child(), asyncio.Event(), asyncio.Event(), asyncio.Event()
    async def spawn(*args, **options):
        assert options["stdin"] == asyncio.subprocess.PIPE
        entered.set()
        return process, False
    stops = []
    async def stop(actual):
        assert actual is process
        stops.append(actual)
        draining.set()
        await release.wait()
        actual.returncode = 0
        actual.waiting.set()
    monkeypatch.setattr(service_module, "spawn_owned_worker", spawn)
    monkeypatch.setattr(service, "_stop_process", stop)
    task = own_task(service, directory, service._run(directory))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        first = await asyncio.wait_for(service.request_stop(service.guide.identity, 3, directory.name), .2)
        await draining.wait()
        count = task.cancelling()
        second = await service.request_stop(service.guide.identity, 3, directory.name)
        assert first["stop_requested"] and second["cleanup_pending"] and second["cleanup_in_progress"]
        assert task.cancelling() == count and stops == [process]
        assert service.processes[directory.name] is process and not task.done()
        with pytest.raises(DraftConflict, match="cleanup"):
            await service.submit(service.guide.identity, 3, "Another build",
                                 expected_selected_build_id=directory.name)
        # An independent shutdown/cancellation must not cut off admitted drain.
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and stops == [process]
        release.set()
        await asyncio.gather(task, return_exceptions=True)
        receipt = read_receipt(directory)
        assert receipt["status"] == "interrupted" and receipt["worker_stop_confirmed"]
        assert not receipt["cleanup_pending"] and not service.processes and not service.tasks
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)


async def test_cleanup_failure_retains_exact_handle_and_stop_retries_without_spawn(build, monkeypatch):
    service, directory = build
    process = Child()
    calls, spawn_count = [], 0
    retry_release = asyncio.Event()
    async def spawn(*args, **options):
        nonlocal spawn_count
        spawn_count += 1
        return process, False
    async def stop(actual):
        assert actual is process
        calls.append(actual)
        if len(calls) == 1:
            raise PermissionError("Synthetic inaccessible owned group")
        await retry_release.wait()
        actual.returncode = 0
        actual.waiting.set()
    monkeypatch.setattr(service_module, "spawn_owned_worker", spawn)
    monkeypatch.setattr(service, "_stop_process", stop)
    task = own_task(service, directory, service._run(directory))
    await asyncio.sleep(0)
    await service.request_stop(service.guide.identity, 3, directory.name)
    await asyncio.gather(task, return_exceptions=True)
    assert read_receipt(directory)["cleanup_pending"]
    assert service.processes[directory.name] is process and directory.name not in service.tasks
    retry = await service.request_stop(service.guide.identity, 3, directory.name)
    assert retry["cleanup_pending"] and retry["cleanup_in_progress"]
    retry_task = service.tasks[directory.name]
    await service.request_stop(service.guide.identity, 3, directory.name)
    retry_release.set()
    await retry_task
    assert spawn_count == 1 and len(calls) == 2
    assert not service.processes and not read_receipt(directory)["cleanup_pending"]


async def test_spawn_cancel_race_keeps_child_handle_and_awaits_cleanup(build, monkeypatch):
    service, directory = build
    admitted, release = asyncio.Event(), asyncio.Event()
    process, stopped = Child(), []
    async def spawn(*args, **options):
        assert options["start_new_session"] is True
        admitted.set()
        await release.wait()
        return process
    async def stop(actual):
        stopped.append(actual)
        actual.returncode = 0
    monkeypatch.setattr(owned_worker.asyncio, "create_subprocess_exec", spawn)
    monkeypatch.setattr(service, "_stop_process", stop)
    task = own_task(service, directory, service._run(directory))
    await admitted.wait()
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    release.set()
    await asyncio.gather(task, return_exceptions=True)
    assert stopped == [process] and not service.processes
    assert read_receipt(directory)["worker_stop_confirmed"]


async def test_completed_source_receipt_survives_failed_cleanup_and_retry(build, monkeypatch):
    service, directory = build
    process = Child()
    process.returncode = 0
    process.waiting.set()
    authority = {"source_digest": "frozen-source", "reviewer_interaction_id": "completed-native-review"}
    update_receipt(directory, status="completed", ready_to_package=True, semantic_acceptance=authority,
                   checked_digest="frozen-source", completed_at="frozen-completion")
    async def spawn(*args, **options):
        return process, False
    async def fail(actual):
        raise TimeoutError("Synthetic surviving owned group")
    monkeypatch.setattr(service_module, "spawn_owned_worker", spawn)
    monkeypatch.setattr(service, "_stop_process", fail)
    task = own_task(service, directory, service._run(directory))
    await asyncio.gather(task, return_exceptions=True)
    stored = read_receipt(directory)
    assert stored["status"] == "completed" and stored["ready_to_package"]
    assert stored["semantic_acceptance"] == authority and stored["checked_digest"] == "frozen-source"
    assert stored["cleanup_pending"] and service.processes[directory.name] is process
    release = asyncio.Event()
    async def stopped(actual):
        assert actual is process
        await release.wait()
    monkeypatch.setattr(service, "_stop_process", stopped)
    await service.request_stop(service.guide.identity, 3, directory.name)
    retry_task = service.tasks[directory.name]
    release.set()
    await retry_task
    assert read_receipt(directory)["status"] == "completed"
    assert read_receipt(directory)["semantic_acceptance"] == authority


async def test_unstarted_stop_has_no_worker_and_is_durable(build):
    service, directory = build
    task = own_task(service, directory, service._run(directory))
    await service.request_stop(service.guide.identity, 3, directory.name)
    await asyncio.gather(task, return_exceptions=True)
    receipt = read_receipt(directory)
    assert receipt["status"] == "interrupted" and receipt["stop_requested"]
    assert receipt["worker_stop_confirmed"] and not receipt["cleanup_pending"]
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("wrong", ["version", "build"])
async def test_stop_rejects_wrong_current_version_or_selected_build_without_mutation(build, wrong):
    service, directory = build
    before = (directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict):
        await service.request_stop(service.guide.identity, 2 if wrong == "version" else 3,
                                   str(uuid.uuid4()) if wrong == "build" else directory.name)
    assert (directory / "receipt.json").read_bytes() == before


async def test_changed_guide_can_stop_its_exact_selected_old_worker(build):
    service, directory = build
    async def waiting():
        await asyncio.Event().wait()
    task = own_task(service, directory, waiting())
    service.guide.state["version"] = 4
    service.guide.state["brief"]["mission"] = "A revised job"
    service.guide.state["proposal"]["project"]["name"] = "Revised proposal"
    result = await service.request_stop(service.guide.identity, 4, directory.name)
    assert result["stop_requested"] and result["current"] is False
    await asyncio.gather(task, return_exceptions=True)


async def test_lost_pending_custody_is_visible_and_cannot_acknowledge_cleanup_retry(build):
    service, directory = build
    before = (directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict, match="no longer owns"):
        await service.request_stop(service.guide.identity, 3, directory.name)
    assert (directory / "receipt.json").read_bytes() == before
    current = await service.status(service.guide.identity)
    assert current["status"] == "interrupted" and current["cleanup_pending"]
    assert current["cleanup_custody_lost"] and not current["cleanup_in_progress"]
    assert not current["ready_to_package"]
    with pytest.raises(DraftConflict, match="cleanup cannot be confirmed"):
        await service.submit(service.guide.identity, 3, expected_selected_build_id=directory.name)
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("first", ["publication", "stop"])
async def test_receipt_two_writers_preserve_publication_and_host_stop_flags(build, monkeypatch, first):
    service, directory = build
    entered, release, second_started = threading.Event(), threading.Event(), threading.Event()
    original = service_module.read_receipt
    reads, failures = [], []
    def controlled_read(root):
        name = threading.current_thread().name
        reads.append(name)
        receipt = original(root)
        if name == "first-writer":
            entered.set()
            if not release.wait(2):
                raise TimeoutError("Synthetic receipt writer was not released")
        return receipt
    monkeypatch.setattr(service_module, "read_receipt", controlled_read)
    authority = {"source_digest": "fixture-source", "reviewer_interaction_id": "fixture-review"}
    def publication():
        update_receipt(directory, status="completed", ready_to_package=True,
                       semantic_acceptance=authority, completed_at="fixture-completion")
    def stop():
        service._stop_failed(directory, TimeoutError("Synthetic owned group remains"))
        update_receipt(directory, stop_requested=True)
    def invoke(callback, started=None):
        if started:
            started.set()
        try:
            callback()
        except BaseException as error:
            failures.append(error)
    first_callback, second_callback = (publication, stop) if first == "publication" else (stop, publication)
    first_thread = threading.Thread(target=invoke, args=(first_callback,), name="first-writer")
    second_thread = threading.Thread(target=invoke, args=(second_callback, second_started), name="second-writer")
    first_thread.start()
    try:
        assert entered.wait(30)
        second_thread.start()
        assert second_started.wait(30)
        # A separate open lock descriptor must block this other writer's read.
        await asyncio.sleep(.02)
        assert "second-writer" not in reads
    finally:
        release.set()
        first_thread.join(2)
        if second_thread.ident is not None:
            second_thread.join(2)
    assert not failures and not first_thread.is_alive() and not second_thread.is_alive()
    stored = original(directory)
    assert stored["status"] == "completed" and stored["ready_to_package"]
    assert stored["semantic_acceptance"] == authority and stored["completed_at"] == "fixture-completion"
    assert stored["stop_requested"] and stored["cleanup_pending"]
    assert not stored["worker_stop_confirmed"]


async def test_progress_accumulation_serializes_two_writers_without_erasing_stop_flags(build, monkeypatch):
    _, directory = build
    update_receipt(directory, stop_requested=True)
    entered, release, started = threading.Event(), threading.Event(), threading.Event()
    original = service_module.read_receipt
    reads, failures = [], []
    def controlled_read(root):
        name = threading.current_thread().name
        reads.append(name)
        receipt = original(root)
        if name == "first-progress":
            entered.set()
            if not release.wait(2):
                raise TimeoutError("Synthetic progress writer was not released")
        return receipt
    monkeypatch.setattr(service_module, "read_receipt", controlled_read)
    def record(message, event=None):
        if event:
            event.set()
        try:
            service_module.progress(directory, message)
        except BaseException as error:
            failures.append(error)
    first = threading.Thread(target=record, args=("First actual progress",), name="first-progress")
    second = threading.Thread(target=record, args=("Second actual progress", started), name="second-progress")
    first.start()
    try:
        assert entered.wait(30)
        second.start()
        assert started.wait(30)
        await asyncio.sleep(.02)
        assert "second-progress" not in reads
    finally:
        release.set()
        first.join(2)
        if second.ident is not None:
            second.join(2)
    stored = original(directory)
    assert not failures and not first.is_alive() and not second.is_alive()
    assert [row["message"] for row in stored["progress"]] == ["First actual progress", "Second actual progress"]
    assert stored["stop_requested"] and stored["cleanup_pending"]


@pytest.mark.parametrize("transition", ["task-finished", "process-exit"])
async def test_host_stale_terminal_observation_cannot_overwrite_completed_publication(build, monkeypatch, transition):
    service, directory = build
    original = service_module.read_receipt
    publication = {"status": "completed", "ready_to_package": True,
                   "semantic_acceptance": {"source_digest": "fixture-source"}, "completed_at": "fixture-completion"}
    injected = False
    def publish_after_observation(root):
        nonlocal injected
        stale = original(root)
        if not injected and (transition == "task-finished" or stale.get("worker_stop_confirmed")):
            injected = True
            update_receipt(directory, **publication)
        return stale
    monkeypatch.setattr(service_module, "read_receipt", publish_after_observation)
    if transition == "task-finished":
        task = SimpleNamespace(cancelled=lambda: False, exception=lambda: None)
        service.tasks[directory.name] = task
        service._task_finished(directory, task)
    else:
        child = Child()
        child.returncode = 0
        child.waiting.set()
        async def spawn(*args, **kwargs):
            return child, False
        async def stop(process):
            assert process is child
        monkeypatch.setattr(service_module, "spawn_owned_worker", spawn)
        monkeypatch.setattr(service, "_stop_process", stop)
        await service._run(directory)
    stored = original(directory)
    assert injected
    for key, value in publication.items():
        assert stored[key] == value


@pytest.mark.parametrize("trigger", ["signal", "eof", "timeout"])
async def test_registered_stop_context_disarms_timer_and_does_not_recancel_drain(build, monkeypatch, trigger):
    _, directory = build
    entered, draining, release, cleaned = (asyncio.Event() for _ in range(4))
    callbacks = {}
    loop = asyncio.get_running_loop()
    def register(sig, callback):
        callbacks[sig] = (callback, copy_context())
    monkeypatch.setattr(loop, "add_signal_handler", register)
    monkeypatch.setattr(worker, "IMPLEMENTATION_SECONDS", .03)
    read_fd, write_fd = os.pipe()
    monkeypatch.setattr(worker.sys, "stdin", SimpleNamespace(fileno=lambda: read_fd))
    async def execute(root, stop, *, timing=None):
        entered.set()
        try:
            await asyncio.Event().wait()
        finally:
            draining.set()
            await release.wait()
            cleaned.set()
            owned_worker.begin_worker_cleanup(stop)
    monkeypatch.setattr(worker, "execute", execute)
    task = asyncio.create_task(worker.run(directory, owned_stdin=True))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        callback, context = callbacks[signal.SIGTERM]
        assert context.get(owned_worker.owned_job_deadline) is None
        if trigger == "signal":
            loop.call_soon(callback, context=context)
        elif trigger == "eof":
            os.close(write_fd)
            write_fd = None
        await draining.wait()
        for following, captured in callbacks.values():
            loop.call_soon(following, context=captured)
        await asyncio.sleep(.06)
        assert not task.done() and not cleaned.is_set()
        release.set()
        await asyncio.wait_for(task, 1)
        assert cleaned.is_set()
        assert read_receipt(directory)["status"] == ("failed" if trigger == "timeout" else "interrupted")
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
        if write_fd is not None:
            os.close(write_fd)
        os.close(read_fd)


def fake_native_build(monkeypatch, runtime):
    import meerkat_meta
    class Builder:
        def __getattr__(self, name):
            return lambda *args, **kwargs: self
        async def build(self):
            return runtime
    monkeypatch.setattr(meerkat_meta.Meta, "builder", lambda: Builder())
    monkeypatch.setattr(worker, "planner_capability", lambda: {"provider": "openai"})
    monkeypatch.setenv("MOBKIT_IMPLEMENTATION_MODEL", "gpt-6.1-sol")


@pytest.mark.parametrize("trigger", ["signal", "timeout"])
async def test_actual_execute_retains_initialization_runtime_and_reawaits_shutdown(build, monkeypatch, trigger):
    _, directory = build
    draining, release = asyncio.Event(), asyncio.Event()
    class Runtime:
        cleanup_task = None
        deadline = None
        calls = 0
        finished = False
        async def shutdown(self):
            self.calls += 1
            if self.cleanup_task is None:
                async def cleanup():
                    draining.set()
                    await release.wait()
                    self.finished = True
                self.cleanup_task = asyncio.create_task(cleanup())
            await asyncio.shield(self.cleanup_task)
        async def identity_bootstrap_status(self):
            # Current Meta initializes the identity directly. Cancellation must
            # retain this same runtime and reawait its original cleanup task.
            self.deadline = owned_worker.owned_job_deadline.get()
            await self.shutdown()
            pytest.fail("Cancelled initialization must not admit a model turn.")
    runtime = Runtime()
    fake_native_build(monkeypatch, runtime)
    async def no_turn(*args, **kwargs):
        pytest.fail("No native/model call is allowed in this lifecycle fixture.")
    monkeypatch.setattr(worker, "native_turn", no_turn)
    callbacks, loop = {}, asyncio.get_running_loop()
    def register(sig, callback):
        callbacks[sig] = (callback, copy_context())
    monkeypatch.setattr(loop, "add_signal_handler", register)
    # Reaching initialization takes tens of milliseconds, so a short job limit
    # could expire before the stop this row sends. The row ends the job itself,
    # once the original cleanup is draining.
    monkeypatch.setattr(worker, "IMPLEMENTATION_SECONDS", 30)
    task = asyncio.create_task(worker.run(directory))
    try:
        await asyncio.wait_for(draining.wait(), 30)
        if trigger == "signal":
            callback, context = callbacks[signal.SIGTERM]
            loop.call_soon(callback, context=context)
        else:
            runtime.deadline.reschedule(loop.time())
        await asyncio.sleep(.08)
        assert not task.done() and not runtime.finished
        if trigger == "signal":
            assert runtime.deadline.when() is None  # The stop disarmed the job limit.
        else:
            assert runtime.deadline.expired()
        release.set()
        await asyncio.wait_for(task, 1)
        assert runtime.finished and runtime.calls == 2
        assert read_receipt(directory)["native_runtime_shutdown_confirmed"]
        assert read_receipt(directory)["status"] == ("failed" if trigger == "timeout" else "interrupted")
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)


async def test_normal_completed_source_cleanup_outlives_job_timer_and_preserves_authority(build, monkeypatch):
    _, directory = build
    draining, release = asyncio.Event(), asyncio.Event()
    class Runtime:
        rust_http_base_url = "http://synthetic.invalid"
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            draining.set()
            await release.wait()
    fake_native_build(monkeypatch, Runtime())
    async def prepared(*args):
        pass
    monkeypatch.setattr(worker, "prepare_storage", prepared)
    authority = {"status": "pass", "source_digest": "fixture-checked-source"}
    class Acceptance:
        def __init__(self, *args):
            pass
        def verification(self):
            return deepcopy(authority)
    monkeypatch.setattr(worker, "SemanticAcceptance", Acceptance)
    monkeypatch.setattr(worker, "AcceptanceTools", lambda *args: object())
    async def completed_turn(*args, **kwargs):
        return {"interaction_id": "synthetic-completed"}
    async def reviewed(*args, **kwargs):
        return deepcopy(authority)
    monkeypatch.setattr(worker, "native_turn", completed_turn)
    monkeypatch.setattr(worker, "review_and_repair", reviewed)
    monkeypatch.setattr(worker, "current_publication", lambda tools: {"summary": "Synthetic checked source", "unresolved": []})
    callbacks, loop = {}, asyncio.get_running_loop()
    def register(sig, callback):
        callbacks[sig] = (callback, copy_context())
    monkeypatch.setattr(loop, "add_signal_handler", register)
    monkeypatch.setattr(worker, "IMPLEMENTATION_SECONDS", .03)
    task = asyncio.create_task(worker.run(directory))
    try:
        await asyncio.wait_for(draining.wait(), 30)
        completed = read_receipt(directory)
        assert completed["status"] == "completed" and completed["ready_to_package"]
        for callback, context in callbacks.values():
            loop.call_soon(callback, context=context)
        await asyncio.sleep(.06)
        assert not task.done()
        release.set()
        await asyncio.wait_for(task, 1)
        final = read_receipt(directory)
        assert final["status"] == "completed" and final["ready_to_package"]
        assert final["semantic_acceptance"] == authority
        assert final["completed_at"] == completed["completed_at"]
        assert final["native_runtime_shutdown_confirmed"]
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
