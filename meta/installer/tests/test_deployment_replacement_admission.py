"""Private replacement admission through real byte custody and inert launch.

Synthetic compiler/native inputs establish only original file and source joins.
No process, socket, provider, installed runtime or native SDK is invoked.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
from types import SimpleNamespace
import uuid

import pytest

from launchkit import deployment
from launchkit import deployment_updates as updates
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from test_deployment import Guide, Implementation, Service, submit
from test_deployment_application_startup import CONFIG, POLICY, Inputs, images


@pytest.fixture(autouse=True)
def no_runtime_operations(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Private admission controls cannot run a process or contact a provider.")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"), (os, "system"),
            (os, "kill"), (os, "killpg"), (socket, "create_connection"),
            (asyncio, "create_subprocess_exec"), (asyncio, "create_subprocess_shell")):
        monkeypatch.setattr(owner, name, forbidden)


@pytest.fixture
async def owned_inputs(tmp_path, monkeypatch):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = hashlib.sha256(b"synthetic accepted previous source").hexdigest()
    implementation.definition["agents"][0]["profile"] = "helper-source"
    implementation.extra_files = {"config/mob.toml": CONFIG,
        "application/tool-policy.json": POLICY, "application/logic.py": "VERSION = 'old'\n"}
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    deployment.materialize(directory)
    await service.shutdown()
    deployment.update_receipt(directory, status="stopped")
    (destination / "data").mkdir(mode=0o700)
    (destination / "data/ledger.db").write_bytes(b"retained user records")
    (destination / "workspace").mkdir(mode=0o700)
    (destination / ".mobkit/state").mkdir(mode=0o700)
    implementation.identity = str(uuid.uuid4())
    implementation.digest = hashlib.sha256(b"synthetic accepted candidate source").hexdigest()
    implementation.extra_files["application/logic.py"] = "VERSION = 'new'\n"
    receipt = await service.stage_update(guide.identity, directory.name,
        guide.state["version"], implementation.identity)
    assert receipt["update"]["status"] == "staged" and receipt["update"]["blockers"] == []
    f = Inputs(directory, destination, receipt["update"]["update_id"])
    f.service, f.guide, f.implementation, f.launches = service, guide, implementation, []

    def captured_launch(path, mode, *, update_selection=None):
        # Retain the actual queue boundary, without scheduling any worker.
        f.launches.append((path, mode, deepcopy(update_selection)))

    monkeypatch.setattr(service, "_launch", captured_launch)
    try:
        yield f
    finally:
        await service.shutdown()


async def apply(f, value=None):
    return await f.service.apply_replacement(f.guide.identity, f.directory.name, f.identity,
        application_startup=f.value if value is None else value)


def retained_admissions(f):
    return list(updates.update_directory(f.directory, f.identity).glob("admission-*.json"))


def assert_not_queued(f, before):
    assert f.launches == [] and images(f) == before
    assert deployment.read_receipt(f.directory)["status"] != "queued"
    assert "update_action" not in deployment.read_receipt(f.directory)


def replace_identical(path):
    replacement = path.with_suffix(".replacement")
    replacement.write_bytes(path.read_bytes())
    replacement.chmod(0o600)
    os.replace(replacement, path)


async def test_private_method_queues_original_selected_pins_without_native_execution(owned_inputs):
    f = owned_inputs
    original = deepcopy(f.value)
    before = images(f)
    await apply(f)
    assert len(f.launches) == 1
    directory, mode, admission = f.launches[0]
    assert directory == f.directory and mode == "update"
    selected = updates.read_update_admission(f.directory, mode, admission)
    assert selected["application_startup"] == original
    assert selected["binding"] == updates.update_binding(updates.transaction(f.directory, f.identity)[2])
    assert selected["application_startup"]["prior_tool_policy"] == f.prior.pin()
    assert selected["application_startup"]["prior_tool_policy"] != f.local_policy.pin()
    assert Path(admission["path"]).stat().st_mode & 0o777 == 0o600
    assert updates.digest(Path(admission["path"]).read_bytes()) == admission["sha256"]
    assert images(f) == before
    assert deployment.read_receipt(f.directory)["update_action"] == {
        "action": "apply-update", "update_id": f.identity}


async def test_private_admission_live_package_hashes_run_off_original_event_loop(owned_inputs, monkeypatch):
    f = owned_inputs
    owner_thread = threading.get_ident()
    original = updates._require_live_package
    calls = []

    def off_loop(*args, **kwargs):
        calls.append(threading.get_ident())
        assert calls[-1] != owner_thread, "The private admission hashed its package on the event loop."
        return original(*args, **kwargs)

    monkeypatch.setattr(updates, "_require_live_package", off_loop)
    await apply(f)
    assert calls and all(thread != owner_thread for thread in calls)
    assert len(f.launches) == 1


async def test_private_legacy_brief_provenance_is_recovered_off_original_event_loop(owned_inputs, monkeypatch):
    f = owned_inputs
    receipt = deployment.read_receipt(f.directory)
    receipt.pop("brief_hash", None)
    updates.write_private(f.directory / "receipt.json", receipt)
    original = updates.checked_package
    owner_thread = threading.get_ident()
    calls = []

    def off_loop(*args, **kwargs):
        calls.append((threading.get_ident(), args[1]))
        assert calls[-1][0] != owner_thread, "Private legacy provenance read ran on the event loop."
        return original(*args, **kwargs)

    monkeypatch.setattr(updates, "checked_package", off_loop)
    result = await apply(f)
    assert len(f.launches) == 1 and calls
    assert any(name == "package.zip" for _, name in calls)
    assert result["brief_hash"] == deployment.fingerprint(f.guide.state.get("brief") or {})
    assert "brief_hash" not in deployment.read_receipt(f.directory)


async def test_public_action_retains_ordinary_admission_without_selected_inputs(owned_inputs):
    f = owned_inputs
    before = images(f)
    await f.service.action(f.guide.identity, f.directory.name, "apply-update", f.identity)
    assert len(f.launches) == 1
    directory, mode, admission = f.launches[0]
    selected = updates.read_update_admission(directory, mode, admission)
    assert "application_startup" not in selected and images(f) == before


def public_replacement_binding(f, request_id):
    return {"action_request_id": request_id, "guide_id": f.guide.identity,
        "deployment_id": f.directory.name, "action": "apply-update", "update_id": f.identity}


async def public_replacement(f, request_id):
    return await f.service.action(f.guide.identity, f.directory.name, "apply-update", f.identity,
        action_request_id=request_id)


async def test_normal_action_selects_off_loop_after_claim_and_lost_reply_cannot_reselect(owned_inputs):
    f = owned_inputs
    request_id = str(uuid.uuid4())
    owner_thread, calls, before = threading.get_ident(), [], images(f)
    def selection(*ids):
        assert ids == (f.guide.identity, f.directory.name, f.identity)
        assert threading.get_ident() != owner_thread and f.service.lock.locked()
        request = f.service._read_action_request(f.service._action_request_path(request_id))
        assert request["phase"] == "pending"
        calls.append(ids)
        return deepcopy(f.value)
    f.service.application_replacement_selection = selection
    result = await public_replacement(f, request_id)
    assert result["action_request"]["phase"] == "settled"
    assert result["action_request"]["outcome"] == "returned"
    assert len(f.launches) == 1 and images(f) == before
    selected = updates.read_update_admission(f.directory, "update", f.launches[0][2])
    assert selected["application_startup"] == f.value
    repeated = await public_replacement(f, request_id)
    assert repeated["action_request"] == result["action_request"]
    assert len(calls) == len(f.launches) == 1


async def test_configured_replacement_requires_uuid_before_selection_but_private_entry_stays_explicit(owned_inputs):
    f = owned_inputs
    before = images(f)
    f.service.application_replacement_selection = lambda *args: pytest.fail("An unclaimed or private action cannot select inputs.")
    with pytest.raises(ValueError, match="exact action request UUID"):
        await f.service.action(f.guide.identity, f.directory.name, "apply-update", f.identity)
    assert_not_queued(f, before)
    assert retained_admissions(f) == []
    await apply(f)
    assert len(f.launches) == 1
    selected = updates.read_update_admission(f.directory, "update", f.launches[0][2])
    assert selected["application_startup"] == f.value


async def test_unrelated_host_selection_none_keeps_normal_admission(owned_inputs):
    f = owned_inputs
    calls, before = [], images(f)
    def unrelated(*ids):
        calls.append(ids)
        return None
    f.service.application_replacement_selection = unrelated
    result = await public_replacement(f, str(uuid.uuid4()))
    assert result["action_request"]["outcome"] == "returned"
    assert calls == [(f.guide.identity, f.directory.name, f.identity)]
    assert len(f.launches) == 1 and images(f) == before
    selected = updates.read_update_admission(f.directory, "update", f.launches[0][2])
    assert "application_startup" not in selected


@pytest.mark.parametrize("phase", ["pending", "unknown"])
async def test_unresolved_normal_replacement_uuid_returns_before_selection(owned_inputs, phase):
    f = owned_inputs
    request_id = str(uuid.uuid4())
    value, claimed = f.service._claim_action_request(public_replacement_binding(f, request_id))
    assert claimed
    if phase == "unknown":
        value["phase"] = phase
        f.service._action_request_path(request_id).write_text(json.dumps(value))
    f.service.application_replacement_selection = lambda *args: pytest.fail("Unresolved admission cannot reselect.")
    before = images(f)
    result = await public_replacement(f, request_id)
    assert result["action_request"]["phase"] == phase
    assert_not_queued(f, before)
    assert retained_admissions(f) == []


async def test_normal_replacement_uuid_collision_refuses_before_selection(owned_inputs):
    f = owned_inputs
    request_id = str(uuid.uuid4())
    original = public_replacement_binding(f, request_id)
    original["update_id"] = str(uuid.uuid4())
    f.service._claim_action_request(original)
    f.service.application_replacement_selection = lambda *args: pytest.fail("A foreign request cannot reselect.")
    before = images(f)
    with pytest.raises(DraftConflict, match="different deployment action"):
        await public_replacement(f, request_id)
    assert_not_queued(f, before)
    assert retained_admissions(f) == []


async def test_normal_replacement_selection_refusal_settles_without_replay(owned_inputs):
    f = owned_inputs
    request_id, calls, before = str(uuid.uuid4()), [], images(f)
    def unavailable(*ids):
        calls.append(ids)
        raise CustodyError("The original replacement envelope is unavailable.")
    f.service.application_replacement_selection = unavailable
    with pytest.raises(CustodyError, match="unavailable"):
        await public_replacement(f, request_id)
    repeated = await public_replacement(f, request_id)
    assert repeated["action_request"]["phase"] == "settled"
    assert repeated["action_request"]["outcome"] == "error"
    assert len(calls) == 1
    assert_not_queued(f, before)
    assert retained_admissions(f) == []


@pytest.mark.parametrize("change", ["private-input", "accepted-source", "selection-owner"])
async def test_normal_replacement_selection_cannot_bypass_existing_source_and_input_fences(owned_inputs, change):
    f = owned_inputs
    request_id, before = str(uuid.uuid4()), images(f)
    def stale(*ids):
        if change == "private-input":
            f.prior.path.write_bytes(f.prior.read() + b" ")
        elif change == "accepted-source":
            f.implementation.digest = hashlib.sha256(b"superseded candidate source").hexdigest()
        else:
            f.service.application_replacement_selection = lambda *args: None
        return deepcopy(f.value)
    f.service.application_replacement_selection = stale
    with pytest.raises((CustodyError, DraftConflict)):
        await public_replacement(f, request_id)
    assert_not_queued(f, before)
    assert retained_admissions(f) == []
    request = f.service._read_action_request(f.service._action_request_path(request_id))
    assert request["phase"] == "settled" and request["outcome"] == "error"


async def test_normal_replacement_cancelled_selection_drains_original_thread_and_leaves_pending(owned_inputs):
    f = owned_inputs
    request_id = str(uuid.uuid4())
    before, calls, exited = images(f), [], []
    release, entered = threading.Event(), asyncio.Event()
    loop = asyncio.get_running_loop()
    def held(*ids):
        calls.append(ids)
        loop.call_soon_threadsafe(entered.set)
        assert release.wait(5), "The test-owned selection thread was not released."
        exited.append(True)
        return deepcopy(f.value)
    f.service.application_replacement_selection = held
    task = asyncio.create_task(public_replacement(f, request_id))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and f.service.lock.locked()
        assert_not_queued(f, before)
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, 5)
    assert exited == [True] and not f.service.lock.locked()
    repeated = await public_replacement(f, request_id)
    assert repeated["action_request"]["phase"] == "pending"
    assert len(calls) == 1
    assert_not_queued(f, before)
    assert retained_admissions(f) == []


async def test_mutable_caller_is_frozen_before_waiting_for_original_service_lock(owned_inputs):
    f = owned_inputs
    original = deepcopy(f.value)
    async with f.service.lock:
        task = asyncio.create_task(apply(f))
        await asyncio.sleep(0)
        assert not task.done() and f.launches == []
        f.value["expected_revision"] = 999
        f.value["gateway"]["sha256"] = "0" * 64
    await task
    assert len(f.launches) == 1
    selected = updates.read_update_admission(f.directory, "update", f.launches[0][2])
    assert selected["application_startup"] == original


@pytest.mark.parametrize("change", ["prior", "admission"])
async def test_original_thread_observation_refuses_swapped_input_or_envelope_before_queue(owned_inputs, monkeypatch, change):
    f = owned_inputs
    before = images(f)
    original = updates.retain_replacement_update_admission

    def swapped(*args, **kwargs):
        result = original(*args, **kwargs)
        path = f.prior.path if change == "prior" else Path(result[0]["path"])
        if change == "prior":
            path.write_bytes(path.read_bytes() + b" ")
        else:
            # Identical bytes do not authorize replacement of the retained file.
            replace_identical(path)
        return result

    monkeypatch.setattr(updates, "retain_replacement_update_admission", swapped)
    with pytest.raises((CustodyError, DraftConflict), match="changed"):
        await apply(f)
    assert_not_queued(f, before)
    assert len(retained_admissions(f)) == 1


@pytest.mark.parametrize("archive", ["package.zip", "candidate.zip"])
async def test_parent_archive_identity_survives_original_thread_return(owned_inputs, monkeypatch, archive):
    f = owned_inputs
    before = images(f)
    original = updates.retain_replacement_update_admission
    root = updates.update_directory(f.directory, f.identity)
    path = (f.directory if archive == "package.zip" else root) / archive

    def swapped(*args, **kwargs):
        result = original(*args, **kwargs)
        replace_identical(path)
        return result

    monkeypatch.setattr(updates, "retain_replacement_update_admission", swapped)
    with pytest.raises((CustodyError, DraftConflict), match="changed"):
        await apply(f)
    assert_not_queued(f, before)
    assert len(retained_admissions(f)) == 1


@pytest.mark.parametrize("file", ["receipt.json", "update.json"])
@pytest.mark.parametrize("boundary", ["original-read", "awaited-build"])
async def test_original_parent_publication_identity_remains_until_final_queue(owned_inputs, monkeypatch, file, boundary):
    f = owned_inputs
    before = images(f)
    root = updates.update_directory(f.directory, f.identity)
    path = (f.directory if file == "receipt.json" else root) / file
    local = threading.local()
    retained, swaps = [], []
    original = updates.retain_replacement_update_admission
    inspect = updates.inspect_transaction
    status = f.implementation.status

    def swapped_read(*args, **kwargs):
        result = inspect(*args, **kwargs)
        if getattr(local, "parent_read", False) and not swaps and boundary == "original-read":
            # The helper must keep metadata captured before this full read.
            replace_identical(path)
            swaps.append(True)
        return result

    def observing(*args, **kwargs):
        local.parent_read = True
        try:
            result = original(*args, **kwargs)
            retained.append(deepcopy(result[0]))
            return result
        finally:
            local.parent_read = False

    async def awaited_build(identity):
        value = await status(identity)
        if retained and not swaps and boundary == "awaited-build":
            # Semantically equal bytes cannot replace the parent's observation.
            replace_identical(path)
            swaps.append(True)
        return value

    monkeypatch.setattr(updates, "inspect_transaction", swapped_read)
    monkeypatch.setattr(updates, "retain_replacement_update_admission", observing)
    monkeypatch.setattr(f.implementation, "status", awaited_build)
    with pytest.raises((CustodyError, DraftConflict), match="changed"):
        await apply(f)
    assert swaps == [True]
    assert_not_queued(f, before)
    assert len(retained_admissions(f)) == (0 if boundary == "original-read" else 1)


@pytest.mark.parametrize("change", ["build", "guide", "installation"])
async def test_authority_changed_after_thread_return_refuses_before_queue(owned_inputs, monkeypatch, change):
    f = owned_inputs
    before = images(f)
    original = updates.retain_replacement_update_admission
    returned = [False]

    def observed(*args, **kwargs):
        result = original(*args, **kwargs)
        returned[0] = True
        return result

    status = f.implementation.status

    async def superseded(identity):
        if returned[0]:
            if change == "build":
                f.implementation.identity = str(uuid.uuid4())
            elif change == "guide":
                f.guide.state["brief"] = {"outcome": "A different required application"}
            else:
                deployment.update_receipt(f.directory, build_id=str(uuid.uuid4()))
        return await status(identity)

    monkeypatch.setattr(updates, "retain_replacement_update_admission", observed)
    monkeypatch.setattr(f.implementation, "status", superseded)
    with pytest.raises((CustodyError, DraftConflict), match="changed"):
        await apply(f)
    assert returned == [True]
    assert_not_queued(f, before)
    assert len(retained_admissions(f)) == 1


@pytest.mark.parametrize("status", ["failed", "interrupted"])
async def test_private_replacement_requires_explicit_stopped_state(owned_inputs, status):
    f = owned_inputs
    before = images(f)
    deployment.update_receipt(f.directory, status=status)
    with pytest.raises(DraftConflict, match="stopped"):
        await apply(f)
    assert_not_queued(f, before)
    assert deployment.read_receipt(f.directory)["status"] == status
    assert retained_admissions(f) == []


async def test_private_replacement_refuses_unverifiable_pending_support_custody(owned_inputs):
    f = owned_inputs
    before = images(f)
    deployment.update_receipt(f.directory, supervisor_refresh={
        "revision": str(uuid.uuid4()), "descriptor_sha256": "0" * 64})
    original = (f.directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict, match="support|custody"):
        await apply(f)
    assert_not_queued(f, before)
    assert retained_admissions(f) == []
    assert (f.directory / "receipt.json").read_bytes() == original


@pytest.mark.parametrize("owner", ["background", "active-task", "active-process"])
async def test_private_replacement_refuses_unqualified_or_active_runtime_owner(owned_inputs, owner):
    f = owned_inputs
    before = images(f)
    task = None
    if owner == "background":
        deployment.update_receipt(f.directory, execution_mode="background")
    elif owner == "active-task":
        task = asyncio.create_task(asyncio.Event().wait())
        f.service.tasks[f.directory.name] = task
    else:
        f.service.processes[f.directory.name] = SimpleNamespace(returncode=None)
    try:
        with pytest.raises(DraftConflict, match="interactive update owner|owned runtime|runtime owner"):
            await apply(f)
        assert_not_queued(f, before)
        assert retained_admissions(f) == []
    finally:
        f.service.processes.clear()
        if task is not None:
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)
            f.service.tasks.clear()


async def test_repeated_cancellation_joins_held_admission_and_never_queues(owned_inputs, monkeypatch):
    f = owned_inputs
    before = images(f)
    original = updates.retain_replacement_update_admission
    release, entered, exited, retained = threading.Event(), asyncio.Event(), [], []
    loop = asyncio.get_running_loop()

    def held(*args, **kwargs):
        result = original(*args, **kwargs)
        retained.append(deepcopy(result[0]))
        loop.call_soon_threadsafe(entered.set)
        assert release.wait(5), "The test-owned admission thread was not released."
        exited.append(True)
        return result

    monkeypatch.setattr(updates, "retain_replacement_update_admission", held)
    task = asyncio.create_task(apply(f))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and f.service.lock.locked()
        assert_not_queued(f, before)
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, 5)
    assert exited == [True] and not f.service.lock.locked()
    assert_not_queued(f, before)
    assert len(retained) == 1
    path = Path(retained[0]["path"])
    assert path.stat().st_mode & 0o777 == 0o600
    assert updates.digest(path.read_bytes()) == retained[0]["sha256"]
    selected = updates.read_update_admission(f.directory, "update", retained[0])
    assert selected["application_startup"] == f.value
