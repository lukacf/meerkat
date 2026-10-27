"""Normal UUID Restore-service dispatch and read-only capability, all boundaries inert.

Existing product/source/bootstrap/manager fixtures retain exact bytes. No real
SDK, OS service, native preparation, provider, cloud or application runs.
"""
import asyncio
from copy import deepcopy
import inspect
import json
from pathlib import Path
import sys
import threading
import uuid

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from test_product_service_restore import service_owner, completed_fresh
from test_meta_toolkit_initial_bootstrap import product_bootstrap
from test_meta_toolkit_initial_install import initial, implementation_service, product, no_product_execution, no_processes
from test_deployment_application_bootstrap import no_runtime_operations, write
from test_forge_application_host import selected, startup, put
from test_forge_application_host_server import api, select


@pytest.fixture
def selected_restore(service_owner):
    f = service_owner
    f.selected_value, f.selection_calls = deepcopy(f.restore), []
    def supplier(guide, installation):
        assert (guide, installation) == (f.receipt["guide_id"], f.receipt["deployment_id"])
        f.selection_calls.append((guide, installation))
        return deepcopy(f.selected_value)
    f.owner.application_setup_selection = supplier
    f.supplier = supplier
    return f


async def start(f, request_id=None, **extra):
    return await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "start-background",
        action_request_id=request_id or str(uuid.uuid4()), **extra)


def request_binding(f, request_id):
    return {"action_request_id": request_id, "guide_id": f.receipt["guide_id"],
        "deployment_id": f.receipt["deployment_id"], "action": "start-background", "update_id": None}


async def test_normal_action_reuses_actual_restore_join_and_lost_reply_never_selects_twice(selected_restore):
    f = selected_restore
    request_id = str(uuid.uuid4())
    result = await start(f, request_id)
    assert result["status"] == "starting" and result["console_url"] is None and not result["process_live"]
    assert result["action_request"]["outcome"] == "returned"
    pin = result["service"]["bootstrap_selection"]
    assert bootstrap.read_bootstrap_admission(f.directory, pin)["application_startup"] == f.restore
    calls, manager_calls = list(f.selection_calls), list(f.manager.calls)
    repeated = await start(f, request_id)
    assert repeated["action_request"] == result["action_request"]
    assert f.selection_calls == calls and f.manager.calls == manager_calls
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


@pytest.mark.parametrize("phase", ["pending", "unknown"])
async def test_pending_original_command_returns_before_supplier_or_manager(selected_restore, phase):
    f = selected_restore
    request_id = str(uuid.uuid4())
    value, claimed = f.owner._claim_action_request(request_binding(f, request_id))
    assert claimed
    if phase == "unknown":
        value["phase"] = phase
        deployment.write_private(f.owner._action_request_path(request_id), value)
    result = await start(f, request_id)
    assert result["action_request"]["phase"] == phase
    assert not f.selection_calls and not f.manager.calls and not (f.directory / "service-host").exists()


@pytest.mark.parametrize("field", ["guide_id", "deployment_id", "action", "update_id"])
async def test_command_binding_collision_refuses_before_supplier(selected_restore, field):
    f = selected_restore
    request_id = str(uuid.uuid4())
    binding = request_binding(f, request_id)
    binding[field] = "restart" if field == "action" else str(uuid.uuid4())
    f.owner._claim_action_request(binding)
    with pytest.raises(DraftConflict, match="different deployment action"):
        await start(f, request_id)
    assert not f.selection_calls and not f.manager.calls


@pytest.mark.parametrize("fields", [{"action_request_id": None}, {"update_id": "another-update"},
    {"action_request_id": "bad-uuid"}])
async def test_missing_uuid_or_update_substitution_refuses_before_supplier(selected_restore, fields):
    f = selected_restore
    with pytest.raises(ValueError):
        await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "start-background",
            **{"action_request_id": str(uuid.uuid4()), **fields})
    assert not f.selection_calls and not f.manager.calls


@pytest.mark.parametrize("kind", ["absent", "fresh", "malformed", "async"])
async def test_unavailable_or_non_restore_supplier_cannot_create_unit(selected_restore, kind):
    f = selected_restore
    if kind == "absent": f.owner.application_setup_selection = None
    elif kind == "fresh": f.selected_value = deepcopy(f.fresh)
    elif kind == "malformed": f.selected_value = {"kind": "restore"}
    else:
        async def never_run(*args):
            pytest.fail("An async startup supplier must not be invoked")
        f.owner.application_setup_selection = never_run
    request_id = str(uuid.uuid4())
    with pytest.raises((CustodyError, DraftConflict)):
        await start(f, request_id)
    assert not (f.directory / "service-host").exists() and not f.manager.calls
    calls = list(f.selection_calls)
    repeated = await start(f, request_id)
    assert repeated["action_request"]["outcome"] == "error" and f.selection_calls == calls


async def test_sync_supplier_returning_coroutine_is_closed_without_execution(selected_restore):
    f = selected_restore
    returned = []
    async def never_run():
        pytest.fail("A returned startup coroutine must never execute")
    def supplier(*args):
        result = never_run()
        returned.append(result)
        return result
    f.owner.application_setup_selection = supplier
    with pytest.raises(DraftConflict, match="synchronously"):
        await start(f)
    assert len(returned) == 1 and inspect.getcoroutinestate(returned[0]) == inspect.CORO_CLOSED
    assert not f.manager.calls and not (f.directory / "service-host").exists()


@pytest.mark.parametrize("change", ["callback", "envelope", "source"])
async def test_owner_change_during_awaited_manager_capability_refuses_before_unit(selected_restore, change):
    f = selected_restore
    async def capability():
        if change == "callback": f.owner.application_setup_selection = lambda *args: deepcopy(f.restore)
        elif change == "envelope": f.selected_value["gateway"]["sha256"] = "f" * 64
        else: write(f.destination / "application/tool-policy.json", b"{}\n", 0o644)
        return {"available": True}
    f.manager.capability = capability
    with pytest.raises((CustodyError, DraftConflict)):
        await start(f)
    assert not (f.directory / "service-host").exists()
    assert not any(row[0] == "start" for row in f.manager.calls)


async def test_cancellation_during_restore_file_read_drains_thread_before_unlock_and_no_replay(selected_restore, monkeypatch):
    f = selected_restore
    entered, release = threading.Event(), threading.Event()
    original = f.owner._installed_configuration
    def suspended(*args, **kwargs):
        entered.set()
        assert release.wait(5)
        return original(*args, **kwargs)
    monkeypatch.setattr(f.owner, "_installed_configuration", suspended)
    request_id = str(uuid.uuid4())
    task = asyncio.create_task(start(f, request_id))
    await asyncio.to_thread(entered.wait, 5)
    assert entered.is_set()
    task.cancel()
    await asyncio.sleep(0)
    assert f.owner.lock.locked() and not task.done()
    release.set()
    with pytest.raises(asyncio.CancelledError): await task
    assert not f.owner.lock.locked() and not f.manager.calls
    assert (await start(f, request_id))["action_request"]["phase"] == "pending"
    assert not (f.directory / "service-host").exists()


async def test_cancelled_normal_admission_drains_manager_then_original_stop_without_replay(selected_restore):
    f = selected_restore
    entered, cancelled, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
    original_start = f.manager.start
    async def suspended(service):
        await original_start(service)
        entered.set()
        try: await release.wait()
        except asyncio.CancelledError:
            cancelled.set()
            await release.wait()
            raise
    f.manager.start = suspended
    request_id = str(uuid.uuid4())
    task = asyncio.create_task(start(f, request_id))
    await entered.wait()
    task.cancel()
    await cancelled.wait()
    assert not task.done() and f.owner.lock.locked()
    release.set()
    with pytest.raises(asyncio.CancelledError): await task
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["console_url"] is None
    assert f.manager.calls.count(("stop", receipt["service"]["label"])) == 1
    calls = list(f.selection_calls), list(f.manager.calls)
    repeated = await start(f, request_id)
    assert repeated["action_request"]["phase"] == "pending"
    assert (f.selection_calls, f.manager.calls) == calls


async def capability(f, **options):
    return await f.owner.restored_service_capability(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=deepcopy(f.restore), require_owner_current=lambda: None, **options)


async def test_read_only_capability_checks_real_restore_history_source_and_manager_without_admission(selected_restore, monkeypatch):
    f = selected_restore
    before = (f.directory / "receipt.json").read_bytes()
    names = set(f.directory.iterdir())
    def forbidden(*args, **kwargs):
        pytest.fail("GET capability must not publish an admission or construct/start a native owner")
    monkeypatch.setattr(bootstrap, "retain_bootstrap_admission", forbidden)
    monkeypatch.setattr(f.owner, "_start_restored_service_locked", forbidden)
    monkeypatch.setattr(f.owner, "prepare_application", forbidden)
    result = await capability(f)
    assert result == {"available": True, "persistence": "After login"}
    assert (f.directory / "receipt.json").read_bytes() == before
    assert set(f.directory.iterdir()) == names and not (f.directory / "service-host").exists()
    assert not any(row[0] in {"start", "stop"} for row in f.manager.calls)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


@pytest.mark.parametrize("change", ["active", "source", "fresh", "manager", "private", "lineage", "manager-owner", "entry-replaced"])
async def test_read_only_capability_declines_invalid_or_changed_ownership_without_effect(selected_restore, change):
    f = selected_restore
    selected_value = deepcopy(f.restore)
    if change == "active": deployment.update_receipt(f.directory, status="running", process_live=True)
    elif change == "source": write(f.destination / "application/tool-policy.json", b"{}\n", 0o644)
    elif change == "fresh": selected_value = deepcopy(f.fresh)
    elif change == "lineage":
        entry = json.loads((f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes())
        entry["provision_attempt_path"] += ".foreign"
        write(f.directory / bootstrap.FRESH_ENTRY_NAME, entry)
    async def changed():
        if change == "private": write(f.destination / ".env", b"OPENAI_API_KEY=changed-private-fixture\n")
        elif change == "manager-owner": f.owner.service_manager = object()
        elif change == "entry-replaced":
            path = f.directory / bootstrap.FRESH_ENTRY_NAME
            content = path.read_bytes()
            path.unlink()
            write(path, content)
        return {"available": change != "manager", "persistence": "After login"}
    f.manager.capability = changed
    result = await f.owner.restored_service_capability(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=selected_value, require_owner_current=lambda: None)
    assert result["available"] is False
    assert not (f.directory / "service-host").exists()
    assert not any(row[0] in {"start", "stop"} for row in f.manager.calls)
    assert set(result) <= {"available", "detail", "persistence"} and len(result["detail"]) <= 500


@pytest.mark.parametrize("persistence", ["", "x" * 501, 1, "After login"])
async def test_projection_exposes_only_bounded_actual_manager_persistence(selected_restore, persistence):
    f = selected_restore
    async def manager_capability(): return {"available": True, "persistence": persistence, "secret": "never-public"}
    f.manager.capability = manager_capability
    result = await capability(f)
    assert result == ({"available": True, "persistence": persistence} if persistence == "After login" else {"available": True})


def test_http_action_forwards_ids_only_and_never_selects_private_supplier(api):
    select(api)
    api.selected.host._startup_selection = lambda *args: pytest.fail("HTTP forwarding must not select a startup envelope")
    request_id = str(uuid.uuid4())
    payload = {"deployment_id": "original-installation", "action": "start-background", "action_request_id": request_id}
    response = api.client.post("/api/guide/original-guide/deployment/action", json=payload)
    assert response.status_code == 200
    assert api.calls == [("action", ("original-guide", "original-installation", "start-background"), {"action_request_id": request_id})]
    api.calls.clear()
    for fields in ({"action_request_id": None}, {"update_id": "update"}, {"application_startup": {}},
                   {"gateway": "/private/path"}, {"principal": {}}, {"resume": True}):
        response = api.client.post("/api/guide/original-guide/deployment/action", json={**payload, **fields})
        assert response.status_code == 422 and not api.calls


def test_actual_host_http_readback_adds_capability_only_after_original_restore_and_rechecks_owner(api):
    select(api)
    startup(api.selected, "restore")
    calls = []
    async def read(guide, installation, *, application_startup, require_owner_current):
        assert (guide, installation) == ("original-guide", "original-installation")
        assert application_startup["kind"] == "restore"
        require_owner_current()
        calls.append("read-only-capability")
        return {"available": True, "persistence": "After login"}
    api.deployment.restored_service_capability = read
    response = api.client.get("/api/guide/original-guide/deployment/application-setup", params={"deployment_id": "original-installation"})
    assert response.status_code == 200 and response.json()["background"] == {"available": True, "persistence": "After login"}
    assert response.json()["kind"] == "restore" and calls == ["read-only-capability"]
    startup(api.selected, "fresh")
    response = api.client.get("/api/guide/original-guide/deployment/application-setup", params={"deployment_id": "original-installation"})
    assert response.status_code == 200 and "background" not in response.json()
    assert calls == ["read-only-capability"]


@pytest.mark.parametrize("change", ["callback", "envelope", "settings"])
def test_actual_get_post_capability_drift_cannot_publish_background_or_stale_ready(api, change):
    select(api)
    startup(api.selected, "restore")
    async def read(guide, installation, *, application_startup, require_owner_current):
        require_owner_current()
        if change == "callback": api.deployment.application_setup_selection = lambda *args: application_startup
        elif change == "envelope":
            put(api.selected.root / "restore-startup.json", {"kind": "restore"})
        else: api.settings["fields"] = [{"required": True, "label": "Password", "configured": False}]
        return {"available": True, "persistence": "After login"}
    api.deployment.restored_service_capability = read
    response = api.client.get("/api/guide/original-guide/deployment/application-setup", params={"deployment_id": "original-installation"})
    assert response.status_code == 200
    value = response.json()
    assert value["available"] is False and value["reason"] == "source_unavailable" and "background" not in value


# The setup card's own button is Start installed application, so its reason names that.
FOLDER_REFUSAL = ("macOS does not let a background login item read Desktop, Documents, Downloads, iCloud Drive or "
    "cloud storage folders. To run this application in the background, install it again with the Forge folder and "
    "the installation folder in a folder such as ~/MetaForge. Start installed application runs it while Forge is open.")
PROTECTED = ("Desktop", "Documents", "Downloads", "Library/Mobile Documents", "Library/CloudStorage")
PROTECTED_IDS = ("Desktop", "Documents", "Downloads", "iCloud-Drive", "CloudStorage")


@pytest.mark.parametrize("folder", PROTECTED, ids=PROTECTED_IDS)
async def test_restore_background_capability_names_protected_folders(selected_restore, monkeypatch, folder):
    # The test manager is launchd; Forge's Python is in a protected folder of
    # the fixture's home.
    f = selected_restore
    monkeypatch.setattr(sys, "executable", str(Path.home() / folder / "MetaForge/.venv/bin/python"))
    before = (f.directory / "receipt.json").read_bytes()
    result = await capability(f)
    assert result == {"available": False, "detail": FOLDER_REFUSAL}
    assert (f.directory / "receipt.json").read_bytes() == before
    assert not (f.directory / "service-host").exists()
    assert not any(row[0] in {"start", "stop"} for row in f.manager.calls)


@pytest.mark.parametrize("folder", PROTECTED, ids=PROTECTED_IDS)
async def test_restore_background_start_refuses_protected_folders_before_any_unit(selected_restore, monkeypatch, folder):
    f = selected_restore
    monkeypatch.setattr(sys, "executable", str(Path.home() / folder / "MetaForge/.venv/bin/python"))
    request_id = str(uuid.uuid4())
    with pytest.raises(DraftConflict) as refused:
        await start(f, request_id)
    assert str(refused.value) == FOLDER_REFUSAL
    assert not (f.directory / "service-host").exists()
    assert not any(row[0] in {"start", "stop"} for row in f.manager.calls)
    assert deployment.read_receipt(f.directory).get("service") is None
    repeated = await start(f, request_id)
    assert repeated["action_request"]["outcome"] == "error"


async def test_restore_background_checks_the_installation_folder(selected_restore, monkeypatch):
    from launchkit import deployment_service as services
    f = selected_restore
    checked, original = [], getattr(services, "launchd_folder_refusal", None)

    def record(paths, **options):
        checked.append([Path(path) for path in paths])
        return original(paths, **options)
    monkeypatch.setattr(services, "launchd_folder_refusal", record, raising=False)
    assert (await capability(f))["available"] is True
    await start(f)
    assert len(checked) == 2 and all(f.destination in paths for paths in checked)
