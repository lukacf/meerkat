"""Source updates after setup through the launcher's per-installation registry.

The real registry selects owners for Stage, Apply, Start, setup status and
Stop, as the trusted launcher does. Build, acceptance, Install, preparation,
the original finalizer and the update worker entry are real; inventory and
native outputs are inert specimens, and no application or provider runs.
"""
from copy import deepcopy
import json
import os
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit import deployment_updates as updates, deployment_worker as worker, runtime_dependencies
from launchkit import forge_application_host
from launchkit.connect_installer_caller import CustodyError
from launchkit.deployment import DeploymentService
from launchkit.guide import GuideService
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.projects import DraftConflict
from launchkit.runtime_selection import selected_runtime_artifacts
from launchkit.templates import install_candidate as adapter
from test_candidate_deployment_updates import packed
from test_forge_application_host import encoded
from test_local_application_prepared_recovery import unavailable_service_query
from test_local_application_registry import (accept_source_fixture, bind_registry, build, install_fixture,
    new_guide, no_processes, no_product_execution, product, registry, workspace)
from test_local_application_restore import finalize_original


INERT_WORKER_PID = 981237


@pytest.fixture
async def set_up(registry, monkeypatch):
    """Install, prepare, Fresh and a normal Stop through the real registry."""
    f = registry
    # The launcher's callbacks, with no replacement supplier: the packaged entry.
    assert f.registry.replacement_selection is None
    assert f.service.application_replacement_selection == f.registry._selected_replacement
    monkeypatch.setattr(f.service.service_manager, "command", unavailable_service_query)
    saved = new_guide(f)
    installed = await build(f, saved)
    await accept_source_fixture(f, installed)
    host = f.registry.host_for_build(saved["id"], installed["build_id"])
    directory, destination = await install_fixture(f, saved, installed, monkeypatch)
    owner = host._preparation_inputs.__self__
    assert type(owner) is LocalApplicationPreparationOwner
    inputs = owner.preparation_inputs(saved["id"], directory.name)
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

    monkeypatch.setattr(inputs.supervisor, "command", inventory_command)
    prepared = await f.service.action(saved["id"], directory.name, "prepare-setup",
        action_request_id=str(uuid4()))
    assert prepared["action_request"]["outcome"] == "returned"
    fresh_pin = owner.startup_selection(saved["id"], directory.name)
    specimen = SimpleNamespace(host=host, owner=owner, inputs=inputs, directory=directory,
        destination=destination, guide_id=saved["id"], setup_root=inputs.startup_selection_path.parent,
        fresh_pin=fresh_pin, fresh=json.loads(fresh_pin.read()), native_commands=[])
    await finalize_original(specimen, monkeypatch)
    stopped = await f.service.action(saved["id"], directory.name, "stop", action_request_id=str(uuid4()))
    assert stopped["action_request"]["outcome"] == "returned"
    restore = owner.startup_selection(saved["id"], directory.name)
    assert json.loads(restore.read())["kind"] == "restore"
    f.saved, f.installed, f.host, f.owner = saved, installed, host, owner
    f.guide_id, f.directory, f.destination, f.restore = saved["id"], directory, destination, restore
    f.entry = (directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()
    return f


async def continuation(f, change=None):
    """An accepted same-source Build that continues the installed one."""
    saved = f.store.get(f.guide_id)
    second = await build(f, saved, "Improve the installed application.", previous=f.installed)
    project = workspace(f.implementation, second)
    project.write_file("application/update_marker.py", "# An accepted change after setup.\n")
    os.chmod(project.project / "application/update_marker.py", 0o600)
    if change is not None:
        change(project)
    await accept_source_fixture(f, second)
    return second


async def stage(f, build_id):
    staged = await f.service.stage_update(f.guide_id, f.directory.name,
        f.store.get(f.guide_id)["version"], build_id)
    return staged["update"]


async def run_worker(f, monkeypatch, mode, selection):
    """The real interactive worker entry; after setup it never starts the app."""
    class SetupWorker(worker.Supervisor):
        checks, starts = [], []

        async def command(self, args, *other, **kwargs):
            SetupWorker.checks.append((deployment.read_receipt(self.directory)["build_id"], args[-1]))

        async def start(self, *args, **kwargs):
            SetupWorker.starts.append(args)
            raise AssertionError("After setup, the update worker never starts the application.")

    async def owned_input(supervisor):
        await supervisor.stop.wait()
        return "stopped"

    lifecycle = str(uuid4())
    deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle,
        interactive_cleanup_attempt_id=None, interactive_cleanup=None)
    with monkeypatch.context() as patch:
        patch.setattr(worker, "Supervisor", SetupWorker)
        patch.setattr(worker, "owner_input", owned_input)
        SetupWorker.result = await deployment.interactive_worker(f.directory, mode, lifecycle,
            selection["path"], selection["sha256"])
    # No worker process runs here; the fixture's inert worker process stands
    # for the one whose exit the fixed wrapper confirms.
    cleanup = deployment.read_receipt(f.directory)["interactive_cleanup"]
    deployment.update_receipt(f.directory, interactive_cleanup={**cleanup,
        "supervisor_pid": INERT_WORKER_PID, "worker_exit_confirmed": True})
    return SetupWorker


async def apply(f, monkeypatch, update):
    """Apply as the browser sends it: with its own action request UUID."""
    f.service.launched.clear()
    applied = await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"],
        action_request_id=str(uuid4()))
    assert applied["action_request"]["outcome"] == "returned", applied["action_request"]
    directory, mode, options = f.service.launched[-1]
    assert (directory, mode) == (f.directory, "update")
    return await run_worker(f, monkeypatch, "update", options["update_selection"])


async def start(f, service=None):
    """The user's next Start: the owner's existing Restore admission."""
    service = f.service if service is None else service
    service.launched.clear()
    started = await service.action(f.guide_id, f.directory.name, "finish-setup",
        action_request_id=str(uuid4()))
    assert started["action_request"]["outcome"] == "returned", started["action_request"]
    directory, mode, options = service.launched[-1]
    assert mode == "start"
    admitted = bootstrap.read_bootstrap_admission(directory, options["bootstrap_selection"])
    assert admitted["application_startup"]["kind"] == "restore"
    # Native execution is outside this fixture; the launched runtime stopped.
    deployment.update_receipt(f.directory, status="stopped")
    return admitted["application_startup"]


async def test_set_up_product_updates_in_place_through_its_retained_owner(set_up, monkeypatch):
    f = set_up
    assert f.registry.selected_host(f.guide_id, f.directory.name) is f.host
    second = await continuation(f)
    candidate_host = f.registry.host_for_build(f.guide_id, second["build_id"])
    assert candidate_host is not f.host
    update = await stage(f, second["build_id"])
    assert update["status"] == "staged" and update["blockers"] == []
    assert update["setup_restore_sha256"] == f.restore.sha256
    worker_type = await apply(f, monkeypatch, update)
    assert worker_type.starts == [] and worker_type.checks == [(second["build_id"], "--application-check")]
    receipt = deployment.read_receipt(f.directory)
    assert receipt["build_id"] == second["build_id"] and receipt["status"] == "stopped"
    assert receipt["update"]["status"] == "completed"
    # The installation stays with the host that holds its setup custody.
    assert f.registry.selected_host(f.guide_id, f.directory.name) is f.host
    assert f.registry.host_for_build(f.guide_id, second["build_id"]) is candidate_host
    status = await f.registry.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["available"] and status["kind"] == "restore"
    assert status["build_id"] == second["build_id"]
    first = await start(f)
    assert first == json.loads(f.restore.read())
    stopped = await f.service.action(f.guide_id, f.directory.name, "stop", action_request_id=str(uuid4()))
    assert stopped["action_request"]["outcome"] == "returned"
    assert f.owner.startup_selection(f.guide_id, f.directory.name) == f.restore
    assert await start(f) == first
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry


def changed_protected_file(f, monkeypatch, build_id):
    """The candidate package carries another protected file than its owner pins.

    A Build workspace cannot author protected files, so this edits the
    accepted package bytes at the Stage boundary, after acceptance.
    """
    original = f.implementation.package

    async def package(guide_id, selected_build_id, *args, **kwargs):
        payload, other = await original(guide_id, selected_build_id, *args, **kwargs)
        if selected_build_id != build_id:
            return payload, other
        files = deployment.package_files(payload)
        content, mode = files["application/configuration.json"]
        configuration = json.loads(content)
        configuration["changed_after_setup"] = True
        files["application/configuration.json"] = (json.dumps(configuration).encode(), mode)
        return packed(files), other

    monkeypatch.setattr(f.implementation, "package", package)


async def test_stage_refuses_a_candidate_that_changes_the_owner_protected_choices(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    changed_protected_file(f, monkeypatch, second["build_id"])
    update = await stage(f, second["build_id"])
    assert update["status"] == "blocked" and updates.SETUP_OWNER_BLOCKER in update["blockers"]
    with pytest.raises(DraftConflict, match="cannot be activated"):
        updates.activate(f.directory, update["update_id"])
    assert deployment.read_receipt(f.directory)["build_id"] == f.installed["build_id"]
    assert f.registry.selected_host(f.guide_id, f.directory.name) is f.host


async def changed_plan_build(f):
    """An accepted Build of a changed plan: its source owner is a new selection."""
    changed = f.store.get(f.guide_id)
    changed["proposal"]["project"]["name"] += " with a changed plan"
    changed = f.store.save(changed, changed["version"])
    fresh = await build(f, changed, "Implement the changed plan.", previous=f.installed)
    await accept_source_fixture(f, fresh)
    return fresh


async def test_stage_refuses_a_candidate_from_a_new_source_selection(set_up, monkeypatch):
    f = set_up
    fresh = await changed_plan_build(f)
    assert f.registry.host_for_build(f.guide_id, fresh["build_id"]) is not f.host
    payload, _ = await f.implementation.package(f.guide_id, fresh["build_id"])
    assert not forge_application_host.owner_choices_match(f.host, deployment.package_files(payload))
    digest = (await f.implementation.status(f.guide_id))["source_digest"]
    assert not f.registry.continues_installed_source(f.guide_id, f.directory.name, fresh["build_id"], digest)
    # The signed source chain refuses on its own, even if the protected
    # choices were equal.
    monkeypatch.setattr(forge_application_host, "owner_choices_match", lambda host, files: True)
    update = await stage(f, fresh["build_id"])
    assert update["status"] == "blocked" and updates.SETUP_OWNER_BLOCKER in update["blockers"]
    assert deployment.read_receipt(f.directory)["build_id"] == f.installed["build_id"]
    assert f.registry.selected_host(f.guide_id, f.directory.name) is f.host


async def test_the_candidate_build_host_never_owns_the_set_up_installation(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    await apply(f, monkeypatch, await stage(f, second["build_id"]))
    candidate_host = f.registry.host_for_build(f.guide_id, second["build_id"])
    assert f.registry.selected_host(f.guide_id, f.directory.name) is f.host
    # Without its retained entry, as in a process that built the candidate
    # but never selected this installation, no build host is chosen.
    key = (f.guide_id, f.directory.name)
    retained = f.registry._installations.pop(key)
    try:
        with pytest.raises(CustodyError, match="needs its retained original host"):
            f.registry.selected_host(f.guide_id, f.directory.name)
        assert key not in f.registry._installations
        hint = f.registry._reconnect_status(f.service, f.guide_id, f.directory.name)
        assert hint["reason"] == "owner_reconnect" and hint["action"] == "prepare-setup"
        assert f.registry.host_for_build(f.guide_id, second["build_id"]) is candidate_host
    finally:
        f.registry._installations[key] = retained


def reloaded(f, monkeypatch):
    """A new Forge process over the same storage, with no retained owners."""
    store = GuideStore(f.store.root)
    guide = GuideService(store, architect=None)
    implementation = ImplementationService(f.implementation.root.parent, guide)
    service = DeploymentService(f.service.root.parent, guide, implementation)
    owner = LocalApplicationRegistry(guide_store=store, implementation=implementation,
        deployment=service, runtime_target=f.target)
    implementation.prepare_workspace = owner.prepare_workspace
    bind_registry(service, owner)
    service.launched = []
    monkeypatch.setattr(service, "_launch",
        lambda directory, mode, **kwargs: service.launched.append((directory, mode, kwargs)))
    monkeypatch.setattr(service.service_manager, "command", unavailable_service_query)
    proof = deepcopy(deployment.read_receipt(f.directory)["installed_candidate_runtime_proof"])

    def verify_installed_prefix(selected_destination, expected_version, *, runtime_target, **kwargs):
        assert Path(selected_destination) == f.destination and runtime_target == f.target
        adapter.verify_meta_toolkit_prefix(selected_runtime_artifacts(f.target), deployment_root=f.destination)
        return deepcopy(proof)

    async def no_native_command(*args, **kwargs):
        pytest.fail("Recovery must not run inventory, native preparation, Fresh or an Install command.")

    def no_install_retainer(*args, **kwargs):
        pytest.fail("Recovery must reopen original private choices without replaying Install retention.")

    monkeypatch.setattr(runtime_dependencies, "verify_runtime", verify_installed_prefix)
    monkeypatch.setattr(worker.Supervisor, "command", no_native_command)
    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", no_install_retainer)
    return SimpleNamespace(store=store, guide=guide, implementation=implementation, service=service,
        registry=owner)


async def test_after_a_forge_restart_the_updated_owner_recovers_through_its_build_chain(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    await apply(f, monkeypatch, await stage(f, second["build_id"]))
    await f.implementation.shutdown()
    restarted = reloaded(f, monkeypatch)
    try:
        assert not restarted.registry._builds and not restarted.registry._installations
        assert await start(f, restarted.service) == json.loads(f.restore.read())
        host = restarted.registry.selected_host(f.guide_id, f.directory.name)
        assert host is not f.host and host._installation_supervisor is not None
        assert restarted.registry._installations[(f.guide_id, f.directory.name)][0] is host
        stopped = await restarted.service.action(f.guide_id, f.directory.name, "stop",
            action_request_id=str(uuid4()))
        assert stopped["action_request"]["outcome"] == "returned"
        assert await start(f, restarted.service) == json.loads(f.restore.read())
    finally:
        await restarted.implementation.shutdown()


async def test_recovery_after_an_update_refuses_a_broken_build_chain(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    await apply(f, monkeypatch, await stage(f, second["build_id"]))
    await f.implementation.shutdown()
    # The installed Build's predecessor lost its independent acceptance.
    original = f.implementation._directory(f.guide_id, f.installed["build_id"])
    (original / "acceptance/acceptance-state.json").unlink()
    restarted = reloaded(f, monkeypatch)
    try:
        restarted.service.launched.clear()
        with pytest.raises((CustodyError, DraftConflict, OSError)):
            await restarted.service.action(f.guide_id, f.directory.name, "finish-setup",
                action_request_id=str(uuid4()))
        assert not restarted.service.launched
        assert (f.guide_id, f.directory.name) not in restarted.registry._installations
    finally:
        await restarted.implementation.shutdown()


async def test_after_a_forge_restart_stage_waits_for_the_reconnected_setup_owner(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    await f.implementation.shutdown()
    restarted = reloaded(f, monkeypatch)
    try:
        version = restarted.store.get(f.guide_id)["version"]
        # A retained candidate build cannot stand in for the setup owner.
        with pytest.raises(DraftConflict) as refused:
            await restarted.service.stage_update(f.guide_id, f.directory.name, version, second["build_id"])
        assert str(refused.value) == updates.SETUP_OWNER_RECONNECT
        assert deployment.read_receipt(f.directory).get("update") is None
        reconnected = await restarted.service.action(f.guide_id, f.directory.name, "prepare-setup",
            action_request_id=str(uuid4()))
        assert reconnected["action_request"]["outcome"] == "returned"
        staged = await restarted.service.stage_update(f.guide_id, f.directory.name, version, second["build_id"])
        assert staged["update"]["status"] == "staged" and staged["update"]["blockers"] == []
        assert staged["update"]["setup_restore_sha256"] == f.restore.sha256
    finally:
        await restarted.implementation.shutdown()


async def test_apply_keeps_refusing_a_redirect_to_another_installation(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    update = await stage(f, second["build_id"])
    key = (f.guide_id, f.directory.name)
    host, binding = f.registry._installations[key]
    # The retained owner anchors another installation folder than this receipt.
    f.registry._installations[key] = (host, {**binding,
        "destination": str(f.destination.parent / "another installation")})
    f.service.launched.clear()
    with pytest.raises(CustodyError, match="cannot be redirected to another build or source owner"):
        await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"],
            action_request_id=str(uuid4()))
    assert not f.service.launched
    receipt = deployment.read_receipt(f.directory)
    assert receipt["build_id"] == f.installed["build_id"] and receipt["update"]["status"] == "staged"


async def test_apply_before_the_first_setup_refuses_before_any_worker(registry, monkeypatch):
    """A product application that has not finished its setup cannot take an update in place.

    Its update worker could not start the application without the setup's
    startup inputs, so Apply refuses with guidance before any worker is
    queued. The installed source and the staged update stay as they were.
    """
    f = registry
    saved = new_guide(f)
    installed = await build(f, saved)
    await accept_source_fixture(f, installed)
    directory, destination = await install_fixture(f, saved, installed, monkeypatch)
    f.saved, f.installed, f.guide_id, f.directory, f.destination = saved, installed, saved["id"], directory, destination
    second = await continuation(f)
    update = await stage(f, second["build_id"])
    assert update["status"] == "staged" and "setup_restore_sha256" not in update
    before = deployment.read_receipt(directory)
    f.service.launched.clear()
    with pytest.raises(CustodyError, match="has not finished its setup, so it cannot take a staged update in place yet"):
        await f.service.action(f.guide_id, directory.name, "apply-update", update["update_id"],
            action_request_id=str(uuid4()))
    assert not f.service.launched
    after = deployment.read_receipt(directory)
    assert {key: after.get(key) for key in ("status", "build_id", "package_sha256", "update", "update_action")} == {
        key: before.get(key) for key in ("status", "build_id", "package_sha256", "update", "update_action")}


async def test_apply_without_an_action_request_refuses_before_selection(set_up, monkeypatch):
    """The launcher's Apply needs the browser's action request UUID, as before."""
    f = set_up
    second = await continuation(f)
    update = await stage(f, second["build_id"])

    def no_selection(*args):
        pytest.fail("Apply without its action request UUID cannot select an owner.")

    monkeypatch.setattr(f.registry, "selected_host", no_selection)
    f.service.launched.clear()
    with pytest.raises(ValueError, match="requires its exact action request UUID"):
        await f.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"])
    assert not f.service.launched
    assert deployment.read_receipt(f.directory)["update"]["status"] == "staged"


async def test_apply_of_a_non_product_application_takes_the_ordinary_update_path(registry, monkeypatch):
    """An installation without a selected product runtime has no setup owner.

    On the launcher's callbacks, its Apply queues the ordinary update worker,
    whose checks and start then complete the update.
    """
    f = registry
    saved = new_guide(f, ordinary=True)
    first = await build(f, saved, ordinary=True)
    await accept_source_fixture(f, first)
    directory, _ = await install_fixture(f, saved, first, monkeypatch, ordinary=True)
    assert f.registry.selected_host(saved["id"], directory.name) is None
    second = await build(f, saved, "Improve the requested local job.", ordinary=True, previous=first)
    await accept_source_fixture(f, second)
    staged = await f.service.stage_update(saved["id"], directory.name, saved["version"], second["build_id"])
    update = staged["update"]
    assert update["status"] == "staged" and update["blockers"] == []
    f.service.launched.clear()
    applied = await f.service.action(saved["id"], directory.name, "apply-update", update["update_id"],
        action_request_id=str(uuid4()))
    assert applied["action_request"]["outcome"] == "returned", applied["action_request"]
    selected_directory, mode, options = f.service.launched[-1]
    assert (selected_directory, mode) == (directory, "update")
    assert "application_startup" not in updates.read_update_admission(directory, "update", options["update_selection"])
    calls = []

    class Ready(worker.Supervisor):
        async def command(self, args, *other, **kwargs):
            calls.append(args[-1])

        async def start(self, definition, *, on_ready):
            calls.append("start")
            on_ready()

    await worker.update_runtime(Ready(directory), "update", update_selection=options["update_selection"])
    receipt = deployment.read_receipt(directory)
    assert calls == ["--doctor", "--application-check", "start"]
    assert receipt["update"]["status"] == "completed" and receipt["build_id"] == second["build_id"]


async def test_after_a_forge_restart_apply_waits_for_the_reconnected_setup_owner(set_up, monkeypatch):
    f = set_up
    second = await continuation(f)
    update = await stage(f, second["build_id"])
    await f.implementation.shutdown()
    restarted = reloaded(f, monkeypatch)
    try:
        keys = ("status", "build_id", "package_sha256", "update", "update_action")
        before = deployment.read_receipt(f.directory)
        with pytest.raises(CustodyError, match="has not reconnected this application since it restarted"):
            await restarted.service.action(f.guide_id, f.directory.name, "apply-update", update["update_id"],
                action_request_id=str(uuid4()))
        after = deployment.read_receipt(f.directory)
        assert not restarted.service.launched
        assert {key: after.get(key) for key in keys} == {key: before.get(key) for key in keys}
        # Continue setup once reconnects the owner; then Apply installs the update.
        reconnected = await restarted.service.action(f.guide_id, f.directory.name, "prepare-setup",
            action_request_id=str(uuid4()))
        assert reconnected["action_request"]["outcome"] == "returned"
        restarted.service.launched.clear()
        applied = await restarted.service.action(f.guide_id, f.directory.name, "apply-update",
            update["update_id"], action_request_id=str(uuid4()))
        assert applied["action_request"]["outcome"] == "returned", applied["action_request"]
        directory, mode, options = restarted.service.launched[-1]
        assert (directory, mode) == (f.directory, "update")
        worker_type = await run_worker(f, monkeypatch, "update", options["update_selection"])
        receipt = deployment.read_receipt(f.directory)
        assert worker_type.starts == [] and receipt["update"]["status"] == "completed"
        assert receipt["build_id"] == second["build_id"]
    finally:
        await restarted.implementation.shutdown()


async def test_after_a_forge_restart_apply_before_the_first_setup_waits_for_the_owner(registry, monkeypatch):
    f = registry
    saved = new_guide(f)
    installed = await build(f, saved)
    await accept_source_fixture(f, installed)
    directory, destination = await install_fixture(f, saved, installed, monkeypatch)
    f.saved, f.installed, f.guide_id, f.directory, f.destination = saved, installed, saved["id"], directory, destination
    second = await continuation(f)
    update = await stage(f, second["build_id"])
    await f.implementation.shutdown()
    restarted = reloaded(f, monkeypatch)
    try:
        keys = ("status", "build_id", "package_sha256", "update", "update_action")
        before = deployment.read_receipt(directory)
        with pytest.raises(CustodyError, match="has not reconnected this application since it restarted"):
            await restarted.service.action(f.guide_id, directory.name, "apply-update", update["update_id"],
                action_request_id=str(uuid4()))
        after = deployment.read_receipt(directory)
        assert not restarted.service.launched
        assert {key: after.get(key) for key in keys} == {key: before.get(key) for key in keys}
    finally:
        await restarted.implementation.shutdown()


async def test_update_staged_before_the_first_setup_keeps_that_setup_available_on_the_registry(registry, monkeypatch):
    """Stage right after Install, then the first setup, through the launcher's registry."""
    f = registry
    monkeypatch.setattr(f.service.service_manager, "command", unavailable_service_query)
    saved = new_guide(f)
    installed = await build(f, saved)
    await accept_source_fixture(f, installed)
    host = f.registry.host_for_build(saved["id"], installed["build_id"])
    directory, destination = await install_fixture(f, saved, installed, monkeypatch)
    f.saved, f.installed, f.guide_id, f.directory, f.destination = saved, installed, saved["id"], directory, destination
    second = await continuation(f)
    update = await stage(f, second["build_id"])
    assert update["status"] == "staged" and "setup_restore_sha256" not in update
    owner = host._preparation_inputs.__self__
    inputs = owner.preparation_inputs(saved["id"], directory.name)
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

    monkeypatch.setattr(inputs.supervisor, "command", inventory_command)
    status = await f.registry.setup_status(f.service, saved["id"], directory.name)
    assert status["available"] and status["action"] == "prepare-setup", status.get("detail")
    prepared = await f.service.action(saved["id"], directory.name, "prepare-setup",
        action_request_id=str(uuid4()))
    assert prepared["action_request"]["outcome"] == "returned", prepared["action_request"]
    fresh_pin = owner.startup_selection(saved["id"], directory.name)
    assert json.loads(fresh_pin.read())["kind"] == "fresh"
    f.service.launched.clear()
    started = await f.service.action(saved["id"], directory.name, "finish-setup", action_request_id=str(uuid4()))
    assert started["action_request"]["outcome"] == "returned", started["action_request"]
    selected, mode, options = f.service.launched[-1]
    assert (selected, mode) == (directory, "start")
    admitted = bootstrap.read_bootstrap_admission(directory, options["bootstrap_selection"])
    assert admitted["application_startup"] == json.loads(fresh_pin.read())
    receipt = deployment.read_receipt(directory)
    assert receipt["build_id"] == installed["build_id"]
    assert (receipt["update"]["update_id"], receipt["update"]["status"]) == (update["update_id"], "staged")
