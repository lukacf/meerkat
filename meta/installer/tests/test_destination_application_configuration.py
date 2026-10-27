"""Real byte Install and None,None guest settings with inert native boundaries.

Source/artifact intake, canonical receipt/package, actual factory/host, project
lease, private settings writer and UUID journal remain production code. Reused
venv/pip/check specimens are inert and do not qualify SDK/native/cloud behavior.
"""
import asyncio
from copy import deepcopy
import json
import threading
from uuid import uuid4

import pytest

from launchkit import deployment, product_installed_source as installed
from launchkit.connect_installer_caller import CustodyError
from launchkit.projects import DraftConflict
from launchkit.remote_product_artifacts import ProductArtifactTransferError
from launchkit.templates.storage_setup import project_lock
from test_destination_application_inputs import inputs_destination, install, select_caller, snapshot
from test_destination_application_factory import destination
# Reuse the existing accepted workspace scaffold fixture before first staging.
from test_product_installed_inputs import installation
from test_product_destination_install import destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution
from test_meta_toolkit_runtime_selection import no_product_execution


async def guest(f):
    f.service = deployment.DeploymentService(f.selected.custody.home / ".local/state/meta-forge", None, None)
    owner, _ = await install(f)
    assert f.service.guide is None and f.service.implementation is None
    return owner


def arguments(f, **changes):
    value = {"version": f.selected.context["guide_version"],
        "build_id": f.selected.context["build_id"],
        "source_digest": f.selected.context["source_digest"],
        "package_sha256": f.selected.context["package_sha256"]}
    return {**value, **changes}


async def configure(f, request=None, **kwargs):
    return await f.factory.configure(f.service, f.factory.guide_id, f.factory.deployment_id,
        action_request_id=request or str(uuid4()), **arguments(f), **kwargs)


def retained(f):
    return {name: (f.root / name).read_bytes() for name in f.selected.files}, (f.directory / "package.zip").read_bytes()


def assert_stopped(f):
    receipt = deployment.read_receipt(f.directory)
    assert receipt["status"] == "stopped" and receipt["runtime_pid"] is None
    assert not f.service.tasks and not f.service.processes and not f.inventory_calls
    assert not f.held.inventory_attempt_path.exists() and not f.held.preparation_plan_path.exists()
    assert not f.held.startup_selection_path.exists()
    assert f.factory._source is None and f.factory.host._product_source_custody._closed
    with project_lock(f.root) as lease:
        lease.require_current()


async def test_real_guest_status_and_configure_preserve_original_source_and_workspace_role(
        inputs_destination, monkeypatch, tmp_path):
    f = inputs_destination
    owner = await guest(f)
    saved = retained(f)
    reference = f.root / "workspace" / "original.txt"
    reference.write_text("Original connected reference, not application source.\n")
    reference.chmod(0o600)
    original_writer = deployment.replace_private_configuration
    observations = []

    def write(destination, values, expected):
        assert threading.get_ident() != f.thread
        with pytest.raises(RuntimeError):
            asyncio.get_running_loop()
        source = f.factory._source
        assert source._installed_view._configuration == expected
        assert source._lease.root == f.root and not source._closed and not source._invalidated
        source.require_filesystem_current()
        original_writer(destination, values, expected)
        observations.append(source)

    monkeypatch.setattr(deployment, "replace_private_configuration", write)
    before = snapshot(f.root)
    for _ in range(2):
        async with asyncio.timeout(20):
            status = await f.factory.setup_status(f.service, f.factory.guide_id, f.factory.deployment_id)
            options = await f.factory.settings_options(f.service, f.factory.guide_id, f.factory.deployment_id)
        assert status["available"] and status["action"] == "prepare-setup"
        assert options["version"] == f.selected.context["guide_version"]
        assert options["source_digest"] == f.selected.context["source_digest"]
        assert options["source_verified"] and options["requires_stopped"]
        assert f.settings["CONSOLE_LOGIN_PASSWORD"] not in json.dumps(options)
    assert snapshot(f.root) == before
    external = tmp_path / "chosen-reference-folder"
    external.mkdir(mode=0o700)
    new_password = "new-private-guest-password-canary"
    result = await configure(f, settings={"CONSOLE_LOGIN_PASSWORD": new_password,
        "FILES_ROOT": str(external), "MOBKIT_PORT": "19973"})
    assert result["action_request"]["phase"] == "settled" and result["action_request"]["outcome"] == "returned"
    assert result["health"] == "unknown" and result["startup_attempt_id"] is None
    assert new_password not in json.dumps(result)
    assert len(observations) == 1 and observations[0]._closed and not observations[0]._invalidated
    values, private = deployment.private_configuration_snapshot(f.root)
    assert values["CONSOLE_LOGIN_PASSWORD"] == new_password and values["FILES_ROOT"] == str(external)
    assert values["MOBKIT_PORT"] == "19973" and private[0] == deployment._serialized_private_configuration(values).encode()
    assert (f.root / ".env").stat().st_mode & 0o777 == 0o600
    options = await f.factory.settings_options(f.service, f.factory.guide_id, f.factory.deployment_id)
    assert "FILES_ROOT" in {row["key"] for row in options["fields"]}
    assert retained(f) == saved and reference.read_text().startswith("Original connected reference")
    assert owner is f.input_owner and f.held is owner.preparation_owner._inputs
    assert_stopped(f)
    for path in f.service.root.rglob("*.json"):
        assert new_password not in path.read_text()


@pytest.mark.parametrize("settings,removed,reason", [
    ({"CONSOLE_USER_ID": "foreign-operator"}, None, "Console user"),
    ({"UNREQUESTED_SECRET": "refused-private-canary"}, None, "not requested"),
    ({}, ["CONSOLE_LOGIN_PASSWORD"], "Only optional"),
    ({"MOBKIT_PORT": "12"}, None, "Console port"),
])
async def test_guest_configure_uses_original_subject_field_and_port_validation(
        inputs_destination, settings, removed, reason):
    f = inputs_destination
    await guest(f)
    before, saved = deployment.private_configuration_snapshot(f.root), retained(f)
    request = str(uuid4())
    with pytest.raises((CustodyError, DraftConflict, ValueError), match=reason):
        await configure(f, request, settings=settings, remove_fields=removed)
    assert deployment.private_configuration_snapshot(f.root) == before and retained(f) == saved
    value = (await f.service.action_request(f.factory.guide_id, f.factory.deployment_id, request))["action_request"]
    assert value["phase"] == "settled" and value["outcome"] == "error"
    assert_stopped(f)


async def test_guest_configure_exact_noop_and_same_uuid_never_reopens_or_writes(inputs_destination, monkeypatch):
    f = inputs_destination
    await guest(f)
    before = snapshot(f.root), (f.directory / "receipt.json").read_bytes()
    request = str(uuid4())
    first = await configure(f, request, settings={"CONSOLE_LOGIN_PASSWORD": ""})
    assert before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())
    def no_operation(*args, **kwargs):
        pytest.fail("A settled or pending Configure UUID cannot reopen a source operation.")
    monkeypatch.setattr(f.factory, "_operation", no_operation)
    again = await configure(f, request, settings={"CONSOLE_LOGIN_PASSWORD": "must-not-be-written"})
    assert again == first and before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())
    pending = str(uuid4())
    binding = {"action_request_id": pending, "guide_id": f.factory.guide_id,
        "deployment_id": f.factory.deployment_id, "action": "configure", "update_id": None,
        "guide_version": arguments(f)["version"], **{k: v for k, v in arguments(f).items() if k != "version"}}
    f.service._claim_action_request(binding)
    answer = await configure(f, pending, settings={"CONSOLE_LOGIN_PASSWORD": "must-not-be-written"})
    assert answer["action_request"]["phase"] == "pending"
    assert before == (snapshot(f.root), (f.directory / "receipt.json").read_bytes())
    assert_stopped(f)


@pytest.mark.parametrize("change", ["source-bytes", "source-permissions", "private-permissions", "lost-owner"])
async def test_guest_readback_and_configure_refuse_original_source_or_private_owner_drift(inputs_destination, change):
    f = inputs_destination
    await guest(f)
    before = (f.root / ".env").read_bytes()
    if change == "source-bytes":
        (f.root / "app.py").write_bytes((f.root / "app.py").read_bytes() + b"# foreign source\n")
    elif change == "source-permissions":
        (f.root / "app.py").chmod(0o664)
    elif change == "private-permissions":
        (f.root / ".env").chmod(0o644)
    else:
        f.current = False
    with pytest.raises((CustodyError, DraftConflict, ProductArtifactTransferError, ValueError)):
        await f.factory.settings_options(f.service, f.factory.guide_id, f.factory.deployment_id)
    with pytest.raises((CustodyError, DraftConflict, ProductArtifactTransferError, ValueError)):
        await configure(f, settings={"CONSOLE_LOGIN_PASSWORD": "must-not-be-written"})
    assert (f.root / ".env").read_bytes() == before
    assert not f.service.tasks and not f.service.processes and not f.inventory_calls
    with project_lock(f.root) as lease:
        lease.require_current()


async def test_guest_configure_rejects_foreign_installation_or_request_binding_before_write(inputs_destination):
    f = inputs_destination
    await guest(f)
    before = snapshot(f.root)
    request = str(uuid4())
    for gid, did, service in ((str(uuid4()), f.factory.deployment_id, f.service),
            (f.factory.guide_id, str(uuid4()), f.service),
            (f.factory.guide_id, f.factory.deployment_id,
                deployment.DeploymentService(f.selected.custody.home / "foreign-service", None, None))):
        with pytest.raises(CustodyError, match="original installation|another deployment"):
            await f.factory.configure(service, gid, did, action_request_id=request, **arguments(f))
    with pytest.raises(CustodyError, match="original installed source"):
        await f.factory.configure(f.service, f.factory.guide_id, f.factory.deployment_id,
            action_request_id=request, **arguments(f, version=arguments(f)["version"] + 1))
    with pytest.raises(ValueError, match="canonical action request"):
        await configure(f, "not-a-uuid")
    assert not f.service._action_request_path(request).exists() and snapshot(f.root) == before
    assert_stopped(f)


async def test_guest_configure_cancellation_joins_exact_writer_and_keeps_uuid_pending(inputs_destination, monkeypatch):
    f = inputs_destination
    await guest(f)
    original = deployment.replace_private_configuration
    entered, release = threading.Event(), threading.Event()
    writes = []
    request = str(uuid4())
    def held(destination, values, expected):
        assert threading.get_ident() != f.thread
        entered.set()
        assert release.wait(10)
        original(destination, values, expected)
        writes.append((destination, dict(values)))
    monkeypatch.setattr(deployment, "replace_private_configuration", held)
    async def begin():
        select_caller(f)
        await configure(f, request, settings={"CONSOLE_LOGIN_PASSWORD": "cancelled-reply-private-canary"})
    task = asyncio.create_task(begin())
    try:
        async with asyncio.timeout(20):
            while not entered.is_set():
                if task.done():
                    await task
                await asyncio.sleep(0.01)
            task.cancel()
            await asyncio.sleep(0.01)
            task.cancel()
            assert not task.done() and f.factory._source is not None
            with pytest.raises(RuntimeError, match="running"):
                with project_lock(f.root):
                    pytest.fail("Cancellation discarded the actual Configure writer's lease.")
            release.set()
            with pytest.raises(asyncio.CancelledError):
                await task
    finally:
        release.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)
    assert len(writes) == 1
    assert deployment.private_configuration_snapshot(f.root)[0]["CONSOLE_LOGIN_PASSWORD"] == "cancelled-reply-private-canary"
    value = (await f.service.action_request(f.factory.guide_id, f.factory.deployment_id, request))["action_request"]
    assert value["phase"] == "pending" and value["outcome"] is None
    select_caller(f)
    result = await configure(f, request, settings={"CONSOLE_LOGIN_PASSWORD": "never-replay"})
    assert result["action_request"] == value and len(writes) == 1
    assert_stopped(f)


async def test_authorized_writer_cannot_renew_closed_or_invalidated_source(inputs_destination):
    f = inputs_destination
    await guest(f)
    before = deployment.private_configuration_snapshot(f.root)
    async with f.service.lock:
        async with f.factory._operation(installed=True):
            closed = f.factory._source
            assert type(closed._installed_view) is installed._InstalledSourceView
            assert not closed._closed and not closed._lease._closed
            closed.require_owner_current()
        assert closed._closed and closed._lease._closed and f.factory._source is None
        with pytest.raises(ProductArtifactTransferError,
                match="^The original stopped source lease is no longer current\\.$"):
            await f.service._configuration_validation(closed,
                installed.replace_installed_private_configuration, closed, before[0], before[1])
        with pytest.raises(ProductArtifactTransferError,
                match="^The original source custody is closed or changed\\.$"):
            installed.replace_installed_private_configuration(closed, before[0], before[1])
    assert deployment.private_configuration_snapshot(f.root) == before
    assert closed._closed and closed._lease._closed and closed._invalidated
    async with f.service.lock:
        with pytest.raises(ProductArtifactTransferError, match="no longer current"):
            async with f.factory._operation(installed=True):
                source = f.factory._source
                values, old = deployment.private_configuration_snapshot(f.root)
                values["FILES_ROOT"] = "/foreign-connected-input"
                deployment.replace_private_configuration(f.root, values, old)
                with pytest.raises((CustodyError, ProductArtifactTransferError), match="private configuration"):
                    installed.replace_installed_private_configuration(source, values,
                        deployment.private_configuration_snapshot(f.root)[1])
                assert source._invalidated
    assert source._closed and source._invalidated and source._lease._closed
    assert deployment.private_configuration_snapshot(f.root)[0]["FILES_ROOT"] == "/foreign-connected-input"
    with project_lock(f.root) as lease:
        lease.require_current()
