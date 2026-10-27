"""Trusted launcher and shipping AST route dispatch with actual local owners.

The source handlers are extracted without importing the server or constructing
its global runtime. Real disposable GuideStore/GuideService/implementation/
deployment objects own the registry. Submit/configure/action boundaries are
inert; product wheels contain poisoned bytes. These controls do not establish a
running server, qualified runtime, native setup, installation or user journey.
"""
import ast
import asyncio
from copy import deepcopy
from dataclasses import replace
import logging
from pathlib import Path
from types import SimpleNamespace
import uuid

from fastapi import FastAPI, HTTPException, Query
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field, model_validator
import pytest

from launchkit import application_host_launch as launch
from launchkit.architect import ArchitectUnavailable
from launchkit.deployment import DeploymentService
from launchkit.forge_application_host import ForgeApplicationHost, application_setup_binding
from launchkit.guide import GuideService
from launchkit.implementation import BuildSelectionUnavailable, ImplementationService
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.installed_feedback import InstalledFeedbackNotAdmitted, InstalledSuccessNotAdmitted
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.remote_deployment import RemoteProductActionNotSent
from launchkit.local_application_source import create_local_application_source
from launchkit.projects import DraftConflict
from launchkit.runtime_selection import MetaToolkitRuntimeTarget
from test_local_application_source import initial
from test_meta_toolkit_runtime_selection import product, no_product_execution


CALLBACKS = (("application_setup_selection", "_selected_startup"),
    ("application_setup_preparation", "prepare_application"),
    ("application_setup_retention", "retain_installation_inputs"),
    ("application_setup_continuation", "continue_application_startup"),
    ("application_replacement_selection", "_selected_replacement"),
    ("application_host_selection", "selected_host"))


def disk_bytes(root):
    return {path.relative_to(root).as_posix(): path.read_bytes()
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


@pytest.fixture
def dispatch(initial, tmp_path, monkeypatch):
    guide = GuideService(initial.store, None)
    implementation = ImplementationService(tmp_path / "dispatch-state", guide)
    deployment = DeploymentService(tmp_path / "dispatch-state", guide, implementation)
    calls = []

    async def submit_build(*args, **kwargs):
        calls.append(("build", args, kwargs))
        return {"guide_id": args[0], "status": "queued"}

    async def submit_install(*args, **kwargs):
        calls.append(("install", args, kwargs))
        return {"guide_id": args[0], "status": "queued"}

    async def configure(*args, **kwargs):
        calls.append(("configure", args, kwargs))
        return {"deployment_id": args[2], "status": "stopped"}

    async def action(*args, **kwargs):
        calls.append(("action", args, kwargs))
        return {"deployment_id": args[1], "status": "stopped"}

    async def options(*args, **kwargs):
        calls.append(("options", args, kwargs))
        return {"guide_id": args[0], "version": initial.saved["version"]}

    monkeypatch.setattr(implementation, "submit", submit_build)
    monkeypatch.setattr(deployment, "submit", submit_install)
    monkeypatch.setattr(deployment, "configure", configure)
    monkeypatch.setattr(deployment, "action", action)
    monkeypatch.setattr(deployment, "options", options)

    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    wanted = {"select_application_host", "select_local_application_runtime", "guide_result",
        "ImplementationRequest", "DeploymentRequest", "DeploymentSettingsRequest", "DeploymentAction",
        "build_implementation", "install_deployment", "configure_installed_deployment",
        "operate_deployment", "deployment_options"}
    tree = ast.parse(source.read_text())
    body = [node for node in tree.body if isinstance(node,
        (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) and node.name in wanted]
    assert {node.name for node in body} == wanted
    app = FastAPI()
    namespace = {"__name__": __name__, "__package__": "launchkit", "app": app, "guide": guide,
        "implementation": implementation, "deployment": deployment, "application_host": None,
        "ImplementationService": ImplementationService, "ForgeApplicationHost": ForgeApplicationHost,
        "LocalApplicationRegistry": LocalApplicationRegistry, "application_setup_binding": application_setup_binding,
        "asyncio": asyncio, "HTTPException": HTTPException, "JSONResponse": JSONResponse, "Query": Query,
        "BaseModel": BaseModel, "ConfigDict": ConfigDict, "Field": Field, "model_validator": model_validator,
        "DraftConflict": DraftConflict, "BuildSelectionUnavailable": BuildSelectionUnavailable,
        "ArchitectUnavailable": ArchitectUnavailable, "InstalledFeedbackNotAdmitted": InstalledFeedbackNotAdmitted,
        "InstalledSuccessNotAdmitted": InstalledSuccessNotAdmitted,
        "RemoteProductActionNotSent": RemoteProductActionNotSent, "LOG": logging.getLogger(__name__)}
    exec(compile(ast.Module(body=body, type_ignores=[]), str(source), "exec"), namespace)
    return SimpleNamespace(initial=initial, guide=guide, implementation=implementation,
        deployment=deployment, namespace=namespace, client=TestClient(app), app=app, calls=calls)


def select_runtime(f):
    return f.namespace["select_local_application_runtime"](f.initial.target)


def callback_snapshot(f):
    return (f.implementation.prepare_workspace,
        *(getattr(f.deployment, name) for name, _ in CALLBACKS))


def prepare_build(f, registry):
    build_id = str(uuid.uuid4())
    directory = f.implementation._directory(f.initial.saved["id"], build_id)
    directory.mkdir(parents=True, mode=0o700)
    workspace = ImplementationWorkspace(directory / "workspace", f.initial.saved["proposal"])
    workspace.seed(runtime_target=f.initial.target)
    registry.prepare_workspace(f.initial.saved["id"], build_id, workspace,
        guide_version=f.initial.saved["version"])
    return build_id, registry.host_for_build(f.initial.saved["id"], build_id)


def manual_host(f):
    factory = create_local_application_source(guide_store=f.guide.store,
        guide_id=f.initial.saved["id"], guide_version=f.initial.saved["version"],
        runtime_target=f.initial.target, output_directory=f.initial.output)
    return factory()


def test_exact_target_launcher_selects_actual_registry_before_returning_original_app(dispatch, monkeypatch):
    f = dispatch
    assert type(f.initial.target) is MetaToolkitRuntimeTarget
    calls = []

    def factory():
        calls.append(("factory", f.initial.target))
        return f.initial.target

    def selected(value):
        assert value is f.initial.target
        calls.append(("select", value))
        return f.namespace["select_local_application_runtime"](value)

    modules = {"trusted_local.selection": SimpleNamespace(runtime=factory),
        "launchkit.forge_application_host": SimpleNamespace(ForgeApplicationHost=ForgeApplicationHost),
        "launchkit.runtime_selection": SimpleNamespace(MetaToolkitRuntimeTarget=MetaToolkitRuntimeTarget),
        "launchkit.server": SimpleNamespace(app=f.app, select_local_application_runtime=selected,
            select_application_host=lambda value: pytest.fail("A product target cannot select a manual host."))}

    def load(name):
        calls.append(("import", name))
        assert name in modules, "Unexpected global server/runtime import"
        return modules[name]

    monkeypatch.setenv(launch.FACTORY_ENV, "trusted_local.selection:runtime")
    monkeypatch.setattr(launch.importlib, "import_module", load)
    before = disk_bytes(f.initial.store.root)
    assert launch.create_app() is f.app
    assert calls == [("import", "trusted_local.selection"),
        ("import", "launchkit.forge_application_host"), ("import", "launchkit.runtime_selection"),
        ("factory", f.initial.target), ("import", "launchkit.server"), ("select", f.initial.target)]
    registry = f.namespace["application_host"]
    assert type(registry) is LocalApplicationRegistry
    assert registry.runtime_target is f.initial.target and registry.guide_store is f.guide.store
    assert registry.implementation is f.implementation and registry.deployment is f.deployment
    assert f.implementation.prepare_workspace == registry.prepare_workspace
    assert all(getattr(f.deployment, name) == getattr(registry, method) for name, method in CALLBACKS)
    assert not registry._builds and not registry._installations and not registry._attempts
    assert disk_bytes(f.initial.store.root) == before and not f.initial.output.exists()
    assert not f.implementation.root.exists() and not f.deployment.root.exists() and f.calls == []


def test_toolkit_application_launcher_binds_registry_and_build_owner_to_model_owner(dispatch, tmp_path, monkeypatch):
    from launchkit.connect_application_startup import _ModelConfiguration
    from test_managed_model_factory_selection import model_source
    f = dispatch
    selection = model_source(tmp_path, monkeypatch)
    monkeypatch.setenv("XDG_CONFIG_HOME", selection["credential_config_root"])
    owner = _ModelConfiguration.observe(selection)
    chosen = launch.ToolkitApplicationSelection(f.initial.target, owner)
    modules = {"trusted_local.selection": SimpleNamespace(runtime=lambda: chosen),
        "launchkit.forge_application_host": SimpleNamespace(ForgeApplicationHost=ForgeApplicationHost),
        "launchkit.runtime_selection": SimpleNamespace(MetaToolkitRuntimeTarget=MetaToolkitRuntimeTarget),
        "launchkit.server": SimpleNamespace(app=f.app,
            select_local_application_runtime=f.namespace["select_local_application_runtime"],
            select_application_host=lambda value: pytest.fail("A product target cannot select a manual host."))}

    def load(name):
        assert name in modules, "Unexpected global server/runtime import"
        return modules[name]

    monkeypatch.setenv(launch.FACTORY_ENV, "trusted_local.selection:runtime")
    monkeypatch.setattr(launch.importlib, "import_module", load)
    assert launch.create_app() is f.app
    registry = f.namespace["application_host"]
    assert type(registry) is LocalApplicationRegistry and registry.runtime_target is f.initial.target
    assert registry._selected_model_configuration() == selection
    build_id, host = prepare_build(f, registry)
    factory = registry._builds[(f.initial.saved["id"], build_id)].factory
    assert factory.host is host and factory._selected_model_configuration() == selection


def test_duplicate_original_runtime_selection_preserves_callbacks_without_source_creation(dispatch):
    f = dispatch
    registry = select_runtime(f)
    callbacks = callback_snapshot(f)
    before = disk_bytes(f.initial.store.root)
    assert select_runtime(f) is registry
    assert callback_snapshot(f) == callbacks
    assert f.namespace["application_host"] is registry
    assert disk_bytes(f.initial.store.root) == before and not f.initial.output.exists()
    assert not f.implementation.root.exists() and not f.deployment.root.exists()


@pytest.mark.parametrize("slot", ["workspace", *(name for name, _ in CALLBACKS)])
def test_changed_registry_callback_refuses_reselection_without_rebinding(dispatch, slot):
    f = dispatch
    registry = select_runtime(f)
    foreign = lambda *args, **kwargs: pytest.fail("A changed owner must not be invoked by selection.")
    if slot == "workspace":
        f.implementation.prepare_workspace = foreign
    else:
        setattr(f.deployment, slot, foreign)
    callbacks = callback_snapshot(f)
    with pytest.raises(DraftConflict, match="callbacks changed"):
        select_runtime(f)
    assert callback_snapshot(f) == callbacks and f.namespace["application_host"] is registry
    assert not f.implementation.root.exists() and not f.deployment.root.exists()


@pytest.mark.parametrize("kind", ["equal-copy", "foreign-target", "lookalike"])
def test_runtime_reselection_requires_original_exact_target_object(dispatch, kind):
    f = dispatch
    registry = select_runtime(f)
    selected = (replace(f.initial.target) if kind == "equal-copy" else
        replace(f.initial.target, assembly=replace(f.initial.target.assembly, sha256="0" * 64)) if kind == "foreign-target" else
        SimpleNamespace(**vars(f.initial.target)))
    callbacks = callback_snapshot(f)
    with pytest.raises(TypeError if kind == "lookalike" else DraftConflict):
        f.namespace["select_local_application_runtime"](selected)
    assert callback_snapshot(f) == callbacks and f.namespace["application_host"] is registry
    assert registry.runtime_target is f.initial.target and f.calls == []


@pytest.mark.parametrize("slot", ["workspace", "application_host_selection"])
def test_initial_runtime_selection_preserves_existing_owner(dispatch, slot):
    f = dispatch
    foreign = lambda *args, **kwargs: pytest.fail("A preselected owner must not be invoked.")
    if slot == "workspace":
        f.implementation.prepare_workspace = foreign
    else:
        setattr(f.deployment, slot, foreign)
    callbacks = callback_snapshot(f)
    with pytest.raises(DraftConflict, match="existing application"):
        select_runtime(f)
    assert callback_snapshot(f) == callbacks and f.namespace["application_host"] is None


@pytest.mark.parametrize("previous", [None, "original-earlier-build"])
def test_build_route_reads_exact_current_version_and_forwards_original_target(dispatch, monkeypatch, previous):
    f = dispatch
    registry = select_runtime(f)
    original = registry.initial_build_options
    observed = []

    def options(guide_id, version):
        observed.append((guide_id, version))
        return original(guide_id, version)

    monkeypatch.setattr(registry, "initial_build_options", options)
    value = {"version": f.initial.saved["version"], "request": "Build the agreed design"}
    if previous is not None:
        value["previous_build_id"] = previous
    response = f.client.post(f'/api/guide/{f.initial.saved["id"]}/implementation', json=value)
    assert response.status_code == 202
    assert observed == [(f.initial.saved["id"], f.initial.saved["version"])]
    assert f.calls == [("build", (f.initial.saved["id"], f.initial.saved["version"], value["request"]),
        {"expected_selected_build_id": previous, "refresh_framework": False, "runtime_target": f.initial.target})]
    assert f.calls[0][2]["runtime_target"] is f.initial.target
    assert f.namespace["application_host"] is registry and not registry._builds
    assert not f.implementation.root.exists() and not f.initial.output.exists()


def test_stale_build_version_refuses_before_underlying_submit(dispatch):
    f = dispatch
    select_runtime(f)
    newer = deepcopy(f.initial.saved)
    newer["proposal"]["project"]["name"] += " current revision"
    f.guide.store.save(newer, newer["version"])
    response = f.client.post(f'/api/guide/{f.initial.saved["id"]}/implementation',
        json={"version": f.initial.saved["version"]})
    assert response.status_code == 422 and "exact current completed version" in response.json()["detail"]
    assert f.calls == [] and not f.implementation.root.exists()


@pytest.mark.parametrize("route", ["implementation", "deployment"])
def test_http_source_requests_cannot_select_another_runtime(dispatch, route):
    f = dispatch
    registry = select_runtime(f)
    value = {"version": f.initial.saved["version"], "runtime_target": {"assembly": "foreign"}}
    if route == "deployment":
        value.update(build_id=str(uuid.uuid4()), destination="/INERT/NEVER/INSTALL")
    response = f.client.post(f'/api/guide/{f.initial.saved["id"]}/{route}', json=value)
    assert response.status_code == 422 and f.calls == []
    assert f.namespace["application_host"] is registry and registry.runtime_target is f.initial.target
    assert not f.implementation.root.exists() and not f.deployment.root.exists()


def test_framework_refresh_keeps_existing_route_owner_without_initial_source_selection(dispatch, monkeypatch):
    f = dispatch
    registry = select_runtime(f)
    monkeypatch.setattr(registry, "initial_build_options",
        lambda *args: pytest.fail("Framework refresh must not create a new initial source choice."))
    response = f.client.post(f'/api/guide/{f.initial.saved["id"]}/implementation', json={
        "version": f.initial.saved["version"], "previous_build_id": "original-build", "refresh_framework": True})
    assert response.status_code == 202
    assert f.calls == [("build", (f.initial.saved["id"], f.initial.saved["version"], ""),
        {"expected_selected_build_id": "original-build", "refresh_framework": True})]
    assert f.namespace["application_host"] is registry


def test_install_route_validates_exact_retained_build_and_original_console_choice(dispatch, monkeypatch):
    f = dispatch
    registry = select_runtime(f)
    build_id, host = prepare_build(f, registry)
    original = registry.validate_settings
    observed = []

    def validate(guide_id, settings, *, build_id):
        observed.append((guide_id, deepcopy(settings), build_id))
        return original(guide_id, settings, build_id=build_id)

    monkeypatch.setattr(registry, "validate_settings", validate)
    subject = host.console_operator_declaration()["id"]
    settings = {"CONSOLE_USER_ID": subject}
    path = f'/api/guide/{f.initial.saved["id"]}/deployment'
    value = {"version": f.initial.saved["version"], "build_id": build_id,
        "destination": "/INERT/NEVER/INSTALL", "settings": settings}
    response = f.client.post(path, json=value)
    assert response.status_code == 202
    assert observed == [(f.initial.saved["id"], settings, build_id)]
    assert f.calls == [("install", (f.initial.saved["id"], f.initial.saved["version"], build_id,
        value["destination"], settings, False), {"execution_mode": "interactive"})]
    assert registry.host_for_build(f.initial.saved["id"], build_id) is host
    assert host._installation_supervisor is None

    f.calls.clear()
    response = f.client.post(path, json={**value, "settings": {"CONSOLE_USER_ID": "foreign-subject"}})
    assert response.status_code == 409 and "Console user" in response.json()["detail"]
    assert observed[-1] == (f.initial.saved["id"], {"CONSOLE_USER_ID": "foreign-subject"}, build_id)
    assert f.calls == [] and f.namespace["application_host"] is registry


def test_native_team_install_preserves_ordinary_owner_and_skips_product_host_validation(dispatch, monkeypatch):
    f = dispatch
    registry = select_runtime(f)
    monkeypatch.setattr(registry, "validate_settings",
        lambda *args, **kwargs: pytest.fail("Native-team source has no application viewer owner."))
    value = {"version": f.initial.saved["version"], "source_kind": "native-team",
        "source_id": "a" * 64, "source_digest": "b" * 64,
        "destination": "/INERT/NEVER/INSTALL", "settings": {"DECLARED": "chosen"}}
    response = f.client.post(f'/api/guide/{f.initial.saved["id"]}/deployment', json=value)
    assert response.status_code == 202
    assert f.calls == [("install", (f.initial.saved["id"], f.initial.saved["version"], None,
        value["destination"], value["settings"], False), {"execution_mode": "interactive",
        "source_kind": "native-team", "source_id": "a" * 64, "source_digest": "b" * 64})]
    assert f.namespace["application_host"] is registry


@pytest.mark.parametrize("owner", ["default", "manual"])
def test_existing_default_and_manual_build_install_dispatch_remain_original(dispatch, owner):
    f = dispatch
    host = None
    if owner == "manual":
        host = manual_host(f)
        f.namespace["select_application_host"](host)
        callbacks = callback_snapshot(f)
    value = {"version": f.initial.saved["version"]}
    assert f.client.post(f'/api/guide/{f.initial.saved["id"]}/implementation', json=value).status_code == 202
    expected = {"expected_selected_build_id": None, "refresh_framework": False}
    if host is not None:
        expected["runtime_target"] = f.initial.target
    assert f.calls == [("build", (f.initial.saved["id"], f.initial.saved["version"], ""), expected)]
    f.calls.clear()
    build_id = str(uuid.uuid4())
    install = {**value, "build_id": build_id, "destination": "/INERT/NEVER/INSTALL", "settings": {}}
    assert f.client.post(f'/api/guide/{f.initial.saved["id"]}/deployment', json=install).status_code == 202
    assert f.calls == [("install", (f.initial.saved["id"], f.initial.saved["version"], build_id,
        install["destination"], {}, False), {"execution_mode": "interactive"})]
    assert f.namespace["application_host"] is host
    if host is not None:
        assert callback_snapshot(f) == callbacks


def test_regular_routes_keep_registry_owner_and_forward_original_callbacks(dispatch):
    f = dispatch
    registry = select_runtime(f)
    callbacks = callback_snapshot(f)
    identity = f.initial.saved["id"]
    deployment_id, build_id, request_id = (str(uuid.uuid4()) for _ in range(3))
    response = f.client.get(f"/api/guide/{identity}/deployment/options",
        params={"build_id": build_id, "console_port": 4321})
    assert response.status_code == 200
    assert f.calls == [("options", (identity, build_id), {"console_port": 4321})]
    configure = {"version": f.initial.saved["version"], "deployment_id": deployment_id,
        "build_id": build_id, "source_digest": "a" * 64, "package_sha256": "b" * 64,
        "settings": {"DECLARED": "chosen"}, "remove_fields": ["OPTIONAL"]}
    response = f.client.post(f"/api/guide/{identity}/deployment/settings", json=configure)
    assert response.status_code == 200
    assert f.calls[-1] == ("configure", (identity, f.initial.saved["version"], deployment_id,
        build_id, "a" * 64, "b" * 64), {"settings": configure["settings"], "remove_fields": ["OPTIONAL"],
        "use_host_provider_credentials": False, "application_settings_validator": registry.validate_installed_settings})
    response = f.client.post(f"/api/guide/{identity}/deployment/action", json={
        "deployment_id": deployment_id, "action": "stop", "action_request_id": request_id})
    assert response.status_code == 200
    assert f.calls[-1] == ("action", (identity, deployment_id, "stop"), {"action_request_id": request_id})
    assert f.namespace["application_host"] is registry and callback_snapshot(f) == callbacks
