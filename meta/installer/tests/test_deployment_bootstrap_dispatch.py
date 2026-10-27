"""Trusted Fresh/Restore dispatch with retained source and inert process seams.

The actual service, fixed wrapper and worker boundaries are exercised separately.
Synthetic inputs establish byte custody only, never native startup qualification.
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
import uuid

import pytest

from launchkit import deployment
from launchkit import deployment_application_bootstrap as bootstrap
from launchkit import deployment_worker as worker
from launchkit import deployment_updates as updates
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, submit
from test_deployment_application_bootstrap import CONFIG, POLICY, Inputs, write


@pytest.fixture(autouse=True)
def no_runtime_operations(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Bootstrap dispatch controls cannot execute or signal a process or use a socket.")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"), (os, "system"),
            (os, "kill"), (os, "killpg"), (socket, "create_connection"),
            (asyncio, "create_subprocess_exec"), (asyncio, "create_subprocess_shell"),
            (asyncio, "open_connection"), (asyncio, "start_server")):
        monkeypatch.setattr(owner, name, forbidden)
    available = lambda port: {"scope": "local-console-port-observation",
        "status": "available", "port": port}
    monkeypatch.setattr(deployment, "require_available_console_port", available)
    monkeypatch.setattr(worker, "require_available_console_port", available)


@pytest.fixture
async def owned_inputs(tmp_path, monkeypatch):
    tmp_path.chmod(0o700)
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "b" * 64
    implementation.definition["agents"][0]["profile"] = "helper-source"
    implementation.extra_files = {"config/mob.toml": CONFIG,
        "application/tool-policy.json": POLICY,
        "remote_operations.py": "# inert maintained command source specimen\n"}
    service = deployment.DeploymentService(tmp_path / "private-server", guide, implementation)
    monkeypatch.setattr(service, "_launch", lambda *args, **kwargs: None)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    deployment.materialize(directory)
    receipt = deployment.update_receipt(directory, status="stopped")
    f = Inputs(directory, receipt, destination, monkeypatch)
    f.service, f.guide, f.implementation, f.launches = service, guide, implementation, []

    def captured_launch(path, mode, *, update_selection=None, bootstrap_selection=None):
        f.launches.append((path, mode, deepcopy(update_selection), deepcopy(bootstrap_selection)))

        async def waiting():
            try:
                await asyncio.Event().wait()
            finally:
                service.tasks.pop(path.name, None)

        service.tasks[path.name] = asyncio.create_task(waiting())

    monkeypatch.setattr(service, "_launch", captured_launch)
    try:
        yield f
    finally:
        await service.shutdown()


async def start(f, value=None):
    return await f.service.start_application(f.guide.identity, f.directory.name,
        application_startup=f.fresh if value is None else value)


def replace_identical(path):
    replacement = path.with_name(path.name + ".replacement")
    replacement.write_bytes(path.read_bytes())
    replacement.chmod(path.stat().st_mode & 0o777)
    os.replace(replacement, path)


def assert_not_queued(f):
    assert f.launches == [] and f.service.tasks == {}
    assert deployment.read_receipt(f.directory)["status"] != "queued"
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


@pytest.mark.parametrize("kind", ["fresh", "restore"])
async def test_private_start_queues_exact_original_bootstrap_pins(owned_inputs, kind):
    f = owned_inputs
    value = f.fresh if kind == "fresh" else f.restore
    original = deepcopy(value)
    result = await start(f, value)
    assert len(f.launches) == 1
    directory, mode, update_selection, pin = f.launches[0]
    assert directory == f.directory and mode == "start" and update_selection is None
    selected = bootstrap.read_bootstrap_admission(directory, pin)
    assert selected["kind"] == "application-bootstrap"
    assert selected["application_startup"] == original
    assert selected["installation"] == bootstrap.bootstrap_installation_binding(deployment.read_receipt(directory))
    assert Path(pin["path"]).stat().st_mode & 0o777 == 0o600
    assert hashlib.sha256(Path(pin["path"]).read_bytes()).hexdigest() == pin["sha256"]
    assert result["deployment_id"] == f.directory.name
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
    assert not Path(f.fresh["provision_attempt_path"]).exists()


@pytest.mark.parametrize("value", [None, {}, {"kind": "replacement"}])
async def test_private_start_refuses_missing_or_wrong_kind_before_queue(owned_inputs, value):
    f = owned_inputs
    with pytest.raises((CustodyError, DraftConflict, ValueError), match="[Bb]ootstrap|[Ff]resh|[Rr]estore|selection|inputs"):
        await f.service.start_application(f.guide.identity, f.directory.name, application_startup=value)
    assert_not_queued(f)
    assert list(f.directory.glob("bootstrap-admission-*.json")) == []


@pytest.mark.parametrize("status", ["running", "failed", "interrupted"])
async def test_private_start_requires_explicit_stopped_owner(owned_inputs, status):
    f = owned_inputs
    deployment.update_receipt(f.directory, status=status)
    with pytest.raises((CustodyError, DraftConflict, ValueError), match="stopped"):
        await start(f)
    assert_not_queued(f)
    assert deployment.read_receipt(f.directory)["status"] == status


async def test_mutable_start_inputs_are_frozen_before_original_service_lock(owned_inputs):
    f = owned_inputs
    original = deepcopy(f.fresh)
    async with f.service.lock:
        task = asyncio.create_task(start(f))
        await asyncio.sleep(0)
        assert not task.done() and f.launches == []
        f.fresh["gateway"]["sha256"] = "0" * 64
        f.fresh["kind"] = "restore"
    await task
    assert len(f.launches) == 1
    selected = bootstrap.read_bootstrap_admission(f.directory, f.launches[0][3])
    assert selected["application_startup"] == original


@pytest.mark.parametrize("changed", ["gateway", "package", "admission"])
async def test_original_thread_observation_prevents_queue_after_selected_file_swap(owned_inputs, monkeypatch, changed):
    f = owned_inputs
    original = bootstrap.retain_bootstrap_admission
    retained = []

    def swapped(*args, **kwargs):
        result = original(*args, **kwargs)
        retained.append(deepcopy(result[0]))
        path = f.gateway.path if changed == "gateway" else f.directory / "package.zip" if changed == "package" else Path(result[0]["path"])
        replace_identical(path)
        return result

    monkeypatch.setattr(bootstrap, "retain_bootstrap_admission", swapped)
    with pytest.raises((CustodyError, DraftConflict, ValueError), match="changed"):
        await start(f)
    assert len(retained) == 1
    assert_not_queued(f)


@pytest.mark.parametrize("changed", ["installation", "owner", "latest"])
async def test_original_installation_authority_is_rechecked_after_admission_thread(owned_inputs, monkeypatch, changed):
    f = owned_inputs
    original = bootstrap.retain_bootstrap_admission
    admitted = []

    def observed(*args, **kwargs):
        result = original(*args, **kwargs)
        admitted.append(result[0])
        if changed == "installation":
            deployment.update_receipt(f.directory, build_id=str(uuid.uuid4()))
        elif changed == "owner":
            replace_identical(f.destination / deployment.OWNER_FILE)
        else:
            deployment.write_private(f.directory.parent / "latest.json", {"deployment_id": str(uuid.uuid4())})
        return result

    monkeypatch.setattr(bootstrap, "retain_bootstrap_admission", observed)
    with pytest.raises((CustodyError, DraftConflict, ValueError, LookupError), match="changed|stopped|current|found"):
        await start(f)
    assert len(admitted) == 1
    assert_not_queued(f)


async def test_private_start_preserves_installed_source_when_new_design_is_selected(owned_inputs):
    f = owned_inputs
    f.guide.state["brief"] = {"outcome": "A future changed design"}
    f.implementation.identity = str(uuid.uuid4())
    f.implementation.accepted = False
    await start(f)
    assert len(f.launches) == 1
    selected = bootstrap.read_bootstrap_admission(f.directory, f.launches[0][3])
    assert selected["installation"]["build_id"] == f.receipt["build_id"]


async def test_repeated_cancel_joins_original_bootstrap_admission_without_queue(owned_inputs, monkeypatch):
    f = owned_inputs
    original = bootstrap.retain_bootstrap_admission
    loop = asyncio.get_running_loop()
    entered, release, exited, pins = asyncio.Event(), threading.Event(), [], []

    def held(*args, **kwargs):
        result = original(*args, **kwargs)
        pins.append(deepcopy(result[0]))
        loop.call_soon_threadsafe(entered.set)
        assert release.wait(5), "The inert bootstrap admission thread was not released."
        exited.append(True)
        return result

    monkeypatch.setattr(bootstrap, "retain_bootstrap_admission", held)
    task = asyncio.create_task(start(f))
    try:
        await asyncio.wait_for(entered.wait(), 5)
        task.cancel("first bootstrap waiter")
        await asyncio.sleep(0)
        task.cancel("second bootstrap waiter")
        await asyncio.sleep(0)
        assert not task.done() and f.service.lock.locked()
        assert_not_queued(f)
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, 5)
    assert exited == [True] and not f.service.lock.locked()
    assert_not_queued(f)
    assert len(pins) == 1
    assert bootstrap.read_bootstrap_admission(f.directory, pins[0])["application_startup"] == f.fresh


async def test_ordinary_restart_keeps_original_dispatch_without_bootstrap_selection(owned_inputs):
    f = owned_inputs
    await f.service.action(f.guide.identity, f.directory.name, "restart")
    assert f.launches == [(f.directory, "start", None, None)]
    assert list(f.directory.glob("bootstrap-admission-*.json")) == []


@pytest.mark.parametrize("selected", [False, True])
async def test_actual_launch_retains_selected_pin_before_worker_task_first_turn(owned_inputs, monkeypatch, selected):
    f = owned_inputs
    pin = f.admit()[0] if selected else None
    original = deepcopy(pin)
    calls = []

    async def captured_run(directory, mode, *, update_selection=None, bootstrap_selection=None):
        calls.append((directory, mode, deepcopy(update_selection), deepcopy(bootstrap_selection)))
        f.service.tasks.pop(directory.name, None)

    monkeypatch.setattr(f.service, "_run", captured_run)
    deployment.DeploymentService._launch(f.service, f.directory, "start", bootstrap_selection=pin)
    if pin is not None:
        pin["path"] = str(f.directory / "wrong-envelope.json")
        pin["sha256"] = "0" * 64
    await f.service.tasks[f.directory.name]
    assert calls == [(f.directory, "start", None, original)]
    receipt = deployment.read_receipt(f.directory)
    assert str(uuid.UUID(receipt["interactive_lifecycle_id"])) == receipt["interactive_lifecycle_id"]
    assert receipt["interactive_cleanup"] is None


@pytest.mark.parametrize("selected", [False, True])
async def test_actual_service_run_uses_distinct_fixed_wrapper_argv(owned_inputs, monkeypatch, selected):
    f = owned_inputs
    pin = f.admit()[0] if selected else None
    calls = []
    failure = OSError("inert spawn boundary")

    async def captured_spawn(*args, **kwargs):
        calls.append((args, kwargs))
        raise failure

    monkeypatch.setattr(asyncio, "create_subprocess_exec", captured_spawn)
    await deployment.DeploymentService._run(f.service, f.directory, "start", bootstrap_selection=pin)
    assert len(calls) == 1
    arguments, options = calls[0]
    assert arguments[1] == "-c" and options["start_new_session"] is True
    assert options["cwd"] == str(Path(deployment.__file__).resolve().parents[1])
    if selected:
        assert "bootstrap_interactive_worker" in arguments[2]
        assert arguments[-2:] == (pin["path"], pin["sha256"])
        assert arguments[3] == str(f.directory)
    else:
        assert "bootstrap_interactive_worker" not in arguments[2]
        assert "interactive_worker" in arguments[2]
        assert arguments[3:5] == (str(f.directory), "start")
    assert f.service.processes == {}
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


async def test_explicit_bootstrap_wrapper_delegates_original_pin_without_positional_replacement(owned_inputs, monkeypatch):
    f = owned_inputs
    pin, _ = f.admit()
    calls = []

    async def captured(*args, **kwargs):
        calls.append((args, kwargs))
        return 1

    monkeypatch.setattr(deployment, "interactive_worker", captured)
    lifecycle = str(uuid.uuid4())
    assert await deployment.bootstrap_interactive_worker(str(f.directory), lifecycle, pin["path"], pin["sha256"]) == 1
    assert calls == [((str(f.directory), "start", lifecycle), {"bootstrap_selection": pin})]


async def inert_owner(supervisor):
    await supervisor.stop.wait()
    return "stopped"


def inert_worker(monkeypatch, f, starts):
    class InertSupervisor(worker.Supervisor):
        async def start(self, definition, *, on_ready=None, application_startup=None, bootstrap_selection=None):
            starts.append((deepcopy(definition), on_ready, deepcopy(application_startup), deepcopy(bootstrap_selection)))
            self.stop.set()

        async def terminate(self, *args, **kwargs):
            assert self.child is None

    monkeypatch.setattr(worker, "Supervisor", InertSupervisor)
    monkeypatch.setattr(worker, "owner_input", inert_owner)
    loop = asyncio.get_running_loop()
    monkeypatch.setattr(loop, "add_signal_handler", lambda *args: None)


@pytest.mark.parametrize("selected", [False, True])
async def test_actual_worker_run_passes_original_bootstrap_pin_only_to_start(owned_inputs, monkeypatch, selected):
    f = owned_inputs
    pin = f.admit()[0] if selected else None
    starts = []
    inert_worker(monkeypatch, f, starts)
    assert await worker.run(f.directory, "start", bootstrap_selection=pin) in (None, 0)
    assert starts == [(f.implementation.definition, None, None, pin)]
    assert deployment.read_receipt(f.directory)["status"] == "stopped"


async def test_actual_interactive_wrapper_keeps_original_pin_and_cleanup_boundary(owned_inputs, monkeypatch):
    f = owned_inputs
    pin, _ = f.admit()
    starts = []
    inert_worker(monkeypatch, f, starts)
    lifecycle = str(uuid.uuid4())
    deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle,
        interactive_cleanup=None, interactive_cleanup_attempt_id=None)
    assert await deployment.bootstrap_interactive_worker(str(f.directory), lifecycle, pin["path"], pin["sha256"]) in (None, 0)
    assert starts == [(f.implementation.definition, None, None, pin)]
    cleanup = deployment.read_receipt(f.directory)["interactive_cleanup"]
    assert cleanup["confirmed"] is True and cleanup["lifecycle_id"] == lifecycle
    assert cleanup["boundary"] == "owned-process-groups-absent"
    assert cleanup["worker_exit_confirmed"] is False


@pytest.mark.parametrize("mode", ["install", "update"])
async def test_worker_refuses_bootstrap_pin_in_another_mode_before_runtime_effects(owned_inputs, monkeypatch, mode):
    f = owned_inputs
    pin, _ = f.admit()
    starts = []
    inert_worker(monkeypatch, f, starts)
    with pytest.raises((CustodyError, DraftConflict, ValueError), match="[Bb]ootstrap|start|mode"):
        await worker.run(f.directory, mode, bootstrap_selection=pin)
    assert starts == [] and not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


async def test_service_bootstrap_requires_its_recorded_operating_system_owner_before_runtime(owned_inputs, monkeypatch):
    f = owned_inputs
    pin, _ = f.admit()
    starts = []
    inert_worker(monkeypatch, f, starts)
    # Service bootstrap is a supported Restore lane. This interactive fixture
    # has no recorded unit and must fail before constructing a native runtime.
    with pytest.raises(ValueError, match="owned operating-system service"):
        await worker.run(f.directory, "service", bootstrap_selection=pin)
    assert starts == [] and not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


@pytest.mark.parametrize("changed", ["gateway", "envelope"])
async def test_actual_worker_refuses_swap_after_original_bootstrap_read(owned_inputs, monkeypatch, changed):
    f = owned_inputs
    pin, _ = f.admit()
    starts = []
    inert_worker(monkeypatch, f, starts)
    original = bootstrap.observe_bootstrap_admission

    def swapped(*args, **kwargs):
        result = original(*args, **kwargs)
        replace_identical(f.gateway.path if changed == "gateway" else Path(pin["path"]))
        return result

    monkeypatch.setattr(bootstrap, "observe_bootstrap_admission", swapped)
    with pytest.raises((CustodyError, DraftConflict, ValueError), match="changed"):
        await worker.run(f.directory, "start", bootstrap_selection=pin)
    assert starts == [] and not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


@pytest.mark.parametrize("selected", [False, True])
async def test_actual_supervisor_selects_fixed_isolated_child_or_original_app(owned_inputs, monkeypatch, selected):
    f = owned_inputs
    pin = f.admit()[0] if selected else None
    supervisor = worker.Supervisor(f.directory)
    calls = []
    failure = OSError("inert application spawn")

    async def captured_spawn(*args, **kwargs):
        calls.append((args, kwargs))
        raise failure

    monkeypatch.setattr(supervisor, "spawn_child", captured_spawn)
    monkeypatch.setattr(worker, "require_available_console_port", lambda port: {
        "scope": "local-console-port-observation", "status": "available", "port": port})
    with pytest.raises(OSError) as raised:
        await supervisor.start(f.implementation.definition, bootstrap_selection=pin)
    assert raised.value is failure and len(calls) == 1
    arguments, options = calls[0]
    assert arguments[0] == str(f.destination / ".venv/bin/python")
    assert options["cwd"] == f.destination and options["pass_fds"]
    if selected:
        assert arguments[1:] == bootstrap.bootstrap_child_arguments(f.directory, pin)
        assert arguments[1] == "-I"
    else:
        assert arguments[1:] == ("app.py",)
    assert supervisor.child is None and supervisor._child_admission is None


async def test_supervisor_final_pipe_await_cannot_rebaseline_swapped_selected_input(owned_inputs, monkeypatch):
    f = owned_inputs
    pin, _ = f.admit()
    supervisor = worker.Supervisor(f.directory)
    calls = []
    loop = asyncio.get_running_loop()
    original = loop.connect_read_pipe

    async def swapped(*args, **kwargs):
        result = await original(*args, **kwargs)
        replace_identical(f.gateway.path)
        return result

    async def forbidden_spawn(*args, **kwargs):
        calls.append(args)
        pytest.fail("Changed original bootstrap inputs cannot enter application child admission.")

    monkeypatch.setattr(loop, "connect_read_pipe", swapped)
    monkeypatch.setattr(supervisor, "spawn_child", forbidden_spawn)
    monkeypatch.setattr(worker, "require_available_console_port", lambda port: {
        "scope": "local-console-port-observation", "status": "available", "port": port})
    with pytest.raises((CustodyError, DraftConflict, ValueError), match="changed"):
        await supervisor.start(f.implementation.definition, bootstrap_selection=pin)
    assert calls == [] and supervisor.child is None


@pytest.mark.parametrize("changed", ["lifecycle", "returned-selection"])
async def test_parent_original_bootstrap_observation_refuses_reselection_before_spawn(owned_inputs, monkeypatch, changed):
    f = owned_inputs
    pin, _ = f.admit()
    lifecycle = str(uuid.uuid4())
    deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle)
    original = bootstrap.observe_bootstrap_admission
    observed, spawned = [], []

    def changed_after_authentication(*args, **kwargs):
        result = original(*args, **kwargs)
        observed.append(deepcopy(result[0]))
        if changed == "lifecycle":
            deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
        else:
            # A syntactically valid mutation cannot become authenticated merely
            # because its dict came back with the original file observation.
            result[0]["application_startup"]["gateway"]["sha256"] = "0" * 64
        return result

    async def captured_spawn(*args, **kwargs):
        spawned.append((args, kwargs))
        raise OSError("inert unexpected worker admission")

    monkeypatch.setattr(bootstrap, "observe_bootstrap_admission", changed_after_authentication)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", captured_spawn)
    await deployment.DeploymentService._run(f.service, f.directory, "start", bootstrap_selection=pin)
    assert len(observed) == 1
    assert spawned == [], "The changed original bootstrap observation entered worker spawning."
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "failed" and "CustodyError" in receipt["detail"]
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


@pytest.mark.parametrize("changed", ["lifecycle", "returned-selection"])
async def test_worker_original_bootstrap_observation_refuses_reselection_before_supervisor(owned_inputs, monkeypatch, changed):
    f = owned_inputs
    pin, _ = f.admit()
    deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
    original = bootstrap.observe_bootstrap_admission
    observed, constructed, starts = [], [], []

    def changed_after_authentication(*args, **kwargs):
        result = original(*args, **kwargs)
        observed.append(deepcopy(result[0]))
        if changed == "lifecycle":
            deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
        else:
            result[0]["application_startup"]["gateway"]["sha256"] = "0" * 64
        return result

    class InertSupervisor(worker.Supervisor):
        def __init__(self, directory):
            constructed.append(directory)
            super().__init__(directory)

        async def start(self, definition, *, on_ready=None, application_startup=None, bootstrap_selection=None):
            starts.append(deepcopy(bootstrap_selection))
            self.stop.set()

        async def terminate(self, *args, **kwargs):
            assert self.child is None

    monkeypatch.setattr(bootstrap, "observe_bootstrap_admission", changed_after_authentication)
    monkeypatch.setattr(worker, "Supervisor", InertSupervisor)
    monkeypatch.setattr(worker, "owner_input", inert_owner)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    failure = None
    try:
        await worker.run(f.directory, "start", bootstrap_selection=pin)
    except (CustodyError, DraftConflict, ValueError) as error:
        failure = error
    assert len(observed) == 1
    assert constructed == [] and starts == [], "The changed original bootstrap observation constructed a Supervisor."
    assert failure is not None and "changed" in str(failure).lower()
    assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()


async def test_original_queued_lifecycle_cannot_rebase_before_parent_worker_first_turn(owned_inputs, monkeypatch):
    f = owned_inputs
    pin, _ = f.admit()
    spawned = []

    async def captured_spawn(*args, **kwargs):
        spawned.append(args)
        raise OSError("inert unexpected queue admission")

    monkeypatch.setattr(asyncio, "create_subprocess_exec", captured_spawn)
    deployment.DeploymentService._launch(f.service, f.directory, "start", bootstrap_selection=pin)
    task = f.service.tasks[f.directory.name]
    original = f.service.worker_admissions[f.directory.name]["lifecycle_id"]
    deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
    await task
    assert spawned == []
    assert f.service.worker_admissions[f.directory.name]["lifecycle_id"] == original
    assert deployment.read_receipt(f.directory)["status"] == "failed"


async def test_wrapper_original_lifecycle_survives_delayed_worker_entry(owned_inputs, monkeypatch):
    f = owned_inputs
    pin, _ = f.admit()
    lifecycle = str(uuid.uuid4())
    deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle)
    original = worker.run
    captured, constructed = [], []

    async def delayed_worker(directory, mode, **kwargs):
        captured.append(deepcopy(kwargs))
        deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
        return await original(directory, mode, **kwargs)

    class ForbiddenSupervisor:
        def __init__(self, directory):
            constructed.append(directory)
            pytest.fail("Changed wrapper lifecycle cannot construct its Supervisor.")

    monkeypatch.setattr(worker, "run", delayed_worker)
    monkeypatch.setattr(worker, "Supervisor", ForbiddenSupervisor)
    with pytest.raises((CustodyError, DraftConflict), match="lifecycle.*changed"):
        await deployment.bootstrap_interactive_worker(str(f.directory), lifecycle, pin["path"], pin["sha256"])
    assert captured == [{"bootstrap_selection": pin, "bootstrap_lifecycle_id": lifecycle}]
    assert constructed == [] and deployment.read_receipt(f.directory).get("interactive_cleanup") is None


@pytest.mark.parametrize("boundary", ["parent", "worker"])
async def test_lifecycle_change_before_original_validation_thread_cannot_rebase(owned_inputs, monkeypatch, boundary):
    f = owned_inputs
    pin, _ = f.admit()
    deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
    original = deployment.await_validation
    entered, effects = [], []

    async def before_original_thread(function, *args, **kwargs):
        if function is bootstrap.observe_bootstrap_admission:
            entered.append(True)
            deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
        return await original(function, *args, **kwargs)

    async def captured_spawn(*args, **kwargs):
        effects.append(args)
        raise OSError("inert unexpected parent admission")

    class ForbiddenSupervisor:
        def __init__(self, directory):
            effects.append(directory)
            pytest.fail("Changed pre-validation lifecycle cannot construct its Supervisor.")

    if boundary == "parent":
        monkeypatch.setattr(deployment, "await_validation", before_original_thread)
        monkeypatch.setattr(asyncio, "create_subprocess_exec", captured_spawn)
        await deployment.DeploymentService._run(f.service, f.directory, "start", bootstrap_selection=pin)
        assert deployment.read_receipt(f.directory)["status"] == "failed"
    else:
        # Installed bootstrap now observes under its source/lease wrapper via
        # await_product_validation, rather than worker.await_validation.
        from launchkit import product_runtime_custody
        product_original = product_runtime_custody.await_product_validation
        async def before_product_thread(source_custody, function, *args, **kwargs):
            if function is bootstrap.observe_bootstrap_admission:
                entered.append(True)
                deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
            return await product_original(source_custody, function, *args, **kwargs)
        monkeypatch.setattr(product_runtime_custody, "await_product_validation", before_product_thread)
        monkeypatch.setattr(worker, "Supervisor", ForbiddenSupervisor)
        with pytest.raises((CustodyError, DraftConflict), match="lifecycle.*changed"):
            await worker.run(f.directory, "start", bootstrap_selection=pin)
    assert entered == [True] and effects == []


async def test_cached_supervisor_lifecycle_cannot_rebase_in_original_observation(owned_inputs, monkeypatch):
    f = owned_inputs
    pin, _ = f.admit()
    lifecycle = str(uuid.uuid4())
    deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle)
    supervisor = worker.Supervisor(f.directory)
    original = bootstrap.observe_bootstrap_admission
    spawned = []

    def replaced(*args, **kwargs):
        result = original(*args, **kwargs)
        deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
        return result

    async def captured_spawn(*args, **kwargs):
        spawned.append(args)
        raise OSError("inert unexpected application admission")

    monkeypatch.setattr(bootstrap, "observe_bootstrap_admission", replaced)
    monkeypatch.setattr(supervisor, "spawn_child", captured_spawn)
    with pytest.raises((CustodyError, DraftConflict), match="lifecycle.*changed"):
        await supervisor.start(f.implementation.definition, bootstrap_selection=pin)
    assert supervisor.receipt["interactive_lifecycle_id"] == lifecycle and spawned == []


@pytest.mark.parametrize("changed", ["receipt", "attribute"])
async def test_inner_factory_retains_original_child_lifecycle_after_entry(owned_inputs, changed):
    f = owned_inputs
    pin, _ = f.admit()
    lifecycle = str(uuid.uuid4())
    deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle)
    factory = bootstrap.BootstrapFactory(f.directory, pin, lifecycle_id=lifecycle)
    with project_lock(f.destination) as lease:
        async with factory(lease) as startup:
            if changed == "receipt":
                deployment.update_receipt(f.directory, interactive_lifecycle_id=str(uuid.uuid4()))
            else:
                factory._lifecycle_id = str(uuid.uuid4())
            with pytest.raises(CustodyError, match="lifecycle|selection"):
                factory.require_current()
            assert startup.lease is lease and not factory.source._closed
        assert factory.source._closed


def retained_fresh_entry(f, pin):
    write(f.directory / bootstrap.FRESH_ENTRY_NAME, {"kind": "application-bootstrap-entry",
        "admission": pin, "provision_attempt_path": f.fresh["provision_attempt_path"]})
    return (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()


async def test_entered_fresh_history_refuses_ordinary_restart_and_update_staging(owned_inputs):
    f = owned_inputs
    pin, _ = f.admit()
    marker = retained_fresh_entry(f, pin)
    receipt = (f.directory / "receipt.json").read_bytes()
    with pytest.raises(CustodyError, match="Fresh"):
        await f.service.action(f.guide.identity, f.directory.name, "restart")
    f.implementation.identity = str(uuid.uuid4())
    payload, _ = await f.implementation.package(f.guide.identity, f.implementation.identity)
    files = deployment.package_files(payload)
    report = json.loads(files["implementation-report.json"][0])
    with pytest.raises(CustodyError, match="Fresh"):
        updates.stage(f.directory, payload, report)
    assert f.launches == [] and list((f.directory / "updates").glob("*")) == []
    assert (f.directory / "receipt.json").read_bytes() == receipt
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == marker


async def test_restore_of_older_current_cannot_settle_another_fresh_output(owned_inputs):
    f = owned_inputs
    pin, _ = f.admit()
    marker = retained_fresh_entry(f, pin)
    restore, _ = f.admit("restore")
    with pytest.raises(CustodyError, match="Fresh output paths"):
        bootstrap.require_bootstrap_history_settled(f.directory, restore)
    with pytest.raises(CustodyError, match="Fresh output paths"):
        bootstrap.BootstrapFactory(f.directory, restore)
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == marker


async def test_restore_exact_fresh_output_paths_keeps_original_entry(owned_inputs):
    f = owned_inputs
    pin, _ = f.admit()
    marker = retained_fresh_entry(f, pin)
    bundle = Path(f.fresh["bundle_path"])
    bundle.mkdir(mode=0o700)
    activation = write(Path(f.fresh["activation_path"]), {"synthetic": "completed Fresh activation"})
    local_policy = write(bundle / "policy.json", {"synthetic": "completed Fresh compiled policy"})
    setup = write(bundle / "setup.json", {"synthetic": "completed Fresh setup"})
    manifest = write(bundle / "manifest.json", {"artifacts": [
        {"path": local_policy.path.name, "sha256": local_policy.sha256},
        {"path": setup.path.name, "sha256": setup.sha256}]})
    (f.destination / "current").unlink()
    (f.destination / "current").symlink_to(activation.path.name)
    selection = write(f.private / "completed-fresh-selection.json", {
        "deployment_root": str(f.destination), "activation": activation.pin(),
        "policy_manifest": manifest.pin(), "local_policy": local_policy.pin()})
    value = {**f.restore, "selection_file": selection.pin(), "completed_setup": setup.pin()}
    restore, _ = bootstrap.retain_bootstrap_admission(f.directory, deployment.read_receipt(f.directory), value)
    bootstrap.require_bootstrap_history_settled(f.directory, restore)
    with project_lock(f.destination) as lease:
        factory = bootstrap.BootstrapFactory(f.directory, restore)
        async with factory(lease) as startup:
            assert startup.selection_file == selection and startup.setup == setup
        assert factory.source._closed
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == marker
