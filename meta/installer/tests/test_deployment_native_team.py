"""Compiled teams use the real installation boundary without a builder receipt."""
import asyncio
from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import io
import json
import os
import stat
import uuid
import zipfile
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from launchkit import deployment, public_meta_runtime, server
from launchkit.deployment import materialize, package_files, read_receipt, update_receipt
from launchkit.deployment_evidence import project_deployment_evidence
from launchkit.deployment_sources import compile_native_source, read_package_source, NATIVE_SOURCE
from launchkit.guide_models import REVIEW_TOPICS
from launchkit.projects import DraftConflict, fingerprint
from test_deployment import Guide, Service


PRIVATE = "native-team-private-value"


class NoImplementation:
    def __getattr__(self, name):
        raise AssertionError("A compiled team must not access the application builder: " + name)


def accepted_guide():
    guide = Guide()
    guide.state.update(status="ready", blueprint=guide.state["proposal"], proposal=None, question=None,
        brief={"facts": {topic: {"summary": "User supplied " + topic, "status": "confirmed",
                "source_message_ids": ["user-request"]} for topic in REVIEW_TOPICS}, "requirements": []})
    return guide


@pytest.fixture
def service(tmp_path, monkeypatch):
    guide = accepted_guide()
    service = Service(tmp_path / "host", guide, NoImplementation())
    # Capability is an OS service-manager observation, unrelated to native work.
    service.service_manager = SimpleNamespace(capability=AsyncMock(return_value={"available": False}))
    monkeypatch.setattr(deployment, "require_available_console_port", lambda *a, **k: None)
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda *a, **k: {"available": True})
    return service


async def admit(service, destination, **changes):
    _, source, _ = await service.native_package(service.guide.identity, service.guide.state["version"])
    body = dict(version=service.guide.state["version"], build_id=None, destination=str(destination),
        source_kind="native-team", source_id=source["source_id"], source_digest=source["source_digest"],
        settings={"OPENAI_API_KEY": PRIVATE})
    return await service.submit(service.guide.identity, **{**body, **changes})


async def installed(service, destination):
    receipt = await admit(service, destination)
    directory = service._directory(service.guide.identity)
    materialize(directory)
    # The fixture worker never admits a subprocess. Use its ordinary stop path
    # to retain the host's exact pre-spawn cleanup evidence.
    receipt = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    assert receipt["status"] == "stopped"
    return directory, receipt


def rehash(files):
    files = deepcopy(files)
    value = json.loads(files[NATIVE_SOURCE][0])
    value["files"] = {name: {"sha256": hashlib.sha256(content).hexdigest(), "mode": mode}
        for name, (content, mode) in sorted(files.items()) if name != NATIVE_SOURCE}
    value["source_digest"] = fingerprint(value["files"])
    value["source_id"] = fingerprint({key: item for key, item in value.items() if key != "source_id"})
    files[NATIVE_SOURCE] = (json.dumps(value).encode(), 0o644)
    return files


def test_native_package_binds_complete_compiler_bytes_modes_and_origin():
    guide = accepted_guide()
    payload, source, _ = compile_native_source(guide.state, 4)
    files = package_files(payload)
    assert source["source_kind"] == "native-team" and source["build_id"] is None
    assert source["guide_id"] == guide.identity and source["guide_version"] == 4
    assert len(source["source_id"]) == len(source["source_digest"]) == 64
    assert files[".env.example"][1] == 0o600 and files["install.sh"][1] == 0o755
    assert not any(name.startswith("application/") or name == "implementation-report.json" for name in files)
    receipt = {**source, "deployment_id": str(uuid.uuid4())}
    assert read_package_source(files, receipt) == source
    with pytest.raises(ValueError, match="binding"):
        read_package_source(files, {**receipt, "guide_id": str(uuid.uuid4())})
    with pytest.raises(ValueError, match="binding"):
        read_package_source(files, {**receipt, "source_id": "a" * 64})


@pytest.mark.parametrize("extra", ["application/__init__.py", "requirements.application.txt",
    "first-use.json", "application/configuration.json", "acceptance-report.json", "extra.js",
    "launchkit/unapproved.py"])
def test_rehashed_extensions_cannot_become_native_compiler_source(extra):
    payload, _, _ = compile_native_source(accepted_guide().state, 4)
    files = package_files(payload)
    files[extra] = (b"unreviewed extension", 0o644)
    with pytest.raises(ValueError, match="manifest"):
        read_package_source(rehash(files))


def test_native_package_retains_maintained_startup_helper_export_exactly():
    from launchkit.compiler import connect_application_finalizer_export

    payload, source, _ = compile_native_source(accepted_guide().state, 4)
    files = package_files(payload)
    exported = connect_application_finalizer_export()
    assert exported
    for artifact in exported:
        assert files[artifact.path] == (artifact.as_bytes(), artifact.mode)
    assert read_package_source(files) == source


@pytest.mark.parametrize("changed", ["content", "mode", "duplicate-manifest-key"])
def test_native_source_tampering_fails_closed(changed):
    payload, _, _ = compile_native_source(accepted_guide().state, 4)
    files = package_files(payload)
    if changed == "content":
        files["app.py"] = (b"replaced", files["app.py"][1])
    elif changed == "mode":
        files["install.sh"] = (files["install.sh"][0], 0o644)
    else:
        value = files[NATIVE_SOURCE][0]
        files[NATIVE_SOURCE] = (b'{"protocol":1,' + value[1:], 0o644)
    with pytest.raises(ValueError, match="manifest"):
        read_package_source(files)


@pytest.mark.parametrize("change", ["proposal", "question", "operation", "decision", "implementation", "version", "origin"])
async def test_only_current_accepted_complete_plan_can_prepare_installation(service, tmp_path, change):
    state = service.guide.state
    if change == "proposal": state["proposal"] = deepcopy(state["blueprint"])
    elif change == "question": state["question"] = {"id": "unanswered"}
    elif change == "operation": state["operation"] = {"status": "running"}
    elif change == "decision": state["brief"]["facts"]["authority"]["status"] = "unresolved"
    elif change == "implementation":
        state["brief"]["requirements"] = [{"id": "custom", "outcome": "Custom page", "extension": "Code needed",
            "disposition": "open", "source_message_ids": ["user-request"]}]
    elif change == "origin": state["id"] = str(uuid.uuid4())
    else: state["version"] = 5
    with pytest.raises(DraftConflict):
        await service.options(service.guide.identity, source_kind="native-team", version=4)
    assert not service.root.exists() and not service.tasks


async def test_native_options_and_duplicate_admission_use_no_builder_or_acceptance(service, tmp_path):
    options = await service.options(service.guide.identity, source_kind="native-team", version=4)
    assert options["build_id"] is None and options["current_guide_version"] == 4
    assert options["source_kind"] == "native-team" and options["supported"] is True
    assert "/deployment/package?" in options["handoff"]["package_url"]
    receipt = await admit(service, tmp_path / "team")
    try:
        again = await admit(service, tmp_path / "team")
        assert again["deployment_id"] == receipt["deployment_id"] and len(service.tasks) == 1
        assert receipt["build_id"] is None and receipt["source_id"] == options["source_id"]
        assert "acceptance" not in receipt and PRIVATE not in json.dumps(receipt)
        assert PRIVATE in (tmp_path / "team/.env").read_text()
    finally:
        await service.shutdown()


async def test_native_options_supplies_host_home_for_destination_display_without_admission(service, tmp_path, monkeypatch):
    selected_home = tmp_path / 'explicit-host-home'
    monkeypatch.setattr(deployment.Path, 'home', classmethod(lambda cls: selected_home))
    original = deepcopy(service.guide.state)
    options = await service.options(service.guide.identity, source_kind='native-team', version=4)
    assert options['home_directory'] == str(selected_home)
    assert options['default_destination'].startswith(str(selected_home / 'src') + '/')
    assert service.guide.state == original and not service.root.exists() and not service.tasks
    assert not selected_home.exists()


async def test_native_linux_destination_on_mac_prepares_manual_handoff_without_local_admission(service, tmp_path, monkeypatch):
    service.guide.state["blueprint"]["target"] = "linux"
    monkeypatch.setattr(deployment.sys, "platform", "darwin")
    original = deepcopy(service.guide.state)
    options = await service.options(service.guide.identity, source_kind="native-team", version=4)
    assert options["target"] == "linux" and options["host_target"] == "mac"
    assert options["supported"] is False and options["fresh_install"]["available"] is False
    assert options["build_id"] is None and options["source_kind"] == "native-team"
    assert options["handoff"]["mode"] == "remote-package" and options["handoff"]["status"] == "not-connected"
    assert options["handoff"]["package_url"] == f"/api/guide/{service.guide.identity}/deployment/package?source_kind=native-team&version=4"
    assert any("bash install.sh" in step["commands"] for step in options["handoff"]["steps"])
    assert all(mode["available"] is False for mode in options["execution_modes"])
    service.service_manager.capability.assert_not_awaited()
    with pytest.raises(ValueError, match="targets another host"):
        await admit(service, tmp_path / "linux-team")
    assert service.guide.state == original
    assert not service.root.exists() and not (tmp_path / "linux-team").exists() and not service.tasks


async def test_changed_source_id_refused_before_any_owned_effect(service, tmp_path):
    with pytest.raises(DraftConflict, match="source changed"):
        await admit(service, tmp_path / "team", source_id="a" * 64)
    assert not service.root.exists() and not (tmp_path / "team").exists() and not service.tasks


async def test_source_rechecked_after_private_preparation_before_admission(service, tmp_path, monkeypatch):
    original = deployment.prepare_private_settings
    def change(*args, **kwargs):
        result = original(*args, **kwargs)
        service.guide.state["brief"]["facts"]["outcome"]["summary"] = "New outcome while awaiting"
        return result
    monkeypatch.setattr(deployment, "prepare_private_settings", change)
    with pytest.raises(DraftConflict, match="team changed before installation"):
        await admit(service, tmp_path / "team")
    assert not service.root.exists() and not (tmp_path / "team").exists() and not service.tasks


async def test_materialize_restores_compiler_modes_under_private_umask_and_checks_tamper(service, tmp_path):
    receipt = await admit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    mask = os.umask(0o077)
    try:
        files = materialize(directory)
    finally:
        os.umask(mask)
    try:
        assert all(stat.S_IMODE((tmp_path / "team" / name).stat().st_mode) == mode for name, (_, mode) in files.items())
        observed = await service.guide_status(service.guide.identity, state=service.guide.state)
        assert observed["source_verified"] is True and observed["source_digest"] == receipt["source_digest"]
        (tmp_path / "team/install.sh").chmod(0o644)
        assert (await service.guide_status(service.guide.identity, state=service.guide.state))["source_verified"] is False
    finally:
        await service.shutdown()


async def test_installed_native_settings_and_restart_remain_bound_after_guide_advances(service, tmp_path):
    directory, receipt = await installed(service, tmp_path / "team")
    service.guide.state["version"] = 5
    service.guide.state["brief"]["facts"]["outcome"]["summary"] = "A different future outcome"
    options = await service.settings_options(service.guide.identity, receipt["deployment_id"])
    assert options["source_id"] == receipt["source_id"] and options["build_id"] is None and options["version"] == 5
    assert PRIVATE not in json.dumps(options)
    args = dict(guide_id=service.guide.identity, version=5, deployment_id=receipt["deployment_id"], build_id=None,
        source_digest=receipt["source_digest"], package_sha256=receipt["package_sha256"], source_kind="native-team",
        source_id=receipt["source_id"], settings={"OPENAI_API_KEY": "new-private-value"})
    with pytest.raises(DraftConflict, match="installed source changed"):
        await service.configure(**{**args, "source_id": "b" * 64})
    result = await service.configure(**args)
    assert result["source_id"] == receipt["source_id"]
    assert (await service.status(service.guide.identity))["current"] is False
    before = (tmp_path / "team/.env").read_bytes()
    restarted = await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    try:
        assert restarted["source_id"] == receipt["source_id"] and restarted["build_id"] is None
        assert read_receipt(directory)["package_sha256"] == receipt["package_sha256"]
        assert (tmp_path / "team/.env").read_bytes() == before
    finally:
        await service.shutdown()


async def test_native_use_names_exact_installed_contact_and_disables_application_paths(service, tmp_path):
    directory, receipt = await installed(service, tmp_path / "team")
    # Health is a fixture observation only. It demonstrates the public
    # projection and never starts a native worker or asserts task success.
    update_receipt(directory, status="running", process_live=True, health="ready", console_url="http://127.0.0.1:18088/console",
        health_checked_at=datetime.now(timezone.utc).isoformat(), bootstrap={"complete": True, "ready": True, "counts": {"broken": 0}})
    async def same_running_state(guide_id, *, state):
        value = read_receipt(directory)
        value.update(source_verified=True, current=True, current_guide_version=state["version"])
        return value
    service.guide_status = same_running_state
    use = await service.use(service.guide.identity)
    definition = json.loads(package_files((directory / "package.zip").read_bytes())["deployment.json"][0])
    assert use["entrypoint"]["identity"] == definition["entrypoint"]
    assert use["source_id"] == receipt["source_id"] and use["build_id"] is None
    assert use["native_runtime_available"] and use["console_url"] and not use["application_outcomes_verified"]
    assert not use["success_reporting_available"] and not use["continuation"]["available"]
    update_receipt(directory, status="stopped", process_live=False, health="unknown")
    with pytest.raises(DraftConflict, match="updates are not supported"):
        await service.stage_update(service.guide.identity, receipt["deployment_id"], 4, "invented-build")
    for action in ("apply-update", "rollback", "recover-current"):
        with pytest.raises(DraftConflict, match="updates are not supported"):
            await service.action(service.guide.identity, receipt["deployment_id"], action)
    with pytest.raises(DraftConflict, match="no application candidate"):
        await service.feedback(service.guide.identity, 4, receipt["deployment_id"], None, receipt["source_digest"],
            receipt["package_sha256"], "invented-build", "b" * 64, request="Improve it", outcome="partly")
    with pytest.raises(DraftConflict, match="success reports are unavailable"):
        await service.record_success(service.guide.identity, 4, receipt["deployment_id"], "invented-build",
            receipt["source_digest"], receipt["package_sha256"], request_id=str(uuid.uuid4()), task="An alleged job")


def test_native_http_schema_and_routes_have_no_fake_application_build(monkeypatch):
    owner = SimpleNamespace(options=AsyncMock(return_value={"source_kind": "native-team", "build_id": None}),
        submit=AsyncMock(return_value={"status": "queued"}), configure=AsyncMock(return_value={"status": "stopped"}),
        native_package=AsyncMock(return_value=(b"owned-package", {}, "test-team")))
    monkeypatch.setattr(server, "deployment", owner)
    client = TestClient(server.app)  # No lifespan; no native or main-store access.
    identity = str(uuid.uuid4())
    base = f"/api/guide/{identity}/deployment"
    response = client.get(base + "/options?source_kind=native-team&version=4")
    assert response.status_code == 200
    owner.options.assert_awaited_once_with(identity, None, source_kind="native-team", version=4)
    body = {"version": 4, "source_kind": "native-team", "source_id": "a" * 64, "source_digest": "b" * 64,
        "destination": "/owned/team", "settings": {}, "use_host_provider_credentials": False}
    assert client.post(base, json=body).status_code == 202
    owner.submit.assert_awaited_once_with(identity, 4, None, "/owned/team", {}, False,
        execution_mode="interactive", source_kind="native-team", source_id="a" * 64, source_digest="b" * 64)
    assert client.post(base, json={**body, "build_id": "fake-builder"}).status_code == 422
    assert client.post(base, json={"version": 4, "destination": "/owned/team"}).status_code == 422
    settings = {key: value for key, value in body.items() if key != "destination"}
    settings.update(deployment_id=str(uuid.uuid4()), build_id=None, package_sha256="c" * 64)
    assert client.post(base + "/settings", json=settings).status_code == 200
    owner.configure.assert_awaited_once_with(identity, 4, settings["deployment_id"], None, "b" * 64, "c" * 64,
        settings={}, remove_fields=[], use_host_provider_credentials=False, source_kind="native-team", source_id="a" * 64)
    package = client.get(base + "/package?source_kind=native-team&version=4")
    assert package.status_code == 200 and package.content == b"owned-package"
    assert client.get(base + "/package?source_kind=application&version=4").status_code == 422


def product_launched(service, launcher):
    """Select an application owner the way a trusted product launcher does."""
    service.application_setup_selection = lambda *args, **kwargs: None
    if launcher == "registry":  # The per-user application registry also selects its host.
        service.application_host_selection = lambda *args, **kwargs: None
    return service


@pytest.mark.parametrize("launcher", ["registry", "application-host"])
async def test_product_launched_forge_exports_offers_and_installs_the_agent_team(service, tmp_path, launcher):
    # A compiled agent team installs the public Meta release it pins, checked
    # against that release's checksums, whatever product this Forge selected
    # for its own applications. Every entry offers it.
    product_launched(service, launcher)
    assert service.agent_team_availability() == {"available": True}
    payload, source, _ = await service.native_package(service.guide.identity, 4)
    files = package_files(payload)
    assert json.loads(files["deployment.json"][0])["mobkitVersion"] == public_meta_runtime.VERSION
    assert b"meerkat-mobkit" not in files["requirements.txt"][0]
    options = await service.options(service.guide.identity, source_kind="native-team", version=4)
    assert options["source_kind"] == "native-team" and options["source_id"] == source["source_id"]
    receipt = await admit(service, tmp_path / "team")
    try:
        assert receipt["source_kind"] == "native-team" and receipt["build_id"] is None
        assert receipt["source_id"] == source["source_id"]
        assert PRIVATE in (tmp_path / "team/.env").read_text()
    finally:
        await service.shutdown()


async def test_source_entry_keeps_exporting_and_offering_the_agent_team(service):
    # The developer source entry selects no application owner.
    assert service.application_setup_selection is None and service.application_host_selection is None
    assert service.agent_team_availability() == {"available": True}
    payload, source, _ = await service.native_package(service.guide.identity, 4)
    assert payload and source["source_kind"] == "native-team"
    options = await service.options(service.guide.identity, source_kind="native-team", version=4)
    assert options["source_kind"] == "native-team" and options["source_id"] == source["source_id"]


def test_product_launched_http_routes_offer_agent_team_packages(service, monkeypatch):
    product_launched(service, "registry")
    monkeypatch.setattr(server, "deployment", service)
    monkeypatch.setattr(server, "planner_capability", lambda: {"available": False})
    monkeypatch.setattr(server, "cloud_capability", lambda: {"available": False})
    client = TestClient(server.app)  # No lifespan; no native or main-store access.
    assert client.get("/api/capabilities").json()["agent_team"] == {"available": True}
    package = client.get(f"/api/guide/{service.guide.identity}/deployment/package?source_kind=native-team&version=4")
    assert package.status_code == 200
    archive = client.post("/api/blueprint/package", json=service.guide.state["blueprint"])
    assert archive.status_code == 200


def test_http_routes_still_follow_a_reported_refusal(monkeypatch):
    # The routes keep honouring an owner that reports agent teams unavailable.
    refusal = "Agent team packages are not available in this fixture."
    owner = SimpleNamespace(agent_team_availability=lambda: {"available": False, "reason": refusal},
        native_package=AsyncMock(side_effect=DraftConflict(refusal)))
    monkeypatch.setattr(server, "deployment", owner)
    monkeypatch.setattr(server, "planner_capability", lambda: {"available": False})
    monkeypatch.setattr(server, "cloud_capability", lambda: {"available": False})
    client = TestClient(server.app)  # No lifespan; no native or main-store access.
    capabilities = client.get("/api/capabilities")
    assert capabilities.status_code == 200
    assert capabilities.json()["agent_team"] == {"available": False, "reason": refusal}
    package = client.get(f"/api/guide/{uuid.uuid4()}/deployment/package?source_kind=native-team&version=4")
    assert (package.status_code, package.json()["detail"]) == (409, refusal)
    archive = client.post("/api/blueprint/package", json={"schema_version": 2})
    assert (archive.status_code, archive.json()["detail"]) == (409, refusal)
