"""Actual destination composition with explicitly inert external boundaries.

The complete source/artifact intake, canonical Install, real host/private input
retainer, preparation readers and action journal remain production code. The
reused fixture poisons SDK/application bytes and replaces only venv/pip/check
effects. Inventory stdout is a synthetic retained native specimen. Start is
admitted by production code but its process launch and final status projection
are inert. None of these controls qualifies a runtime or a GCP installation.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys
import threading
from types import ModuleType, SimpleNamespace
from uuid import UUID, uuid4

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap, deployment_worker, guide_store
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.destination_application_factory import DestinationApplicationFactory
from launchkit.forge_application_host import ApplicationPreparationInputs, ForgeApplicationHost
from launchkit.guide import GuideService
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_forge_application_host import declarations, encoded, put
from test_product_destination_install import (installation, destination_source,
    artifact_custody, product)
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution
from test_product_installed_bootstrap import _completed_output_specimen


@pytest.fixture
def destination(installation, monkeypatch):
    f = installation
    selected = f.selected
    # The inert destination is the selected Linux guest. The actual local
    # target predicate must observe that guest, just like the artifact reader.
    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setattr(Path, "home", lambda: selected.custody.home)
    f.store = GuideStore(selected.custody.home / ".local/state/meta-forge/guides")
    with monkeypatch.context() as patch:
        patch.setattr(guide_store, "uuid4", lambda: UUID(selected.context["guide_id"]))
        f.saved = f.store.create({"proposal": deepcopy(f.spec), "version": 3,
            "messages": [], "status": "refining", "stage": "purpose"})
    f.guide = GuideService(f.store, None)
    f.implementation = ImplementationService(selected.custody.home / ".local/state/meta-forge", f.guide)
    f.service = deployment.DeploymentService(selected.custody.home / ".local/state/meta-forge",
        f.guide, f.implementation)
    f.held, f.retains, f.inventory_calls, f.starts = None, [], [], []
    f.root = selected.destination
    f.inventory = ExactFile(Path(json.loads(f.acquisition.read())["original_inventory"]["path"]),
        json.loads(f.acquisition.read())["original_inventory"]["sha256"])
    f.definition = ExactFile(Path(json.loads(f.acquisition.read())["definition"]["path"]),
        json.loads(f.acquisition.read())["definition"]["sha256"])
    f.private = selected.destination / ".mobkit/preparation"
    f.plan_path, f.selection_path, f.inventory_attempt = (
        f.private / name for name in ("plan.json", "fresh.json", "inventory.json"))

    def current():
        assert asyncio.current_task() is f.parent
        assert asyncio.get_running_loop() is f.loop
        if not f.current:
            raise CustodyError("Original destination worker changed.")

    def provider(gid, did):
        assert (gid, did) == (selected.context["guide_id"], selected.context["deployment_id"])
        return f.held

    def startup(gid, did):
        assert (gid, did) == (selected.context["guide_id"], selected.context["deployment_id"])
        return ExactFile(f.selection_path, hashlib.sha256(f.selection_path.read_bytes()).hexdigest())

    def retain(gid, did, supervisor):
        current()
        assert f.service.lock.locked() and f.held is None
        assert type(supervisor) is deployment_worker.Supervisor
        assert supervisor is not f.factory.host._destination_installation_supervisor
        assert supervisor._product_source_custody is f.factory._source
        f.retains.append(supervisor)
        f.private.mkdir(mode=0o700)
        work = f.private / "work"
        work.mkdir(mode=0o700)
        template = put(f.private / "activation-template.json", {"original": "inert-native-template"})
        original = json.loads(f.inventory.read())
        f.actual = {"schema": 1, "mob_id": original["mob_id"],
            "helper_identity": original["helper_identity"], "realm_id": "inert-original-realm",
            "members": [{"identity": row["identity"], "member": {"mob_id": original["mob_id"],
                "role": row["profile"], "member": "inert-member-" + str(i)}}
                for i, row in enumerate(original["roster"])]}

        def choose(inventory):
            current()
            original_input, actual = inventory.authenticate()
            assert original_input == original and actual == f.actual
            chosen = declarations(f, actual)
            chosen["deployment_root"] = selected.destination
            chosen["realm_storage"] = {"context_root": str(selected.destination),
                "state_root": str(selected.destination / ".mobkit/state")}
            chosen["output"] = work / "original-native-output.json"
            return chosen

        f.held = ApplicationPreparationInputs(supervisor=supervisor, declarations=choose,
            inventory_attempt_path=f.inventory_attempt, preparation_plan_path=f.plan_path,
            startup_selection_path=f.selection_path, activation_template=template,
            fresh_paths={"provision_attempt_path": str(f.private / "provision.json"),
                "working_directory": str(work), "bundle_path": str(selected.destination / "original-bundle"),
                "activation_path": str(selected.destination / "original-activation.json")}, command_timeout=20)
        return f.held

    previous_initialize = deployment_worker.Supervisor.__init__

    def initialize(supervisor, *args, **kwargs):
        previous_initialize(supervisor, *args, **kwargs)
        f.host = f.factory.host
        f.source = supervisor._product_source_custody

    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", initialize)

    def configure(**changes):
        import threading
        f.parent, f.loop, f.thread = asyncio.current_task(), asyncio.get_running_loop(), threading.get_ident()
        f.factory = DestinationApplicationFactory(deployment=f.service, context=selected.context,
            owner_source=selected.source, acquisition_selection=f.acquisition, viewer_selection=f.viewer,
            require_current=current, startup_selection=startup, preparation_inputs=provider,
            retain_preparation_inputs=retain, **changes)
        f.factory.bind()
        return f.factory

    async def inventory_command(argv, phase, timeout, *, raw_outcome):
        current()
        assert raw_outcome and f.service.lock.locked()
        assert f.held.supervisor._product_source_custody is f.factory._source
        with pytest.raises(RuntimeError, match="running"):
            with project_lock(selected.destination):
                pytest.fail("Inventory lost its actual destination project lease.")
        f.inventory_calls.append(tuple(argv))
        return {"returncode": 0, "stdout": encoded(f.actual), "stderr": b"inert inventory stderr\n"}

    f.configure, f.inventory_command = configure, inventory_command
    return f


def accepted_source(f):
    return {name: (f.selected.destination / name).read_bytes() for name in f.selected.files}


async def install(f):
    f.configure()
    result = await f.factory.install(settings=f.settings)
    assert f.factory._source is None and f.factory.host._product_source_custody._closed
    assert f.factory.host._installation_supervisor is f.held.supervisor
    assert f.retains == [f.held.supervisor]
    with project_lock(f.selected.destination):
        pass
    return result


async def prepare(f, monkeypatch):
    await install(f)
    monkeypatch.setattr(f.held.supervisor, "command", f.inventory_command)
    request = str(uuid4())
    result = await f.service.action(f.factory.guide_id, f.factory.deployment_id,
        "prepare-setup", action_request_id=request)
    assert result["action_request"]["phase"] == "settled" and result["action_request"]["outcome"] == "returned"
    return request


async def test_destination_factory_installs_one_host_and_read_only_setup_reopens_real_lease(destination):
    f = destination
    before = accepted_source(f)
    installer = await install(f)
    host, supervisor, inputs = f.factory.host, f.held.supervisor, f.held
    assert type(host) is ForgeApplicationHost and installer is host._destination_installation_supervisor
    assert installer is not supervisor and f.supervisors == [installer, supervisor]
    assert deployment.read_receipt(f.directory)["status"] == "stopped"
    after = accepted_source(f), (f.selected.destination / ".env").read_bytes(), f.held.activation_template.read()
    for _ in range(2):
        async with asyncio.timeout(20):
            status = await f.factory.setup_status(f.service, f.factory.guide_id, f.factory.deployment_id)
        assert status["available"] and status["action"] == "prepare-setup"
        assert f.factory.host is host and f.held is inputs and host._installation_supervisor is supervisor
        assert supervisor._product_source_custody._closed
    assert after == (accepted_source(f), (f.selected.destination / ".env").read_bytes(), f.held.activation_template.read())
    assert accepted_source(f) == before and not f.inventory_calls
    assert not f.inventory_attempt.exists() and not f.plan_path.exists() and not f.selection_path.exists()


async def retain_restore_file_specimen(f, monkeypatch):
    """Original entered Fresh lineage and exact files, never native completion."""
    fresh = json.loads(f.factory._startup_pin.read())
    selected = SimpleNamespace(fresh=fresh, gateway=ExactFile(Path(fresh["gateway"]["path"]),
        fresh["gateway"]["sha256"]), plan=ExactFile(Path(fresh["preparation_plan"]["path"]),
        fresh["preparation_plan"]["sha256"]),
        selection=put(f.private / "inert-original-native-selection.json", {}),
        restore={"kind": "restore"})
    f.bootstrap = selected
    remote = ModuleType("remote_operations")
    remote.__file__ = str(f.root / "remote_operations.py")
    remote.command = lambda *args, **kwargs: pytest.fail("Restore file custody must not execute a native command")
    monkeypatch.setitem(sys.modules, "remote_operations", remote)
    async with f.service.lock:
        async with f.factory._operation(installed=True):
            source = f.factory._source
            pin, _ = bootstrap.retain_bootstrap_admission(f.directory,
                deployment.read_receipt(f.directory), fresh, source_custody=source)
            factory = bootstrap.BootstrapFactory(f.directory, pin, source_custody=source)
            async with factory(source._lease) as startup:
                assert startup.runtime is None
                _completed_output_specimen(f)
                source.require_current()
            restore = put(f.private / "inert-retained-restore.json", selected.restore)
            f.selection_path = restore.path
            f.factory._retain_startup_pin()
    assert f.factory._source is None
    assert json.loads(f.factory._startup_pin.read())["kind"] == "restore"


@pytest.mark.parametrize("manager_changed", [False, True], ids=["same-owner", "owner-drift"])
async def test_destination_restore_readback_borrows_original_lock_source_and_capability_owner(
        destination, monkeypatch, manager_changed):
    f = destination
    await prepare(f, monkeypatch)
    await retain_restore_file_specimen(f, monkeypatch)
    before = (accepted_source(f), (f.root / ".env").read_bytes(),
        (f.directory / "receipt.json").read_bytes(), f.factory._startup_pin.read(),
        (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes())
    manager, calls = f.service.service_manager, []
    from launchkit import product_runtime_custody
    original_validation = product_runtime_custody.await_product_validation
    original_proof = product_runtime_custody.require_product_installation
    proof_reads = []

    async def validate(source, function, *args, **kwargs):
        if function is not original_proof:
            return await original_validation(source, function, *args, **kwargs)
        assert asyncio.current_task() is f.parent and asyncio.get_running_loop() is f.loop
        assert source is f.factory._source

        def read(*values, **options):
            assert threading.get_ident() != f.thread
            with pytest.raises(RuntimeError):
                asyncio.get_running_loop()
            proof_reads.append(source)
            return function(*values, **options)

        return await original_validation(source, read, *args, **kwargs)

    monkeypatch.setattr(product_runtime_custody, "await_product_validation", validate)

    async def capability():
        assert asyncio.current_task() is f.parent and asyncio.get_running_loop() is f.loop
        assert f.service.lock.locked() and f.factory._source is not None
        assert f.factory._source is f.factory.host._product_source_custody
        f.factory._source._lease.require_current()
        with pytest.raises(RuntimeError, match="running"):
            with project_lock(f.root):
                pytest.fail("Restore capability released the original readback project lease")
        calls.append(manager)
        if manager_changed:
            # A different genuine manager object is a refusal, not a second
            # capability/Start operation. No manager is operated in this test.
            f.service.service_manager = type(manager)()
        await asyncio.sleep(0)
        return {"available": True, "persistence": "Inert observed manager persistence."}

    monkeypatch.setattr(manager, "capability", capability)
    async with asyncio.timeout(30):
        status = await f.factory.setup_status(f.service, f.factory.guide_id, f.factory.deployment_id)
    assert status["available"] and status["kind"] == "restore"
    assert status["background"]["available"] is (not manager_changed)
    if not manager_changed:
        assert status["background"]["persistence"] == "Inert observed manager persistence."
    assert calls == [manager] and len(f.inventory_calls) == 1 and not f.starts
    assert len(proof_reads) == 1 and proof_reads[0]._closed
    assert f.factory._source is None and f.held.supervisor._product_source_custody._closed
    assert before == (accepted_source(f), (f.root / ".env").read_bytes(),
        (f.directory / "receipt.json").read_bytes(), f.factory._startup_pin.read(),
        (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes())


async def test_destination_prepare_uses_original_inputs_and_uuid_replay_does_not_reopen_source(destination, monkeypatch):
    f = destination
    request = await prepare(f, monkeypatch)
    pin = f.factory._startup_pin
    assert type(pin) is ExactFile and json.loads(pin.read())["kind"] == "fresh"
    assert len(f.inventory_calls) == 1 and f.retains == [f.held.supervisor]
    assert f.factory.host._installation_supervisor is f.held.supervisor
    assert f.held.supervisor._product_source_custody._closed
    assert not Path(f.held.fresh_paths["provision_attempt_path"]).exists()
    before = pin.read(), f.inventory_attempt.read_bytes(), f.plan_path.read_bytes()
    def no_operation(*args, **kwargs):
        pytest.fail("An existing action UUID cannot select another source operation.")
    monkeypatch.setattr(f.factory, "_operation", no_operation)
    duplicate = await f.service.action(f.factory.guide_id, f.factory.deployment_id,
        "prepare-setup", action_request_id=request)
    assert duplicate["action_request"]["phase"] == "settled" and duplicate["action_request"]["outcome"] == "returned"
    assert before == (pin.read(), f.inventory_attempt.read_bytes(), f.plan_path.read_bytes())
    assert len(f.inventory_calls) == 1 and not f.starts


async def test_destination_finish_closes_parent_source_before_real_start_admission(destination, monkeypatch):
    f = destination
    await prepare(f, monkeypatch)
    expected = json.loads(f.factory._startup_pin.read())
    def launch(directory, mode, **kwargs):
        assert directory == f.directory and mode == "start"
        assert f.factory._source is None and f.factory.host._product_source_custody._closed
        with project_lock(f.selected.destination):
            pass
        f.starts.append(kwargs)
    async def observed_status(gid, **kwargs):
        # No child runs. This replaces only the post-admission UI projection.
        return deployment.read_receipt(f.directory)
    monkeypatch.setattr(f.service, "_launch", launch)
    monkeypatch.setattr(f.service, "status", observed_status)
    request = str(uuid4())
    result = await f.service.action(f.factory.guide_id, f.factory.deployment_id,
        "finish-setup", action_request_id=request)
    assert result["action_request"]["phase"] == "settled" and result["action_request"]["outcome"] == "returned"
    assert result["status"] == "queued"
    assert len(f.starts) == 1 and f.starts[0]["bootstrap_owner_source"] == f.selected.source
    from launchkit.deployment_application_bootstrap import read_bootstrap_admission
    admitted = read_bootstrap_admission(f.directory, f.starts[0]["bootstrap_selection"])
    assert admitted["application_startup"] == expected
    assert len(f.inventory_calls) == 1 and not Path(expected["provision_attempt_path"]).exists()
    duplicate = await f.service.action(f.factory.guide_id, f.factory.deployment_id,
        "finish-setup", action_request_id=request)
    assert duplicate["action_request"]["phase"] == "settled" and duplicate["action_request"]["outcome"] == "returned"
    assert len(f.starts) == 1


async def test_background_after_closed_continuation_selects_original_pin_then_refuses_fresh(destination, monkeypatch):
    f = destination
    await prepare(f, monkeypatch)
    pin, host, supervisor = f.factory._startup_pin, f.factory.host, f.held.supervisor
    original = pin.read()
    expected = json.loads(original)
    assert f.factory._source is None and host._product_source_custody._closed
    # This closed read is data only. Normal service admission must still refuse
    # Fresh, rather than mistake an ended source lease for startup authority.
    assert f.factory._selected_startup(f.factory.guide_id, f.factory.deployment_id) == expected
    async def no_service_admission(*args, **kwargs):
        pytest.fail("Fresh cannot reach background service source admission or manager start.")
    monkeypatch.setattr(f.service, "_start_restored_service_locked", no_service_admission)
    request = str(uuid4())
    with pytest.raises(DraftConflict,
            match="Background startup requires completed Restore inputs; Fresh is never replayed"):
        await f.service.action(f.factory.guide_id, f.factory.deployment_id,
            "start-background", action_request_id=request)
    recorded = f.service._read_action_request(f.service._action_request_path(request))
    assert recorded["phase"] == "settled" and recorded["outcome"] == "error"
    assert f.factory._startup_pin == pin and pin.read() == original
    assert f.factory.host is host and host._installation_supervisor is supervisor
    assert f.factory._source is None and supervisor._product_source_custody._closed
    assert len(f.inventory_calls) == 1 and f.retains == [supervisor] and not f.starts
    assert not Path(expected["provision_attempt_path"]).exists()
    with project_lock(f.selected.destination):
        pass


async def test_closed_startup_data_refuses_changed_original_private_pin_without_new_source(destination, monkeypatch):
    f = destination
    await prepare(f, monkeypatch)
    pin, host, source = f.factory._startup_pin, f.factory.host, f.factory.host._product_source_custody
    assert source._closed and f.factory._source is None
    original = pin.read()
    pin.path.write_bytes(original + b" ")
    with pytest.raises(CustodyError, match="retained caller file no longer matches its selected bytes"):
        f.factory._selected_startup(f.factory.guide_id, f.factory.deployment_id)
    assert f.factory.host is host and f.factory._startup_pin is pin
    assert source._closed and f.factory._source is None and len(f.inventory_calls) == 1
    assert not f.starts and not Path(f.held.fresh_paths["provision_attempt_path"]).exists()


@pytest.mark.parametrize("change", ["source", "private-pin", "owner", "callback"])
async def test_destination_operation_refuses_original_owner_drift_before_inventory(destination, monkeypatch, change):
    f = destination
    await install(f)
    before = (f.selected.destination / ".env").read_bytes(), f.held.activation_template.read()
    if change == "source":
        (f.selected.destination / "assets/empty").write_bytes(b"foreign source")
    elif change == "private-pin":
        f.viewer.path.write_bytes(f.viewer.read() + b" ")
    elif change == "owner":
        f.current = False
    else:
        f.service.application_setup_selection = lambda *args: None
    detail = (r"The retained caller file no longer matches its selected bytes\." if change == "private-pin"
        else "source|Source|changed|selections|callbacks")
    with pytest.raises((CustodyError, ValueError, OSError), match=detail):
        await f.service.action(f.factory.guide_id, f.factory.deployment_id,
            "prepare-setup", action_request_id=str(uuid4()))
    assert not f.inventory_calls and not f.selection_path.exists()
    assert before == ((f.selected.destination / ".env").read_bytes(), f.held.activation_template.read())
    assert f.factory.host._installation_supervisor is f.held.supervisor and f.retains == [f.held.supervisor]


async def test_destination_factory_rejects_foreign_service_before_install_publication(destination):
    f = destination
    f.configure()
    f.factory.deployment = deployment.DeploymentService(f.selected.custody.home / "foreign-state",
        f.guide, f.implementation)
    with pytest.raises(CustodyError, match="original destination factory"):
        await f.factory.install(settings=f.settings)
    assert not f.directory.exists() and not f.supervisors and not f.retains


async def test_destination_uncertain_install_retains_same_source_lease_until_original_cleanup(destination, monkeypatch):
    f = destination
    f.configure()
    child = SimpleNamespace(pid=718223, returncode=None)
    refusal = RuntimeError("inert original teardown remains unsettled")
    drains, calls = (), []
    previous_init = deployment_worker.Supervisor.__init__
    def initialize(supervisor, *args, **kwargs):
        previous_init(supervisor, *args, **kwargs)
        async def cleanup(process, tasks=(), **kwargs):
            assert process is child and tuple(tasks) == drains
            calls.append((process, tuple(tasks)))
            if len(calls) == 1:
                raise refusal
            await asyncio.gather(*tasks)
            child.returncode = 0
            supervisor.child = supervisor._child_admission = None
        supervisor.cleanup_child = cleanup
    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", initialize)
    previous_command = deployment_worker.Supervisor.command
    async def command(supervisor, argv, phase, timeout, **kwargs):
        nonlocal drains
        if phase != "Installed application contract":
            return await previous_command(supervisor, argv, phase, timeout, **kwargs)
        supervisor.child = child
        drains = (asyncio.create_task(asyncio.sleep(0)),)
        await supervisor.cleanup_child(child, drains)
        pytest.fail("A failed app-check cleanup cannot complete Install.")
    async def group_absent(process):
        # Explicit inert OS observation, for this same synthetic child only.
        assert process is child and child.returncode == 0
    monkeypatch.setattr(deployment_worker.Supervisor, "command", command)
    monkeypatch.setattr(deployment, "require_owned_group_absent", group_absent)
    with pytest.raises(RuntimeError) as caught:
        await f.factory.install(settings=f.settings)
    assert caught.value is refusal
    host, source = f.factory.host, f.factory._source
    installer = host._destination_installation_supervisor
    retained = installer._destination_install_cleanup
    assert retained.child is child and retained.drain_tasks == drains and retained.cleanup_task.done()
    assert not source._closed and installer.stop.is_set() and f.factory._pending_scope is not None
    with pytest.raises(RuntimeError, match="running"):
        with project_lock(f.selected.destination):
            pytest.fail("Uncertain original child cleanup released its source lease.")
    with pytest.raises(CustodyError, match="retained"):
        await f.factory.install(settings=f.settings)
    await f.factory.cleanup_installation()
    assert source._closed and f.factory._pending_scope is None and f.factory._source is None
    assert f.factory.host is host and host._destination_installation_supervisor is installer
    assert calls == [(child, drains), (child, drains)] and f.supervisors == [installer] and not f.retains
    with project_lock(f.selected.destination):
        pass
