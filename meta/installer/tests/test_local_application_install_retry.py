"""A retried Install completion after a Forge restart, through the launcher's registry.

The real registry, deployment service, Install completion owner and setup
action are used. The worker process and native outputs are inert specimens; no
application, provider or native runtime executes here.
"""
import asyncio
from copy import deepcopy
import json
from types import SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from test_forge_application_host import encoded
from test_local_application_prepared_recovery import unavailable_service_query
from test_local_application_registry import (  # noqa: F401 (fixtures)
    accept_source_fixture, build, install_fixture, new_guide, no_processes, no_product_execution,
    product, registry,
)
from test_update_after_setup_registry import reloaded


INERT_WORKER_PID = 981237
NOT_RETAINED = "Trusted setup inputs were not retained"


async def relaunch_install(service, directory, monkeypatch):
    """The launch a guarded Retry makes, through the real Install completion owner."""
    proof = deepcopy(deployment.read_receipt(directory)["installed_candidate_runtime_proof"])
    process = SimpleNamespace(pid=INERT_WORKER_PID, returncode=None, stdin=None)

    async def wait():
        current = deployment.read_receipt(directory)
        cleanup = {name: current.get(name) for name in ("guide_id", "deployment_id", "build_id",
            "guide_version", "blueprint_hash", "destination", "package_sha256")}
        cleanup.update(deployment.native_fields(current))
        cleanup.update(deployment.interactive_source_binding(directory, current))
        cleanup.update(confirmed=True, worker_exit_confirmed=False,
            lifecycle_id=current["interactive_lifecycle_id"], startup_attempt_id=None,
            supervisor_pid=process.pid, return_code=0, boundary="owned-process-groups-absent")
        deployment.update_receipt(directory, status="stopped", phase="configure", runtime_pid=None,
            process_live=False, interactive_cleanup=cleanup, installed_candidate_runtime_proof=proof)
        process.returncode = 0
        return 0

    process.wait = wait

    async def spawn(*args, **kwargs):
        return process

    def absent(pid, sig):
        assert (pid, sig) == (process.pid, 0)
        raise ProcessLookupError

    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    monkeypatch.setattr(deployment.os, "kill", absent)
    deployment.DeploymentService._launch(service, directory, "install")
    task = service.tasks[directory.name]
    (outcome,) = await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 5)
    assert not service.tasks and not service.processes
    return outcome


def inventory_for(host):
    inventory = deepcopy(host._inventory)
    actual = {"schema": 1, "mob_id": inventory["mob_id"],
        "helper_identity": inventory["helper_identity"], "realm_id": "opaque-inert-realm",
        "members": [{"identity": row["identity"], "member": {
            "mob_id": inventory["mob_id"], "role": row["profile"],
            "member": "inert-original-native-" + str(index)}}
            for index, row in enumerate(inventory["roster"])]}

    async def inventory_command(argv, phase, timeout, *, raw_outcome):
        assert argv[1] == "--lower-application-inventory"
        return {"returncode": 0, "stdout": encoded(actual), "stderr": b"inert inventory boundary\n"}

    return inventory_command


async def test_retried_install_after_a_forge_restart_defers_to_continue_setup(registry, monkeypatch):
    """The saved setup from the original Install reconnects through Continue setup.

    The retried Install's completion retains nothing new and journals no
    refusal; setup status offers Continue setup, which rebuilds the owner.
    """
    f = registry
    monkeypatch.setattr(f.service.service_manager, "command", unavailable_service_query)
    saved = new_guide(f)
    installed = await build(f, saved)
    await accept_source_fixture(f, installed)
    directory, destination = await install_fixture(f, saved, installed, monkeypatch)
    assert (destination / ".mobkit/forge-local-setup/local-application-policy.json").exists()
    f.guide_id, f.directory, f.destination = saved["id"], directory, destination
    await f.implementation.shutdown()
    restarted = reloaded(f, monkeypatch)
    try:
        outcome = await relaunch_install(restarted.service, directory, monkeypatch)
        receipt = deployment.read_receipt(directory)
        assert NOT_RETAINED not in (receipt.get("detail") or ""), receipt.get("detail")
        assert outcome is None, outcome
        assert receipt["status"] == "stopped" and receipt["phase"] == "configure"
        status = await restarted.registry.setup_status(restarted.service, saved["id"], directory.name)
        assert status["reason"] == "owner_reconnect" and status["action"] == "prepare-setup", status
    finally:
        await restarted.implementation.shutdown()


async def test_retried_install_after_a_forge_restart_retains_a_setup_it_never_saved(registry, monkeypatch):
    """An Install that never retained its setup recovers its Build owner, as Install does."""
    f = registry
    monkeypatch.setattr(f.service.service_manager, "command", unavailable_service_query)
    saved = new_guide(f)
    installed = await build(f, saved)
    await accept_source_fixture(f, installed)
    # The original process stopped before its Install completion retained anything.
    retention = f.service.application_setup_retention
    f.service.application_setup_retention = None
    directory, destination = await install_fixture(f, saved, installed, monkeypatch)
    f.service.application_setup_retention = retention
    assert not (destination / ".mobkit/forge-local-setup").exists()
    f.guide_id, f.directory, f.destination = saved["id"], directory, destination
    await f.implementation.shutdown()
    restarted = reloaded(f, monkeypatch)
    # This retried completion is the Install retainer's first run.
    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", RETAIN)
    try:
        outcome = await relaunch_install(restarted.service, directory, monkeypatch)
        receipt = deployment.read_receipt(directory)
        assert NOT_RETAINED not in (receipt.get("detail") or ""), receipt.get("detail")
        assert outcome is None, outcome
        assert (destination / ".mobkit/forge-local-setup/local-application-policy.json").exists()
        host = restarted.registry.selected_host(saved["id"], directory.name)
        assert host is not None and host._installation_supervisor is not None
        owner = host._preparation_inputs.__self__
        inputs = owner.preparation_inputs(saved["id"], directory.name)
        monkeypatch.setattr(inputs.supervisor, "command", inventory_for(host))
        prepared = await restarted.service.action(saved["id"], directory.name, "prepare-setup",
            action_request_id=str(uuid4()))
        assert prepared["action_request"]["outcome"] == "returned", prepared["action_request"]
        assert json.loads(owner.startup_selection(saved["id"], directory.name).read())["kind"] == "fresh"
    finally:
        await restarted.implementation.shutdown()


RETAIN = LocalApplicationPreparationOwner.retain
