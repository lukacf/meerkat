"""Stopped remote settings edits with inert destination and transport fixtures."""
from copy import deepcopy
import json
import uuid

import pytest

from launchkit import remote_destination as controller
from launchkit.projects import DraftConflict
from launchkit.remote_deployment import read_receipt, update_receipt, write_private
from launchkit.ssh_transport import SshTransportError
from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS
from test_remote_destination import bundle, completed_service, destination, install as destination_install
from test_remote_deployment import connect, install as host_install, longest_loop_gap, service, slow_package_hashing


OPTIONAL_FIELD = {"key": "REPLY_STYLE", "label": "Reply style", "description": "An optional preference.",
                  "secret": False, "required": False, "default": "brief"}
OLD_PASSWORD = "old-private-password-123"
NEW_PASSWORD = " \tnew-private-password-456\t "


def disk_bytes(root, *, except_env=False):
    return {path.relative_to(root): path.read_bytes() for path in root.rglob("*")
            if path.is_file() and not (except_env and path == root / ".env")}


def env_values(root):
    return {key: json.loads(value) for row in (root / ".env").read_text().splitlines()
            if row and not row.startswith("#") for key, value in [row.split("=", 1)]}


def settings_request(request, action, **extra):
    return {**{key: value for key, value in request.items()
               if key not in {"operation_id", "archive", "settings"}}, "action": action, **extra}


@pytest.fixture
def private_destination(destination, monkeypatch):
    root, request, files, live = destination
    files["application/console.json"] = b'{"authentication":"local-password"}'
    files["application/configuration.json"] = json.dumps(
        {"fields": [*CONSOLE_FIELDS, OPTIONAL_FIELD]}).encode()
    request["archive"], request["binding"]["transferred_source_digest"] = bundle(files)
    request["settings"].update(CONSOLE_USER_ID="owner@example.test", CONSOLE_LOGIN_PASSWORD=OLD_PASSWORD,
                               MOBKIT_PORT="24173")
    destination_install(destination)
    completed_service(root, live, "inactive")
    worker_dead = {"value": True}
    monkeypatch.setattr(controller, "_worker_dead", lambda evidence: worker_dead["value"])
    data = root / "data/retained-fixture.txt"
    data.parent.mkdir(exist_ok=True)
    data.write_bytes(b"retained application data\n")
    with (root / ".env").open("a") as output:
        output.write('LEGACY_SETTING="untouched-private-value"\n')
    return root, request, files, live, worker_dead


def test_stopped_password_edit_preserves_source_data_owner_and_operation(private_destination):
    root, request, files, live, worker_dead = private_destination
    retained, original = disk_bytes(root, except_env=True), env_values(root)
    result = controller.handle(settings_request(request, "configure",
                                               settings={"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}))
    assert "error" not in result, result
    assert result["configuration_saved"] and result["private_settings_supported"]
    assert result["private_settings_configurable"] and result["status"] == "stopped"
    assert not result["worker_live"] and not result["ready"]
    assert result["operation_id"] == request["operation_id"]
    assert env_values(root) == {**original, "CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}
    assert disk_bytes(root, except_env=True) == retained
    assert (root / ".env").stat().st_mode & 0o777 == 0o600
    assert {path.name for path in root.glob(".env.*")} == {".env.example"}
    assert NEW_PASSWORD not in json.dumps(result) and OLD_PASSWORD not in json.dumps(result)


def test_settings_options_read_only_returns_names_and_actual_port(private_destination):
    root, request, files, live, worker_dead = private_destination
    before = disk_bytes(root)
    result = controller.handle(settings_request(request, "settings-options"))
    assert "error" not in result, result
    assert result["private_settings_supported"] and result["private_settings_configurable"]
    assert result["console_port"] == 24173
    assert {"OPENAI_API_KEY", "CONSOLE_USER_ID", "CONSOLE_LOGIN_PASSWORD", "REPLY_STYLE"} <= set(result["configured_keys"])
    assert "LEGACY_SETTING" not in result["configured_keys"]
    assert "settings" not in result and "values" not in result
    assert disk_bytes(root) == before
    for value in (OLD_PASSWORD, "private-canary", "untouched-private-value", "owner@example.test"):
        assert value not in json.dumps(result)


def test_blank_keeps_saved_values_and_removed_optional_default_stays_absent(private_destination):
    root, request, files, live, worker_dead = private_destination
    original = env_values(root)
    kept = controller.handle(settings_request(request, "configure",
        settings={"CONSOLE_LOGIN_PASSWORD": "", "REPLY_STYLE": ""}))
    assert "error" not in kept and env_values(root) == original
    initial = (root / ".env").stat()
    no_op = controller.handle(settings_request(request, "configure", settings={}))
    assert "error" not in no_op and env_values(root) == original
    unchanged = (root / ".env").stat()
    assert (unchanged.st_ino, unchanged.st_mtime_ns) == (initial.st_ino, initial.st_mtime_ns)
    removed = controller.handle(settings_request(request, "configure", settings={}, remove_fields=["REPLY_STYLE"]))
    assert "error" not in removed and "REPLY_STYLE" not in env_values(root)
    again = controller.handle(settings_request(request, "configure", settings={}))
    assert "error" not in again and "REPLY_STYLE" not in env_values(root)
    assert env_values(root)["CONSOLE_LOGIN_PASSWORD"] == OLD_PASSWORD
    fields, validator = controller._fields(files, json.loads(files["deployment.json"]), with_validator=True)
    legacy = controller._configuration(root, {}, fields, validator)
    assert b'REPLY_STYLE="brief"' in legacy[-1]


def test_private_edit_input_refuses_undeclared_required_automatic_and_unbounded_edits():
    fields = [*CONSOLE_FIELDS, OPTIONAL_FIELD,
              {"key": "GENERATED_TOKEN", "label": "Generated token", "secret": True,
               "required": False, "automatic": True}]
    assert controller.private_settings_input(fields, {"CONSOLE_LOGIN_PASSWORD": ""}, ["REPLY_STYLE"]) is None
    invalid = [({"UNKNOWN_SETTING": "private-input-marker"}, ()),
               ({"GENERATED_TOKEN": ""}, ()), ({"GENERATED_TOKEN": "private-input-marker"}, ()),
               ({"CONSOLE_LOGIN_PASSWORD": 42}, ()), ({"CONSOLE_LOGIN_PASSWORD": "private-input-marker\0"}, ()),
               ({"CONSOLE_LOGIN_PASSWORD": "private-input-marker" + "x" * 32768}, ()),
               ({}, ["CONSOLE_LOGIN_PASSWORD"]), ({}, ["GENERATED_TOKEN"]), ({}, ["UNKNOWN_SETTING"]),
               ({}, ["MOBKIT_PORT"]), ({}, ["REPLY_STYLE", "REPLY_STYLE"]),
               ({"REPLY_STYLE": "replacement"}, ["REPLY_STYLE"]), ({}, "REPLY_STYLE")]
    for settings, removed in invalid:
        with pytest.raises(controller.ControllerError) as failure:
            controller.private_settings_input(fields, settings, removed)
        assert failure.value.code == "configuration-invalid"
        assert "private-input-marker" not in str(failure.value)


@pytest.mark.parametrize("settings,key", [
    ({"CONSOLE_USER_ID": "private invalid-principal"}, "CONSOLE_USER_ID"),
    ({"CONSOLE_LOGIN_PASSWORD": "tiny-marker"}, "CONSOLE_LOGIN_PASSWORD"),
    ({"CONSOLE_LOGIN_PASSWORD": "private-surrogate-marker-\ud800"}, "CONSOLE_LOGIN_PASSWORD"),
])
def test_invalid_console_replacement_preserves_installed_private_values(private_destination, settings, key):
    root, request, files, live, worker_dead = private_destination
    before = disk_bytes(root)
    result = controller.handle(settings_request(request, "configure", settings=settings))
    assert result["error"]["code"] == "configuration-invalid"
    assert [issue["key"] for issue in result["error"]["fields"]] == [key]
    assert disk_bytes(root) == before
    assert "tiny-marker" not in json.dumps(result) and "private invalid-principal" not in json.dumps(result)
    assert "surrogate-marker" not in json.dumps(result)


def test_destination_requires_stopped_quiescent_current_owned_and_dead_worker(private_destination):
    root, request, files, live, worker_dead = private_destination
    directory = root / ".mobkit/launch-remote"
    service_path = root / ".mobkit/remote-service.json"
    originals = {path: path.read_bytes() for path in (service_path, directory / "operation.json",
                                                     directory / "owner.json", directory / "admission.json", root / "app.py")}
    for condition in ("live", "manager-unknown", "nonquiescent", "cleanup", "worker-unknown", "job-active",
                      "job-action", "admission", "source", "owner", "controller"):
        for path, content in originals.items():
            path.write_bytes(content)
        live["value"], worker_dead["value"] = False, True
        body = settings_request(request, "configure", settings={"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
        if condition == "live":
            live["value"] = True
        elif condition == "worker-unknown":
            worker_dead["value"] = False
        elif condition in {"manager-unknown", "nonquiescent"}:
            value = controller._json_read(service_path)
            value.update({"status": "unknown"} if condition == "manager-unknown" else {"control_pid": 1234})
            controller._json_write(service_path, value)
        elif condition in {"cleanup", "job-active", "job-action"}:
            value = controller._json_read(directory / "operation.json")
            value.update({"cleanup_confirmed": False} if condition == "cleanup" else
                         {"status": "installing"} if condition == "job-active" else {"action": "unknown"})
            controller._json_write(directory / "operation.json", value)
        elif condition == "admission":
            (directory / "admission.json").unlink()
        elif condition == "source":
            (root / "app.py").write_bytes(b"changed accepted source\n")
        elif condition == "owner":
            value = controller._json_read(directory / "owner.json")
            value["deployment_id"] = str(uuid.uuid4())
            controller._json_write(directory / "owner.json", value)
        elif condition == "controller":
            body["binding"] = {**body["binding"], "controller_sha256": "0" * 64}
        before = disk_bytes(root)
        result = controller.handle(body)
        assert "error" in result and result.get("configuration_saved") is not True, condition
        assert disk_bytes(root) == before, condition
        assert NEW_PASSWORD not in json.dumps(result)


def test_destination_host_drift_refuses_before_private_publication(private_destination):
    root, request, files, live, worker_dead = private_destination
    before = disk_bytes(root)
    body = settings_request(request, "configure", settings={"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
    body["expected_host"] = {**body["expected_host"], "destination": str(root.parent / "other")}
    result = controller.handle(body)
    assert result["error"]["code"] == "host-changed"
    assert disk_bytes(root) == before


def test_concurrent_private_edit_wins_over_prepared_password_update(private_destination):
    root, request, files, live, worker_dead = private_destination
    fields, validator = controller._fields(files, json.loads(files["deployment.json"]), with_validator=True)
    prepared = controller._configuration(root, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}, fields, validator, installed=True)
    newer = (root / ".env").read_bytes() + b'EXTERNAL_SETTING="newer-value"\n'
    (root / ".env").write_bytes(newer)
    with pytest.raises(controller.ControllerError) as failure:
        controller._save_configuration(prepared)
    assert failure.value.code == "configuration-changed"
    assert (root / ".env").read_bytes() == newer and {path.name for path in root.glob(".env.*")} == {".env.example"}


def test_concurrent_change_during_temp_write_preserves_newer_private_file(private_destination, monkeypatch):
    root, request, files, live, worker_dead = private_destination
    fields, validator = controller._fields(files, json.loads(files["deployment.json"]), with_validator=True)
    prepared = controller._configuration(root, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}, fields, validator, installed=True)
    newer = (root / ".env").read_bytes() + b'EXTERNAL_SETTING="changed-during-publication"\n'
    original_fsync = controller.os.fsync
    def concurrent_write(descriptor):
        (root / ".env").write_bytes(newer)
        original_fsync(descriptor)
    monkeypatch.setattr(controller.os, "fsync", concurrent_write)
    with pytest.raises(controller.ControllerError) as failure:
        controller._save_configuration(prepared)
    assert failure.value.code == "configuration-changed"
    assert (root / ".env").read_bytes() == newer and {path.name for path in root.glob(".env.*")} == {".env.example"}


@pytest.fixture
async def private_service(service, monkeypatch):
    service.implementation.application_configuration = {"fields": [*CONSOLE_FIELDS, OPTIONAL_FIELD]}
    service.implementation.extra_files["application/console.json"] = '{"authentication":"local-password"}'
    receipt = await connect(service)
    await host_install(service, receipt, CONSOLE_USER_ID="owner@example.test", CONSOLE_LOGIN_PASSWORD=OLD_PASSWORD)
    operation = read_receipt(service.directory(service.guide.identity))["install_operation_id"]
    service.transport.remote = {"status": "stopped", "admitted": True, "operation_id": operation,
        "operation_status": "completed", "worker_live": False, "cleanup_confirmed": True,
        "source_current": True, "ready": False, "service_owned": True, "service_observed": True,
        "private_settings_supported": True, "private_settings_configurable": True,
        "service": {"status": "inactive", "quiescent": True, "unit": "owned-test.service"}}
    original_request = service.transport.request
    async def request(endpoint, program, payload, timeout=30, expected_fingerprint=None):
        if payload["action"] not in {"settings-options", "configure"}:
            return await original_request(endpoint, program, payload, timeout, expected_fingerprint)
        service.transport.calls.append({"endpoint": deepcopy(endpoint), "payload": deepcopy(payload), "pin": expected_fingerprint})
        if service.transport.hook:
            await service.transport.hook(payload)
        if service.transport.fail == payload["action"]:
            raise SshTransportError("timeout")
        result = {"protocol": 1, "deployment_id": payload["deployment_id"], "binding": deepcopy(payload["binding"]),
                  "_transport": {"host_key_fingerprint": service.transport.key}, **deepcopy(service.transport.remote)}
        if payload["action"] == "settings-options":
            return {**result, "configured_keys": ["OPENAI_API_KEY", "CONSOLE_USER_ID", "CONSOLE_LOGIN_PASSWORD"],
                    "console_port": 24173}
        return {**result, "configuration_saved": True}
    monkeypatch.setattr(service.transport, "request", request)
    return service


async def configure_host(service, options, settings=None, remove_fields=None, **changes):
    selected = {key: options[key] for key in ("version", "deployment_id", "build_id", "source_digest", "package_sha256")}
    selected.update(changes)
    return await service.configure(service.guide.identity, **selected, settings=settings, remove_fields=remove_fields)


async def test_host_password_edit_keeps_operation_and_never_persists_or_reads_back_secret(private_service):
    service = private_service
    options = await service.settings_options(service.guide.identity,
        read_receipt(service.directory(service.guide.identity))["deployment_id"])
    assert options["requires_stopped"] and options["supported"] and options["console_port"] == 24173
    assert options["can_use_host_provider_credentials"] is False
    assert options["console_scope"]["authentication"] == "local-password"
    fields = {field["key"]: field for field in options["fields"]}
    assert fields["CONSOLE_LOGIN_PASSWORD"]["configured"] and not fields["CONSOLE_LOGIN_PASSWORD"]["removable"]
    assert fields["REPLY_STYLE"]["removable"] and not fields["REPLY_STYLE"]["configured"]
    directory = service.directory(service.guide.identity)
    original = read_receipt(directory)
    frozen = {path: path.read_bytes() for path in (directory / "package.zip", directory / "controller.py")}
    result = await configure_host(service, options, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
    payload = next(call["payload"] for call in reversed(service.transport.calls) if call["payload"]["action"] == "configure")
    assert payload["settings"] == {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}
    assert "operation_id" not in payload and "archive" not in payload
    assert payload["binding"]["package_sha256"] == options["package_sha256"]
    assert result["configuration_saved"] is True and result["status"] == "stopped" and not result["ready"]
    after = read_receipt(directory)
    for key in ("operation_id", "install_operation_id", "pending_operation", "controller_sha256", "package_sha256",
                "accepted_source_digest", "transferred_source_digest"):
        assert after.get(key) == original.get(key)
    assert all(path.read_bytes() == content for path, content in frozen.items())
    for value in (OLD_PASSWORD, NEW_PASSWORD):
        assert value not in json.dumps(options) + json.dumps(result)
        assert all(value.encode() not in content for content in disk_bytes(service.root).values())
    assert not service.tasks


async def test_host_lost_save_response_requires_observation_without_replaying_secret(private_service):
    service = private_service
    directory = service.directory(service.guide.identity)
    options = await service.settings_options(service.guide.identity, read_receipt(directory)["deployment_id"])
    original = read_receipt(directory)
    service.transport.fail = "configure"
    result = await configure_host(service, options, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
    assert result["status"] == "observation-unavailable" and not result["observation"]["available"]
    assert result.get("configuration_saved") is not True and not result["can_configure"]
    after = read_receipt(directory)
    for key in ("operation_id", "install_operation_id", "pending_operation"):
        assert after.get(key) == original.get(key)
    observed = await service.status(service.guide.identity, force=True)
    assert observed["status"] == "stopped" and observed.get("configuration_saved") is not True
    assert sum(call["payload"]["action"] == "configure" for call in service.transport.calls) == 1
    assert all(NEW_PASSWORD.encode() not in content for content in disk_bytes(service.root).values())
    assert not service.tasks


async def test_host_bound_source_and_version_changes_never_send_private_edit(private_service):
    service = private_service
    identity = read_receipt(service.directory(service.guide.identity))["deployment_id"]
    options = await service.settings_options(service.guide.identity, identity)
    proposal = deepcopy(service.guide.state["proposal"])
    digest = service.implementation.digest
    for condition in ("guide", "acceptance", "source", "version", "build", "digest", "package"):
        service.guide.state["proposal"] = deepcopy(proposal)
        service.implementation.accepted, service.implementation.digest = True, digest
        changes = {}
        if condition == "guide":
            service.guide.state["proposal"]["name"] = "Changed project"
        elif condition == "acceptance":
            service.implementation.accepted = False
        elif condition == "source":
            service.implementation.digest = "d" * 64
        else:
            changes[{"version": "version", "build": "build_id", "digest": "source_digest", "package": "package_sha256"}[condition]] = (
                3 if condition == "version" else str(uuid.uuid4()) if condition == "build" else "e" * 64)
        before = sum(call["payload"]["action"] == "configure" for call in service.transport.calls)
        with pytest.raises((DraftConflict, ValueError)):
            await configure_host(service, options, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}, **changes)
        assert sum(call["payload"]["action"] == "configure" for call in service.transport.calls) == before, condition
    assert all(NEW_PASSWORD.encode() not in content for content in disk_bytes(service.root).values())


async def test_host_selected_destination_and_frozen_controller_changes_block_private_edit(private_service):
    service = private_service
    directory = service.directory(service.guide.identity)
    options = await service.settings_options(service.guide.identity, read_receipt(directory)["deployment_id"])
    latest = directory.parent / "latest.json"
    latest_bytes = latest.read_bytes()
    controller_bytes = (directory / "controller.py").read_bytes()
    transport_key = service.transport.key
    for condition in ("selected", "controller", "host-key"):
        latest.write_bytes(latest_bytes)
        (directory / "controller.py").write_bytes(controller_bytes)
        service.transport.key = transport_key
        if condition == "selected":
            write_private(latest, {"deployment_id": str(uuid.uuid4())})
        elif condition == "controller":
            (directory / "controller.py").write_bytes(b"changed frozen controller\n")
        else:
            service.transport.key = "SHA256:" + "B" * 43
        before = sum(call["payload"]["action"] == "configure" for call in service.transport.calls)
        with pytest.raises(DraftConflict):
            await configure_host(service, options, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
        assert sum(call["payload"]["action"] == "configure" for call in service.transport.calls) == before, condition


async def test_host_cloud_removal_blocks_settings_without_destination_request(private_service):
    service = private_service
    directory = service.directory(service.guide.identity)
    options = await service.settings_options(service.guide.identity, read_receipt(directory)["deployment_id"])
    update_receipt(directory, target="gcp")
    async def removal(endpoint):
        return "removing"
    service.cloud_removal_status = removal
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict):
        await configure_host(service, options, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
    assert len(service.transport.calls) == before


async def test_old_controller_or_incomplete_stopped_observation_cannot_configure(private_service):
    service = private_service
    directory = service.directory(service.guide.identity)
    identity = read_receipt(directory)["deployment_id"]
    original = deepcopy(service.transport.remote)
    for condition in ("old-controller", "unknown-worker", "foreign-service", "source", "capability", "pending-control"):
        service.transport.remote = deepcopy(original)
        update_receipt(directory, pending_operation=None)
        if condition == "old-controller":
            service.transport.remote.pop("private_settings_supported")
        elif condition == "unknown-worker":
            service.transport.remote["worker_live"] = None
        elif condition == "foreign-service":
            service.transport.remote["service_owned"] = False
        elif condition == "source":
            service.transport.remote["source_current"] = False
        elif condition == "capability":
            service.transport.remote["private_settings_configurable"] = False
        else:
            update_receipt(directory, pending_operation={"id": str(uuid.uuid4()), "action": "stop"})
        observed = await service.status(service.guide.identity, force=True)
        assert not observed["can_configure"], condition
        with pytest.raises(DraftConflict):
            await service.settings_options(service.guide.identity, identity)
    assert not any(call["payload"]["action"] == "configure" for call in service.transport.calls)


async def test_host_rejects_undeclared_private_replacements_before_configure_rpc(private_service):
    service = private_service
    directory = service.directory(service.guide.identity)
    options = await service.settings_options(service.guide.identity, read_receipt(directory)["deployment_id"])
    for settings, removed in (({"PATH": "private-rejected-marker"}, ()), ({}, ["CONSOLE_LOGIN_PASSWORD"]),
                              ({}, ["UNKNOWN_SETTING"]), ({"REPLY_STYLE": "new"}, ["REPLY_STYLE"])):
        with pytest.raises(ValueError) as failure:
            await configure_host(service, options, settings, removed)
        assert "private-rejected-marker" not in str(failure.value)
    assert not any(call["payload"]["action"] == "configure" for call in service.transport.calls)
    assert all(b"private-rejected-marker" not in content for content in disk_bytes(service.root).values())


def test_remote_settings_api_forwards_exact_binding_and_redacts_invalid_bodies(monkeypatch):
    from unittest.mock import AsyncMock
    from fastapi.testclient import TestClient
    from launchkit import server
    identity, deployment, build = (str(uuid.uuid4()) for _ in range(3))
    body = {"version": 4, "deployment_id": deployment, "build_id": build,
            "source_digest": "a" * 64, "package_sha256": "b" * 64,
            "settings": {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}, "remove_fields": ["REPLY_STYLE"]}
    options = {key: body[key] for key in ("version", "deployment_id", "build_id", "source_digest", "package_sha256")}
    result = {"deployment_id": deployment, "status": "stopped", "configuration_saved": True}
    read = AsyncMock(return_value=options)
    configure = AsyncMock(return_value=result)
    monkeypatch.setattr(server.remote_deployment, "settings_options", read)
    monkeypatch.setattr(server.remote_deployment, "configure", configure)
    client = TestClient(server.app)
    path = f"/api/guide/{identity}/remote-deployment/settings"
    response = client.get(path, params={"deployment_id": deployment})
    assert response.status_code == 200 and response.json() == options
    read.assert_awaited_once_with(identity, deployment)
    response = client.post(path, json=body)
    assert response.status_code == 200 and response.json() == result
    configure.assert_awaited_once_with(identity, 4, deployment, build, "a" * 64, "b" * 64,
        settings={"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD}, remove_fields=["REPLY_STYLE"])
    assert NEW_PASSWORD not in response.text
    for changes in ({"settings": {"private-invalid-key": 42}},
                    {"private-extra-key": "private-extra-value"},
                    {"source_digest": "private-malformed-digest"},
                    {"remove_fields": [{"private-removal-key": "private-removal-value"}]}):
        response = client.post(path, json={**body, **changes})
        assert response.status_code == 422
        for marker in ("private-invalid-key", "private-extra", "private-malformed-digest", "private-removal", NEW_PASSWORD):
            assert marker not in response.text
        assert '"input"' not in response.text and '"ctx"' not in response.text
    assert configure.await_count == 1


async def test_host_private_post_rechecks_current_version_and_refuses_busy_guide(private_service):
    service = private_service
    directory = service.directory(service.guide.identity)
    options = await service.settings_options(service.guide.identity, read_receipt(directory)["deployment_id"])
    original_state = deepcopy(service.guide.state)
    original_receipt = read_receipt(directory)
    for condition in ("status-version-change", "busy-guide"):
        service.guide.state = deepcopy(original_state)
        service.transport.hook = None
        if condition == "status-version-change":
            async def changed(payload):
                if payload["action"] == "status":
                    service.guide.state["version"] += 1
            service.transport.hook = changed
        else:
            service.guide.state["operation"] = {"status": "running"}
            observed = await service.status(service.guide.identity, force=True)
            assert not observed["can_configure"]
        before = len(service.transport.calls)
        with pytest.raises(DraftConflict):
            await configure_host(service, options, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
        calls = service.transport.calls[before:]
        assert not any(call["payload"]["action"] == "configure" for call in calls), condition
        if condition == "status-version-change":
            assert any(call["payload"]["action"] == "status" for call in calls)
        else:
            assert calls == []
        saved = read_receipt(directory)
        for key in ("operation_id", "install_operation_id", "pending_operation", "guide_version"):
            assert saved.get(key) == original_receipt.get(key)
    assert all(NEW_PASSWORD.encode() not in content for content in disk_bytes(service.root).values())


async def test_host_settings_and_configure_hash_the_package_off_the_event_loop(private_service, monkeypatch):
    service = private_service
    deployment_id = read_receipt(service.directory(service.guide.identity))["deployment_id"]
    slow_package_hashing(monkeypatch)
    async def configured():
        options = await service.settings_options(service.guide.identity, deployment_id)
        return await configure_host(service, options, {"CONSOLE_LOGIN_PASSWORD": NEW_PASSWORD})
    result, gap = await longest_loop_gap(configured())
    assert result["configuration_saved"] is True and result["status"] == "stopped"
    assert gap < 0.5
