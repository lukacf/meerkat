"""Inert stopped-owner composition controls, never native qualification.

The maintained caller parses exact synthetic exited bytes. No gateway, SDK,
server, provider or real native child is executed by these controls.
"""
import asyncio
from copy import deepcopy
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit import deployment, deployment_application_preparation as bridge
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.deployment_worker import Supervisor
from launchkit.forge_application_host import ForgeApplicationHost
from launchkit.projects import DraftConflict
from launchkit.runtime_selection import selected_runtime_artifacts, selected_runtime_platform
from launchkit.templates import install_candidate as adapter
from launchkit.templates.storage_setup import project_lock
from test_candidate_implementation import service as implementation_service, accepted_fixture
from test_implementation import workspace
from test_forge_application_host import selected, declarations, encoded, put
from test_meta_toolkit_initial_install import AdmissionOnly, no_processes
from test_meta_toolkit_runtime_selection import product, no_product_execution


@pytest.fixture
async def prepared(tmp_path, selected, implementation_service, monkeypatch):
    f = selected
    # This inert package declares its own target; no native artifact executes.
    monkeypatch.setattr("launchkit.runtime_dependencies.platform_target",
        lambda: selected_runtime_platform(f.target))
    f.guide_id = implementation_service.guide.identity
    f.protected["application/configuration.json"] = put(f.root / "application/configuration.json", {"fields": [
        {"key": "CONSOLE_USER_ID", "label": "User ID", "required": True, "secret": False},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Password", "required": True, "secret": True}]})
    viewer = json.loads(f.viewer.read())
    viewer["source_inputs"]["application/configuration.json"] = f.protected["application/configuration.json"].pin()
    f.viewer = put(f.viewer.path, viewer)
    f.host = ForgeApplicationHost(guide_id=f.guide_id, runtime_target=f.target,
        acquisition_selection=f.acquisition, viewer_selection=f.viewer, require_current=lambda: None)
    implementation_service.guide.state["proposal"] = deepcopy(f.spec)
    monkeypatch.setattr(implementation_service, "prepare_workspace", f.host.prepare_workspace)
    built = await implementation_service.submit(f.guide_id, 3, runtime_target=f.target)
    await accepted_fixture(implementation_service, built)
    service = AdmissionOnly(tmp_path / "private-server", implementation_service.guide, implementation_service)
    service.launched = []
    monkeypatch.setattr(deployment, "require_available_console_port", lambda *args, **kwargs: None)
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda *args, **kwargs: None)
    f.destination = tmp_path / "installed application"
    f.receipt = await service.submit(f.guide_id, 3, built["build_id"], str(f.destination), settings={
        "OPENAI_API_KEY": "inert-not-a-provider-key", "CONSOLE_USER_ID": "inert-selected-operator",
        "CONSOLE_LOGIN_PASSWORD": "inert-private-fixture-password"})
    f.service, f.directory = service, service._directory(f.guide_id)
    deployment.materialize(f.directory)
    f.receipt = deployment.update_receipt(f.directory, status="stopped", phase="configure",
        process_live=False, runtime_pid=None)
    service.launched.clear()
    adapter.install_meta_toolkit_prefix(selected_runtime_artifacts(f.target),
        deployment_root=f.destination, require_current=lambda: None)
    f.supervisor = Supervisor(f.directory)
    f.private = f.destination / ".mobkit/preparation"
    f.private.mkdir(mode=0o700)
    f.work = f.private / "work"
    f.work.mkdir(mode=0o700)
    f.template = put(f.private / "activation-template.json", {"original": ["template", False]})
    f.paths = {"provision_attempt_path": str(f.private / "provision.json"),
        "working_directory": str(f.work), "bundle_path": str(f.destination / "original-bundle"),
        "activation_path": str(f.destination / "original-activation.json")}
    f.inventory_attempt, f.plan_path, f.selection_path = (
        f.private / name for name in ("inventory.json", "plan.json", "fresh.json"))
    original = json.loads(f.inventory.read())
    f.actual = {"schema": 1, "mob_id": original["mob_id"], "helper_identity": original["helper_identity"],
        "realm_id": "opaque-inert-realm", "members": [{"identity": row["identity"],
            "member": {"mob_id": original["mob_id"], "role": row["profile"], "member": "inert-native-" + str(i)}}
            for i, row in enumerate(original["roster"])]}
    f.calls, f.after_command = [], lambda: None
    loop_thread = __import__("threading").get_ident()
    def source_current():
        assert __import__("threading").get_ident() == loop_thread
    f.host._require_owner_current = source_current
    async def command(argv, phase, timeout, *, raw_outcome):
        assert __import__("threading").get_ident() == loop_thread
        assert service.lock.locked() and raw_outcome
        with pytest.raises(RuntimeError, match="running"):
            with project_lock(f.destination):
                pytest.fail("The original lease was released during inventory.")
        f.calls.append(tuple(argv))
        await asyncio.sleep(0)
        f.after_command()
        return {"returncode": 0, "stdout": encoded(f.actual), "stderr": b"inert retained stderr\n"}
    monkeypatch.setattr(f.supervisor, "command", command)
    def choose(inventory):
        assert __import__("threading").get_ident() == loop_thread
        original_input, actual = inventory.authenticate()
        assert actual == f.actual and original_input == original
        chosen = declarations(f, actual)
        chosen["deployment_root"] = f.destination
        chosen["realm_storage"] = {"context_root": str(f.destination), "state_root": str(f.destination / ".mobkit/state")}
        chosen["output"] = f.work / "original-native-output.json"
        return chosen
    f.choose = choose
    f.kwargs = {"application_host": f.host, "supervisor": f.supervisor, "declarations": choose,
        "inventory_attempt_path": f.inventory_attempt, "preparation_plan_path": f.plan_path,
        "startup_selection_path": f.selection_path, "activation_template": f.template,
        "fresh_paths": f.paths, "command_timeout": 20}
    async def run(**changes):
        return await service.prepare_application(f.guide_id, f.receipt["deployment_id"], **{**f.kwargs, **changes})
    f.run = run
    yield f
    await implementation_service.shutdown()


async def test_actual_stopped_owner_caller_and_private_fresh_envelope(prepared):
    f = prepared
    before = f.template.read(), f.policy.read(), deepcopy(f.paths)
    pin = await f.run()
    value = json.loads(pin.read())
    assert value == {"kind": "fresh", "gateway": {"path": f.calls[0][0], "sha256": value["gateway"]["sha256"]},
        "preparation_plan": ExactFile(f.plan_path, value["preparation_plan"]["sha256"]).pin(),
        "source_policy": f.policy.pin(), "activation_template": f.template.pin(), **f.paths}
    assert f.calls == [(value["gateway"]["path"], "--lower-application-inventory", "--input", str(f.inventory.path))]
    assert Path(value["gateway"]["path"]).is_relative_to(f.destination / ".bin")
    assert json.loads(f.plan_path.read_bytes())["application"]["members"][0]["member"] == f.actual["members"][0]["member"]
    assert (f.template.read(), f.policy.read(), f.paths) == before
    assert f.service.launched == [] and f.service._application_preparations == {}
    assert deployment.read_receipt(f.directory)["status"] == "stopped"
    assert not Path(f.paths["provision_attempt_path"]).exists()
    assert not Path(f.paths["bundle_path"]).exists() and not Path(f.paths["activation_path"]).exists()
    assert not (f.directory / "application-bootstrap-entry.json").exists()
    with project_lock(f.destination):
        pass


@pytest.mark.parametrize("change", ["target", "settings", "template", "owner"])
async def test_changed_original_input_after_lower_cannot_publish_selection(prepared, change):
    f = prepared
    def mutate():
        if change == "target":
            current = deployment.read_receipt(f.directory)
            current["runtime_candidate"]["assembly"]["sha256"] = "f" * 64
            deployment.update_receipt(f.directory, runtime_candidate=current["runtime_candidate"])
        elif change == "settings":
            with (f.destination / ".env").open("ab") as stream:
                stream.write(b"CHANGED_PRIVATE_INPUT=fixture\n")
        elif change == "template":
            f.template.path.write_bytes(b'{"changed":true}\n')
        else:
            deployment.update_receipt(f.directory, status="running")
    f.after_command = mutate
    with pytest.raises((CustodyError, DraftConflict, ValueError)):
        await f.run()
    assert not f.selection_path.exists() and not f.plan_path.exists()
    assert f.service._application_preparations == {} and not f.service.launched


@pytest.mark.parametrize("case", ["wrong-supervisor", "entered-child", "entered-sdk", "plan-alias", "existing-selection", "async-declarations"])
async def test_initial_refusals_do_not_enter_native_inventory(prepared, case):
    f = prepared
    changes = {}
    if case == "wrong-supervisor":
        f.supervisor.directory = f.directory.parent
    elif case == "entered-child":
        f.supervisor.child = object()
    elif case == "entered-sdk":
        f.supervisor._selected_sdk_install_task = asyncio.create_task(asyncio.sleep(0))
    elif case == "plan-alias":
        changes["preparation_plan_path"] = f.inventory_attempt
    elif case == "existing-selection":
        put(f.selection_path, {"original": True})
    else:
        async def asynchronous(inventory):
            pytest.fail("An async declaration cannot be invoked.")
        changes["declarations"] = asynchronous
    with pytest.raises((CustodyError, DraftConflict, RuntimeError)):
        await f.run(**changes)
    assert f.calls == [] and not f.plan_path.exists()
    assert f.service._application_preparations == {}


async def test_group_cleanup_failure_retains_original_lease_and_blocks_known_selection(prepared, monkeypatch):
    f = prepared
    child = SimpleNamespace(returncode=0, pid=12345)
    drains = (asyncio.create_task(asyncio.sleep(0)),)
    async def original_cleanup(process, tasks=(), **kwargs):
        assert process is child and tuple(tasks) == drains
        await asyncio.gather(*tasks)
        f.supervisor.child = None
    monkeypatch.setattr(f.supervisor, "cleanup_child", original_cleanup)
    async def command(*args, **kwargs):
        f.supervisor.child = child
        await f.supervisor.cleanup_child(child, drains)
        return {"returncode": 0, "stdout": encoded(f.actual), "stderr": b""}
    monkeypatch.setattr(f.supervisor, "command", command)
    async def unresolved(process, **kwargs):
        assert process is child
        raise RuntimeError("original group absence remains unconfirmed")
    monkeypatch.setattr(deployment, "require_owned_group_absent", unresolved)
    with pytest.raises(RuntimeError, match="group absence"):
        await f.run()
    assert not f.selection_path.exists()
    held = f.service._application_preparations[f.receipt["deployment_id"]]
    assert held.supervisor is f.supervisor and held.child is child and held.drain_tasks == drains
    held.lease.require_current()
    with pytest.raises(RuntimeError):
        with project_lock(f.destination):
            pytest.fail("Unconfirmed cleanup must retain the original lease.")
    with pytest.raises(CustodyError, match="original preparation custody"):
        await f.run()
    with pytest.raises(DraftConflict, match="preparation custody"):
        await f.service.start_application(f.guide_id, f.receipt["deployment_id"], application_startup={"kind": "fresh"})
    with pytest.raises(CustodyError, match="exact inactive"):
        await f.service.cleanup_application_preparation(f.guide_id, f.receipt["deployment_id"], supervisor=object())
    # Lost source cannot authorize a replacement signal or bar original cleanup.
    deployment.update_receipt(f.directory, status="running")
    async def absent(process, **kwargs):
        assert process is child
    monkeypatch.setattr(deployment, "require_owned_group_absent", absent)
    await f.service.cleanup_application_preparation(f.guide_id, f.receipt["deployment_id"], supervisor=f.supervisor)
    assert f.service._application_preparations == {}
    with project_lock(f.destination):
        pass


async def test_cancel_during_final_settlement_drains_before_propagation(prepared, monkeypatch):
    f = prepared
    entered, release = asyncio.Event(), asyncio.Event()
    original = bridge._PreparationCustody.settle
    async def delayed(self):
        entered.set()
        await release.wait()
        await original(self)
    monkeypatch.setattr(bridge._PreparationCustody, "settle", delayed)
    task = asyncio.create_task(f.run())
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done() and f.service._application_preparations
    task.cancel()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert f.supervisor.stop.is_set() and f.service._application_preparations == {}


@pytest.mark.parametrize("ordinary_failure", [False, True])
async def test_shutdown_rejoins_original_cleanup_and_drain_tuple(prepared, monkeypatch, ordinary_failure):
    f = prepared
    child = SimpleNamespace(returncode=0, pid=12345)
    drain = asyncio.create_task(asyncio.sleep(0))
    count = 0
    async def original_cleanup(process, tasks=(), **kwargs):
        nonlocal count
        count += 1
        assert process is child and tuple(tasks) == (drain,)
        if count < 3:
            raise RuntimeError("inert first original cleanup failures")
        await asyncio.gather(*tasks)
        f.supervisor.child = None
    monkeypatch.setattr(f.supervisor, "cleanup_child", original_cleanup)
    async def command(*args, **kwargs):
        f.supervisor.child = child
        await f.supervisor.cleanup_child(child, (drain,))
    monkeypatch.setattr(f.supervisor, "command", command)
    async def absent(process, **kwargs):
        assert process is child
    monkeypatch.setattr(deployment, "require_owned_group_absent", absent)
    with pytest.raises(RuntimeError, match="cleanup failures"):
        await f.run()
    assert f.service._application_preparations and not f.selection_path.exists()
    if ordinary_failure:
        async def failed_ordinary(*args):
            raise RuntimeError("inert ordinary cleanup failure")
        monkeypatch.setattr(f.service, "_shutdown_locked", failed_ordinary)
        with pytest.raises(RuntimeError, match="ordinary cleanup failure"):
            await f.service.shutdown()
    else:
        await f.service.shutdown()
    assert count == 3 and f.service._application_preparations == {}


@pytest.mark.parametrize("history", ["fresh", "update", "replacement"])
async def test_existing_original_history_refuses_new_inventory(prepared, history):
    f = prepared
    if history == "fresh":
        put(f.directory / "application-bootstrap-entry.json", {"inert": "entered original Fresh"})
    elif history == "update":
        deployment.update_receipt(f.directory, update={"status": "recovery-needed"})
    else:
        retained = f.directory / "updates/retained-original"
        retained.mkdir(parents=True, mode=0o700)
        put(retained / "application-replacement-entry.json", {"inert": "original uncertain replacement"})
    with pytest.raises((CustodyError, DraftConflict), match="Fresh|recovery|replacement"):
        await f.run()
    assert f.calls == [] and not f.selection_path.exists()


@pytest.mark.parametrize("change", ["absent-installed-gateway", "changed-mode"])
async def test_staging_product_cannot_replace_original_installed_prefix(prepared, change):
    f = prepared
    artifacts = selected_runtime_artifacts(f.target)
    gateway = f.destination / ".bin" / artifacts.prefix / "bin/toolkit_gateway"
    if change == "absent-installed-gateway":
        gateway.unlink()
    else:
        gateway.chmod(0o700)
    with pytest.raises((CustodyError, OSError, ValueError, RuntimeError)):
        await f.run()
    assert not f.calls and not f.inventory_attempt.exists() and not f.selection_path.exists()


async def test_default_same_child_cleanup_cannot_discard_original_pending_drains(prepared, monkeypatch):
    f = prepared
    child = SimpleNamespace(returncode=0, pid=12345)
    ready = asyncio.Event()
    drain = asyncio.create_task(ready.wait())
    count = 0
    async def original_cleanup(process, tasks=(), **kwargs):
        nonlocal count
        count += 1
        assert process is child and tuple(tasks) == (drain,)
        if count < 3:
            raise RuntimeError("inert unsettled original drain")
        await asyncio.gather(*tasks)
        f.supervisor.child = None
    monkeypatch.setattr(f.supervisor, "cleanup_child", original_cleanup)
    async def command(*args, **kwargs):
        f.supervisor.child = child
        await f.supervisor.cleanup_child(child, (drain,))
    monkeypatch.setattr(f.supervisor, "command", command)
    async def absent(process, **kwargs):
        assert process is child
    monkeypatch.setattr(deployment, "require_owned_group_absent", absent)
    with pytest.raises(RuntimeError, match="original drain"):
        await f.run()
    held = f.service._application_preparations[f.receipt["deployment_id"]]
    cleanup = asyncio.create_task(f.supervisor.cleanup_child(child))
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    assert held.drain_tasks == (drain,) and not cleanup.done()
    different = asyncio.create_task(asyncio.sleep(0))
    with pytest.raises(CustodyError, match="original preparation cleanup must settle"):
        await f.supervisor.cleanup_child(child, (different,))
    ready.set()
    await cleanup
    await different
    with pytest.raises(CustodyError, match="original drain tasks"):
        await f.supervisor.cleanup_child(child, (different,))
    await f.service.cleanup_application_preparation(f.guide_id, f.receipt["deployment_id"], supervisor=f.supervisor)
    assert f.service._application_preparations == {}


@pytest.mark.parametrize("worker_failure", [False, True])
async def test_cancelled_joined_write_keeps_lease_until_worker_stops(prepared, monkeypatch, worker_failure):
    import threading
    f = prepared
    entered, release = threading.Event(), threading.Event()
    original = bridge._write_bytes
    def delayed(path, data):
        if path == f.plan_path:
            entered.set()
            assert release.wait(5)
            if worker_failure:
                raise OSError("inert original publication failure")
        return original(path, data)
    monkeypatch.setattr(bridge, "_write_bytes", delayed)
    task = asyncio.create_task(f.run())
    for _ in range(1000):
        if entered.is_set():
            break
        await asyncio.sleep(0.005)
    assert entered.is_set()
    task.cancel()
    await asyncio.sleep(0)
    assert f.supervisor.stop.is_set() and not task.done()
    held = f.service._application_preparations[f.receipt["deployment_id"]]
    held.lease.require_current()
    task.cancel()
    release.set()
    with pytest.raises(asyncio.CancelledError) as cancelled:
        await task
    if worker_failure:
        assert isinstance(cancelled.value.__cause__, OSError)
        assert str(cancelled.value.__cause__) == "inert original publication failure"
    assert not f.selection_path.exists() and f.service._application_preparations == {}


@pytest.mark.parametrize("interruption", ["stop", "cancel"])
async def test_stop_during_original_file_fence_refuses_private_publication(prepared, monkeypatch, interruption):
    import threading
    from launchkit.connect_accepted_source import AcceptedApplicationSource
    f = prepared
    entered, release = threading.Event(), threading.Event()
    armed = False
    original_plan, original_files = f.host.preparation_plan, AcceptedApplicationSource.require_filesystem_current
    def assemble(*args, **kwargs):
        nonlocal armed
        plan = original_plan(*args, **kwargs)
        armed = True
        return plan
    def files_current(source):
        nonlocal armed
        original_files(source)
        if armed:
            armed = False
            entered.set()
            assert release.wait(5)
    writes = []
    original_write = bridge._write_bytes
    def write(path, content):
        writes.append(path)
        return original_write(path, content)
    monkeypatch.setattr(f.host, "preparation_plan", assemble)
    monkeypatch.setattr(AcceptedApplicationSource, "require_filesystem_current", files_current)
    monkeypatch.setattr(bridge, "_write_bytes", write)
    task = asyncio.create_task(f.run())
    for _ in range(1000):
        if entered.is_set():
            break
        await asyncio.sleep(0.005)
    assert entered.is_set()
    if interruption == "cancel":
        task.cancel()
        await asyncio.sleep(0)
    else:
        f.supervisor.stop.set()
    assert f.supervisor.stop.is_set() and not task.done()
    held = f.service._application_preparations[f.receipt["deployment_id"]]
    held.lease.require_current()
    release.set()
    expected = asyncio.CancelledError if interruption == "cancel" else CustodyError
    with pytest.raises(expected) as interrupted:
        await task
    if interruption == "cancel":
        assert isinstance(interrupted.value.__cause__, CustodyError)
        assert "stopped before private publication" in str(interrupted.value.__cause__)
    assert writes == [] and not f.plan_path.exists() and not f.selection_path.exists()
    assert f.service._application_preparations == {}
    with project_lock(f.destination):
        pass
