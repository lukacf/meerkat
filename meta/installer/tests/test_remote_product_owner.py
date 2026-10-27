"""Actual retained product owner with real private IPC and inert native edges.

The original artifact/source readers, factory byte Install, same Task/loop owner,
private socket routing, Configure writer, canonical UUID journal and lifecycle
admissions remain real. SDK/native execution, launched worker body, Linux births
and peer credential facts are explicit inert boundaries. No SSH, VM, SDK/native
readiness, login, useful application job or restart-after-reboot is established.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
import os
import socket
import struct
from uuid import uuid4

import pytest

from launchkit import deployment, remote_destination as controller, remote_product_control as control
from launchkit.remote_deployment import product_control_observation
from test_remote_product_control import control_destination
from test_destination_application_inputs import inputs_destination
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution
from test_destination_control_actions import inert_worker


async def start_owner(f, monkeypatch):
    # Mac-hosted source controls retain actual IPC but explicitly supply inert
    # Linux peer facts, just like the existing fixture's Linux process births.
    monkeypatch.setattr(control, "_control_peer", lambda channel: (os.getpid(), os.getuid(), os.getgid()))
    loop = asyncio.get_running_loop()
    monkeypatch.setattr(loop, "add_signal_handler", lambda *args: None)
    monkeypatch.setattr(loop, "remove_signal_handler", lambda *args: True)
    with controller._input_descriptor(control._json(f.settings), f.selected.destination) as descriptor:
        owner = asyncio.create_task(control.run_initial_install(f.selected.destination,
            f.remote_operation, os.dup(descriptor), retain_control=True))
    try:
        async with asyncio.timeout(20):
            while not (f.remote_folder / "product-control-owner.json").exists():
                if owner.done():
                    await owner
                await asyncio.sleep(0.01)
        f.root = f.selected.destination
        f.held = f.input_owner.preparation_owner._inputs
        assert f.parent is owner and f.service.guide is None and f.service.implementation is None
        assert not owner.done() and f.factory._source is None and f.factory._pending_scope is None
        assert f.factory is f.input_owner.factory and f.factory.host is f.input_owner.host
        return owner
    except BaseException:
        owner.cancel()
        await asyncio.gather(owner, return_exceptions=True)
        raise


def request(f, action="status", **extra):
    return {"action": action, "deployment_id": f.remote_owner["deployment_id"],
        "binding": deepcopy(f.remote_owner["binding"]), "destination": str(f.selected.destination),
        "expected_host": deepcopy(f.remote_owner["host"]), **extra}


async def rpc(f, action="status", **extra):
    async with asyncio.timeout(30):
        return await asyncio.to_thread(control.request_control, vars(controller), f.selected.destination,
            f.remote_owner, f.control_bundle.manifest, request(f, action, **extra))


async def finish_owner(owner):
    owner.cancel()
    async with asyncio.timeout(20):
        with pytest.raises(asyncio.CancelledError):
            await owner


async def prepare(f, monkeypatch):
    inventory = f.input_owner.host._inventory
    f.actual = {"schema": 1, "mob_id": inventory["mob_id"], "helper_identity": inventory["helper_identity"],
        "realm_id": "inert-destination-realm", "members": [{"identity": row["identity"],
            "member": {"mob_id": inventory["mob_id"], "role": row["profile"], "member": "inert-member-" + str(i)}}
            for i, row in enumerate(inventory["roster"])]}
    monkeypatch.setattr(f.held.supervisor, "command", f.inventory_command)
    answer = await rpc(f, "prepare-setup", operation_id=str(uuid4()))
    assert "error" not in answer and answer["product_control"]["action_request"]["phase"] == "settled"
    assert len(f.inventory_calls) == 1 and f.factory._view()["kind"] == "fresh"
    return answer


async def test_retained_actual_owner_configures_after_install_without_reusing_initial_mutable_receipt(control_destination, monkeypatch):
    f = control_destination
    owner = await start_owner(f, monkeypatch)
    service, factory, inputs = f.service, f.factory, f.input_owner
    before = (f.remote_folder / "product-install-completed.json").read_bytes()
    try:
        first = await rpc(f)
        assert first["status"] == "stopped" and first["worker_live"] is True and first["cleanup_confirmed"] is False
        assert first["ready"] is False and first["product_control"]["initial_install_complete"] is True
        assert first["product_control"]["setup_status"]["action"] == "prepare-setup"
        origin = {**f.remote_owner["binding"], "deployment_id": f.remote_owner["deployment_id"],
            "host": f.remote_owner["host"], "destination": str(f.root), "install_operation_id": f.remote_operation,
            "product_control_manifest_sha256": hashlib.sha256(control._json(f.control_bundle.manifest)).hexdigest()}
        assert product_control_observation(origin, first) == first["product_control"]
        options = await rpc(f, "settings-options")
        assert options["private_settings_configurable"] and options["configured_keys"]
        identity = str(uuid4())
        value = await rpc(f, "configure", operation_id=identity,
            settings={"CONSOLE_LOGIN_PASSWORD": "inert-retained-owner-password"}, remove_fields=[])
        assert value["configuration_saved"] is True and value["operation_id"] == identity
        assert value["product_control"]["action_request"]["outcome"] == "returned"
        assert deployment.private_configuration_snapshot(f.root)[0]["CONSOLE_LOGIN_PASSWORD"] == "inert-retained-owner-password"
        current = await rpc(f)
        assert current["status"] == "stopped" and current["source_current"] and current["worker_live"] is True
        assert current["product_control"]["binding"] == first["product_control"]["binding"]
        assert before == (f.remote_folder / "product-install-completed.json").read_bytes()
        assert hashlib.sha256((f.directory / "receipt.json").read_bytes()).hexdigest() != json.loads(before)["installation_receipt_sha256"]
        assert (service, factory, inputs) == (f.service, f.factory, f.input_owner)
        assert f.parent is owner and not owner.done() and len(f.supervisors) == 2
        assert "inert-retained-owner-password" not in json.dumps(value) + json.dumps(current)
        assert not f.service.tasks and not f.service.processes and not f.inventory_calls
    finally:
        await finish_owner(owner)
    assert not f.service.tasks and not f.service.processes and f.factory._source is None
    f.live = False  # The inert original worker birth now observes exit.
    with pytest.raises(ValueError, match="unavailable or changed"):
        await rpc(f)


async def test_finish_runtime_survives_request_and_same_owner_stop_joins_original_task(control_destination, monkeypatch):
    f = control_destination
    owner = await start_owner(f, monkeypatch)
    try:
        await prepare(f, monkeypatch)
        calls = inert_worker(f, monkeypatch)
        identity = str(uuid4())
        result = await rpc(f, "finish-setup", operation_id=identity)
        assert "error" not in result and result["product_control"]["action_request"]["outcome"] == "returned"
        actual_runtime = calls[0][1]
        assert f.service.tasks[f.factory.deployment_id] is actual_runtime and not actual_runtime.done()
        assert not owner.done() and f.parent is owner and f.factory._source is None
        # The SSH request already returned; the retained loop still owns runtime.
        await asyncio.sleep(0)
        observed = await rpc(f)
        assert f.service.tasks[f.factory.deployment_id] is actual_runtime and not actual_runtime.done()
        assert observed["product_control"]["factory_status"]["status"] == "queued"
        duplicate = await rpc(f, "finish-setup", operation_id=identity)
        assert duplicate["product_control"]["action_request"] == result["product_control"]["action_request"]
        assert len(calls) == 1 and not actual_runtime.done()
        stopped = await rpc(f, "stop", operation_id=str(uuid4()))
        assert stopped["status"] == "stopped" and stopped["worker_live"] is True and not stopped["cleanup_confirmed"]
        assert stopped["product_control"]["action_request"]["outcome"] == "returned"
        assert actual_runtime.cancelled() and not f.service.tasks and not f.service.processes
        assert not owner.done() and len(f.inventory_calls) == 1
    finally:
        await finish_owner(owner)


async def test_lost_finish_socket_reply_reads_canonical_uuid_without_runtime_replay(control_destination, monkeypatch):
    f = control_destination
    owner = await start_owner(f, monkeypatch)
    try:
        await prepare(f, monkeypatch)
        calls = inert_worker(f, monkeypatch)
        identity = str(uuid4())
        record = control._control_record(vars(controller), f.root, f.remote_owner, f.control_bundle.manifest)
        payload = control._json({"control": control._public_control_binding(record),
            "request": request(f, "finish-setup", operation_id=identity)})
        def lose_reply():
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                channel.connect(record["socket"]["path"])
                channel.sendall(struct.pack("!I", len(payload)) + payload)
                # Only the transport is lost. The owner Task and admitted UUID survive.
        await asyncio.to_thread(lose_reply)
        async with asyncio.timeout(20):
            while len(calls) != 1:
                if owner.done():
                    await owner
                await asyncio.sleep(0.01)
        runtime = calls[0][1]
        answer = await rpc(f, "status", operation_id=identity)
        journal = answer["product_control"]["action_request"]
        assert journal["action_request_id"] == identity and journal["phase"] == "settled"
        assert len(calls) == 1 and not runtime.done() and not owner.done()
        duplicate = await rpc(f, "finish-setup", operation_id=identity)
        assert duplicate["product_control"]["action_request"] == journal and len(calls) == 1
        await rpc(f, "stop", operation_id=str(uuid4()))
        assert runtime.cancelled() and not f.service.tasks and not f.service.processes
    finally:
        await finish_owner(owner)


@pytest.mark.parametrize("change", ["binding", "uuid"])
async def test_retained_control_refuses_foreign_request_without_private_or_journal_effects(control_destination, monkeypatch, change):
    f = control_destination
    owner = await start_owner(f, monkeypatch)
    try:
        record = control._control_record(vars(controller), f.root, f.remote_owner, f.control_bundle.manifest)
        body = request(f, "configure", operation_id=str(uuid4()), settings={"CONSOLE_LOGIN_PASSWORD": "must-not-write"})
        if change == "binding":
            original = body["binding"]["accepted_source_digest"]
            body["binding"]["accepted_source_digest"] = ("b" if original != "b" * 64 else "c") * 64
            assert body["binding"]["accepted_source_digest"] != original
        else:
            body["operation_id"] = None
        before = (f.directory / "receipt.json").read_bytes(), (f.root / ".env").read_bytes()
        answer = await asyncio.to_thread(control.request_control, vars(controller), f.root,
            f.remote_owner, f.control_bundle.manifest, body)
        assert answer["error"]["code"] == "recovery-needed"
        assert before == ((f.directory / "receipt.json").read_bytes(), (f.root / ".env").read_bytes())
        assert not f.service.tasks and not f.service.processes and f.factory._source is None and not owner.done()
        assert not list((f.service.root / "action-requests").glob("*.json"))
        assert control._control_record(vars(controller), f.root, f.remote_owner, f.control_bundle.manifest) == record
    finally:
        await finish_owner(owner)


async def test_genuine_controller_lock_route_observes_completion_before_endpoint_publication(control_destination, monkeypatch):
    f = control_destination
    original_progress, original_adapter = controller._progress, controller._product_adapter
    before_discovery = []

    def actual_adapter_with_inert_peer_facts():
        namespace, manifest = original_adapter()
        namespace["_control_peer"] = control._control_peer
        return namespace, manifest

    def publish(root, selected_owner, operation, phase, **updates):
        if phase == "complete" and updates.get("status") == "completed":
            assert not (f.remote_folder / "product-control-owner.json").exists()
            # The genuine handle acquires the actual original remote lock. Before
            # discovery it must return status without waiting on owner dispatch.
            value = controller.handle(request(f))
            assert "error" not in value and value["worker_live"] is True and value["status"] == "installing"
            before_discovery.append(value)
        return original_progress(root, selected_owner, operation, phase, **updates)

    monkeypatch.setattr(controller, "_product_adapter", actual_adapter_with_inert_peer_facts)
    monkeypatch.setattr(controller, "_progress", publish)
    owner = await start_owner(f, monkeypatch)
    try:
        assert len(before_discovery) == 1
        async with asyncio.timeout(20):
            observed = await asyncio.to_thread(controller.handle, request(f))
        assert "error" not in observed and observed["protocol"] == 1
        assert observed["status"] == "stopped" and observed["worker_live"] is True
        assert observed["product_control"]["initial_install_complete"] is True
        assert observed["cleanup_confirmed"] is False and f.parent is owner and not owner.done()
        job = controller._job(f.root)
        assert job["status"] == "completed" and job["operation_id"] == f.remote_operation
        endpoint = json.loads((f.remote_folder / "product-control-owner.json").read_bytes())
        assert job["worker"] == endpoint["worker"] and job["command"] == endpoint["command"]
    finally:
        await finish_owner(owner)
