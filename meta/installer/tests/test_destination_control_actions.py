"""Actual None,None destination control with inert external boundaries.

The real receiver, byte Install, private defaults, preparation/finalizer readers,
canonical UUID journal, bootstrap admission and launch admission remain intact.
Only native command output, venv/pip/check effects and the launched worker body
are inert. These controls do not qualify native startup, an account or a VM.
"""
import asyncio
from copy import deepcopy
import json
from pathlib import Path
from uuid import uuid4

import pytest

from launchkit import deployment
from launchkit import deployment_application_bootstrap as bootstrap
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_destination_application_configuration import guest, retained
from test_destination_application_inputs import inputs_destination, snapshot
from test_destination_application_factory import destination
from test_destination_completed_configuration import completed_guest
from test_product_destination_install import installation
from test_product_destination_owner import destination_source, gcp_destination_source, no_destination_execution
from test_product_artifact_transfer import custody as artifact_custody
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_deployment_service import Manager


def ids(f):
    return f.factory.guide_id, f.factory.deployment_id


async def action(f, name, request=None):
    return await f.factory.action(f.service, *ids(f), name,
        action_request_id=request or str(uuid4()))


async def prepared_guest(f, monkeypatch):
    owner = await guest(f)
    original = owner.host._inventory
    f.actual = {"schema": 1, "mob_id": original["mob_id"],
        "helper_identity": original["helper_identity"], "realm_id": "inert-destination-realm",
        "members": [{"identity": row["identity"], "member": {"mob_id": original["mob_id"],
            "role": row["profile"], "member": "inert-member-" + str(i)}}
            for i, row in enumerate(original["roster"])]}
    monkeypatch.setattr(f.held.supervisor, "command", f.inventory_command)
    request = str(uuid4())
    result = await action(f, "prepare-setup", request)
    assert result["action_request"]["phase"] == "settled"
    assert result["action_request"]["outcome"] == "returned"
    assert len(f.inventory_calls) == 1
    assert f.factory._source is None and f.held.supervisor._product_source_custody._closed
    return owner, request


def inert_worker(f, monkeypatch):
    """Keep actual _launch/admission/task ownership; no process enters."""
    calls = []
    gate = asyncio.Event()
    original = f.service._launch

    async def body(directory, mode, **kwargs):
        assert directory == f.directory and mode == "start"
        try:
            await gate.wait()
            pytest.fail("The inert worker may only be stopped before process admission.")
        finally:
            if f.service.tasks.get(f.factory.deployment_id) is asyncio.current_task():
                f.service.tasks.pop(f.factory.deployment_id)

    def launch(directory, mode, **kwargs):
        assert f.factory._source is None
        assert f.factory.host._product_source_custody._closed
        with project_lock(f.root) as lease:
            lease.require_current()
        original(directory, mode, **kwargs)
        calls.append((deepcopy(kwargs), f.service.tasks[f.factory.deployment_id]))

    monkeypatch.setattr(f.service, "_run", body)
    monkeypatch.setattr(f.service, "_launch", launch)
    return calls


@pytest.mark.parametrize("installation", [False, "gcp"], indirect=True, ids=["local", "gcp"])
async def test_guest_status_reads_original_installed_source_without_origin_design_or_setup_effects(
        inputs_destination, monkeypatch, installation):
    f = inputs_destination
    assert f is installation
    await guest(f)
    before = snapshot(f.root), snapshot(f.directory)
    async with asyncio.timeout(20):
        result = await f.factory.status(f.service, *ids(f))
    assert result["status"] == "stopped" and result["phase"] == "configure"
    assert result["current"] is False and result["current_guide_version"] is None
    assert result["update_capabilities"]["background_activation_supported"] is False
    assert result["deployment_id"] == f.selected.context["deployment_id"]
    assert result["package_sha256"] == f.selected.context["package_sha256"]
    assert before == (snapshot(f.root), snapshot(f.directory))
    assert f.service.guide is None and f.service.implementation is None
    assert f.factory._source is None and f.factory.host._product_source_custody._closed
    assert not f.inventory_calls and not f.service.tasks and not f.service.processes
    assert not f.held.inventory_attempt_path.exists() and not f.held.startup_selection_path.exists()


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_active_gcp_readback_preserves_original_worker_health_without_parent_lease_admission(
        inputs_destination, monkeypatch, installation):
    f = inputs_destination
    assert f is installation
    await guest(f)
    marker = asyncio.Event()
    original_task = asyncio.create_task(marker.wait())
    f.service.tasks[f.factory.deployment_id] = original_task
    deployment.update_receipt(f.directory, status="running", phase="start", health="ready",
        process_live=True, runtime_pid=61234, console_url="http://127.0.0.1:19876/console")
    before = snapshot(f.root), (f.directory / "receipt.json").read_bytes()

    def no_operation(*args, **kwargs):
        pytest.fail("An active status cannot open parent source custody against the child's lease.")

    from launchkit import deployment_service
    monkeypatch.setattr(f.factory, "_operation", no_operation)
    monkeypatch.setattr(deployment_service, "background_update_capabilities", no_operation)
    try:
        with project_lock(f.root) as original_child_lease:
            async with asyncio.timeout(20):
                result = await f.factory.status(f.service, *ids(f))
            original_child_lease.require_current()
        assert f.service.tasks[f.factory.deployment_id] is original_task and not original_task.done()
        assert result["status"] == "running" and result["process_live"] is True
        assert result["health"] == "ready" and result["runtime_pid"] == 61234
        assert result["console_url"] == "http://127.0.0.1:19876/console"
        assert result["current"] is False and result["current_guide_version"] is None
        assert result["update_capabilities"]["background_activation_supported"] is False
        assert result["update_capabilities"]["requires_stopped"] is True
        assert before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())
        assert f.factory._source is None and f.factory.host._product_source_custody._closed
    finally:
        original_task.cancel()
        await asyncio.gather(original_task, return_exceptions=True)
        f.service.tasks.pop(f.factory.deployment_id)


async def test_prepare_uuid_duplicate_and_pending_do_not_reopen_source_or_repeat_inventory(inputs_destination, monkeypatch):
    f = inputs_destination
    _, request = await prepared_guest(f, monkeypatch)
    before = snapshot(f.root), (f.directory / "receipt.json").read_bytes()
    pin = f.factory._startup_pin

    def no_operation(*args, **kwargs):
        pytest.fail("A retained UUID cannot enter another source or native preparation.")

    monkeypatch.setattr(f.factory, "_operation", no_operation)
    duplicate = await action(f, "prepare-setup", request)
    pending = str(uuid4())
    f.service._claim_action_request({"action_request_id": pending, "guide_id": ids(f)[0],
        "deployment_id": ids(f)[1], "action": "prepare-setup", "update_id": None})
    unknown = await action(f, "prepare-setup", pending)
    assert duplicate["action_request"]["phase"] == "settled"
    assert unknown["action_request"]["phase"] == "pending"
    assert f.factory._startup_pin is pin and len(f.inventory_calls) == 1
    assert before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())


@pytest.mark.parametrize("installation", [False, "gcp"], indirect=True, ids=["local", "gcp"])
async def test_finish_and_stop_use_actual_guest_uuid_owner_and_parent_source_closes_before_launch(
        inputs_destination, monkeypatch, installation):
    f = inputs_destination
    assert f is installation
    await prepared_guest(f, monkeypatch)
    fresh = f.factory._view()
    saved = retained(f), f.factory._startup_pin.read()
    calls = inert_worker(f, monkeypatch)
    request = str(uuid4())
    result = await action(f, "finish-setup", request)
    assert result["status"] == "queued" and result["current"] is False
    assert result["action_request"]["phase"] == "settled"
    assert len(calls) == 1 and calls[0][0]["bootstrap_owner_source"] == f.selected.source
    selected = bootstrap.read_bootstrap_admission(f.directory, calls[0][0]["bootstrap_selection"])
    assert selected["application_startup"] == fresh
    assert f.service.tasks[f.factory.deployment_id] is calls[0][1]
    duplicate = await action(f, "finish-setup", request)
    assert duplicate["action_request"]["phase"] == "settled" and len(calls) == 1
    stop_request = str(uuid4())
    stopped = await action(f, "stop", stop_request)
    assert stopped["status"] == "stopped" and stopped["phase"] == "stop"
    assert stopped["action_request"]["phase"] == "settled" and stopped["action_request"]["outcome"] == "returned"
    assert calls[0][1].cancelled() and not f.service.tasks and not f.service.processes
    assert saved == (retained(f), f.factory._startup_pin.read())
    assert len(f.inventory_calls) == 1 and not bootstrap.FRESH_ENTRY_NAME in snapshot(f.directory)
    assert not Path(f.held.fresh_paths["provision_attempt_path"]).exists()
    with project_lock(f.root) as lease:
        lease.require_current()


async def test_lost_finish_reply_retains_pending_uuid_and_original_worker_without_replay(inputs_destination, monkeypatch):
    f = inputs_destination
    await prepared_guest(f, monkeypatch)
    calls = inert_worker(f, monkeypatch)
    original = f.service.status
    request = str(uuid4())

    async def lost_reply(*args, **kwargs):
        result = await original(*args, **kwargs)
        assert result["status"] == "queued" and len(calls) == 1
        raise asyncio.CancelledError("inert lost caller reply after actual bootstrap admission")

    monkeypatch.setattr(f.service, "status", lost_reply)
    with pytest.raises(asyncio.CancelledError):
        await action(f, "finish-setup", request)
    original_worker = calls[0][1]
    record = (await f.service.action_request(*ids(f), request))["action_request"]
    assert record["phase"] == "pending"
    duplicate = await action(f, "finish-setup", request)
    assert duplicate["action_request"] == record and len(calls) == 1
    assert f.service.tasks[f.factory.deployment_id] is original_worker and not original_worker.done()
    assert f.factory._source is None and f.factory.host._product_source_custody._closed
    monkeypatch.setattr(f.service, "status", original)
    stopped = await action(f, "stop")
    assert stopped["status"] == "stopped" and original_worker.cancelled()
    assert not f.service.tasks and not f.service.processes


async def test_interactive_restart_refuses_unentered_fresh_without_inventory_or_launch_replay(inputs_destination, monkeypatch):
    f = inputs_destination
    await prepared_guest(f, monkeypatch)
    before = snapshot(f.root), (f.directory / "receipt.json").read_bytes()
    request = str(uuid4())
    with pytest.raises(DraftConflict, match="completed Restore inputs; Fresh is never replayed"):
        await action(f, "restart", request)
    record = (await f.service.action_request(*ids(f), request))["action_request"]
    assert record["phase"] == "settled" and record["outcome"] == "error"
    assert before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())
    assert len(f.inventory_calls) == 1 and not f.service.tasks and not f.service.processes
    assert f.factory._view()["kind"] == "fresh"


@pytest.mark.parametrize("installation", [False, "gcp"], indirect=True, ids=["local", "gcp"])
async def test_original_completed_restore_restarts_guest_through_same_stop_and_start_owners(
        inputs_destination, monkeypatch, installation):
    f = inputs_destination
    assert f is installation
    owner, pin, commands, queries = await completed_guest(f, monkeypatch)
    original = pin.read()
    calls = inert_worker(f, monkeypatch)
    request = str(uuid4())
    result = await action(f, "restart", request)
    assert result["status"] == "queued" and result["action_request"]["phase"] == "settled"
    assert len(calls) == 1 and calls[0][0]["bootstrap_owner_source"] == f.selected.source
    admitted = bootstrap.read_bootstrap_admission(f.directory, calls[0][0]["bootstrap_selection"])
    assert admitted["application_startup"] == json.loads(original)
    assert admitted["application_startup"]["kind"] == "restore"
    assert pin.read() == original and f.factory._source is None
    duplicate = await action(f, "restart", request)
    assert duplicate["action_request"]["phase"] == "settled" and len(calls) == 1
    stopped = await action(f, "stop")
    assert stopped["status"] == "stopped" and calls[0][1].cancelled()
    assert len(commands) == 2 and len(f.inventory_calls) == 1
    assert owner.preparation_owner._restore_pin == pin and pin.read() == original


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_actual_gcp_restore_background_start_restart_and_stop_keep_child_lease_and_original_service(
        inputs_destination, monkeypatch, installation):
    f = inputs_destination
    assert f is installation
    owner, pin, commands, _ = await completed_guest(f, monkeypatch)
    from launchkit import deployment_service as services
    manager = Manager()
    manager.cleanup = f.directory
    f.service.service_manager = manager
    original_start, original_stop = manager.start, manager.stop
    held, scopes = [], []

    async def start(service):
        assert f.factory._source is None and owner.host._product_source_custody._closed
        result = await original_start(service)
        scope = project_lock(f.root)
        held.append(scope.__enter__())
        scopes.append(scope)
        held[-1].require_current()
        return result

    async def stop(service):
        if scopes:
            held[-1].require_current()
            scopes.pop().__exit__(None, None, None)
        return await original_stop(service)

    monkeypatch.setattr(manager, "start", start)
    monkeypatch.setattr(manager, "stop", stop)
    selected = pin.read()
    entry = (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()
    request = str(uuid4())
    try:
        result = await action(f, "start-background", request)
        assert result["action_request"]["outcome"] == "returned"
        assert result["execution_mode"] == "background" and result["status"] == "starting"
        assert result["current"] is False and result["console_url"] is None
        service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
        unit = services.private_file(service["unit_path"])
        launcher = services.private_file(f.directory / "service-host/launcher.py")
        assert len(held) == 1 and not held[0]._closed
        observed = await f.factory.status(f.service, *ids(f))
        assert observed["status"] == "starting" and len(held) == 1
        assert not held[0]._closed and f.factory._source is None
        duplicate = await action(f, "start-background", request)
        assert duplicate["action_request"]["outcome"] == "returned"
        assert manager.calls.count(("start", service["label"])) == 1
        restart = await action(f, "restart")
        assert restart["action_request"]["outcome"] == "returned" and restart["status"] == "starting"
        assert len(held) == 2 and held[0]._closed and not held[1]._closed
        stopped = await action(f, "stop")
        assert stopped["status"] == "stopped" and stopped["action_request"]["outcome"] == "returned"
        assert held[1]._closed and not scopes and f.factory._source is None
        assert manager.calls.count(("start", service["label"])) == 2
        assert manager.calls.count(("stop", service["label"])) == 2
        assert services.private_file(service["unit_path"]) == unit
        assert services.private_file(f.directory / "service-host/launcher.py") == launcher
        assert pin.read() == selected and (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == entry
        assert len(commands) == 2 and len(f.inventory_calls) == 1
    finally:
        while scopes:
            scopes.pop().__exit__(None, None, None)


@pytest.mark.parametrize("name", ["retry", "apply-update", "prepare-support"])
async def test_destination_uuid_cannot_select_origin_design_updates_or_retry(inputs_destination, name):
    f = inputs_destination
    await guest(f)
    before = snapshot(f.root), snapshot(f.directory)
    with pytest.raises(ValueError, match="setup/lifecycle action UUID"):
        await action(f, name)
    assert before == (snapshot(f.root), snapshot(f.directory))
    assert not f.inventory_calls and not f.service.tasks


@pytest.mark.parametrize("change", ["source", "callback"])
async def test_actual_guest_prepare_refuses_source_or_original_callback_drift_without_effects(
        inputs_destination, monkeypatch, change):
    f = inputs_destination
    await guest(f)
    saved = deployment.private_configuration_snapshot(f.root), (f.directory / "receipt.json").read_bytes()
    if change == "source":
        (f.root / "assets/empty").write_bytes(b"foreign installed authored source")
    else:
        f.service.application_setup_preparation = lambda *args: None
    with pytest.raises((CustodyError, DraftConflict, ValueError), match="source|Source|changed|callbacks"):
        await action(f, "prepare-setup")
    assert saved == (deployment.private_configuration_snapshot(f.root), (f.directory / "receipt.json").read_bytes())
    assert not f.inventory_calls and not f.held.inventory_attempt_path.exists()
    assert not f.held.startup_selection_path.exists() and not f.service.tasks and not f.service.processes


async def test_selected_guest_rejects_forged_owner_and_cross_installation_uuid_without_native_admission(inputs_destination):
    f = inputs_destination
    await guest(f)
    before = snapshot(f.root), (f.directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict, match="actual retained application factory"):
        await f.service.action(*ids(f), "prepare-setup", action_request_id=str(uuid4()),
            destination_owner={"installed": True})
    with pytest.raises(CustodyError, match="another installed owner"):
        await f.factory.action(f.service, ids(f)[0], str(uuid4()), "prepare-setup", action_request_id=str(uuid4()))
    assert before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())
    assert not f.inventory_calls and not f.held.startup_selection_path.exists() and not f.service.tasks


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_actual_owner_drift_after_joined_installed_read_refuses_before_bootstrap_publication_or_launch(
        inputs_destination, monkeypatch, installation):
    f = inputs_destination
    assert f is installation
    await prepared_guest(f, monkeypatch)
    from launchkit import product_runtime_custody
    original = product_runtime_custody.await_product_validation
    before = snapshot(f.root), (f.directory / "receipt.json").read_bytes()
    pin = f.factory._startup_pin
    entered = []

    async def validate(source, function, *args, **kwargs):
        result = await original(source, function, *args, **kwargs)
        if function == f.service._installed_configuration:
            assert asyncio.current_task() is f.parent and asyncio.get_running_loop() is f.loop
            assert source is kwargs["source_custody"] and not source._closed
            assert f.factory._source is None and source._lease.root == f.root
            assert not entered
            entered.append(source)
            # The real full installed reader already returned valid facts.
            # Change only the original action callback at this awaited seam.
            f.service.application_setup_start = lambda *args: None
        return result

    def forbidden(*args, **kwargs):
        pytest.fail("Drifted original destination authority must refuse before _launch.")

    monkeypatch.setattr(product_runtime_custody, "await_product_validation", validate)
    monkeypatch.setattr(f.service, "_launch", forbidden)
    request = str(uuid4())
    with pytest.raises(CustodyError, match="original destination worker application callbacks changed"):
        await action(f, "finish-setup", request)
    record = (await f.service.action_request(*ids(f), request))["action_request"]
    assert record["phase"] == "settled" and record["outcome"] == "error"
    assert len(entered) == 1 and entered[0]._closed and entered[0]._lease._closed
    assert before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())
    assert f.factory._startup_pin is pin and not list(f.directory.glob("bootstrap-admission-*"))
    assert len(f.inventory_calls) == 1 and not f.service.tasks and not f.service.processes
    with project_lock(f.root) as lease:
        lease.require_current()


@pytest.mark.parametrize("installation", ["gcp"], indirect=True)
async def test_original_worker_drift_during_restart_manager_admission_joins_same_service_cleanup(
        inputs_destination, monkeypatch, installation):
    f = inputs_destination
    assert f is installation
    owner, pin, commands, _ = await completed_guest(f, monkeypatch)
    from launchkit import deployment_service as services
    manager = Manager()
    manager.cleanup = f.directory
    f.service.service_manager = manager
    original_start, original_stop = manager.start, manager.stop
    held, scopes = [], []

    async def start(service):
        assert f.factory._source is None and owner.host._product_source_custody._closed
        result = await original_start(service)
        scope = project_lock(f.root)
        held.append(scope.__enter__())
        scopes.append(scope)
        held[-1].require_current()
        if len(held) == 2:
            # Actual manager admission ran on its retained child Task. Only
            # the original supplier's worker fence changes, not source facts.
            f.current = False
        return result

    async def stop(service):
        if scopes:
            held[-1].require_current()
            scopes.pop().__exit__(None, None, None)
        return await original_stop(service)

    monkeypatch.setattr(manager, "start", start)
    monkeypatch.setattr(manager, "stop", stop)
    try:
        first = await action(f, "start-background")
        assert first["status"] == "starting" and len(held) == 1 and not held[0]._closed
        service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
        unit = services.private_file(service["unit_path"])
        launcher = services.private_file(f.directory / "service-host/launcher.py")
        selected = pin.read()
        request = str(uuid4())
        with pytest.raises(CustodyError, match="Original destination supplier owner changed"):
            await action(f, "restart", request)
        record = (await f.service.action_request(*ids(f), request))["action_request"]
        assert record["phase"] == "settled" and record["outcome"] == "error"
        assert len(held) == 2 and all(lease._closed for lease in held) and not scopes
        assert deployment.read_receipt(f.directory)["status"] == "stopped"
        assert manager.observation == {"loaded": False, "pid": None, "state": "stopped"}
        assert manager.calls.count(("start", service["label"])) == 2
        assert manager.calls.count(("stop", service["label"])) == 2
        assert services.private_file(service["unit_path"]) == unit
        assert services.private_file(f.directory / "service-host/launcher.py") == launcher
        assert pin.read() == selected and f.factory._source is None
        assert not f.service.tasks and not f.service.processes
        assert len(commands) == 2 and len(f.inventory_calls) == 1
        with project_lock(f.root) as lease:
            lease.require_current()
    finally:
        while scopes:
            scopes.pop().__exit__(None, None, None)
