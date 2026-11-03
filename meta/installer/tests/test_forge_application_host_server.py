"""Inert controls for shipping Forge host selection and HTTP call sites.

Only selected server declarations/handlers are loaded, without constructing its
global services. Deployment operations are stubs, and the reused product fixture
forbids SDK/native/process execution. These are not installation or login proof.
"""
import ast
from copy import deepcopy
import logging
import json
from pathlib import Path
from types import SimpleNamespace
import uuid

from fastapi import FastAPI, HTTPException, Query
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field, model_validator
import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.forge_application_host import ForgeApplicationHost, application_setup_binding
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.projects import DraftConflict
from test_forge_application_host import selected, startup, put
from test_meta_toolkit_runtime_selection import product, no_product_execution


@pytest.fixture
def api(selected):
    calls = []
    settings = {"deployment_id": "original-installation", "version": 9, "guide_version": 7,
        "build_id": "installed-original-build", "source_digest": "b" * 64,
        "package_sha256": "a" * 64, "fields": []}
    directory = selected.root / "inert-stopped-installation"
    directory.mkdir(mode=0o700)
    installed_destination = directory / "source"
    installed_destination.mkdir(mode=0o700)
    installed_files = {}
    for name, pin in selected.protected.items():
        content = pin.read()
        path = installed_destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        path.chmod(0o644)
        installed_files[name] = (content, 0o644)

    class Implementation:
        def prepare_workspace(self, *args):
            pytest.fail("HTTP controls must not prepare a workspace.")

    class Deployment:
        application_setup_selection = None
        application_replacement_selection = None
        async def settings_options(self, *args):
            calls.append(("settings_options", args, {}))
            return deepcopy(settings)
        def _directory(self, *args):
            return directory
        async def submit(self, *args, **kwargs):
            calls.append(("submit", args, deepcopy(kwargs)))
            return {"deployment_id": "original-installation", "status": "stopped"}
        async def configure(self, *args, **kwargs):
            validator = kwargs.pop("application_settings_validator", None)
            if validator is not None:
                validator(args[0], {"source_kind": "application"}, installed_destination, installed_files, kwargs["settings"])
            calls.append(("configure", args, deepcopy(kwargs)))
            return {"deployment_id": "original-installation", "status": "stopped"}
        async def action(self, *args, **kwargs):
            calls.append(("action", args, deepcopy(kwargs)))
            return {"deployment_id": args[1], "action_request": {"phase": "pending"}}

    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    wanted = {"select_application_host", "guide_result", "DeploymentRequest", "DeploymentSettingsRequest",
        "DeploymentAction", "install_deployment", "configure_installed_deployment",
        "application_setup_status", "operate_deployment", "installed_deployment_settings"}
    tree = ast.parse(source.read_text())
    body = [node for node in tree.body if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))
        and node.name in wanted]
    assert {node.name for node in body} == wanted
    app = FastAPI()
    implementation, deployment = Implementation(), Deployment()
    namespace = {"app": app, "implementation": implementation, "deployment": deployment,
        "application_host": None, "ImplementationService": Implementation,
        "ForgeApplicationHost": ForgeApplicationHost, "LocalApplicationRegistry": LocalApplicationRegistry,
        "application_setup_binding": application_setup_binding,
        "HTTPException": HTTPException, "JSONResponse": JSONResponse, "Query": Query,
        "BaseModel": BaseModel, "ConfigDict": ConfigDict, "Field": Field,
        "model_validator": model_validator, "DraftConflict": DraftConflict,
        "LOG": logging.getLogger(__name__)}
    for name in ("InstalledFeedbackNotAdmitted", "InstalledSuccessNotAdmitted",
            "BuildSelectionUnavailable", "ArchitectUnavailable", "RemoteProductActionNotSent"):
        namespace[name] = type(name, (RuntimeError,), {})
    exec(compile(ast.Module(body=body, type_ignores=[]), str(source), "exec"), namespace)
    return SimpleNamespace(client=TestClient(app), calls=calls, settings=settings, namespace=namespace,
        implementation=implementation, deployment=deployment, directory=directory, selected=selected)


def select(api):
    api.namespace["select_application_host"](api.selected.host)


def read_setup(api):
    return api.client.get("/api/guide/original-guide/deployment/application-setup",
        params={"deployment_id": "original-installation"})


@pytest.mark.parametrize("callback", ["workspace", "startup", "replacement"])
def test_host_selection_preserves_preexisting_source_or_startup_owner(api, callback):
    existing = lambda *args: pytest.fail("A preexisting callback must not be executed by selection.")
    if callback == "workspace":
        api.implementation.prepare_workspace = existing
    elif callback == "startup":
        api.deployment.application_setup_selection = existing
    else:
        api.deployment.application_replacement_selection = existing
    workspace = api.implementation.prepare_workspace
    startup_callback = api.deployment.application_setup_selection
    replacement_callback = api.deployment.application_replacement_selection
    with pytest.raises(DraftConflict, match="existing"):
        select(api)
    assert api.namespace["application_host"] is None
    assert api.implementation.prepare_workspace == workspace
    assert api.deployment.application_setup_selection is startup_callback
    assert api.deployment.application_replacement_selection is replacement_callback
    assert api.calls == []


def test_exact_original_host_selection_is_repeatable_without_changing_owner(api):
    select(api)
    workspace = api.implementation.prepare_workspace
    startup_callback = api.deployment.application_setup_selection
    replacement_callback = api.deployment.application_replacement_selection
    select(api)
    assert api.namespace["application_host"] is api.selected.host
    assert api.implementation.prepare_workspace == workspace
    assert api.deployment.application_setup_selection == startup_callback
    assert api.deployment.application_replacement_selection is replacement_callback is None
    assert api.calls == []


@pytest.mark.parametrize("callback", ["workspace", "startup", "replacement"])
def test_reselection_preserves_a_foreign_callback_replacement(api, callback):
    select(api)
    existing = lambda *args: pytest.fail("A replacement owner must not be executed by reselecting.")
    if callback == "workspace":
        api.implementation.prepare_workspace = existing
    elif callback == "startup":
        api.deployment.application_setup_selection = existing
    else:
        api.deployment.application_replacement_selection = existing
    workspace = api.implementation.prepare_workspace
    startup_callback = api.deployment.application_setup_selection
    replacement_callback = api.deployment.application_replacement_selection
    with pytest.raises(DraftConflict, match="callbacks"):
        select(api)
    assert api.namespace["application_host"] is api.selected.host
    assert api.implementation.prepare_workspace == workspace
    assert api.deployment.application_setup_selection == startup_callback
    assert api.deployment.application_replacement_selection is replacement_callback
    assert api.calls == []


def test_selected_replacement_callback_is_retained_without_invoking_supplier(api):
    api.selected.host._replacement_selection = lambda *args: pytest.fail("Selection cannot read replacement inputs.")
    select(api)
    original = api.deployment.application_replacement_selection
    assert original == api.selected.host._selected_replacement
    select(api)
    assert api.deployment.application_replacement_selection == original
    assert api.calls == []


def test_reselection_cannot_silently_add_a_replacement_owner(api):
    select(api)
    api.selected.host._replacement_selection = lambda *args: pytest.fail("Reselection cannot execute a new supplier.")
    with pytest.raises(DraftConflict, match="callbacks"):
        select(api)
    assert api.deployment.application_replacement_selection is None
    assert api.calls == []


def test_unavailable_setup_readback_keeps_exact_installed_binding_without_private_inputs(api):
    response = read_setup(api)
    assert response.status_code == 200
    result = response.json()
    assert result == {"guide_id": "original-guide", "deployment_id": "original-installation",
        "scope": "selected-stopped-application-inputs",
        "version": 9, "guide_version": 7, "build_id": "installed-original-build",
        "source_digest": "b" * 64, "package_sha256": "a" * 64,
        "available": False, "reason": "host_unavailable",
        "detail": "Application setup has no trusted host selection yet."}
    assert api.calls == [("settings_options", ("original-guide", "original-installation"), {})]
    assert api.client.post("/api/guide/original-guide/deployment/application-setup", json={}).status_code == 405


def test_missing_settings_readback_is_bound_and_does_not_start_setup(api):
    startup(api.selected, "fresh")
    select(api)
    api.settings["fields"] = [{"key": "CONSOLE_LOGIN_PASSWORD", "label": "Password",
        "required": True, "configured": False}]
    response = read_setup(api)
    assert response.status_code == 200
    result = response.json()
    assert result["available"] is False and result["reason"] == "private_settings_missing"
    assert result["missing_fields"] == ["Password"]
    assert result["guide_version"] == 7 and result["package_sha256"] == "a" * 64
    assert not {"principal", "source_policy", "gateway", "preparation_plan", "application_name"} & result.keys()
    assert all(call[0] == "settings_options" for call in api.calls)


@pytest.mark.parametrize("kind", ["fresh", "restore"])
def test_selected_inputs_readback_is_available_without_claiming_native_readiness(api, kind):
    startup(api.selected, kind)
    select(api)
    result = read_setup(api).json()
    assert result["available"] is True and result["kind"] == kind
    assert result["scope"] == "selected-stopped-application-inputs"
    assert result["label"] == ("Finish application setup" if kind == "fresh" else "Start installed application")
    assert result["guide_id"] == "original-guide" and result["deployment_id"] == "original-installation"
    assert result["build_id"] == "installed-original-build" and result["guide_version"] == 7
    assert "ready" not in result and "activation" not in result
    assert all(call[0] == "settings_options" for call in api.calls)


def request(kind, settings):
    if kind == "install":
        return "/api/guide/original-guide/deployment", {"version": 9,
            "build_id": "installed-original-build", "destination": "/INERT/NEVER/INSTALL",
            "settings": settings}
    return "/api/guide/original-guide/deployment/settings", {"version": 9,
        "deployment_id": "original-installation", "build_id": "installed-original-build",
        "source_digest": "b" * 64, "package_sha256": "a" * 64, "settings": settings}


@pytest.mark.parametrize("kind", ["install", "settings"])
def test_different_console_subject_refuses_before_underlying_operation(api, kind):
    select(api)
    path, payload = request(kind, {"CONSOLE_USER_ID": "different-subject"})
    response = api.client.post(path, json=payload)
    assert response.status_code == 409 and "Console user" in response.json()["detail"]
    assert api.calls == []


@pytest.mark.parametrize("kind", ["install", "settings"])
@pytest.mark.parametrize("settings", [{"CONSOLE_USER_ID": "inert-selected-operator"}, {},
    {"CONSOLE_USER_ID": ""}])
def test_selected_subject_or_omission_forwards_unchanged_without_private_defaults(api, kind, settings):
    select(api)
    path, payload = request(kind, deepcopy(settings))
    response = api.client.post(path, json=payload)
    assert response.status_code == (202 if kind == "install" else 200)
    assert len(api.calls) == 1
    operation, args, options = api.calls[0]
    assert operation == ("submit" if kind == "install" else "configure")
    forwarded = args[4] if kind == "install" else options["settings"]
    assert forwarded == settings
    assert "CONSOLE_LOGIN_PASSWORD" not in forwarded


async def installed_settings_owner(api, tmp_path, *, foreign_viewer=False):
    """Reuse the actual stopped-installation fixture and maintained HTTP handlers."""
    from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS
    from test_deployment_settings import prepared, body

    configuration = {"fields": [*deepcopy(CONSOLE_FIELDS),
        {"key": "OPTIONAL_TOKEN", "label": "Optional token", "secret": True, "required": False},
        {"key": "DISPLAY_THEME", "label": "Theme", "secret": False, "required": False}]}
    api.selected.protected["application/configuration.json"] = put(
        api.selected.root / "application/configuration.json", json.dumps(configuration).encode())
    viewer = json.loads(api.selected.viewer.read())
    viewer["source_inputs"] = {name: pin.pin() for name, pin in api.selected.protected.items()}
    api.selected.viewer = put(api.selected.viewer.path, viewer)
    viewer_files = {name: pin.read() for name, pin in api.selected.protected.items()}
    if foreign_viewer:
        viewer_files["config/access.toml"] = b'[access]\nenabled = false\n'
    service, directory, destination, receipt = await prepared(tmp_path,
        viewer_files=viewer_files, private_settings={"CONSOLE_USER_ID": "inert-selected-operator",
            "CONSOLE_LOGIN_PASSWORD": "inert-test-password-only"})
    host = ForgeApplicationHost(guide_id=service.guide.identity, runtime_target=api.selected.target,
        acquisition_selection=api.selected.acquisition, viewer_selection=api.selected.viewer,
        require_current=lambda: None)
    api.selected.host = host
    api.namespace["deployment"] = service
    select(api)

    def original_proposal_current():
        if service.guide.state["proposal"] != service.implementation.original["proposal"]:
            raise CustodyError("The current proposal differs from the original selected plan.")

    host._require_owner_current = original_proposal_current
    service._launch = lambda *args, **kwargs: pytest.fail("Private settings must not launch or replace installed source.")
    assert service.guide.state["proposal"] != service.implementation.original["proposal"]
    return SimpleNamespace(host=host, service=service, directory=directory, destination=destination,
        receipt=receipt, payload=body(service, receipt),
        url=f"/api/guide/{service.guide.identity}/deployment/settings")


@pytest.mark.parametrize("subject", [None, "", "inert-selected-operator"])
@pytest.mark.parametrize("source_mode", [0o600, 0o640, 0o644])
async def test_http_installed_settings_keep_original_viewer_after_unaccepted_proposal_edit(api, tmp_path, subject, source_mode):
    from launchkit.deployment import private_configuration_snapshot, read_receipt

    selected = await installed_settings_owner(api, tmp_path)
    for name in api.selected.protected:
        (selected.destination / name).chmod(source_mode)
    before = {path: path.read_bytes() for path in selected.destination.rglob("*")
        if path.is_file() and path.name != ".env"}
    package = (selected.directory / "package.zip").read_bytes()
    candidate = deepcopy(selected.service.implementation.status.return_value)
    response = api.client.get(selected.url, params={"deployment_id": selected.receipt["deployment_id"]})
    assert response.status_code == 200
    assert response.json()["version"] != response.json()["guide_version"]
    settings = {"OPTIONAL_TOKEN": "inert-replacement-provider-token"}
    if subject is not None:
        settings["CONSOLE_USER_ID"] = subject
    response = api.client.post(selected.url, json={**selected.payload, "settings": settings})
    assert response.status_code == 200, response.text
    assert response.json()["status"] == "stopped"
    values = private_configuration_snapshot(selected.destination)[0]
    assert values["OPTIONAL_TOKEN"] == "inert-replacement-provider-token"
    assert values["CONSOLE_USER_ID"] == "inert-selected-operator"
    assert all(path.read_bytes() == content for path, content in before.items())
    assert all((selected.destination / name).stat().st_mode & 0o777 == source_mode
        for name in api.selected.protected)
    assert (selected.directory / "package.zip").read_bytes() == package
    assert selected.service.implementation.status.return_value == candidate
    selected.service.implementation.package.assert_not_awaited()
    # New Install still owns initial proposal currentness, even for the same user.
    saved_env, saved_receipt = (selected.destination / ".env").read_bytes(), read_receipt(selected.directory)
    response = api.client.post(f"/api/guide/{selected.service.guide.identity}/deployment", json={
        "version": selected.payload["version"], "build_id": selected.payload["build_id"],
        "destination": str(tmp_path / "never-installed"), "settings": settings})
    assert response.status_code == 422 and "original selected plan" in response.json()["detail"]
    assert (selected.destination / ".env").read_bytes() == saved_env
    assert read_receipt(selected.directory) == saved_receipt
    assert not (tmp_path / "never-installed").exists()


@pytest.mark.parametrize("change", ["subject", "foreign-viewer", "viewer-pin", "retained-link", "installed-link", "installed-mode"])
async def test_http_installed_settings_refuse_foreign_or_changed_viewer_without_private_mutation(api, tmp_path, change):
    selected = await installed_settings_owner(api, tmp_path, foreign_viewer=change == "foreign-viewer")
    settings = {"OPTIONAL_TOKEN": "must-not-be-saved"}
    if change == "subject":
        settings["CONSOLE_USER_ID"] = "different-operator"
    elif change == "viewer-pin":
        value = json.loads(api.selected.viewer.read())
        value["operator"]["subject"] = "changed-retained-viewer"
        api.selected.viewer.path.write_bytes(json.dumps(value).encode())
    elif change == "retained-link":
        path = api.selected.protected["config/access.toml"].path
        moved = path.with_name("original-access.toml")
        path.rename(moved)
        path.symlink_to(moved)
    elif change == "installed-link":
        path = selected.destination / "config/access.toml"
        path.unlink()
        path.symlink_to(api.selected.protected["config/access.toml"].path)
    elif change == "installed-mode":
        (selected.destination / "config/access.toml").chmod(0o664)
    env = (selected.destination / ".env").read_bytes()
    receipt = (selected.directory / "receipt.json").read_bytes()
    package = (selected.directory / "package.zip").read_bytes()
    source = {path: path.read_bytes() for path in selected.destination.rglob("*")
        if path.is_file() and not path.is_symlink()}
    candidate = deepcopy(selected.service.implementation.status.return_value)
    response = api.client.post(selected.url, json={**selected.payload, "settings": settings})
    assert response.status_code in {409, 422}, response.text
    assert "must-not-be-saved" not in response.text
    assert (selected.destination / ".env").read_bytes() == env
    assert (selected.directory / "receipt.json").read_bytes() == receipt
    assert (selected.directory / "package.zip").read_bytes() == package
    assert all(path.read_bytes() == content for path, content in source.items())
    assert selected.service.implementation.status.return_value == candidate
    selected.service.implementation.package.assert_not_awaited()


async def test_http_installed_settings_validator_failure_keeps_private_inputs_and_receipt(api, tmp_path, monkeypatch):
    selected = await installed_settings_owner(api, tmp_path)
    env = (selected.destination / ".env").read_bytes()
    receipt = (selected.directory / "receipt.json").read_bytes()
    package = (selected.directory / "package.zip").read_bytes()
    source = {path: path.read_bytes() for path in selected.destination.rglob("*") if path.is_file()}

    def refusal(*args):
        raise OSError("inert validator failure")

    monkeypatch.setattr(selected.host, "validate_installed_settings", refusal)
    response = api.client.post(selected.url, json={**selected.payload, "settings": {"OPTIONAL_TOKEN": "not-saved"}})
    assert response.status_code == 502
    assert (selected.destination / ".env").read_bytes() == env
    assert (selected.directory / "receipt.json").read_bytes() == receipt
    assert (selected.directory / "package.zip").read_bytes() == package
    assert all(path.read_bytes() == content for path, content in source.items())


def test_installed_settings_native_team_retains_original_currentness_and_subject_validation(api):
    def unavailable():
        raise CustodyError("Original native-team guard")

    api.selected.host._require_owner_current = unavailable
    with pytest.raises(CustodyError, match="Original native-team guard"):
        api.selected.host.validate_installed_settings("original-guide", {"source_kind": "native-team"},
            None, {}, {"CONSOLE_USER_ID": "inert-selected-operator"})
    api.selected.host._require_owner_current = lambda: None
    with pytest.raises(DraftConflict, match="Console user"):
        api.selected.host.validate_installed_settings("original-guide", {"source_kind": "native-team"},
            None, {}, {"CONSOLE_USER_ID": "different-operator"})
    api.selected.host.validate_installed_settings("original-guide", {"source_kind": "native-team"},
        None, {}, {"CONSOLE_USER_ID": "inert-selected-operator"})


@pytest.mark.parametrize("change", [{"action_request_id": None}, {"action_request_id": "invalid"},
    {"action_request_id": True}, {"action_request_id": "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA"},
    {"update_id": "another-transaction"}, {"resume": True}, {"gateway": "/browser/supplied/path"},
    {"principal": {"kind": "human", "id": "browser"}}, {"descriptor_sha256": "a" * 64}])
def test_finish_setup_rejects_missing_uuid_or_browser_selected_authority_before_owner(api, change):
    payload = {"deployment_id": "original-installation", "action": "finish-setup",
        "action_request_id": str(uuid.uuid4()), **change}
    response = api.client.post("/api/guide/original-guide/deployment/action", json=payload)
    assert response.status_code == 422 and api.calls == []


def test_finish_setup_forwards_only_exact_existing_action_identity(api):
    request_id = str(uuid.uuid4())
    response = api.client.post("/api/guide/original-guide/deployment/action", json={
        "deployment_id": "original-installation", "action": "finish-setup", "action_request_id": request_id})
    assert response.status_code == 200
    assert api.calls == [("action", ("original-guide", "original-installation", "finish-setup"),
        {"action_request_id": request_id})]


def test_apply_update_http_forwards_only_original_action_and_update_identity(api):
    api.selected.host._replacement_selection = lambda *args: pytest.fail("HTTP forwarding cannot select private inputs.")
    select(api)
    request_id, update_id = str(uuid.uuid4()), str(uuid.uuid4())
    payload = {"deployment_id": "original-installation", "action": "apply-update",
        "action_request_id": request_id, "update_id": update_id}
    response = api.client.post("/api/guide/original-guide/deployment/action", json=payload)
    assert response.status_code == 200
    assert api.calls == [("action", ("original-guide", "original-installation", "apply-update"),
        {"action_request_id": request_id, "update_id": update_id})]
    api.calls.clear()
    response = api.client.post("/api/guide/original-guide/deployment/action", json={
        **payload, "application_startup": {"gateway": "/browser/selected/path"}})
    assert response.status_code == 422 and api.calls == []
