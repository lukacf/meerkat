"""Restore service/source custody with existing inert bootstrap and manager fixtures.

No SDK, native gateway, application command, OS manager or cloud runs. The
completed bundle bytes below are custody specimens, not qualified native facts.
"""
import asyncio
from contextlib import asynccontextmanager
from copy import deepcopy
import importlib
import inspect
import json
import os
from pathlib import Path
import plistlib
import sys
import uuid

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit import deployment_service as services, deployment_worker as worker
from launchkit.connect_application_startup import RestoredApplicationStartup
from launchkit.connect_installer_caller import CustodyError, ProductGatewayFile
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment_application_bootstrap import write, no_runtime_operations
from test_deployment_service import Manager, owned
from test_forge_application_host import selected, startup
from test_meta_toolkit_initial_bootstrap import product_bootstrap
from test_meta_toolkit_initial_install import initial, implementation_service, product, no_product_execution, no_processes
from test_product_installed_bootstrap import installed_bootstrap, _completed_output_specimen, _forbid_producer_reads
from test_product_destination_install import installation, selected_host
from test_product_destination_owner import destination_source
from test_product_artifact_transfer import custody as artifact_custody


@pytest.fixture
async def completed_fresh(product_bootstrap, monkeypatch, tmp_path):
    f = product_bootstrap
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-service-home"))
    Path.home().mkdir()
    # Enter the actual original Fresh factory. It retains its real durable entry
    # without invoking its native provision/startup methods or any application.
    f.fresh_pin, _ = f.admit("fresh")
    factory = bootstrap.BootstrapFactory(f.directory, f.fresh_pin)
    with project_lock(f.destination) as lease:
        async with factory(lease):
            assert (f.directory / bootstrap.FRESH_ENTRY_NAME).is_file()
    f.entry_bytes = (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()
    # Supply related completed-file specimens at that exact original owner's
    # selected output paths. This does not claim native completion/qualification.
    bundle = Path(f.fresh["bundle_path"])
    bundle.mkdir(mode=0o700)
    f.activation = write(Path(f.fresh["activation_path"]), {"synthetic": "original completed activation"})
    f.local_policy = write(bundle / "policy.json", {"synthetic": "original compiled policy"})
    f.setup = write(bundle / "setup.json", {"synthetic": "original completed setup"})
    f.manifest = write(bundle / "manifest.json", {"artifacts": [
        {"path": f.local_policy.path.name, "sha256": f.local_policy.sha256},
        {"path": f.setup.path.name, "sha256": f.setup.sha256}]})
    (f.destination / "current").unlink()
    (f.destination / "current").symlink_to(f.activation.path.name)
    f.selection = write(f.selection.path, {"deployment_root": str(f.destination),
        "activation": f.activation.pin(), "policy_manifest": f.manifest.pin(),
        "local_policy": f.local_policy.pin()})
    f.restore.update(selection_file=f.selection.pin(), completed_setup=f.setup.pin())
    return f


@pytest.fixture
async def restored_service(completed_fresh):
    f = completed_fresh
    f.pin, _ = f.admit("restore")
    f.original_admission = bootstrap.read_bootstrap_admission(f.directory, f.pin)
    f.original_admission_bytes = Path(f.pin["path"]).read_bytes()
    f.service = services.prepare_service(f.directory, deployment.read_receipt(f.directory),
        bootstrap_selection=f.pin)
    return f


def publish_lifetime(f, monkeypatch, pid=4567):
    monkeypatch.setattr(os, "getppid", lambda: pid)
    deployment.update_receipt(f.directory, service_supervisor_pid=pid,
        service_instance_id=str(uuid.uuid4()), service_cleanup_confirmed=False,
        status="starting", phase="service")


async def test_real_fresh_history_remains_readable_and_restore_service_uses_original_source(restored_service, monkeypatch):
    f = restored_service
    assert bootstrap.read_bootstrap_admission(f.directory, f.pin) == f.original_admission
    assert Path(f.pin["path"]).read_bytes() == f.original_admission_bytes
    assert bootstrap.read_bootstrap_admission(f.directory, f.fresh_pin)["application_startup"]["kind"] == "fresh"
    bootstrap.require_bootstrap_history_settled(f.directory, f.pin)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes
    plan = services.service_plan(f.receipt["deployment_id"], f.directory)
    plan.update(bootstrap_selection=f.pin, interpreter=f.service["interpreter"])
    assert services.private_file(f.service["unit_path"]) == services.unit_bytes(plan)
    if f.service["manager"] == "launchd":
        args = plistlib.loads(services.private_file(f.service["unit_path"]))["ProgramArguments"]
        assert args[-2:] == [f.pin["path"], f.pin["sha256"]]
    else:
        assert f.pin["path"].encode() in services.private_file(f.service["unit_path"])
        assert f.pin["sha256"].encode() in services.private_file(f.service["unit_path"])
    publish_lifetime(f, monkeypatch)
    manager = Manager()
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    factory = bootstrap.BootstrapFactory(f.directory, f.pin)
    with project_lock(f.destination) as lease:
        async with factory(lease) as owner:
            assert isinstance(owner, RestoredApplicationStartup)
            assert type(owner.gateway) is ProductGatewayFile
            assert owner.setup == f.setup and owner.selection_file == f.selection
            factory.require_current()
            # Actual read-only status projection changes only public summary,
            # not the retained service/source/lifetime authority.
            status = await services.background_status(f.directory, manager)
            assert not status["process_live"] and status["console_url"] is None
            await factory.source.require_current_async()
            factory.require_current()
        assert factory.source._closed
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


@pytest.mark.parametrize("entry", ["observe", "child", "new-admission"])
def test_service_never_replays_fresh_before_import_or_native_effect(restored_service, monkeypatch, entry):
    f = restored_service
    def forbidden(*args, **kwargs):
        pytest.fail("A service Fresh selection reached an SDK or authored import")
    monkeypatch.setattr(importlib, "import_module", forbidden)
    with pytest.raises((CustodyError, ValueError), match="Restore admission|frozen Restore service"):
        if entry == "observe": bootstrap.observe_bootstrap_admission(f.directory, f.fresh_pin)
        elif entry == "child": bootstrap.run_child(f.directory, f.fresh_pin["path"], f.fresh_pin["sha256"])
        else: bootstrap.retain_bootstrap_admission(f.directory, deployment.read_receipt(f.directory), f.fresh)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


@pytest.mark.parametrize("change", ["pin", "unit", "host", "source", "prefix", "runtime", "private"])
def test_service_refuses_changed_original_selection_and_frozen_source(restored_service, change):
    f = restored_service
    receipt = deployment.read_receipt(f.directory)
    if change == "pin":
        receipt["service"]["bootstrap_selection"] = f.fresh_pin
        deployment.write_private(f.directory / "receipt.json", receipt)
    elif change == "unit": write(Path(f.service["unit_path"]), b"foreign owned unit\n")
    elif change == "host": write(f.directory / "service-host/launcher.py", b"foreign launcher\n")
    elif change == "source": write(f.destination / "application/tool-policy.json", b"{}\n", 0o644)
    elif change == "prefix":
        path = f.gateway.path.parent.parent / "libexec/meerkat-connect/resources.json"
        path.chmod(0o644); path.write_bytes(b"changed selected resource"); path.chmod(0o444)
    elif change == "private": write(f.setup.path, {"synthetic": "changed completed input"})
    else:
        receipt["runtime_candidate"]["assembly"]["sha256"] = "f" * 64
        deployment.write_private(f.directory / "receipt.json", receipt)
    with pytest.raises((CustodyError, DraftConflict, ValueError, OSError)):
        bootstrap.observe_bootstrap_admission(f.directory, f.pin)


@pytest.mark.parametrize("change", ["pid", "instance", "cleanup"])
def test_child_retains_original_service_lifetime(restored_service, monkeypatch, change):
    f = restored_service
    publish_lifetime(f, monkeypatch)
    factory = bootstrap.BootstrapFactory(f.directory, f.pin)
    values = {"pid": {"service_supervisor_pid": 4568},
        "instance": {"service_instance_id": str(uuid.uuid4())},
        "cleanup": {"service_cleanup_confirmed": True}}
    deployment.update_receipt(f.directory, **values[change])
    with pytest.raises(CustodyError, match="service.*lifetime|lifetime.*changed"):
        factory.require_current()


def inert_restore_worker(f, monkeypatch):
    """Reuse the real worker/factory/source boundary with only process work inert."""
    monkeypatch.setattr(os, "getppid", os.getpid)
    async def owned(service): return {"loaded": True, "pid": os.getpid(), "state": "running"}
    monkeypatch.setattr(services.UserServiceManager, "observe", lambda self, service: owned(service))
    # Only the process execution boundary is inert. Run the actual service
    # worker, retained BootstrapFactory, project lease and accepted source.
    calls = []
    class InertSupervisor:
        # The real service-mode entry reads the budget the manager enforces from the receipt.
        own_service = worker.Supervisor.own_service
        def __init__(self, directory):
            self.directory = directory
            self.receipt = deployment.read_receipt(directory)
            self.destination = Path(self.receipt["destination"])
            self.stop = asyncio.Event()
        async def start(self, definition, *, on_ready=None, bootstrap_selection=None):
            assert bootstrap_selection == f.pin
            factory = bootstrap.BootstrapFactory(self.directory, bootstrap_selection)
            with project_lock(self.destination) as lease:
                async with factory(lease) as owner:
                    assert isinstance(owner, RestoredApplicationStartup)
                    calls.append(("restore", owner.selection_file.pin(), self.receipt["service_instance_id"]))
        async def terminate(self): calls.append(("cleanup", self.receipt["service_instance_id"]))
        def redact(self, text): return text
    monkeypatch.setattr(worker, "Supervisor", InertSupervisor)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    return calls


async def test_wrong_actual_manager_pid_refuses_before_supervisor_or_sdk(restored_service, monkeypatch):
    f = restored_service
    original = (f.directory / "receipt.json").read_bytes()
    async def wrong(service): return {"loaded": True, "pid": os.getpid() + 1, "state": "running"}
    monkeypatch.setattr(services.UserServiceManager, "observe", lambda self, service: wrong(service))
    monkeypatch.setattr(worker, "Supervisor", lambda *args: pytest.fail("Wrong manager PID reached Supervisor"))
    with pytest.raises(ValueError, match="recorded operating-system service"):
        await worker.run(f.directory, "service", bootstrap_selection=f.pin)
    assert (f.directory / "receipt.json").read_bytes() == original


async def test_fixed_service_worker_reuses_restore_and_joins_original_cleanup(restored_service, monkeypatch):
    f = restored_service
    calls = inert_restore_worker(f, monkeypatch)
    assert await worker.run(f.directory, "service", bootstrap_selection=f.pin) is None
    receipt = deployment.read_receipt(f.directory)
    assert receipt["service_cleanup_confirmed"] is True and receipt["status"] == "stopped"
    assert receipt["service_supervisor_pid"] == os.getpid()
    assert calls == [("restore", f.selection.pin(), receipt["service_instance_id"]),
        ("cleanup", receipt["service_instance_id"])]
    first_instance = receipt["service_instance_id"]
    assert await worker.run(f.directory, "service", bootstrap_selection=f.pin) is None
    next_receipt = deployment.read_receipt(f.directory)
    assert next_receipt["service_instance_id"] != first_instance
    assert next_receipt["service_cleanup_confirmed"] is True
    assert calls[-2:] == [("restore", f.selection.pin(), next_receipt["service_instance_id"]),
        ("cleanup", next_receipt["service_instance_id"])]
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


async def test_automatic_restore_cannot_replace_unconfirmed_original_instance(restored_service, monkeypatch):
    f = restored_service
    deployment.update_receipt(f.directory, service_supervisor_pid=os.getpid() + 1,
        service_instance_id=str(uuid.uuid4()), service_cleanup_confirmed=False)
    original = (f.directory / "receipt.json").read_bytes()
    async def owned(service): return {"loaded": True, "pid": os.getpid(), "state": "running"}
    monkeypatch.setattr(services.UserServiceManager, "observe", lambda self, service: owned(service))
    monkeypatch.setattr(worker, "Supervisor", lambda *args: pytest.fail("Unconfirmed old instance reached Supervisor"))
    with pytest.raises(DraftConflict, match="has not confirmed cleanup"):
        await worker.run(f.directory, "service", bootstrap_selection=f.pin)
    assert (f.directory / "receipt.json").read_bytes() == original


async def test_ordinary_service_unit_keeps_existing_entry_and_no_restore_pin(owned):
    service, manager, directory, prepared = owned
    receipt = deployment.read_receipt(directory)
    plan = services.service_plan(receipt["deployment_id"], directory)
    actual = services.prepare_service(directory, deployment.read_receipt(directory))
    assert "bootstrap_selection" not in actual
    assert services.private_file(actual["unit_path"]) == services.unit_bytes(plan)
    assert services.verify_service(directory, deployment.read_receipt(directory)) == actual


@pytest.fixture
async def service_owner(completed_fresh, initial, monkeypatch):
    f = completed_fresh
    f.owner = initial.service
    f.manager = Manager()
    f.manager.cleanup = f.directory
    f.owner.service_manager = f.manager
    async def status(guide_id, **kwargs):
        assert guide_id == f.receipt["guide_id"]
        return await services.background_status(f.directory, f.manager)
    monkeypatch.setattr(f.owner, "status", status)
    return f


async def test_trusted_stopped_owner_handoff_and_existing_stop_preserve_exact_pin(service_owner):
    f = service_owner
    result = await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=f.restore, require_owner_current=lambda: None)
    assert result["execution_mode"] == "background" and result["status"] == "starting"
    assert not result["process_live"] and result["console_url"] is None
    service = deployment.read_receipt(f.directory)["service"]
    selected = bootstrap.read_bootstrap_admission(f.directory, service["bootstrap_selection"])
    assert selected["application_startup"] == f.restore
    unit = services.private_file(service["unit_path"])
    stopped = await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "stop")
    assert stopped["status"] == "stopped"
    assert f.manager.calls.count(("start", service["label"])) == 1
    assert f.manager.calls.count(("stop", service["label"])) == 1
    assert services.private_file(service["unit_path"]) == unit
    assert services.service_binding(stopped["service"]) == services.service_binding(service)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


@pytest.fixture
async def historical_service_owner(installed_bootstrap, monkeypatch):
    """Actual installed intake A with completed-file specimens, current bundle B."""
    from launchkit import product_installed_source as installed, remote_interface_bundle as bundles
    f = installed_bootstrap
    monkeypatch.setattr(sys, "platform", "linux")
    # This original byte installation has no native lifetime to reconcile. The
    # imported child fixture's unrelated test lifetime is not native evidence.
    deployment.update_receipt(f.directory, interactive_lifecycle_id=None,
        interactive_cleanup=None, interactive_cleanup_attempt_id=None)
    with project_lock(f.selected.destination) as lease:
        async with installed.open_installed_product_source(f.directory,
                project_lease=lease, require_current=lease.require_current,
                owner_source=f.selected.source, application_startup=f.bootstrap.fresh) as source:
            factory = bootstrap.BootstrapFactory(f.directory, f.pin, source_custody=source)
            async with factory(lease):
                pass
    f.entry_bytes = (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()
    _completed_output_specimen(f)
    f.restore = f.bootstrap.restore
    f.manager = Manager()
    f.manager.cleanup = f.directory
    f.owner = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge",
        None, None, service_manager=f.manager)
    f.current_bundle = f.selected.source + b"\n# Different current trusted owner bundle B.\n"
    monkeypatch.setattr(bundles, "product_destination_owner_bytes", lambda: f.current_bundle)
    _forbid_producer_reads(monkeypatch, f)
    monkeypatch.setattr(deployment, "require_available_console_port", lambda *args: None)
    async def status(guide_id, **kwargs):
        assert guide_id == f.selected.context["guide_id"]
        return deployment.read_receipt(f.directory)
    monkeypatch.setattr(f.owner, "status", status)
    return f


async def _start_historical_service(f, monkeypatch, *, normal=False):
    from launchkit import product_installed_source as installed
    original_open = installed.open_installed_product_source
    f.opened = []
    task, loop = asyncio.current_task(), asyncio.get_running_loop()
    calls = []

    def current():
        assert asyncio.get_running_loop() is loop and asyncio.current_task() is task
        calls.append(task)

    @asynccontextmanager
    async def opened(*args, **kwargs):
        async with original_open(*args, **kwargs) as source:
            assert source is not None and source._owner_source == f.selected.source
            f.opened.append((source, kwargs["project_lease"]))
            yield source

    original_start = f.manager.start
    async def start(service):
        assert f.opened and all(source._closed and lease._closed for source, lease in f.opened)
        with project_lock(f.selected.destination) as child_lease:
            child_lease.require_current()
        return await original_start(service)
    monkeypatch.setattr(installed, "open_installed_product_source", opened)
    monkeypatch.setattr(f.manager, "start", start)
    gid, did = f.selected.context["guide_id"], f.selected.context["deployment_id"]
    if normal:
        # Real original host and closed carrier are retained data selection,
        # never a reentrant lease or a fake current installed-source owner.
        assert f.host._product_source_custody is f.parent_source and f.parent_source._closed
        def selected(guide_id, deployment_id):
            current()
            assert (guide_id, deployment_id) == (gid, did)
            return deepcopy(f.restore)
        def host(guide_id, deployment_id):
            current()
            assert (guide_id, deployment_id) == (gid, did)
            return f.host
        f.owner.application_setup_selection = selected
        f.owner.application_host_selection = host
        result = await f.owner.action(gid, did, "start-background", action_request_id=str(uuid.uuid4()))
        assert result["action_request"]["outcome"] == "returned"
    else:
        result = await f.owner.start_restored_service(gid, did, application_startup=f.restore,
            require_owner_current=current, owner_source=f.selected.source)
    assert calls and all(source._closed and lease._closed for source, lease in f.opened)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes
    return result


@pytest.mark.parametrize("normal", [False, True])
async def test_historical_source_service_admission_retains_original_digest_and_closes_parent(
        historical_service_owner, monkeypatch, normal):
    f = historical_service_owner
    result = await _start_historical_service(f, monkeypatch, normal=normal)
    assert result["execution_mode"] == "background" and result["status"] == "starting"
    service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
    selected = bootstrap.read_bootstrap_admission(f.directory, service["bootstrap_selection"])
    assert selected["application_startup"] == f.restore
    sha = services.digest(f.selected.source)
    launcher = services.private_file(f.directory / "service-host/launcher.py")
    assert sha.encode() in launcher and services.digest(f.current_bundle).encode() not in launcher
    assert f.selected.source not in launcher and len(launcher) < 4096
    assert services.retained_bootstrap_owner_source_sha256(f.directory, service) == sha
    assert f.manager.calls.count(("start", service["label"])) == 1


async def test_historical_service_refresh_renderer_preserves_authenticated_original_literal(
        historical_service_owner, monkeypatch):
    from launchkit import deployment_supervisor as refresh
    f = historical_service_owner
    await _start_historical_service(f, monkeypatch)
    service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
    before_receipt = (f.directory / "receipt.json").read_bytes()
    before_unit = services.private_file(service["unit_path"])
    launcher = services.private_file(f.directory / "service-host/launcher.py")
    current = refresh._trusted_files(f.directory, service)
    assert current["launcher.py"] == launcher
    assert current["launchkit/deployment_service.py"] == Path(services.__file__).read_bytes()
    assert services.digest(f.selected.source).encode() in current["launcher.py"]
    assert services.digest(f.current_bundle).encode() not in current["launcher.py"]
    assert (f.directory / "receipt.json").read_bytes() == before_receipt
    assert services.private_file(service["unit_path"]) == before_unit


async def test_historical_service_normal_restart_reuses_original_launcher_and_restore(
        historical_service_owner, monkeypatch):
    f = historical_service_owner
    await _start_historical_service(f, monkeypatch, normal=True)
    service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
    before_unit = services.private_file(service["unit_path"])
    before_launcher = services.private_file(f.directory / "service-host/launcher.py")
    before_pin = deepcopy(service["bootstrap_selection"])
    gid, did = f.selected.context["guide_id"], f.selected.context["deployment_id"]
    result = await f.owner.action(gid, did, "restart", action_request_id=str(uuid.uuid4()))
    assert result["action_request"]["outcome"] == "returned" and result["status"] == "starting"
    assert f.manager.calls.count(("start", service["label"])) == 2
    assert f.manager.calls.count(("stop", service["label"])) == 1
    assert all(source._closed and lease._closed for source, lease in f.opened)
    assert services.private_file(service["unit_path"]) == before_unit
    assert services.private_file(f.directory / "service-host/launcher.py") == before_launcher
    assert services.verify_service(f.directory, deployment.read_receipt(f.directory))["bootstrap_selection"] == before_pin
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


async def test_historical_service_restart_refuses_changed_retained_owner_data_before_start(
        historical_service_owner, monkeypatch):
    f = historical_service_owner
    await _start_historical_service(f, monkeypatch)
    service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
    path = f.store / "source-owner.py"
    path.write_bytes(path.read_bytes() + b"\n# Changed retained data, never execute.\n")
    before_launcher = services.private_file(f.directory / "service-host/launcher.py")
    gid, did = f.selected.context["guide_id"], f.selected.context["deployment_id"]
    with pytest.raises(CustodyError, match="trusted launcher's original selection"):
        await f.owner.action(gid, did, "restart", action_request_id=str(uuid.uuid4()))
    assert f.manager.calls.count(("start", service["label"])) == 1
    assert deployment.read_receipt(f.directory)["status"] == "stopped"
    assert services.private_file(f.directory / "service-host/launcher.py") == before_launcher
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


@pytest.mark.parametrize("operation,change,root_mode", [("initial", None, 0o700), ("restart", None, 0o700),
    ("initial", "source", 0o700), ("restart", "source", 0o700),
    ("initial", "intake", 0o700), ("restart", "prefix", 0o700)])
async def test_live_service_lease_allows_post_manager_file_fences_and_refuses_source_drift(
        historical_service_owner, monkeypatch, operation, change, root_mode):
    f = historical_service_owner
    f.selected.destination.chmod(root_mode)
    if operation == "restart":
        await _start_historical_service(f, monkeypatch)
    original_start, original_stop = f.manager.start, f.manager.stop
    held = []
    scopes = []
    def release():
        if scopes:
            scopes.pop().__exit__(None, None, None)
    async def start(service):
        result = await original_start(service)
        # The real child's app.main holds this exact nonreentrant lease for
        # runtime and cleanup. No native/OS process is needed to model that
        # actual manager handoff boundary and catch a competing parent lease.
        scope = project_lock(f.selected.destination)
        lease = scope.__enter__()
        scopes.append(scope)
        held.append(lease)
        lease.require_current()
        if change == "source":
            write(f.selected.destination / "application/tool-policy.json", b"{}\n", 0o644)
        elif change == "intake":
            path = f.store / "source-owner.py"
            path.write_bytes(path.read_bytes() + b"\n# Changed while live, never execute.\n")
        elif change == "prefix":
            path = f.bootstrap.gateway.path
            mode = path.stat().st_mode & 0o777
            path.chmod(0o700); path.write_bytes(b"changed selected gateway\n"); path.chmod(mode)
        return result
    async def stop(service):
        # Same original inert child-lifetime cleanup releases its actual lease.
        if scopes:
            assert held and not held[-1]._closed
            held[-1].require_current()
            release()
        return await original_stop(service)
    monkeypatch.setattr(f.manager, "start", start)
    monkeypatch.setattr(f.manager, "stop", stop)
    gid, did = f.selected.context["guide_id"], f.selected.context["deployment_id"]
    async def invoke():
        if operation == "initial":
            return await _start_historical_service(f, monkeypatch)
        return await f.owner.action(gid, did, "restart", action_request_id=str(uuid.uuid4()))
    try:
        if change is None:
            result = await invoke()
            assert result["status"] == "starting"
            assert len(held) == 1 and not held[0]._closed
            held[0].require_current()
            assert f.selected.destination.stat().st_mode & 0o777 == root_mode
            with pytest.raises(RuntimeError, match="already running"):
                with project_lock(f.selected.destination):
                    pytest.fail("A running child lease was silently treated as reentrant")
        else:
            with pytest.raises((CustodyError, DraftConflict, ValueError), match="changed|differs|source"):
                await invoke()
            assert len(held) == 1 and held[0]._closed
            assert deployment.read_receipt(f.directory)["status"] == "stopped"
        assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes
    finally:
        release()


@pytest.mark.parametrize("operation", ["initial", "restart"])
async def test_nonprivate_native_publication_root_refuses_service_start_and_restart(
        historical_service_owner, monkeypatch, operation):
    f = historical_service_owner
    root = f.selected.destination
    assert root.stat().st_mode & 0o777 == 0o700
    service = None
    if operation == "restart":
        # Establish the supported original service before changing its root.
        # The completed-file specimens and manager remain inert boundaries.
        result = await _start_historical_service(f, monkeypatch)
        assert result["status"] == "starting"
        service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
        assert f.manager.calls.count(("start", service["label"])) == 1
        unit = services.private_file(service["unit_path"])
        launcher = services.private_file(f.directory / "service-host/launcher.py")
        admission = services.private_file(service["bootstrap_selection"]["path"])
        original_binding = services.service_binding(service)

    paths = {root / name for name in f.selected.files}
    paths.update({root / ".env", f.directory / "package.zip", f.directory / bootstrap.FRESH_ENTRY_NAME,
        f.store / "owner.json", f.store / "source-owner.py",
        Path(f.restore["preparation_plan"]["path"]), Path(f.restore["selection_file"]["path"]),
        Path(f.restore["completed_setup"]["path"]), Path(f.bootstrap.fresh["activation_path"])})
    paths.update(Path(f.bootstrap.fresh["bundle_path"]).iterdir())
    original_files = {path: (path.read_bytes(), path.stat().st_mode & 0o777) for path in paths}
    original_current = os.readlink(root / "current")
    original_receipt = (f.directory / "receipt.json").read_bytes()
    starts = sum(call[0] == "start" for call in f.manager.calls)
    stops = sum(call[0] == "stop" for call in f.manager.calls)
    root.chmod(0o755)
    request_id = str(uuid.uuid4())
    with pytest.raises(CustodyError,
            match=r"The caller's existing parent directory is not private and owned\."):
        if operation == "initial":
            await _start_historical_service(f, monkeypatch)
        else:
            await f.owner.action(f.selected.context["guide_id"], f.selected.context["deployment_id"],
                "restart", action_request_id=request_id)

    # Project/source ownership does not waive the original native output
    # parent's private-publication requirement, or authorize automatic chmod.
    assert root.stat().st_mode & 0o777 == 0o755
    assert {path: (path.read_bytes(), path.stat().st_mode & 0o777) for path in paths} == original_files
    assert os.readlink(root / "current") == original_current
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes
    assert sum(call[0] == "start" for call in f.manager.calls) == starts
    assert f.owner.tasks == {} and f.owner.processes == {}
    if operation == "initial":
        assert (f.directory / "receipt.json").read_bytes() == original_receipt
        assert not (f.directory / "service-host").exists()
        assert sum(call[0] == "stop" for call in f.manager.calls) == stops
    else:
        receipt = deployment.read_receipt(f.directory)
        assert receipt["status"] == "stopped" and receipt["service_cleanup_confirmed"] is True
        assert sum(call[0] == "stop" for call in f.manager.calls) == stops + 1
        assert services.service_binding(services.verify_service(f.directory, receipt)) == original_binding
        assert services.private_file(service["unit_path"]) == unit
        assert services.private_file(f.directory / "service-host/launcher.py") == launcher
        assert services.private_file(service["bootstrap_selection"]["path"]) == admission
        request = f.owner._read_action_request(f.owner._action_request_path(request_id))
        assert request["phase"] == "settled" and request["outcome"] == "error"
    with project_lock(root) as lease:
        lease.require_current()


def test_frozen_service_launcher_runs_original_owner_selection_before_any_runtime(
        historical_service_owner, monkeypatch):
    f = historical_service_owner
    asyncio.run(_start_historical_service(f, monkeypatch))
    service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
    original_run = worker.run
    selected_sha = services.digest(f.selected.source)
    dispatches, runtime_boundaries = [], []
    async def dispatch(directory, mode, **kwargs):
        assert kwargs["bootstrap_owner_source_sha256"] == selected_sha
        assert kwargs["bootstrap_selection"] == service["bootstrap_selection"]
        dispatches.append((directory, mode, kwargs))
        return await original_run(directory, mode, **kwargs)
    async def observation(self, selected):
        return {"loaded": True, "pid": os.getpid(), "state": "running"}
    async def start(supervisor, definition, *, on_ready=None, bootstrap_selection=None):
        assert supervisor._bootstrap_owner_source == f.selected.source
        # The real worker has already authenticated A twice. Reopen its actual
        # retained byte selection here instead of executing SDK/native code.
        async with bootstrap.installed_bootstrap_source(supervisor.directory, bootstrap_selection,
                require_current=supervisor.require_child_slot,
                owner_source=supervisor._bootstrap_owner_source) as source:
            assert source is not None and source._owner_source == f.selected.source
            assert source.origin == f.parent_source.origin and source.relocated == f.parent_source.relocated
            runtime_boundaries.append(source)
        assert source._closed
        supervisor.stop.set()
    monkeypatch.setattr(worker, "run", dispatch)
    monkeypatch.setattr(worker.Supervisor, "start", start)
    monkeypatch.setattr(services.UserServiceManager, "observe", observation)
    original_loop = asyncio.events.new_event_loop
    def loop():
        result = original_loop()
        monkeypatch.setattr(result, "add_signal_handler", lambda *args: None)
        return result
    monkeypatch.setattr(asyncio.events, "new_event_loop", loop)
    monkeypatch.setattr(bootstrap.importlib, "import_module",
        lambda *args: pytest.fail("Service custody controls cannot import installed SDK or authored code"))
    launcher_path = f.directory / "service-host/launcher.py"
    launcher = services.private_file(launcher_path)
    monkeypatch.setattr(sys, "argv", [str(launcher_path), str(f.directory),
        service["bootstrap_selection"]["path"], service["bootstrap_selection"]["sha256"]])
    before_path, before_environment = list(sys.path), dict(os.environ)
    try:
        with pytest.raises(SystemExit) as result:
            exec(compile(launcher, str(launcher_path), "exec"), {"__file__": str(launcher_path)})
        assert result.value.code == 0
    finally:
        sys.path[:] = before_path
        os.environ.clear(); os.environ.update(before_environment)
    assert len(dispatches) == len(runtime_boundaries) == 1
    receipt = deployment.read_receipt(f.directory)
    assert receipt["service_cleanup_confirmed"] is True and receipt["status"] == "stopped"
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


@pytest.mark.parametrize("selected", [None, "foreign"])
async def test_historical_service_worker_cannot_replace_original_selection_with_current_bundle(
        historical_service_owner, monkeypatch, selected):
    f = historical_service_owner
    await _start_historical_service(f, monkeypatch)
    service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
    before = (f.directory / "receipt.json").read_bytes()
    monkeypatch.setattr(worker, "Supervisor", lambda *args: pytest.fail("Lost owner selection reached Supervisor"))
    monkeypatch.setattr(services.UserServiceManager, "observe",
        lambda *args: pytest.fail("Lost owner selection reached OS manager"))
    options = ({"bootstrap_owner_source_sha256": services.digest(f.current_bundle)}
        if selected == "foreign" else {})
    with pytest.raises((CustodyError, ValueError), match="frozen destination owner changed|trusted launcher's original selection"):
        await worker.run(f.directory, "service", bootstrap_selection=service["bootstrap_selection"], **options)
    assert (f.directory / "receipt.json").read_bytes() == before


@pytest.mark.parametrize("change", ["duplicate", "computed", "unbound"])
async def test_refresh_refuses_ambiguous_owner_literal_in_manifest_checked_launcher(
        historical_service_owner, monkeypatch, change):
    from launchkit import deployment_supervisor as refresh
    f = historical_service_owner
    await _start_historical_service(f, monkeypatch)
    service = services.verify_service(f.directory, deployment.read_receipt(f.directory))
    path = f.directory / "service-host/launcher.py"
    launcher = services.private_file(path)
    slot = b"BOOTSTRAP_OWNER_SOURCE_SHA256 = " + repr(services.digest(f.selected.source)).encode()
    if change == "duplicate":
        changed = launcher + slot + b"\n"
    elif change == "computed":
        changed = launcher.replace(slot, slot + b" + ''")
    else:
        changed = launcher.replace(b"bootstrap_owner_source_sha256=BOOTSTRAP_OWNER_SOURCE_SHA256",
            b"bootstrap_owner_source_sha256='" + services.digest(f.selected.source).encode() + b"'")
    write(path, changed)
    service["host_files"]["launcher.py"] = services.digest(changed)
    deployment.update_receipt(f.directory, service=service)
    # The ordinary complete host reader succeeds. The actual fixed literal
    # reader must still reject a convenient hash extracted from another shape.
    assert services.verify_service(f.directory, deployment.read_receipt(f.directory)) == service
    with pytest.raises(DraftConflict, match="exact original owner-source literal"):
        refresh._trusted_files(f.directory, service)


async def test_service_owner_digest_without_original_bootstrap_is_refused_before_reads(tmp_path, monkeypatch):
    monkeypatch.setattr(worker, "read_receipt", lambda *args: pytest.fail("Orphan digest read an installation"))
    with pytest.raises(ValueError, match="source owner requires its selected bootstrap"):
        await worker.run(tmp_path, "service", bootstrap_owner_source_sha256="a" * 64)


async def test_ordinary_service_snapshot_keeps_original_no_literal_launcher(owned):
    from launchkit import deployment_supervisor as refresh
    _, _, directory, service = owned
    before_receipt = (directory / "receipt.json").read_bytes()
    before_unit = services.private_file(service["unit_path"])
    launcher = services.private_file(directory / "service-host/launcher.py")
    assert b"BOOTSTRAP_OWNER_SOURCE_SHA256" not in launcher
    assert b"bootstrap_owner_source_sha256" not in launcher
    assert launcher.splitlines()[-1] == (
        b"sys.exit(asyncio.run(run(pathlib.Path(sys.argv[1]), 'service', bootstrap_selection=selection)) or 0)")
    assert services.retained_bootstrap_owner_source_sha256(directory, service) is None
    assert refresh._trusted_files(directory, service)["launcher.py"] == launcher
    assert (directory / "receipt.json").read_bytes() == before_receipt
    assert services.private_file(service["unit_path"]) == before_unit


@pytest.mark.parametrize("change", ["selector", "host", "owner-bytes", "context"])
async def test_normal_service_action_refuses_changed_original_host_source_before_publication(
        historical_service_owner, monkeypatch, change):
    f = historical_service_owner
    gid, did = f.selected.context["guide_id"], f.selected.context["deployment_id"]
    f.owner.application_setup_selection = lambda *args: deepcopy(f.restore)
    selected_host = f.host
    def select(*args):
        return selected_host
    f.owner.application_host_selection = select
    async def capability():
        nonlocal selected_host
        if change == "selector":
            f.owner.application_host_selection = lambda *args: f.host
        elif change == "host":
            selected_host = None
        elif change == "owner-bytes":
            f.parent_source._owner_source = f.current_bundle
        else:
            f.parent_source._context["build_id"] = str(uuid.uuid4())
        return {"available": True}
    monkeypatch.setattr(f.manager, "capability", capability)
    with pytest.raises(DraftConflict, match="service host source"):
        await f.owner.action(gid, did, "start-background", action_request_id=str(uuid.uuid4()))
    assert not (f.directory / "service-host").exists()
    assert not any(call[0] == "start" for call in f.manager.calls)
    assert deployment.read_receipt(f.directory)["execution_mode"] == "interactive"


async def test_normal_service_action_joins_selected_original_context_to_actual_installed_owner(
        historical_service_owner):
    f = historical_service_owner
    gid, did = f.selected.context["guide_id"], f.selected.context["deployment_id"]
    f.owner.application_setup_selection = lambda *args: deepcopy(f.restore)
    f.owner.application_host_selection = lambda *args: f.host
    # Even the right original bundle A is shared by many installation owners.
    # Its digest cannot substitute a foreign retained source context.
    f.parent_source._context["deployment_id"] = str(uuid.uuid4())
    with pytest.raises(DraftConflict, match="host source differs from this installed owner"):
        await f.owner.action(gid, did, "start-background", action_request_id=str(uuid.uuid4()))
    assert not (f.directory / "service-host").exists()
    assert not any(call[0] == "start" for call in f.manager.calls)


async def test_normal_stop_restart_reaches_restore_worker_with_confirmed_previous_cleanup(service_owner, monkeypatch):
    f = service_owner
    await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=f.restore, require_owner_current=lambda: None)
    service = deployment.read_receipt(f.directory)["service"]
    f.pin = service["bootstrap_selection"]
    unit = services.private_file(service["unit_path"])
    calls = inert_restore_worker(f, monkeypatch)
    assert await worker.run(f.directory, "service", bootstrap_selection=f.pin) is None
    previous = deployment.read_receipt(f.directory)
    f.manager.observation = {"loaded": False, "pid": None, "state": "stopped"}
    stop_id, restart_id = str(uuid.uuid4()), str(uuid.uuid4())
    stopped = await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "stop",
        action_request_id=stop_id)
    assert stopped["service_cleanup_confirmed"] is True
    assert stopped["action_request"]["outcome"] == "returned"
    start = f.manager.start

    async def start_worker(plan):
        # This is the actual dispatcher-to-worker boundary. It must still carry
        # the old positively confirmed lifetime until worker.run publishes anew.
        before = deployment.read_receipt(f.directory)
        assert before["service_cleanup_confirmed"] is True
        assert before["service_instance_id"] == previous["service_instance_id"]
        assert before["service_supervisor_pid"] == previous["service_supervisor_pid"]
        await start(plan)
        assert await worker.run(f.directory, "service", bootstrap_selection=f.pin) is None
        f.manager.observation = {"loaded": False, "pid": None, "state": "stopped"}
        return deepcopy(f.manager.observation)

    f.manager.start = start_worker
    restarted = await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "restart",
        action_request_id=restart_id)
    assert restarted["status"] == "stopped" and restarted["console_url"] is None
    assert restarted["action_request"]["phase"] == "settled"
    assert restarted["action_request"]["outcome"] == "returned"
    assert restarted["service_cleanup_confirmed"] is True
    assert restarted["service_instance_id"] != previous["service_instance_id"]
    assert calls[-2:] == [("restore", f.selection.pin(), restarted["service_instance_id"]),
        ("cleanup", restarted["service_instance_id"])]
    assert f.manager.calls.count(("start", service["label"])) == 2
    assert f.manager.calls.count(("stop", service["label"])) == 2
    assert services.private_file(service["unit_path"]) == unit
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


async def test_cancelled_normal_restart_drains_admitted_manager_then_stops_same_lifetime(service_owner):
    f = service_owner
    await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=f.restore, require_owner_current=lambda: None)
    service = deployment.read_receipt(f.directory)["service"]
    previous, successor = str(uuid.uuid4()), str(uuid.uuid4())
    deployment.update_receipt(f.directory, service_supervisor_pid=4567,
        service_instance_id=previous, service_cleanup_confirmed=True, status="stopped")
    f.manager.observation = {"loaded": False, "pid": None, "state": "stopped"}
    entered, cancelled, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
    start = f.manager.start

    async def admitted(plan):
        before = deployment.read_receipt(f.directory)
        assert before["service_cleanup_confirmed"] is True and before["service_instance_id"] == previous
        await start(plan)
        # Inert manager admission publishes the same worker lifetime fields;
        # no native child or application is executed in this cancellation case.
        deployment.update_receipt(f.directory, service_supervisor_pid=4567,
            service_instance_id=successor, service_cleanup_confirmed=False, status="starting")
        entered.set()
        try:
            await release.wait()
        except asyncio.CancelledError:
            cancelled.set()
            await release.wait()
            raise

    f.manager.start = admitted
    request_id = str(uuid.uuid4())
    task = asyncio.create_task(f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "restart",
        action_request_id=request_id))
    await entered.wait()
    task.cancel()
    await cancelled.wait()
    assert not task.done() and f.owner.lock.locked()
    assert f.manager.calls.count(("stop", service["label"])) == 1
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["service_cleanup_confirmed"] is True
    assert receipt["service_instance_id"] == successor and receipt["console_url"] is None
    assert f.manager.calls.count(("stop", service["label"])) == 2
    assert not f.owner.lock.locked()
    exact = await f.owner.action_request(f.receipt["guide_id"], f.receipt["deployment_id"], request_id)
    assert exact["action_request"]["phase"] == "pending" and exact["action_request"]["outcome"] is None
    calls = list(f.manager.calls)
    duplicate = await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "restart",
        action_request_id=request_id)
    assert duplicate["action_request"] == exact["action_request"] and f.manager.calls == calls
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.entry_bytes


async def test_normal_restart_manager_failure_joins_original_stop_and_retains_error(service_owner):
    f = service_owner
    await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=f.restore, require_owner_current=lambda: None)
    service = deployment.read_receipt(f.directory)["service"]
    failure = OSError("inert Restore restart manager failure after admission")
    start = f.manager.start

    async def failed(plan):
        await start(plan)
        raise failure

    f.manager.start = failed
    request_id = str(uuid.uuid4())
    with pytest.raises(OSError) as caught:
        await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "restart",
            action_request_id=request_id)
    assert caught.value is failure
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["console_url"] is None
    assert f.manager.calls.count(("stop", service["label"])) == 2
    exact = await f.owner.action_request(f.receipt["guide_id"], f.receipt["deployment_id"], request_id)
    assert exact["action_request"]["phase"] == "settled" and exact["action_request"]["outcome"] == "error"
    assert not f.owner.lock.locked()


async def test_normal_restart_post_manager_source_change_joins_stop(service_owner):
    f = service_owner
    await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=f.restore, require_owner_current=lambda: None)
    service = deployment.read_receipt(f.directory)["service"]
    start = f.manager.start

    async def changed(plan):
        result = await start(plan)
        write(f.destination / "application/tool-policy.json", b"{}\n", 0o644)
        return result

    f.manager.start = changed
    with pytest.raises((CustodyError, DraftConflict)):
        await f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "restart",
            action_request_id=str(uuid.uuid4()))
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["console_url"] is None
    assert f.manager.calls.count(("stop", service["label"])) == 2
    assert not f.owner.lock.locked()


@pytest.mark.parametrize("kind", ["false", "awaitable"])
async def test_mistaken_owner_callback_refuses_before_service_creation(service_owner, kind):
    f = service_owner
    retained = []
    async def never_run():
        pytest.fail("An asynchronous owner currentness callback was executed")
    def current():
        if kind == "false": return False
        result = never_run()
        retained.append(result)
        return result
    with pytest.raises(DraftConflict, match="successful synchronous"):
        await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
            application_startup=f.restore, require_owner_current=current)
    assert not (f.directory / "service-host").exists() and not f.manager.calls
    assert all(inspect.getcoroutinestate(result) == inspect.CORO_CLOSED for result in retained)


@pytest.mark.parametrize("change", ["source", "selection"])
async def test_supplier_or_source_drift_during_capability_refuses_before_unit(service_owner, change):
    f = service_owner
    original_package = (f.directory / "package.zip").read_bytes()
    changed = False
    async def capability():
        nonlocal changed
        changed = True
        if change == "source": write(f.destination / "application/tool-policy.json", b"{}\n", 0o644)
        return {"available": True}
    f.manager.capability = capability
    def current():
        if changed and change == "selection": raise CustodyError("Original retained supplier changed")
    with pytest.raises((CustodyError, DraftConflict)):
        await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
            application_startup=f.restore, require_owner_current=current)
    assert not (f.directory / "service-host").exists()
    assert not any(call[0] == "start" for call in f.manager.calls)
    assert (f.directory / "package.zip").read_bytes() == original_package


async def test_cancelled_manager_admission_is_joined_then_original_unit_stopped(service_owner):
    f = service_owner
    entered, cancelled, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
    async def suspended(service):
        entered.set()
        try:
            await release.wait()
        except asyncio.CancelledError:
            cancelled.set()
            await release.wait()
            raise
    f.manager.start = suspended
    task = asyncio.create_task(f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=f.restore, require_owner_current=lambda: None))
    await entered.wait()
    task.cancel()
    await cancelled.wait()
    assert not task.done()
    release.set()
    with pytest.raises(asyncio.CancelledError): await task
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped"
    assert f.manager.calls.count(("start", receipt["service"]["label"])) == 0
    assert f.manager.calls.count(("stop", receipt["service"]["label"])) == 1
    assert not f.owner.lock.locked()


async def test_manager_failure_joins_original_stop_and_keeps_original_error(service_owner):
    f = service_owner
    failure = OSError("inert original manager admission failure")
    async def failed(service):
        f.manager.calls.append(("start", service["label"]))
        raise failure
    f.manager.start = failed
    with pytest.raises(OSError) as caught:
        await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
            application_startup=f.restore, require_owner_current=lambda: None)
    assert caught.value is failure
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped"
    assert f.manager.calls.count(("stop", receipt["service"]["label"])) == 1
    assert not f.owner.lock.locked()


async def test_later_stop_waits_for_original_manager_handoff_then_stops_same_unit(service_owner):
    f = service_owner
    entered, release = asyncio.Event(), asyncio.Event()
    original_start = f.manager.start
    async def suspended(service):
        entered.set()
        await release.wait()
        return await original_start(service)
    f.manager.start = suspended
    admission = asyncio.create_task(f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
        application_startup=f.restore, require_owner_current=lambda: None))
    await entered.wait()
    stopping = asyncio.create_task(f.owner.action(f.receipt["guide_id"], f.receipt["deployment_id"], "stop",
        action_request_id=str(uuid.uuid4())))
    await asyncio.sleep(0)
    assert not stopping.done() and not any(row[0] == "stop" for row in f.manager.calls)
    release.set()
    admitted = await admission
    stopped = await stopping
    assert admitted["status"] == "starting" and stopped["status"] == "stopped"
    service = deployment.read_receipt(f.directory)["service"]
    assert [row for row in f.manager.calls if row[0] in {"start", "stop"}] == [
        ("start", service["label"]), ("stop", service["label"])]


async def test_post_manager_owner_change_joins_stop_without_claiming_running(service_owner):
    f = service_owner
    original_start = f.manager.start
    selected = True
    async def changed(service):
        nonlocal selected
        result = await original_start(service)
        selected = False
        return result
    f.manager.start = changed
    with pytest.raises(DraftConflict, match="successful synchronous"):
        await f.owner.start_restored_service(f.receipt["guide_id"], f.receipt["deployment_id"],
            application_startup=f.restore, require_owner_current=lambda: selected)
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["console_url"] is None
    assert f.manager.calls.count(("stop", receipt["service"]["label"])) == 1


@pytest.mark.parametrize("kind", ["fresh", "restore"])
async def test_host_service_entry_uses_original_supplier_and_does_not_prepare(selected, kind):
    expected = startup(selected, kind)
    calls = []
    class Owner:
        async def start_restored_service(self, guide, deployment_id, **kwargs):
            kwargs["require_owner_current"]()
            calls.append((guide, deployment_id, kwargs["application_startup"]))
            return {"status": "starting"}
    if kind == "fresh":
        with pytest.raises(CustodyError, match="Restore"):
            await selected.host.start_service(Owner(), "original-guide", "original-installation")
        assert not calls
    else:
        assert await selected.host.start_service(Owner(), "original-guide", "original-installation") == {"status": "starting"}
        assert calls == [("original-guide", "original-installation", expected)]
