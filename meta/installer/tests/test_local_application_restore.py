"""Original local Fresh-to-Restore publication with inert native outputs.

The actual finalizer stages, parses retained exited command receipts, validates
and publishes its pointer. The executor and completed native output specimens
are external test boundaries, not grants, Ready or native qualification.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys
import threading
from types import ModuleType, SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit.connect_installer_caller import CustodyError
from launchkit import local_application_restore as restore_publisher
from launchkit.forge_application_host import ForgeApplicationHost
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
import test_application_finalizer as finalizer_fixture
from test_application_finalizer import FinalizerFixture
from test_connect_completed_application_setup import BundleFixture
from test_forge_application_host_server import api
from test_local_application_factory import (local, retain, prepare, source_bytes,
    retaining, prepared, implementation_service, workspace, selected, product,
    no_product_execution, no_processes)


@pytest.fixture
async def local_restore(local, monkeypatch):
    f = local
    f.select()
    f.service.application_setup_continuation = f.host.continue_application_startup
    await retain(f)
    # Keep the real capability reader, but never route its availability query
    # through the reused synthetic Install process or its receipt-writing wait.
    async def unavailable_service_query(arguments, *, allow_failure, capture_output, timeout=None):
        assert allow_failure is True and capture_output is False
        assert (len(arguments) == 3 and arguments[1] == "print"
            or arguments[1:] == ["--user", "show", "--property=Version"])
        return 1, ""
    monkeypatch.setattr(f.service.service_manager, "command", unavailable_service_query)
    f.fresh_pin = await prepare(f, monkeypatch)
    f.fresh = json.loads(f.fresh_pin.read())
    f.native_commands = []
    return f


def completed_specimen(f):
    """Adapt the existing output specimen to this exact retained source plan.

    Native references remain visibly inert specimens. Real existing completed
    bundle and finalizer readers must authenticate every association and byte.
    """
    plan = json.loads(f.inputs.preparation_plan_path.read_bytes())
    root = f.setup_root / "inert-completed-native-output"
    root.mkdir(mode=0o700)
    specimen = BundleFixture(root, count=len(plan["application"]["members"]))
    # The driver selects its exact completed output path before native work.
    # Relocate only this inert fixture output, preserving the real plan/pins.
    selected_output = Path(plan["output"])
    assert not selected_output.exists() and not selected_output.is_symlink()
    specimen.output.rename(selected_output)
    specimen.output = selected_output
    application = plan["application"]
    before = specimen.setup_value
    members = deepcopy(application["members"])
    for member, old in zip(members, before["native_members"]):
        member["initial_surface"] = deepcopy(old["initial_surface"])
    helper_index = next(i for i, member in enumerate(members)
        if member["member"] == application["helper"]["member"])
    members[helper_index]["member"]["role"] = "inert-native-projected-helper-role"
    human = plan["owner"]["native_host"]["remote_operator"]
    for request in plan["authorities"]["agent_setup"]:
        member = next(row for row in members if row["member"] == request["member"])
        member["setup_capability"] = {"route": request["route"],
            "parent": {"inert_native_parent": "original-agent-setup"}}
        if request.get("helper_targets"):
            def parent(issuance_id):
                return {"grant_id": issuance_id, "root_authority": deepcopy(human),
                    "inert_native_specimen": "original-configured-target"}
            member["setup_capability"]["parent"] = parent(request["issuance_id"])
            member["setup_capability"]["helper_targets"] = [{"target": deepcopy(row["target"]),
                "parent": parent(row["issuance_id"]),
                "restrictions": {"inert_native_specimen": "original-target-restrictions"}}
                for row in request["helper_targets"]]
    helper = deepcopy(application["helper"])
    helper["member"] = deepcopy(members[helper_index]["member"])
    helper["controller_parent"] = [{"inert_native_parent": "original-helper-controller"}]
    helper["report_capability"] = {"parent": [{"inert_native_parent": "original-report"}],
        "public_parent": [{"inert_native_parent": "original-public-service"}]}
    def lineage(request):
        return [{"grant_id": request["issuance_id"], "root_authority": deepcopy(human)}]
    helper["operation_basis"] = {"kind": "targeted_grant_lineages",
        "base": lineage(plan["authorities"]["helper_catalog"]),
        "targets": [{"target": {"kind": "mcp_server", "server": "inert-original-helper-server"},
            "lineage": lineage(plan["authorities"]["helper_mcp"])}]}
    native_owner = deepcopy(plan["owner"])
    native_owner["native_host"]["members"] = [
        {key: value for key, value in member.items() if key != "initial_surface"}
        for member in members]
    for route in native_owner["native_host"]["routes"]:
        route["controller_parent"] = [{"inert_native_parent": "original-route-controller:" + route["id"]}]
    helper_roster = next(row for row in application["roster"]
        if row["identity"] == f.host._inventory["helper_identity"])
    specimen.setup_value = {**before, "realm": {"user_config_root": None, **deepcopy(application["realm"])},
        "definition_source": deepcopy(application["definition"]),
        "source_helper_roster": {**before["source_helper_roster"], **deepcopy(helper_roster)},
        "native_members": members, "source_helper_member": deepcopy(application["helper"]["member"]),
        "helper": helper, "helper_tools": application["helper_tools"],
        "helper_profile": application["helper_profile"], "native_owner": native_owner,
        "server": {"name": "inert-original-helper-server"}}
    specimen.publish()
    assert specimen.manifest.path.parent == Path(plan["output"])
    return specimen


async def finalize_original(f, monkeypatch):
    specimen = completed_specimen(f)
    admission, _ = bootstrap.retain_bootstrap_admission(f.directory,
        deployment.read_receipt(f.directory), f.fresh)
    factory = bootstrap.BootstrapFactory(f.directory, admission)
    remote = ModuleType("remote_operations")
    remote.__file__ = str(f.destination / "remote_operations.py")
    remote.command = lambda *args, **kwargs: pytest.fail("No application command is selected by this fixture.")
    monkeypatch.setitem(sys.modules, "remote_operations", remote)
    executor = SimpleNamespace(f=SimpleNamespace(root=f.destination), commands=f.native_commands,
        on_prepare=lambda: None, on_validate=lambda: None, prepare_process=None, validate_process=None)
    async def execute(argv):
        # Reuse the original prepare/validate executor, retaining this command's
        # actual source acquisition reference in its inert compiled output.
        # No publisher, receipt or completed-bundle authentication is mocked.
        original_write = finalizer_fixture.write
        arguments = dict(zip(argv[2::2], argv[3::2]))
        def native_output(path, data):
            if path.name == "toolkit-local-policy.json":
                data = Path(arguments["--policy"]).read_bytes()
            return original_write(path, data)
        with monkeypatch.context() as patch:
            patch.setattr(finalizer_fixture, "write", native_output)
            return await FinalizerFixture.execute(executor, argv)
    with project_lock(f.destination) as lease:
        async with factory(lease) as startup:
            application = SimpleNamespace(context=SimpleNamespace(root=f.destination, runtime=None),
                dynamic=False, _application_callbacks=None)
            await startup.prepare(application)
            startup.caller.execute = execute
            f.finalizer = startup.finalizer
            f.completed_selection = await startup.finalizer.finalize_selection(specimen.manifest)
    assert factory.source._closed and lease._closed
    f.specimen, f.admission = specimen, admission
    assert [command[1] for command in f.native_commands] == [
        "--prepare-application-policy", "--validate-application-policy"]
    assert json.loads((f.inputs.fresh_paths["working_directory"] / "finalization.outcome.json").read_bytes())["status"] == "published"
    assert (f.destination / "current").resolve() == f.inputs.fresh_paths["activation_path"]
    return f.completed_selection


async def publish(f):
    async with f.service.lock:
        return await f.owner.retain_restore(f.service, f.guide_id, f.directory.name)


async def action(f, command, request_id=None):
    return await f.service.action(f.guide_id, f.directory.name, command,
        action_request_id=request_id or str(uuid4()))


def action_binding(f, command, request_id):
    return {"guide_id": f.guide_id, "deployment_id": f.directory.name,
        "action": command, "action_request_id": request_id, "update_id": None}


def restore_value(f):
    """Exact expected references, never offered as completed-native proof."""
    path = f.inputs.fresh_paths["working_directory"] / "selection.json"
    setup = next(row for row in f.specimen.manifest_value["artifacts"]
        if row["path"] == "application-setup.json")
    return {"kind": "restore", "gateway": f.fresh["gateway"],
        "selection_file": {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()},
        "preparation_plan": f.fresh["preparation_plan"],
        "completed_setup": {"path": str(f.inputs.fresh_paths["bundle_path"] / setup["path"]),
            "sha256": setup["sha256"]}}


async def test_first_finish_without_fresh_entry_creates_no_restore(local_restore):
    f = local_restore
    before = source_bytes(f.setup_root), source_bytes(f.directory)
    assert await publish(f) is None
    assert (source_bytes(f.setup_root), source_bytes(f.directory)) == before
    assert f.owner.startup_selection(f.guide_id, f.directory.name) == f.fresh_pin
    assert not f.native_commands and not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
    # The existing ID-less Stop branch is also read-only before a Fresh entry.
    stopped = await f.service.action(f.guide_id, f.directory.name, "stop")
    assert stopped["status"] == "stopped" and f.owner._restore_pin is None
    assert (source_bytes(f.setup_root), source_bytes(f.directory)) == before


async def test_actual_finalizer_history_yields_same_restore_without_native_reexecution(local_restore, monkeypatch):
    f = local_restore
    await finalize_original(f, monkeypatch)
    before = source_bytes(f.setup_root), f.fresh_pin.read(), tuple(f.native_commands)
    # A GET observes the entered Fresh attempt; it cannot publish continuation.
    status = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert not status["available"] and status["reason"] == "previous_attempt"
    assert (source_bytes(f.setup_root), f.fresh_pin.read(), tuple(f.native_commands)) == before
    pin = await publish(f)
    value = json.loads(pin.read())
    assert value["kind"] == "restore"
    assert value["gateway"] == f.fresh["gateway"] and value["preparation_plan"] == f.fresh["preparation_plan"]
    assert Path(value["selection_file"]["path"]) == f.inputs.fresh_paths["working_directory"] / "selection.json"
    setup = next(row for row in f.specimen.manifest_value["artifacts"]
        if row["path"] == "application-setup.json")
    assert value["completed_setup"] == {"path": str(f.inputs.fresh_paths["bundle_path"] / setup["path"]),
        "sha256": setup["sha256"]}
    bootstrap.require_restore_history_current(f.directory, value)
    assert f.owner.startup_selection(f.guide_id, f.directory.name) == pin
    assert await publish(f) == pin
    assert len(f.native_commands) == 2 and f.fresh_pin.read() == before[1]
    after = source_bytes(f.setup_root)
    status = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["available"] and status["kind"] == "restore"
    assert source_bytes(f.setup_root) == after and len(f.native_commands) == 2


@pytest.mark.parametrize("changed", ["missing-outcome", "unknown-outcome", "selection", "current",
    "source-policy", "completed-setup", "prepare-stream", "validation-status"])
async def test_unverified_or_changed_original_finalizer_never_publishes_restore(local_restore, monkeypatch, changed):
    f = local_restore
    await finalize_original(f, monkeypatch)
    work = f.inputs.fresh_paths["working_directory"]
    if changed == "missing-outcome":
        (work / "finalization.outcome.json").unlink()
    elif changed == "unknown-outcome":
        value = json.loads((work / "finalization.outcome.json").read_bytes())
        value["status"] = "outcome_unknown"
        (work / "finalization.outcome.json").write_text(json.dumps(value))
    elif changed == "current":
        (f.destination / "current").unlink()
        (f.destination / "current").symlink_to("unselected-activation.json")
    elif changed in {"prepare-stream", "validation-status"}:
        path = work / ("prepare.json.outcome.json" if changed == "prepare-stream" else "validate.json.outcome.json")
        value = json.loads(path.read_bytes())
        if changed == "prepare-stream":
            value["process"]["stdout_base64"] = "e30K"
        else:
            value["status"] = "outcome_unknown"
        path.write_text(json.dumps(value))
    else:
        path = {"selection": work / "selection.json", "source-policy": f.policy.path,
            "completed-setup": f.specimen.output / "application-setup.json"}[changed]
        path.write_bytes(path.read_bytes() + b" ")
    before = source_bytes(f.setup_root)
    with pytest.raises((CustodyError, DraftConflict, OSError)):
        await publish(f)
    assert source_bytes(f.setup_root) == before and len(f.native_commands) == 2


@pytest.mark.parametrize("unfinished", ["child", "preparation", "support", "update"])
async def test_unfinished_original_installation_custody_cannot_publish_restore(local_restore, monkeypatch, unfinished):
    f = local_restore
    await finalize_original(f, monkeypatch)
    if unfinished == "child":
        f.inputs.supervisor.child = object()
    elif unfinished == "preparation":
        f.service._application_preparations[f.directory.name] = object()
    elif unfinished == "support":
        # Retained but unreadable support custody is deliberately pending.
        deployment.update_receipt(f.directory, supervisor_refresh={"revision": f.directory.name,
            "descriptor_sha256": "0" * 64})
    else:
        deployment.update_receipt(f.directory, update={"status": "applying"})
    before = source_bytes(f.setup_root)
    try:
        with pytest.raises((CustodyError, DraftConflict)):
            await publish(f)
        assert source_bytes(f.setup_root) == before and len(f.native_commands) == 2
    finally:
        f.inputs.supervisor.child = None
        f.service._application_preparations.clear()


@pytest.mark.parametrize("changed_lifecycle", [False, True])
async def test_normal_stop_publishes_restore_for_get_and_next_uuid_start(local_restore, monkeypatch,
        changed_lifecycle):
    f = local_restore
    await finalize_original(f, monkeypatch)
    if changed_lifecycle:
        # Real Start owns a new worker lifecycle; the retained Install
        # Supervisor still describes the original installation worker.
        lifecycle = str(uuid4())
        receipt = deployment.update_receipt(f.directory, interactive_lifecycle_id=lifecycle,
            interactive_cleanup_attempt_id=None, interactive_cleanup=None)
        assert f.inputs.supervisor.receipt.get("interactive_lifecycle_id") != lifecycle
        # Native execution is outside this fixture. Supply its inert exited
        # cleanup specimen, bound to the real installed source and new worker.
        cleanup = {key: receipt.get(key) for key in ("deployment_id", "guide_id", "build_id",
            "guide_version", "blueprint_hash", "destination", "package_sha256", "startup_attempt_id")}
        cleanup.update(deployment.native_fields(receipt))
        cleanup.update(deployment.interactive_source_binding(f.directory, receipt))
        cleanup.update(lifecycle_id=lifecycle, confirmed=True, worker_exit_confirmed=True,
            supervisor_pid=f.process.pid, return_code=0, boundary="owned-process-groups-absent")
        receipt = deployment.update_receipt(f.directory, interactive_cleanup=cleanup)
        assert deployment.interactive_cleanup_matches(receipt)
        # Initial preparation still requires its original worker binding.
        with pytest.raises(CustodyError, match="original stopped product owner"):
            f.host._selected_preparation(f.service, f.guide_id, f.directory.name)
    original_entry = (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()
    stop_id = str(uuid4())
    stopped = await action(f, "stop", stop_id)
    assert stopped["action_request"]["phase"] == "settled"
    assert stopped["action_request"]["outcome"] == "returned"
    assert deployment.interactive_cleanup_matches(deployment.read_receipt(f.directory))
    pin = f.owner._restore_pin
    assert pin is not None and json.loads(pin.read()) == restore_value(f)
    observed = source_bytes(f.setup_root), pin.path.stat().st_ino, tuple(f.native_commands)
    status = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["available"] and status["kind"] == "restore"
    assert (source_bytes(f.setup_root), pin.path.stat().st_ino, tuple(f.native_commands)) == observed
    # Lost Stop replies return the original journal before either callback.
    original_callback = f.service.application_setup_continuation
    async def replay_forbidden(*args):
        pytest.fail("A settled action UUID must not select or publish again.")
    monkeypatch.setattr(f.service, "application_setup_continuation", replay_forbidden)
    repeated = await action(f, "stop", stop_id)
    assert repeated["action_request"] == stopped["action_request"]
    monkeypatch.setattr(f.service, "application_setup_continuation", original_callback)
    start_id = str(uuid4())
    started = await action(f, "finish-setup", start_id)
    assert started["action_request"]["outcome"] == "returned"
    assert len(f.service.launched) == 1
    directory, mode, options = f.service.launched[0]
    assert directory == f.directory and mode == "start"
    admitted = bootstrap.read_bootstrap_admission(directory, options["bootstrap_selection"])
    assert admitted["application_startup"] == json.loads(pin.read())
    monkeypatch.setattr(f.service, "application_setup_continuation", replay_forbidden)
    repeated = await action(f, "finish-setup", start_id)
    assert repeated["action_request"] == started["action_request"]
    assert len(f.service.launched) == 1 and len(f.native_commands) == 2
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == original_entry


@pytest.mark.parametrize("command", ["stop", "finish-setup", "start-background"])
async def test_pending_original_uuid_never_enters_continuation_or_launch(local_restore, monkeypatch, command):
    f = local_restore
    request_id = str(uuid4())
    async with f.service.lock:
        pending, claimed = f.service._claim_action_request(action_binding(f, command, request_id))
    assert claimed and pending["phase"] == "pending"
    async def forbidden(*args):
        pytest.fail("A pending UUID cannot select or create native continuation.")
    monkeypatch.setattr(f.service, "application_setup_continuation", forbidden)
    before = source_bytes(f.setup_root)
    result = await action(f, command, request_id)
    assert result["action_request"] == pending
    assert source_bytes(f.setup_root) == before and not f.native_commands and not f.service.launched


async def test_cancelled_restore_publication_rejoins_exact_file_on_new_action(local_restore, monkeypatch):
    f = local_restore
    await finalize_original(f, monkeypatch)
    written, release = threading.Event(), threading.Event()
    original_write = restore_publisher._write_bytes
    writes = []
    def held_write(path, content):
        pin = original_write(path, content)
        writes.append(pin)
        written.set()
        assert release.wait(5), "The fixture must release its original publication worker."
        return pin
    monkeypatch.setattr(restore_publisher, "_write_bytes", held_write)
    request_id = str(uuid4())
    task = asyncio.create_task(action(f, "finish-setup", request_id))
    try:
        assert await asyncio.wait_for(asyncio.to_thread(written.wait, 5), 6)
        assert len(writes) == 1 and f.owner._restore_pin is None
        inode, content = writes[0].path.stat().st_ino, writes[0].read()
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and f.service.lock.locked()
        with pytest.raises(RuntimeError, match="running"):
            with project_lock(f.destination):
                pytest.fail("Cancellation released the original publication source lease.")
    finally:
        release.set()
        if not task.done():
            task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(task, 6)
    assert f.owner._restore_pin is None and not f.service.launched
    before = source_bytes(f.setup_root)
    status = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert not status["available"] and status["reason"] == "previous_attempt"
    assert source_bytes(f.setup_root) == before and f.owner._restore_pin is None
    monkeypatch.setattr(restore_publisher, "_write_bytes",
        lambda *args: pytest.fail("Reentry must authenticate and retain the same written file."))
    # The unresolved old UUID observes its original pending request only.
    old = await action(f, "finish-setup", request_id)
    assert old["action_request"]["phase"] == "pending" and f.owner._restore_pin is None
    stopped = await action(f, "stop")
    assert stopped["action_request"]["outcome"] == "returned"
    pin = f.owner._restore_pin
    assert pin == writes[0] and pin.read() == content and pin.path.stat().st_ino == inode
    before = source_bytes(f.setup_root)
    status = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["available"] and status["kind"] == "restore" and source_bytes(f.setup_root) == before
    await action(f, "finish-setup")
    assert len(f.service.launched) == 1 and len(f.native_commands) == 2


@pytest.mark.parametrize("existing", ["foreign-bytes", "exact-without-outcome", "symlink"])
async def test_existing_restore_file_is_not_original_native_completion_proof(local_restore, monkeypatch, existing):
    f = local_restore
    await finalize_original(f, monkeypatch)
    path = f.inputs.startup_selection_path.parent / "restore-selection.json"
    value = restore_value(f)
    content = (json.dumps(value, ensure_ascii=True, allow_nan=False,
        sort_keys=True, separators=(",", ":")) + "\n").encode()
    if existing == "foreign-bytes":
        path.write_bytes(content + b" ")
        path.chmod(0o600)
    elif existing == "exact-without-outcome":
        path.write_bytes(content)
        path.chmod(0o600)
        (f.inputs.fresh_paths["working_directory"] / "finalization.outcome.json").unlink()
    else:
        path.symlink_to(f.fresh_pin.path)
    identity = path.lstat().st_dev, path.lstat().st_ino, path.lstat().st_mode
    target = path.readlink() if path.is_symlink() else None
    before = source_bytes(f.setup_root)
    with pytest.raises((CustodyError, DraftConflict, OSError)):
        await publish(f)
    assert source_bytes(f.setup_root) == before
    assert (path.lstat().st_dev, path.lstat().st_ino, path.lstat().st_mode) == identity
    assert (path.readlink() if path.is_symlink() else None) == target
    assert f.owner._restore_pin is None and len(f.native_commands) == 2


@pytest.mark.parametrize("configured", [False, True])
def test_actual_server_selection_retains_continuation_identity_and_default_none(local, api, configured):
    f = local
    if configured:
        host = f.select()
        owner = f.service
        for name in ("application_setup_selection", "application_setup_preparation",
                "application_setup_retention", "application_setup_continuation"):
            setattr(owner, name, None)
    else:
        host = ForgeApplicationHost(guide_id=f.guide_id, runtime_target=f.target,
            acquisition_selection=f.acquisition, viewer_selection=f.viewer, require_current=lambda: None)
        owner = api.deployment
    api.namespace["deployment"] = owner
    select = api.namespace["select_application_host"]
    select(host)
    expected = host.continue_application_startup if configured else None
    assert owner.application_setup_continuation == expected
    assert owner.application_setup_selection == host._selected_startup
    select(host)
    assert owner.application_setup_continuation == expected
    owner.application_setup_continuation = lambda *args: pytest.fail("Selection must not invoke a foreign owner.")
    with pytest.raises(DraftConflict, match="callbacks changed"):
        select(host)
    assert api.calls == [] and not f.setup_root.exists() and not f.calls
