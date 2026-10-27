"""Completed Restore guest settings through original finalizer and recovery.

Byte Install, native artifact readers, finalizer publication, Restore lineage,
private input recovery, actual None,None guest and Configure writer remain real.
Native outputs, SDK execution and service-manager availability are explicit inert
boundaries. These controls do not qualify native setup, login, restart or GCP.
"""
import asyncio
from copy import deepcopy
import json

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap, deployment_worker
from launchkit.connect_installer_caller import CustodyError
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from launchkit.templates.storage_setup import project_lock
from test_destination_application_configuration import configure, retained
from test_destination_application_inputs import (inputs_destination, install, prepare_original,
    finalize_original_destination, reopen, snapshot)
from test_destination_application_factory import destination
from test_product_installed_inputs import installation
from test_product_destination_install import destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution


async def completed_guest(f, monkeypatch):
    original, _ = await install(f)
    await prepare_original(f, original, monkeypatch)
    commands = await finalize_original_destination(f, original, monkeypatch)
    async with f.service.lock:
        await f.factory.continue_application_startup(f.service, f.factory.guide_id, f.factory.deployment_id)
    pin = original.preparation_owner._restore_pin
    selected = json.loads(pin.read())
    assert selected["kind"] == "restore" and (f.directory / bootstrap.FRESH_ENTRY_NAME).is_file()
    bootstrap.require_restore_history_current(f.directory, selected)
    service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge", None, None)
    before = snapshot(f.root), snapshot(f.directory), pin.read()

    def no_retain(*args, **kwargs):
        pytest.fail("Completed guest recovery cannot replay successful Install defaults.")

    async def no_command(*args, **kwargs):
        pytest.fail("Completed guest settings cannot replay inventory or native commands.")

    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", no_retain)
    monkeypatch.setattr(deployment_worker.Supervisor, "command", no_command)
    recovered = await reopen(f, service, source_view=selected)
    assert before == (snapshot(f.root), snapshot(f.directory), pin.read())
    assert recovered.factory._startup_pin == pin and recovered.factory._install_entered
    f.service, f.factory, f.input_owner = service, recovered.factory, recovered
    f.held = recovered.preparation_owner._inputs
    manager_queries = []

    async def unavailable_manager(arguments, *, allow_failure, capture_output, timeout=None):
        # Availability is an external OS boundary. No service is started here.
        assert allow_failure is True and capture_output is False
        assert arguments[1:] == ["--user", "show", "--property=Version"]
        manager_queries.append(tuple(arguments))
        return 1, ""

    monkeypatch.setattr(service.service_manager, "command", unavailable_manager)
    return recovered, pin, commands, manager_queries


@pytest.mark.parametrize("first", ["read", "write"])
async def test_completed_restore_guest_can_read_and_save_private_settings_without_fresh_replay(
        inputs_destination, monkeypatch, first):
    f = inputs_destination
    owner, pin, commands, manager_queries = await completed_guest(f, monkeypatch)
    assert f.service.guide is None and f.service.implementation is None
    source_binding = deepcopy(f.selected.context)
    selected = json.loads(pin.read())
    before = snapshot(f.root), snapshot(f.directory), retained(f), pin.read()
    binding = bootstrap.bootstrap_installation_binding(deployment.read_receipt(f.directory))

    async def read():
        async with asyncio.timeout(20):
            options = await f.factory.settings_options(f.service, f.factory.guide_id, f.factory.deployment_id)
            status = await f.factory.setup_status(f.service, f.factory.guide_id, f.factory.deployment_id)
        assert options["source_verified"] and options["requires_stopped"]
        assert options["version"] == f.selected.context["guide_version"]
        assert status["available"] and status["kind"] == "restore" and "action" not in status
        assert status["background"]["available"] is False
        return options, status

    if first == "read":
        options, status = await read()
        assert before == (snapshot(f.root), snapshot(f.directory), retained(f), pin.read())
    password = "inert-completed-restore-private-password"
    async with asyncio.timeout(20):
        result = await configure(f, settings={"CONSOLE_LOGIN_PASSWORD": password})
    assert result["action_request"]["phase"] == "settled" and result["action_request"]["outcome"] == "returned"
    assert deployment.private_configuration_snapshot(f.root)[0]["CONSOLE_LOGIN_PASSWORD"] == password
    options, status = await read()
    assert password not in json.dumps(result) + json.dumps(options) + json.dumps(status)
    assert {name: value for name, value in snapshot(f.root).items() if name != ".env"} == {
        name: value for name, value in before[0].items() if name != ".env"}
    assert {name: value for name, value in snapshot(f.directory).items() if name != "receipt.json"} == {
        name: value for name, value in before[1].items() if name != "receipt.json"}
    assert retained(f) == before[2] and pin.read() == before[3] and f.selected.context == source_binding
    assert f.factory._view() == selected and owner.preparation_owner._restore_pin == pin
    assert bootstrap.bootstrap_installation_binding(deployment.read_receipt(f.directory)) == binding
    bootstrap.require_restore_history_current(f.directory, selected)
    assert len(commands) == 2 and len(f.inventory_calls) == 1 and manager_queries
    assert not f.service.tasks and not f.service.processes and not f.service._application_preparations
    assert f.factory._source is None and owner.host._product_source_custody._closed
    assert not owner.host._product_source_custody._invalidated
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["runtime_pid"] is None
    with project_lock(f.root) as lease:
        lease.require_current()


async def test_completed_output_with_original_fresh_selection_cannot_admit_guest_settings(
        inputs_destination, monkeypatch):
    f = inputs_destination
    f.service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge", None, None)
    owner, _ = await install(f)
    inventory = owner.host._inventory
    f.actual = {"schema": 1, "mob_id": inventory["mob_id"],
        "helper_identity": inventory["helper_identity"], "realm_id": "inert-destination-realm",
        "members": [{"identity": row["identity"], "member": {"mob_id": inventory["mob_id"],
            "role": row["profile"], "member": "inert-member-" + str(i)}}
            for i, row in enumerate(inventory["roster"])]}
    monkeypatch.setattr(f.held.supervisor, "command", f.inventory_command)
    async with f.service.lock:
        await f.factory.prepare_application(f.service, f.factory.guide_id, f.factory.deployment_id)
    assert len(f.inventory_calls) == 1
    commands = await finalize_original_destination(f, owner, monkeypatch)
    # Native specimen completion alone cannot publish the original Restore.
    assert owner.preparation_owner._restore_pin is None
    assert f.factory._view()["kind"] == "fresh" and (f.directory / bootstrap.FRESH_ENTRY_NAME).is_file()
    before = snapshot(f.root), snapshot(f.directory), f.factory._startup_pin.read()
    with pytest.raises(CustodyError, match="Fresh bootstrap|completed Restore selection"):
        await f.factory.settings_options(f.service, f.factory.guide_id, f.factory.deployment_id)
    with pytest.raises(CustodyError, match="Fresh bootstrap|completed Restore selection"):
        await configure(f, settings={"CONSOLE_LOGIN_PASSWORD": "must-not-be-written"})
    assert before == (snapshot(f.root), snapshot(f.directory), f.factory._startup_pin.read())
    assert owner.preparation_owner._restore_pin is None and f.factory._view()["kind"] == "fresh"
    assert len(commands) == 2 and len(f.inventory_calls) == 1
    assert not f.service.tasks and not f.service.processes and not f.service._application_preparations
    assert f.factory._source is None
