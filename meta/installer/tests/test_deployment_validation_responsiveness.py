"""Real inert inspection and async-owner custody under thread boundaries.

All packages/state are synthetic. No native, SDK, provider, service manager or
application process may run. Blocking reads are owned threads, not sleeps on
the event loop, and no accepted artifact qualification is repeated here.
"""
import asyncio
from copy import deepcopy
import json
import io
import os
import socket
import shutil
import subprocess
import threading
import uuid
import zipfile
from types import MethodType

import pytest

from launchkit import deployment, deployment_service, deployment_worker, deployment_updates as updates
from launchkit.projects import DraftConflict
from test_deployment_updates import staged


@pytest.fixture(autouse=True)
def inert(monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("Responsiveness tests must not execute a process or contact a provider.")

    async def unavailable(*args, **kwargs):
        return {"available": False, "detail": "Synthetic manager is unavailable."}

    for owner, name in ((subprocess, "run"), (subprocess, "Popen"),
            (os, "system"), (socket, "create_connection"),
            (asyncio, "create_subprocess_exec"), (asyncio, "create_subprocess_shell")):
        monkeypatch.setattr(owner, name, forbidden)
    monkeypatch.setattr(deployment_service.UserServiceManager, "capability", unavailable)


@pytest.fixture
async def owned(tmp_path):
    value = await staged(tmp_path)
    try:
        yield value
    finally:
        await value[0].shutdown()


def coordinated_update_substitution(directory, identity, *, archives=False):
    """Two agreeing records are a different selection, despite their same ID."""
    current = deployment.read_receipt(directory)
    update = deepcopy(current["update"])
    root = updates.update_directory(directory, identity)
    if archives:
        for name, key in (("previous.zip", "previous_package_sha256"),
                          ("candidate.zip", "package_sha256")):
            _, files = updates.checked_package(root, name, update[key])
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, "w", compression=zipfile.ZIP_STORED) as archive:
                archive.comment = b"coordinated substitution"
                for path, (content, mode) in files.items():
                    item = zipfile.ZipInfo(path)
                    item.external_attr = (mode | 0o100000) << 16
                    archive.writestr(item, content)
            payload = stream.getvalue()
            prior = update[key]
            update[key] = updates.digest(payload)
            assert prior != update[key]
            updates.replace_file(root / name, payload, 0o600)
            if current["package_sha256"] == prior:
                current["package_sha256"] = update[key]
                updates.replace_file(directory / "package.zip", payload, 0o600)
    else:
        update["source_digest"] = "coordinated replacement under the same ID"
    current["update"] = update
    updates.write_private(root / "update.json", update)
    updates.write_private(directory / "receipt.json", current)
    if archives:
        updates.write_private(updates.Path(current["destination"]) / deployment.OWNER_FILE,
                              deployment.ownership(current))
    # The old transaction accepts both projections and exact archive hashes.
    assert updates.transaction(directory, identity)[2]["update_id"] == identity


@pytest.mark.parametrize("mode,archives", [("update", False), ("rollback", False),
    ("recover-current", False), ("update", True)])
async def test_original_update_admission_refuses_coordinated_substitution_before_worker_entry(
        owned, monkeypatch, mode, archives):
    service, directory, destination, previous, _, identity = owned
    if mode != "update":
        updates.activate(directory, identity)
        _, root, update = updates.transaction(directory, identity)
        updates.checkpoint(directory, root, update,
            status="completed" if mode == "rollback" else "recovery-needed",
            rollback_available=True, runtime_admission_possible=False)
    release = asyncio.Event()
    entries = []

    async def deferred(self, root, actual_mode, **selection):
        await release.wait()
        return await deployment.DeploymentService._run(self, root, actual_mode, **selection)

    async def inert_worker(*args, **kwargs):
        entries.append((args, kwargs))
        return 0

    class Exited:
        pid = 123456
        returncode = 1
        stdin = None

        async def wait(self):
            return self.returncode

    async def fixed_wrapper(*args, **kwargs):
        assert args[1] == "-c" and "interactive_worker" in args[2]
        try:
            await deployment.interactive_worker(*args[3:])
        except ValueError:
            pass
        return Exited()

    service._run = MethodType(deferred, service)
    monkeypatch.setattr(deployment_worker, "run", inert_worker)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fixed_wrapper)
    await service.action(service.guide.identity, previous["deployment_id"],
        "apply-update" if mode == "update" else mode, update_id=identity)
    task = service.tasks[directory.name]
    coordinated_update_substitution(directory, identity, archives=archives)
    before = preserved(directory, destination)
    release.set()
    await task
    assert entries == [], "The original update selection must refuse before worker use."
    assert {path: path.read_bytes() for path in before if path.name != "receipt.json"} == {
        path: value for path, value in before.items() if path.name != "receipt.json"}
    assert not service.processes


def journal_ahead(directory, identity):
    path = updates.update_directory(directory, identity) / "update.json"
    actual = json.loads(path.read_text())
    actual.update(status="snapshotting", detail="Synthetic checkpoint ahead of receipt.")
    updates.write_private(path, actual)
    return path, actual


def preserved(directory, destination):
    return {path: path.read_bytes() for path in (
        directory / "receipt.json", directory / "package.zip", destination / ".env",
        destination / "data/ledger.db", destination / ".mobkit/state/native.db",
        destination / "workspace/user.txt")}


async def test_original_update_binding_refuses_substitution_during_snapshot(owned, monkeypatch):
    _, directory, destination, _, _, identity = owned
    binding = updates.update_binding(updates.transaction(directory, identity)[2])
    before = preserved(directory, destination)
    snapshot, apply = updates.snapshot, updates.apply_files
    copied = []

    def changed_after_snapshot(*args):
        snapshot(*args)
        coordinated_update_substitution(directory, identity)

    def copy(*args):
        copied.append(True)
        return apply(*args)

    monkeypatch.setattr(updates, "snapshot", changed_after_snapshot)
    monkeypatch.setattr(updates, "apply_files", copy)
    try:
        updates.activate(directory, identity, expected_binding=binding)
    except DraftConflict:
        pass
    assert copied == [], "Snapshot completion must not rebaseline a substituted update."
    assert deployment.read_receipt(directory)["update"]["source_digest"] == "coordinated replacement under the same ID"
    assert {path: path.read_bytes() for path in before if path.name != "receipt.json"} == {
        path: value for path, value in before.items() if path.name != "receipt.json"}


async def test_original_background_binding_refuses_substitution_during_stopped_observation(owned, monkeypatch):
    _, directory, _, _, _, identity = owned
    selection = updates.retain_update_admission(directory,
        updates.transaction(directory, identity)[2], "update")
    entries = []

    class Manager:
        async def start(self, service):
            entries.append(service)

    async def stopped(*args):
        coordinated_update_substitution(directory, identity)
        return {"synthetic": "owned stopped observation"}

    async def unavailable(*args):
        raise RuntimeError("Inert startup observation ends here.")

    async def existing_start(supervisor, *args, **kwargs):
        await supervisor.start_service({})

    monkeypatch.setattr(deployment_worker, "update_runtime", existing_start)
    monkeypatch.setattr(deployment_service, "require_stopped_service", stopped)
    monkeypatch.setattr(deployment_service, "require_update_protocol", lambda service: None)
    monkeypatch.setattr(deployment_service, "background_status", unavailable)
    with pytest.raises((DraftConflict, RuntimeError)):
        await deployment_service.update_background(directory, Manager(), "update",
            update_id=identity, update_selection=selection)
    assert entries == [], "The substituted transaction must refuse before OS-service admission."


def restored_source(directory, identity):
    updates.activate(directory, identity)
    updates.restore(directory, identity, restore_data=False)
    _, root, update = updates.transaction(directory, identity)
    return updates.checkpoint(directory, root, update, status="restored-stopped",
        runtime_admission_possible=True, rollback_available=False)


def inert_runtime(monkeypatch, directory, entries):
    class Ready(deployment_worker.Supervisor):
        async def install(self, definition):
            pass

        async def start(self, definition, *, on_ready=None):
            entries.append((deployment.read_receipt(directory)["build_id"], on_ready is not None))
            if on_ready is not None:
                on_ready()

    async def owner(supervisor):
        await supervisor.stop.wait()
        return "stopped"

    monkeypatch.setattr(deployment_worker, "Supervisor", Ready)
    monkeypatch.setattr(deployment_worker, "owner_input", owner)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    return Ready


async def test_original_recovery_start_refuses_a_different_valid_transaction_source(owned, monkeypatch):
    _, directory, _, previous, _, identity = owned
    update = restored_source(directory, identity)
    selection = updates.retain_update_admission(directory, update, "start")
    # The same transaction and original archive pins allow both sources. A
    # restored restart must still retain its selected previous source.
    updates.recover_candidate(directory, identity)
    assert deployment.read_receipt(directory)["build_id"] != previous["build_id"]
    entries = []
    inert_runtime(monkeypatch, directory, entries)
    try:
        await deployment_worker.run(directory, "start", update_selection=selection)
    except DraftConflict:
        pass
    assert entries == [], "A previous-source restart must not execute the valid candidate."
    assert deployment.read_receipt(directory)["update"]["status"] != "rolled-back"


def retarget_valid_installation(directory, destination):
    target = destination.parent / "other-owned-installation"
    shutil.copytree(destination, target)
    receipt = deployment.read_receipt(directory)
    receipt["destination"] = str(target)
    updates.write_private(directory / "receipt.json", receipt)
    updates.write_private(target / deployment.OWNER_FILE, deployment.ownership(receipt))
    _, files = updates.checked_package(directory, "package.zip", receipt["package_sha256"])
    deployment.verify_destination(receipt, files)
    assert updates.transaction(directory, receipt["update"]["update_id"])[0]["destination"] == str(target)
    return target


async def test_original_update_admission_refuses_valid_installation_retarget(owned, monkeypatch):
    service, directory, destination, previous, _, identity = owned
    release, entries = asyncio.Event(), []

    async def deferred(self, root, mode, **selection):
        await release.wait()
        return await deployment.DeploymentService._run(self, root, mode, **selection)

    class Exited:
        pid, returncode, stdin = 123456, 1, None

        async def wait(self):
            return self.returncode

    async def wrapper(*args, **kwargs):
        try:
            await deployment.interactive_worker(*args[3:])
        except ValueError:
            pass
        return Exited()

    async def worker(*args, **kwargs):
        entries.append(True)
        return 0

    service._run = MethodType(deferred, service)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", wrapper)
    monkeypatch.setattr(deployment_worker, "run", worker)
    await service.action(service.guide.identity, previous["deployment_id"], "apply-update", update_id=identity)
    task = service.tasks[directory.name]
    before = preserved(directory, destination)
    target = retarget_valid_installation(directory, destination)
    release.set()
    await task
    assert entries == [], "The original installation must survive admission to child entry."
    assert (target / "application/logic.py").read_bytes() == (destination / "application/logic.py").read_bytes()
    assert {path: path.read_bytes() for path in before if path.name != "receipt.json"} == {
        path: value for path, value in before.items() if path.name != "receipt.json"}


async def test_original_update_installation_refuses_retarget_after_snapshot(owned, monkeypatch):
    _, directory, destination, _, _, identity = owned
    selected = updates.read_update_admission(directory, "update", updates.retain_update_admission(
        directory, updates.transaction(directory, identity)[2], "update"))
    original, copy = updates.snapshot, updates.apply_files
    copied = []

    def retarget(*args):
        original(*args)
        retarget_valid_installation(directory, destination)

    def apply(*args):
        copied.append(True)
        return copy(*args)

    monkeypatch.setattr(updates, "snapshot", retarget)
    monkeypatch.setattr(updates, "apply_files", apply)
    with pytest.raises(DraftConflict, match="installation"):
        updates.activate(directory, identity, expected_binding=selected["binding"],
            expected_installation=selected["installation"])
    assert copied == []


@pytest.mark.parametrize("bound", [False, True])
@pytest.mark.parametrize("mode", ["start", "install"])
async def test_recovery_start_completes_only_with_original_dynamic_admission(owned, monkeypatch, bound, mode):
    _, directory, _, previous, _, identity = owned
    update = restored_source(directory, identity)
    selection = updates.retain_update_admission(directory, update, mode) if bound else None
    entries, installed = [], []
    runtime = inert_runtime(monkeypatch, directory, entries)

    async def install(*args):
        installed.append(True)

    monkeypatch.setattr(runtime, "install", install)
    assert await deployment_worker.run(directory, mode, update_selection=selection) is None
    assert entries == [(previous["build_id"], bound)]
    assert installed == ([True] if mode == "install" else [])
    assert deployment.read_receipt(directory)["update"]["status"] == ("rolled-back" if bound else "restored-stopped")


async def test_static_service_starts_without_completing_unadmitted_recovery(owned, monkeypatch):
    _, directory, _, previous, _, identity = owned
    restored_source(directory, identity)
    entries = []
    inert_runtime(monkeypatch, directory, entries)

    async def observe(*args):
        return {"pid": os.getpid(), "status": "running"}

    monkeypatch.setattr(deployment_service, "verify_service", lambda *args: {})
    monkeypatch.setattr(deployment_service.UserServiceManager, "observe", observe)
    assert await deployment_worker.run(directory, "service") is None
    assert entries == [(previous["build_id"], False)]
    assert deployment.read_receipt(directory)["update"]["status"] == "restored-stopped"


async def test_original_update_binding_allows_legitimate_progress_and_ready_commit(owned):
    _, directory, destination, _, private, identity = owned
    selection = updates.retain_update_admission(directory, updates.transaction(directory, identity)[2], "update")
    selected = updates.read_update_admission(directory, "update", selection)
    calls = []

    class Ready(deployment_worker.Supervisor):
        async def command(self, *args, **kwargs):
            calls.append("application check")

        async def before_source_change(self):
            _, root, current = updates.transaction(directory, identity)
            updates.checkpoint(directory, root, current, status="snapshotting",
                detail="Legitimate progress", native_definition_observation={"synthetic": "diagnostic"})

        async def start(self, definition, *, on_ready):
            calls.append("startup")
            on_ready()

    await deployment_worker.update_runtime(Ready(directory), "update", update_selection=selection)
    current = deployment.read_receipt(directory)["update"]
    assert current["status"] == "completed" and current["runtime_admission_possible"] is True
    assert updates.update_binding(current) == selected["binding"]
    assert calls == ["application check", "application check", "startup"]
    assert (destination / ".env").read_bytes() == private
    assert (destination / "data/ledger.db").read_bytes() == b"user application records"


@pytest.mark.parametrize("seam", ["before-source", "command", "ready"])
async def test_original_update_binding_refuses_late_coordinated_substitution(owned, seam):
    _, directory, destination, _, private, identity = owned
    selection = updates.retain_update_admission(directory, updates.transaction(directory, identity)[2], "update")
    calls = []

    class Changed(deployment_worker.Supervisor):
        async def before_source_change(self):
            if seam == "before-source":
                coordinated_update_substitution(directory, identity)

        async def command(self, *args, **kwargs):
            calls.append("check")
            if seam == "command":
                coordinated_update_substitution(directory, identity)

        async def start(self, definition, *, on_ready):
            calls.append("startup")
            coordinated_update_substitution(directory, identity)
            on_ready()

    with pytest.raises(DraftConflict, match="originally admitted"):
        await deployment_worker.update_runtime(Changed(directory), "update", update_selection=selection)
    assert calls == ([] if seam == "before-source" else ["check"] if seam == "command" else ["check", "check", "startup"])
    current = deployment.read_receipt(directory)["update"]
    assert current["source_digest"] == "coordinated replacement under the same ID"
    assert current["status"] not in {"completed", "rolled-back"}
    assert (destination / ".env").read_bytes() == private
    assert (destination / "data/ledger.db").read_bytes() == b"user application records"


@pytest.mark.parametrize("damage", ["missing", "bytes", "file-mode", "parent-mode", "wrong-mode", "malformed"])
async def test_original_update_admission_refuses_damaged_private_pin_before_worker(owned, monkeypatch, damage):
    _, directory, _, _, _, identity = owned
    selection = updates.retain_update_admission(directory, updates.transaction(directory, identity)[2], "update")
    path = updates.Path(selection["path"])
    entries = []

    def forbidden(*args):
        entries.append(True)
        pytest.fail("Damaged original admission reached supervisor construction.")

    if damage == "missing":
        path.unlink()
    elif damage == "bytes":
        path.write_bytes(b"changed admission")
    elif damage == "file-mode":
        path.chmod(0o644)
    elif damage == "parent-mode":
        path.parent.chmod(0o755)
    elif damage == "wrong-mode":
        value = json.loads(path.read_text())
        value["mode"] = "rollback"
        path.write_text(json.dumps(value))
        selection["sha256"] = updates.digest(path.read_bytes())
    else:
        path.write_bytes(b"{}"); selection["sha256"] = updates.digest(path.read_bytes())
    monkeypatch.setattr(deployment_worker, "Supervisor", forbidden)
    try:
        with pytest.raises(DraftConflict):
            await deployment_worker.run(directory, "update", update_selection=selection)
    finally:
        path.parent.chmod(0o700)
    assert entries == []


@pytest.mark.parametrize("seam", ["manager-start", "health-observation"])
async def test_original_background_binding_refuses_substitution_after_service_await(owned, monkeypatch, seam):
    _, directory, _, _, _, identity = owned
    selection = updates.retain_update_admission(directory, updates.transaction(directory, identity)[2], "update")
    entries = []

    class Manager:
        async def start(self, service):
            entries.append("start")
            if seam == "manager-start":
                coordinated_update_substitution(directory, identity)

    async def stopped(*args):
        return {"synthetic": "original owned stopped service"}

    async def observed(*args):
        entries.append("health")
        coordinated_update_substitution(directory, identity)
        return {"status": "running", "health": "ready"}

    async def existing_start(supervisor, *args, **kwargs):
        await supervisor.start_service({}, on_ready=lambda: entries.append("ready"))

    monkeypatch.setattr(deployment_worker, "update_runtime", existing_start)
    monkeypatch.setattr(deployment_service, "require_stopped_service", stopped)
    monkeypatch.setattr(deployment_service, "require_update_protocol", lambda service: None)
    monkeypatch.setattr(deployment_service, "background_status", observed)
    with pytest.raises(DraftConflict, match="originally admitted"):
        await deployment_service.update_background(directory, Manager(), "update", update_id=identity,
            update_selection=selection)
    assert entries == (["start"] if seam == "manager-start" else ["start", "health"])
    assert deployment.read_receipt(directory).get("update_service_verification") is None


@pytest.mark.parametrize("change", ["binding", "source"])
async def test_original_recovery_ready_callback_withholds_changed_transaction(owned, monkeypatch, change):
    _, directory, _, _, _, identity = owned
    update = restored_source(directory, identity)
    selection = updates.retain_update_admission(directory, update, "start")
    entries = []
    runtime = inert_runtime(monkeypatch, directory, entries)

    async def started(self, definition, *, on_ready):
        entries.append("started")
        if change == "binding":
            coordinated_update_substitution(directory, identity)
        else:
            updates.recover_candidate(directory, identity)
        on_ready()

    monkeypatch.setattr(runtime, "start", started)
    assert await deployment_worker.run(directory, "start", update_selection=selection) == 1
    assert entries == ["started"]
    assert deployment.read_receipt(directory)["update"]["status"] not in {"rolled-back", "completed"}


async def test_inspection_projects_journal_ahead_without_publishing(owned, monkeypatch):
    _, directory, destination, _, _, identity = owned
    path, actual = journal_ahead(directory, identity)
    before = preserved(directory, destination)

    def no_publication(*args, **kwargs):
        pytest.fail("Inspection may not reconcile an owned receipt.")

    monkeypatch.setattr(updates, "update_receipt", no_publication)
    receipt, root, update = updates.inspect_transaction(directory, identity)
    assert root == path.parent and update == receipt["update"] == actual
    receipt["update"]["detail"] = "Detached caller edit"
    assert json.loads(path.read_text()) == actual
    assert {name: name.read_bytes() for name in before} == before


async def test_later_inspection_failure_does_not_publish_earlier_projection(owned, monkeypatch):
    _, directory, destination, _, _, identity = owned
    path, actual = journal_ahead(directory, identity)
    candidate = path.parent / "candidate.zip"
    candidate.write_bytes(candidate.read_bytes() + b"changed synthetic package")
    before = preserved(directory, destination)
    monkeypatch.setattr(updates, "update_receipt", lambda *a, **k:
        pytest.fail("A failing inspection must not publish its journal projection."))
    with pytest.raises(DraftConflict):
        updates.inspect_transaction(directory, identity)
    assert json.loads(path.read_text()) == actual
    assert {name: name.read_bytes() for name in before} == before


@pytest.mark.parametrize("change", ["pointer", "checkpoint"])
async def test_inspection_refuses_owner_pointer_or_checkpoint_advancing_during_read(owned, monkeypatch, change):
    _, directory, destination, _, _, identity = owned
    before = preserved(directory, destination)
    original = updates.checked_package
    changed = {}

    def advancing(root, name, expected):
        result = original(root, name, expected)
        if name == "candidate.zip" and not changed:
            current = deployment.read_receipt(directory)
            if change == "pointer":
                current["update"] = {**current["update"], "update_id": str(uuid.uuid4())}
            else:
                path = updates.update_directory(directory, identity) / "update.json"
                checkpoint = json.loads(path.read_text())
                checkpoint.update(status="snapshotting", detail="Newer synthetic owner checkpoint.")
                updates.write_private(path, checkpoint)
                current["update"] = checkpoint
            updates.write_private(directory / "receipt.json", current)
            changed["receipt"] = (directory / "receipt.json").read_bytes()
        return result

    monkeypatch.setattr(updates, "checked_package", advancing)
    monkeypatch.setattr(updates, "update_receipt", lambda *a, **k:
        pytest.fail("Inspection may not overwrite a newer owner checkpoint."))
    with pytest.raises(DraftConflict):
        updates.inspect_transaction(directory, identity)
    assert (directory / "receipt.json").read_bytes() == changed["receipt"]
    assert {path: path.read_bytes() for path in before if path.name != "receipt.json"} == {
        path: value for path, value in before.items() if path.name != "receipt.json"}


async def test_validation_read_leaves_event_loop_responsive():
    loop = asyncio.get_running_loop()
    main_thread = threading.get_ident()
    entered = loop.create_future()
    release = threading.Event()
    threads = []

    def read(value, *, marker):
        threads.append(threading.get_ident())
        loop.call_soon_threadsafe(entered.set_result, True)
        if not release.wait(2):
            raise AssertionError("The test did not release its owned validation read.")
        return value, marker

    task = asyncio.create_task(deployment.await_validation(read, "checked", marker="exact"))
    try:
        await asyncio.wait_for(entered, 30)
        ticks = []
        loop.call_soon(ticks.append, "alive")
        await asyncio.sleep(0)
        assert ticks == ["alive"] and not task.done()
        assert threads == [threads[0]] and threads[0] != main_thread
    finally:
        release.set()
    assert await task == ("checked", "exact")


@pytest.mark.parametrize("kind", ["transaction", "capabilities"])
async def test_real_status_validation_reads_leave_event_loop_responsive(owned, monkeypatch, kind):
    service, _, _, _, _, _ = owned
    owner, name = ((updates, "inspect_transaction") if kind == "transaction"
        else (deployment_service, "background_update_capabilities"))
    original = getattr(owner, name)
    loop = asyncio.get_running_loop()
    entered = loop.create_future()
    release = threading.Event()
    threads = []
    main_thread = threading.get_ident()

    def blocked(*args, **kwargs):
        threads.append(threading.get_ident())
        loop.call_soon_threadsafe(entered.set_result, True)
        if not release.wait(2):
            raise AssertionError("The test did not release its owned status read.")
        return original(*args, **kwargs)

    monkeypatch.setattr(owner, name, blocked)
    task = asyncio.create_task(service.status(service.guide.identity))
    try:
        await asyncio.wait_for(entered, 30)
        ticks = []
        loop.call_soon(ticks.append, "alive")
        await asyncio.sleep(0)
        assert ticks == ["alive"] and not task.done()
        assert len(threads) == 1 and threads[0] != main_thread
    finally:
        release.set()
    receipt = await task
    assert receipt["update"]["status"] == "staged"


async def test_repeated_action_cancellation_drains_same_read_before_unlock_without_launch(owned, monkeypatch):
    service, directory, destination, previous, _, identity = owned
    before = preserved(directory, destination)
    loop = asyncio.get_running_loop()
    entered = loop.create_future()
    release = threading.Event()
    completed = threading.Event()
    calls, launched = [], []
    original = updates.inspect_transaction

    def blocked(*args, **kwargs):
        calls.append(threading.get_ident())
        loop.call_soon_threadsafe(entered.set_result, True)
        if not release.wait(2):
            raise AssertionError("The test did not release its owned inspection.")
        try:
            return original(*args, **kwargs)
        finally:
            completed.set()

    monkeypatch.setattr(updates, "inspect_transaction", blocked)
    monkeypatch.setattr(service, "_launch", lambda *a, **k: launched.append((a, k)))
    task = asyncio.create_task(service.action(service.guide.identity,
        previous["deployment_id"], "apply-update", update_id=identity))
    try:
        await asyncio.wait_for(entered, 30)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert service.lock.locked() and not task.done() and not completed.is_set()
        assert len(calls) == 1 and not launched
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert completed.is_set() and not service.lock.locked() and not launched
    assert {path: path.read_bytes() for path in before} == before


async def test_normal_owned_action_still_admits_one_inert_worker(owned, monkeypatch):
    service, directory, destination, previous, _, identity = owned
    before = preserved(directory, destination)
    launched = []
    original = service._launch

    def admitted(root, mode, **selection):
        launched.append((root, mode))
        original(root, mode, **selection)

    monkeypatch.setattr(service, "_launch", admitted)
    receipt = await service.action(service.guide.identity, previous["deployment_id"],
        "apply-update", update_id=identity)
    assert launched == [(directory, "update")]
    assert receipt["status"] == "queued" and directory.name in service.tasks
    assert receipt["update_action"] == {"action": "apply-update", "update_id": identity}
    assert {path: path.read_bytes() for path in before if path.name != "receipt.json"} == {
        path: value for path, value in before.items() if path.name != "receipt.json"}


async def test_cancellation_after_launch_retains_the_already_admitted_inert_task(owned, monkeypatch):
    service, directory, destination, previous, _, identity = owned
    before = preserved(directory, destination)
    launched = []
    original = service._launch

    def admitted(root, mode, **selection):
        launched.append((root, mode))
        original(root, mode, **selection)
        asyncio.current_task().cancel()

    monkeypatch.setattr(service, "_launch", admitted)
    caller = asyncio.create_task(service.action(service.guide.identity,
        previous["deployment_id"], "apply-update", update_id=identity))
    with pytest.raises(asyncio.CancelledError):
        await caller
    assert launched == [(directory, "update")] and not service.lock.locked()
    worker = service.tasks[directory.name]
    assert not worker.done() and not worker.cancelled()
    receipt = deployment.read_receipt(directory)
    assert receipt["status"] == "queued"
    assert receipt["update_action"] == {"action": "apply-update", "update_id": identity}
    assert {path: path.read_bytes() for path in before if path.name != "receipt.json"} == {
        path: value for path, value in before.items() if path.name != "receipt.json"}


async def test_stale_capability_input_cannot_skip_a_new_selected_update(owned, monkeypatch):
    _, directory, _, _, _, _ = owned
    current = deployment.update_receipt(directory, execution_mode="background",
        service={"manager": "synthetic", "label": "owned-test"})
    stale = deepcopy(current)
    stale.pop("update")
    before = (directory / "receipt.json").read_bytes()

    def no_later_check(*args, **kwargs):
        pytest.fail("Stale supplied authority must refuse before package or service inspection.")

    monkeypatch.setattr(updates, "checked_package", no_later_check)
    monkeypatch.setattr(updates, "inspect_transaction", no_later_check)
    monkeypatch.setattr(deployment_service, "verify_service", no_later_check)
    result = deployment_service.background_update_capabilities(directory, stale)
    assert result["background_activation_supported"] is False
    assert "reason" in result and (directory / "receipt.json").read_bytes() == before


async def test_status_retains_new_background_checkpoint_before_capability_projection(owned, monkeypatch):
    service, directory, _, _, _, identity = owned
    deployment.update_receipt(directory, execution_mode="background",
        service={"manager": "synthetic", "label": "owned-test"},
        service_supervisor_pid=101, service_instance_id="old-instance")
    seen = []

    async def reread(root, manager):
        current = deployment.read_receipt(root)
        path = updates.update_directory(root, identity) / "update.json"
        latest = json.loads(path.read_text())
        latest.update(status="snapshotting", detail="Checkpoint from current background owner.")
        updates.write_private(path, latest)
        current.update(update=latest, service_instance_id="fresh-instance", service_supervisor_pid=102)
        updates.write_private(root / "receipt.json", current)
        return deepcopy(current)

    def capabilities(root, receipt):
        seen.append(deepcopy(receipt))
        return {"background_activation_supported": True}

    monkeypatch.setattr(deployment_service, "background_status", reread)
    monkeypatch.setattr(deployment_service, "background_update_capabilities", capabilities)
    receipt = await service.status(service.guide.identity)
    assert len(seen) == 1
    assert seen[0]["service_instance_id"] == receipt["service_instance_id"] == "fresh-instance"
    assert seen[0]["service_supervisor_pid"] == receipt["service_supervisor_pid"] == 102
    assert seen[0]["update"]["detail"] == receipt["update"]["detail"] == "Checkpoint from current background owner."
    assert receipt["update_capabilities"]["background_activation_supported"] is True


@pytest.mark.parametrize("change", ["guide", "build", "pointer", "package",
    "null-runtime-carrier", "incomplete-refresh"])
async def test_action_refuses_drift_after_awaited_final_inspection_before_launch(owned, monkeypatch, change):
    service, directory, destination, previous, _, identity = owned
    before = preserved(directory, destination)
    original = updates.inspect_transaction
    calls, launched, mutation = [], [], {}

    def raced(*args, **kwargs):
        result = original(*args, **kwargs)
        calls.append(result[2]["update_id"])
        if len(calls) == 3:
            if change == "guide":
                service.guide.state["version"] += 1
                service.guide.state["brief"] = {"outcome": "A contrary synthetic task after validation."}
            elif change == "build":
                service.implementation.identity = str(uuid.uuid4())
            elif change in {"pointer", "null-runtime-carrier", "incomplete-refresh"}:
                receipt = deployment.read_receipt(directory)
                if change == "pointer":
                    receipt["update"] = {**receipt["update"], "update_id": str(uuid.uuid4())}
                elif change == "null-runtime-carrier":
                    receipt["runtime_candidate"] = None
                else:
                    receipt["supervisor_refresh"] = {"revision": str(uuid.uuid4()),
                        "descriptor_sha256": "a" * 64, "status": "prepared"}
                updates.write_private(directory / "receipt.json", receipt)
                mutation["receipt"] = (directory / "receipt.json").read_bytes()
            else:
                path = directory / "package.zip"
                path.write_bytes(path.read_bytes() + b"changed after the checked read")
                mutation["package"] = path.read_bytes()
        return result

    monkeypatch.setattr(updates, "inspect_transaction", raced)
    monkeypatch.setattr(service, "_launch", lambda *a, **k: launched.append((a, k)))
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, previous["deployment_id"],
            "apply-update", update_id=identity)
    assert len(calls) == 3 and not launched
    if change in {"pointer", "null-runtime-carrier", "incomplete-refresh"}:
        assert (directory / "receipt.json").read_bytes() == mutation["receipt"]
    else:
        assert (directory / "receipt.json").read_bytes() == before[directory / "receipt.json"]
    if change == "package":
        assert (directory / "package.zip").read_bytes() == mutation["package"]
    else:
        assert (directory / "package.zip").read_bytes() == before[directory / "package.zip"]
    assert {path: path.read_bytes() for path in before if path.parent != directory} == {
        path: value for path, value in before.items() if path.parent != directory}
