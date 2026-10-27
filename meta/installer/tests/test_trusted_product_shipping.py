"""Actual trusted multi-target Build/installed owners with inert boundaries.

Real GuideStore/services/workspaces/signed source/byte readers are reused.
The selected SDK/native bytes and control wheels are poisoned inert specimens.
External builder, SDK proof and installer exit are explicit existing fixtures.
Server handlers are extracted, never imported with their global runtime.
No network, cloud, application, SDK or native execution occurs in this family.
"""
import ast
import asyncio
from copy import deepcopy
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from launchkit import application_host_launch as launch
from launchkit import deployment
from launchkit.blueprint import Blueprint
from launchkit.connect_installer_caller import CustodyError
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.projects import DraftConflict
from launchkit.remote_deployment import RemoteDeploymentService
from launchkit.remote_product_control import ProductControlSelection
from launchkit.runtime_candidates import ArtifactFile
from launchkit.runtime_selection import MetaToolkitRuntimeTarget
import test_meta_toolkit_runtime_selection as specimens
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_meta_toolkit_runtime_update import catalog_target
from test_selected_dependency_install import inert_wheel
from test_local_application_dispatch import dispatch, initial, CALLBACKS
from test_local_application_registry import (registry, new_guide, build, workspace,
    accept_source_fixture, install_fixture, source_bytes)
from test_local_application_recovery import restarted_installation, stop

pytestmark = pytest.mark.usefixtures("no_product_execution")


@pytest.fixture
def linux_target(product, tmp_path, monkeypatch):
    # The ordinary Mac specimen is already constructed. Build a separate full
    # Linux specimen before any source reader runs, not an equal-hash relabel.
    parent = tmp_path / "separate-linux-producer"
    parent.mkdir(mode=0o700)
    with monkeypatch.context() as selected:
        selected.setattr(specimens, "TARGET", "linux-x86_64")
        selected.setattr(specimens, "PREFIX", "meerkat-connect-linux-x86_64")
        selected.setattr(specimens, "RESOURCE", "meerkat-connect-linux-x86_64/libexec/meerkat-connect")
        producer = specimens.product.__wrapped__(parent)
        original = next(producer)
        target = catalog_target(original)
    assert product.target.target == "macos-arm64" and target.target == "linux-x86_64"
    yield target
    with pytest.raises(StopIteration):
        next(producer)


@pytest.fixture
def remote_control(linux_target, tmp_path):
    root = tmp_path / "private-explicit-control-graph"
    root.mkdir(mode=0o700)
    wheels, lines = [], []
    for name in ("pydantic", "pydantic-core", "httpx"):
        specimen = inert_wheel(name, "1.0")
        path = root / specimen.filename
        path.write_bytes(specimen.content)
        path.chmod(0o600)
        artifact = ArtifactFile(path, len(specimen.content), specimens.digest(specimen.content))
        wheels.append(artifact)
        lines.append(name + "==1.0 --hash=sha256:" + artifact.sha256)
    path = root / "requirements.txt"
    content = ("\n".join(lines) + "\n").encode()
    path.write_bytes(content)
    path.chmod(0o600)
    state = SimpleNamespace(current=True, calls=[])
    def current():
        state.calls.append(asyncio.current_task() if _has_loop() else None)
        return state.current
    selection = ProductControlSelection(runtime_target=linux_target,
        requirements=ArtifactFile(path, len(content), specimens.digest(content)),
        wheels=tuple(wheels), python_tag="cp312", abi_tag="cp312", glibc_minimum=(2, 31),
        require_current=current)
    return SimpleNamespace(selection=selection, state=state)


def _has_loop():
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return False
    return True


def bind_registry(f, control, *, local=None):
    assert not f.registry._builds and not f.registry._installations
    owner = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=local or f.target, remote_control_selection=control)
    f.implementation.prepare_workspace = owner.prepare_workspace
    for name, method in (("application_host_selection", "selected_host"),
        ("application_setup_retention", "retain_installation_inputs"),
        ("application_setup_selection", "_selected_startup"),
        ("application_setup_preparation", "prepare_application"),
        ("application_setup_continuation", "continue_application_startup")):
        setattr(f.service, name, getattr(owner, method))
    f.registry = owner
    return owner


def target_plan(f, saved, target):
    current = f.store.get(saved["id"])
    current["proposal"]["target"] = target
    if target == "gcp":
        current["proposal"]["gcp"].update(project="inert-owned-cloud-project", zone="europe-west1-b")
    current["proposal"] = Blueprint.model_validate(current["proposal"]).model_dump(mode="json")
    return f.store.save(current, current["version"])


async def test_actual_build_changes_mac_to_gcp_without_rewriting_plan_or_old_source(registry, remote_control):
    f = registry
    owner = bind_registry(f, remote_control.selection)
    first = new_guide(f)
    a = await build(f, first)
    await accept_source_fixture(f, a)
    old = owner.host_for_build(first["id"], a["build_id"])
    before = source_bytes(f.implementation._directory(first["id"], a["build_id"]) / "local-application-source")
    gcp = target_plan(f, first, "gcp")
    assert owner.initial_build_options(gcp["id"], gcp["version"])["runtime_target"] is remote_control.selection.runtime_target
    b = await build(f, gcp, "Build the selected GCP application", previous=a)
    project = workspace(f.implementation, b)
    new = owner.host_for_build(gcp["id"], b["build_id"])
    assert new is not old and new._target == remote_control.selection.runtime_target
    assert new._blueprint == gcp["proposal"] and project.blueprint.target == "gcp"
    assert project.blueprint.gcp.model_dump(mode="json") == gcp["proposal"]["gcp"]
    for name, pin in new._protected:
        assert project.read_file(name).encode() == pin.read()
    assert owner._builds[(first["id"], a["build_id"])].host is old
    assert before == source_bytes(f.implementation._directory(first["id"], a["build_id"]) / "local-application-source")
    assert owner.runtime_target is f.target and owner.remote_runtime_target is remote_control.selection.runtime_target
    assert not remote_control.state.calls


async def test_installed_mac_owner_survives_new_gcp_build_and_unused_control_failure(registry, remote_control, monkeypatch):
    f = registry
    owner = bind_registry(f, remote_control.selection)
    first = new_guide(f)
    a = await build(f, first)
    await accept_source_fixture(f, a)
    old = owner.host_for_build(first["id"], a["build_id"])
    directory, destination = await install_fixture(f, first, a, monkeypatch)
    inputs = old._retained_preparation_inputs
    before = (source_bytes(destination / ".mobkit/forge-local-setup"),
        (directory / "package.zip").read_bytes(), (destination / ".env").read_bytes())
    gcp = target_plan(f, first, "gcp")
    b = await build(f, gcp, "Use the selected GCP destination", previous=a)
    await accept_source_fixture(f, b)
    remote_control.state.current = False
    assert owner.host_for_build(gcp["id"], b["build_id"])._target == remote_control.selection.runtime_target
    assert owner.selected_host(first["id"], directory.name) is old
    status = await owner.setup_status(f.service, first["id"], directory.name)
    assert status["available"] and status["action"] == "prepare-setup", (status.get("reason"), status.get("detail"))
    assert status["build_id"] == a["build_id"] and status["guide_version"] == first["version"]
    assert old._retained_preparation_inputs is inputs
    assert before == (source_bytes(destination / ".mobkit/forge-local-setup"),
        (directory / "package.zip").read_bytes(), (destination / ".env").read_bytes())
    assert not remote_control.state.calls


async def test_recovery_uses_installed_mac_blueprint_while_current_draft_selects_gcp(restarted_installation, remote_control):
    f = restarted_installation
    owner = bind_registry(f, remote_control.selection)
    gcp = target_plan(f, f.changed, "gcp")
    assert owner.initial_build_options(f.guide_id, gcp["version"])["runtime_target"] is remote_control.selection.runtime_target
    before = source_bytes(f.destination / ".mobkit/forge-local-setup"), (f.directory / "package.zip").read_bytes()
    hint = await owner.setup_status(f.service, f.guide_id, f.directory.name)
    assert hint["available"] is False and hint["reason"] == "owner_reconnect"
    result = await stop(f)
    host = owner.selected_host(f.guide_id, f.directory.name)
    assert result["action_request"]["phase"] == "settled" and host._target is f.target
    assert host._blueprint == f.old_plan and owner._builds[(f.guide_id, f.installed["build_id"])].factory._installed_admission.runtime_target is f.target
    assert before == (source_bytes(f.destination / ".mobkit/forge-local-setup"), (f.directory / "package.zip").read_bytes())
    assert f.accepted_reads and f.proof_reads and not remote_control.state.calls


@pytest.mark.parametrize("target", ["linux", "gcp"])
def test_current_blueprint_selects_linux_but_stale_version_and_changed_owner_refuse(registry, remote_control, target):
    f = registry
    owner = bind_registry(f, remote_control.selection)
    first = new_guide(f)
    selected = target_plan(f, first, target)
    assert owner.initial_build_options(selected["id"], selected["version"])["runtime_target"] is remote_control.selection.runtime_target
    with pytest.raises(CustodyError, match="changed|current|version"):
        owner.initial_build_options(first["id"], first["version"])
    owner.remote_runtime_target = replace(remote_control.selection.runtime_target)
    with pytest.raises(CustodyError, match="original services and source selections"):
        owner.initial_build_options(selected["id"], selected["version"])
    assert not f.implementation.root.exists() and not owner._builds and not remote_control.state.calls


def test_linux_local_selection_is_not_overwritten_by_separate_gcp_selection(registry, remote_control):
    f = registry
    local = replace(remote_control.selection.runtime_target)
    owner = bind_registry(f, remote_control.selection, local=local)
    saved = target_plan(f, new_guide(f), "linux")
    assert owner.initial_build_options(saved["id"], saved["version"])["runtime_target"] is local
    gcp = target_plan(f, saved, "gcp")
    assert owner.initial_build_options(gcp["id"], gcp["version"])["runtime_target"] is remote_control.selection.runtime_target
    assert not f.implementation.root.exists()


async def test_same_physical_product_binding_still_requires_original_refresh(registry, remote_control, tmp_path):
    f = registry
    first = new_guide(f)
    a = await build(f, first)
    await accept_source_fixture(f, a)
    old_owner = f.registry
    old = old_owner.host_for_build(first["id"], a["build_id"])
    before = source_bytes(f.implementation._directory(first["id"], a["build_id"]) / "local-application-source")
    parent = tmp_path / "separate-mac-producer"
    parent.mkdir(mode=0o700)
    producer = specimens.product.__wrapped__(parent)
    original = next(producer)
    try:
        changed_target = catalog_target(original)
        assert changed_target.target == f.target.target and changed_target != f.target
        owner = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
            deployment=f.service, runtime_target=changed_target)
        f.implementation.prepare_workspace = owner.prepare_workspace
        assert owner.initial_build_options(first["id"], first["version"])["runtime_target"] is changed_target
        with pytest.raises(DraftConflict, match="explicit runtime refresh owner"):
            await f.implementation.submit(first["id"], first["version"], "Change source",
                expected_selected_build_id=a["build_id"], runtime_target=changed_target)
        assert not owner._builds and old_owner.host_for_build(first["id"], a["build_id"]) is old
        assert before == source_bytes(f.implementation._directory(first["id"], a["build_id"]) / "local-application-source")
    finally:
        with pytest.raises(StopIteration):
            next(producer)


@pytest.fixture
def shipping_dispatch(dispatch, remote_control):
    f = dispatch
    remote = RemoteDeploymentService(f.implementation.root.parent, f.guide,
        f.implementation, f.deployment, transport=object())
    f.namespace["remote_deployment"] = remote
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    wanted = {"select_product_application_runtime", "_selected_remote_product_owner",
        "connect_gcp_deployment", "connect_remote_deployment", "install_remote_deployment"}
    nodes = [n for n in ast.parse(source.read_text()).body
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name in wanted]
    assert {n.name for n in nodes} == wanted
    for node in nodes:
        node.decorator_list = []
        for arg in node.args.args:
            arg.annotation = None
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), "exec"), f.namespace)
    f.remote = remote
    f.control = remote_control
    f.selection = launch.ApplicationRuntimeSelection(f.initial.target, remote_control.selection)
    return f


def test_exact_launcher_pair_retains_one_registry_and_both_original_targets(shipping_dispatch, monkeypatch):
    f = shipping_dispatch
    modules = {"trusted_shipping": SimpleNamespace(select=lambda: f.selection),
        "launchkit.forge_application_host": SimpleNamespace(ForgeApplicationHost=type("UnusedHost", (), {})),
        "launchkit.runtime_selection": SimpleNamespace(MetaToolkitRuntimeTarget=MetaToolkitRuntimeTarget),
        "launchkit.server": SimpleNamespace(app=f.app,
            select_product_application_runtime=f.namespace["select_product_application_runtime"])}
    monkeypatch.setenv(launch.FACTORY_ENV, "trusted_shipping:select")
    monkeypatch.setattr(launch.importlib, "import_module", modules.__getitem__)
    assert launch.create_app() is f.app
    owner = f.namespace["application_host"]
    assert type(owner) is LocalApplicationRegistry and owner.runtime_target is f.initial.target
    assert owner.remote_runtime_target is f.control.selection.runtime_target
    assert f.remote.product_control_selection is f.remote._product_control_selection is f.control.selection
    callbacks = (f.implementation.prepare_workspace, *(getattr(f.deployment, name) for name, _ in CALLBACKS))
    assert f.namespace["select_product_application_runtime"](f.selection) is owner
    assert callbacks == (f.implementation.prepare_workspace, *(getattr(f.deployment, name) for name, _ in CALLBACKS))
    assert not f.implementation.root.exists() and not owner._builds


@pytest.mark.parametrize("defect", ["lookalike", "remote-subclass", "stale", "reselect"])
def test_invalid_or_changed_trusted_pair_never_rebinds_original_callbacks(shipping_dispatch, defect):
    f = shipping_dispatch
    select = f.namespace["select_product_application_runtime"]
    if defect == "lookalike":
        with pytest.raises(TypeError, match="exact local"):
            launch.ApplicationRuntimeSelection(f.initial.target, SimpleNamespace(runtime_target=f.control.selection.runtime_target))
    elif defect == "remote-subclass":
        class Subclass(ProductControlSelection):
            pass
        with pytest.raises(TypeError, match="exact local"):
            launch.ApplicationRuntimeSelection(f.initial.target, object.__new__(Subclass))
    elif defect == "stale":
        f.control.state.current = False
        with pytest.raises(ValueError, match="original control selection owner changed"):
            select(f.selection)
    else:
        old = select(f.selection)
        before = (f.implementation.prepare_workspace, *(getattr(f.deployment, name) for name, _ in CALLBACKS))
        with pytest.raises(DraftConflict, match="original selected application runtime owner"):
            select(launch.ApplicationRuntimeSelection(replace(f.initial.target), f.control.selection))
        assert f.namespace["application_host"] is old
        assert before == (f.implementation.prepare_workspace, *(getattr(f.deployment, name) for name, _ in CALLBACKS))
    assert not f.implementation.root.exists()


@pytest.mark.parametrize("route", ["cloud", "ssh", "install"])
@pytest.mark.parametrize("selected", [False, True])
async def test_initial_routes_forward_only_original_internal_control(shipping_dispatch, monkeypatch, route, selected):
    f = shipping_dispatch
    if selected:
        f.namespace["select_product_application_runtime"](f.selection)
    expected = f.control.selection if selected else None
    calls = []
    async def boundary(*args, **kwargs):
        calls.append((args, kwargs))
        return {"status": "stopped"}
    name = {"cloud": "connect_cloud", "ssh": "connect", "install": "install"}[route]
    monkeypatch.setattr(f.remote, name, boundary)
    value = SimpleNamespace(version=f.initial.saved["version"], build_id=str(uuid4()),
        operation_id=str(uuid4()), host="example.invalid", user="inert", ssh_port=22,
        destination="~/inert", deployment_id=str(uuid4()), settings={})
    async def observed(*args):
        return {"operation_id": value.operation_id, "status": "vm-ready"}
    f.namespace["gcp_provisioning"] = SimpleNamespace(status=observed)
    handler = {"cloud": "connect_gcp_deployment", "ssh": "connect_remote_deployment",
        "install": "install_remote_deployment"}[route]
    result = await f.namespace[handler](f.initial.saved["id"], value)
    assert result == {"status": "stopped"} and len(calls) == 1
    assert calls[0][1] == {"product_owner": expected}
    assert calls[0][0][:3] == (f.initial.saved["id"], value.version, value.build_id)
    assert not f.implementation.root.exists()


async def test_cloud_observation_cannot_replace_owner_before_initial_connect(shipping_dispatch, monkeypatch):
    f = shipping_dispatch
    f.namespace["select_product_application_runtime"](f.selection)
    called = []
    async def refused_connect(*args, **kwargs):
        called.append((args, kwargs))
    monkeypatch.setattr(f.remote, "connect_cloud", refused_connect)
    async def drift(*args):
        await asyncio.sleep(0)
        f.remote.product_control_selection = None
        return {"status": "vm-ready"}
    f.namespace["gcp_provisioning"] = SimpleNamespace(status=drift)
    with pytest.raises(HTTPException, match="original selected remote control owner") as refused:
        await f.namespace["connect_gcp_deployment"](f.initial.saved["id"],
            SimpleNamespace(version=f.initial.saved["version"], build_id=str(uuid4()), operation_id=str(uuid4())))
    assert refused.value.status_code == 409
    assert not called and not f.implementation.root.exists()


async def test_actual_ssh_endpoint_validation_preserves_default_and_private_forwarding(shipping_dispatch, monkeypatch):
    f = shipping_dispatch
    calls = []
    async def original(*args, **kwargs):
        calls.append((args, kwargs))
        return {"status": "stopped"}
    monkeypatch.setattr(f.remote, "_connect", original)
    assert await f.remote.connect("guide", 1, "build", "example.invalid", "inert") == {"status": "stopped"}
    assert calls[-1][1] == {"product_owner": None}
    assert await f.remote.connect("guide", 1, "build", "example.invalid", "inert", product_owner=f.control.selection) == {"status": "stopped"}
    assert calls[-1][1] == {"product_owner": f.control.selection}
    before = len(calls)
    with pytest.raises(ValueError, match="valid SSH port"):
        await f.remote.connect("guide", 1, "build", "example.invalid", "inert", ssh_port=0, product_owner=f.control.selection)
    assert len(calls) == before


async def test_original_signed_gcp_build_needs_exact_control_owner_before_admission(registry, remote_control):
    f = registry
    owner = bind_registry(f, remote_control.selection)
    saved = target_plan(f, new_guide(f), "gcp")
    result = await build(f, saved)
    await accept_source_fixture(f, result)
    remote = RemoteDeploymentService(f.implementation.root.parent, f.guide, f.implementation,
        f.service, transport=object(), product_control_selection=remote_control.selection)
    with pytest.raises(DraftConflict, match="not supported"):
        await remote.accepted(saved["id"], saved["version"], result["build_id"], target="gcp")
    selected = await remote.accepted(saved["id"], saved["version"], result["build_id"],
        target="gcp", product_owner=remote_control.selection)
    assert selected["product_control"] is remote_control.selection
    assert selected["spec"]["target"] == "gcp" and selected["state"]["proposal"] == saved["proposal"]
    assert owner.host_for_build(saved["id"], result["build_id"])._target == remote_control.selection.runtime_target
    assert remote_control.state.calls and all(task is asyncio.current_task() for task in remote_control.state.calls)
    remote_control.state.current = False
    with pytest.raises(ValueError, match="original control selection owner changed"):
        await remote.accepted(saved["id"], saved["version"], result["build_id"],
            target="gcp", product_owner=remote_control.selection)
    assert not remote.tasks and not remote.root.exists()


@pytest.mark.parametrize("model", ["RemoteConnectRequest", "RemoteInstallRequest", "GcpConnectRequest"])
def test_actual_http_request_models_cannot_select_artifacts_or_control(model):
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    wanted = {"RemoteConnectRequest", "RemoteInstallRequest", "GcpResumeRequest", "GcpConnectRequest"}
    nodes = [n for n in ast.parse(source.read_text()).body if isinstance(n, ast.ClassDef) and n.name in wanted]
    assert {n.name for n in nodes} == wanted
    namespace = {"BaseModel": BaseModel, "ConfigDict": ConfigDict, "Field": Field}
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), "exec"), namespace)
    values = {"version": 1, "build_id": str(uuid4())}
    if model == "RemoteConnectRequest":
        values.update(host="example.invalid", user="inert", destination="~/inert")
    elif model == "RemoteInstallRequest":
        values.update(deployment_id=str(uuid4()), settings={})
    else:
        values.update(operation_id=str(uuid4()))
    assert namespace[model].model_validate(values).version == 1
    for key, value in (("product_owner", {}), ("runtime_target", "/untrusted/wheel"), ("python_tag", "cp312")):
        with pytest.raises(ValidationError) as caught:
            namespace[model].model_validate({**values, key: value})
        assert any(error["type"] == "extra_forbidden" and error["loc"] == (key,)
            for error in caught.value.errors())
