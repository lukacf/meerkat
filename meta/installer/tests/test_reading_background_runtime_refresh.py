"""Stopped owned background runtime refreshes using inert native and OS fixtures."""
import asyncio
import copy
import io
import json
from pathlib import Path
import uuid
import zipfile

import pytest

from launchkit import compiler, deployment, deployment_service as services, deployment_updates as updates
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.deployment_worker import Supervisor
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates import storage_setup
from test_background_updates import FakeManager
from test_deployment import Guide, Implementation, Service, submit
from test_reading_runtime_refresh import refresh_pair
from test_runtime_dependency_updates import mock_dependency_runtime


@pytest.fixture
async def reading_background(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    Path.home().mkdir()
    monkeypatch.setattr(deployment, "require_available_console_port", lambda port: None)
    previous_files, candidate_files, _, _ = refresh_pair(tmp_path, monkeypatch)
    blueprint = json.loads(previous_files["blueprint.json"][0])
    selected_target = blueprint["target"]
    monkeypatch.setattr(deployment, "_local_host_target", lambda: selected_target)
    guide = Guide()
    guide.state["proposal"] = blueprint
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    initial = implementation.identity

    async def package(self, identity, build_id):
        assert identity == guide.identity and build_id == self.identity
        files = copy.deepcopy(previous_files if build_id == initial else candidate_files)
        report = json.loads(files["implementation-report.json"][0])
        report.update(build_id=build_id, guide_version=guide.state["version"],
            blueprint_hash=fingerprint(blueprint), brief_hash=fingerprint({}),
            verification={"digest": self.digest})
        if build_id != initial:
            binding = {"build_id": initial, "source_digest": "a" * 64}
            report["framework"].update(previous=binding, origin=binding)
        files["implementation-report.json"] = (json.dumps(report).encode(), 0o644)
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as archive:
            for name, (content, mode) in files.items():
                entry = zipfile.ZipInfo(name)
                entry.external_attr = (0o100000 | mode) << 16
                archive.writestr(entry, content)
        return stream.getvalue(), json.loads(files["deployment.json"][0])["slug"]
    monkeypatch.setattr(Implementation, "package", package)
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "installed"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    for name in ("data", ".mobkit/state", ".bin"):
        (destination / name).mkdir(parents=True, exist_ok=True)
    (destination / "data/ledger.db").write_bytes(b"accepted correction and superseded-event history")
    (destination / ".mobkit/state/native.db").write_bytes(b"native durable sessions")
    (destination / ".bin/mobkit_gateway").write_bytes(b"inert old maintenance executable")
    storage_setup.write_private(destination / ".mobkit/launch-storage.json",
        {"format": 1, "phase": "ready", "project": json.loads(previous_files["deployment.json"][0])["slug"], "version": "0.8.42"})
    update_receipt(directory, execution_mode="background", status="stopped",
        service_supervisor_pid=4567, service_instance_id="previous-instance",
        service_cleanup_confirmed=True, startup_attempt_id="previous-startup")
    frozen = services.prepare_service(directory, read_receipt(directory))
    class ReadingManager(FakeManager):
        async def start(self, definition):
            observed = await super().start(definition)
            current = read_receipt(directory)
            bootstrap = copy.deepcopy(current["bootstrap"])
            bootstrap["identities"] = {item["identity"]: {}
                for item in json.loads(candidate_files["deployment.json"][0])["agents"]}
            update_receipt(directory, bootstrap=bootstrap)
            return observed
    manager = ReadingManager(directory)
    service.service_manager = manager
    predecessor = read_receipt(directory)
    implementation.identity, implementation.digest = str(uuid.uuid4()), "c" * 64
    staged = await service.stage_update(guide.identity, predecessor["deployment_id"], 4, implementation.identity)
    update_id = staged["update"]["update_id"]

    import httpx
    class Response:
        def raise_for_status(self):
            pass
        def json(self):
            return {"identities": [{"identity": item["identity"]}
                for item in json.loads(candidate_files["deployment.json"][0])["agents"]]}
    class Client:
        def __init__(self, **kwargs):
            assert kwargs["trust_env"] is False
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            pass
        async def get(self, url, **kwargs):
            assert url.endswith("/console/identities")
            return Response()
    monkeypatch.setattr(httpx, "AsyncClient", Client)
    monkeypatch.setattr(services, "SERVICE_START_TIMEOUT", 0)
    monkeypatch.setattr(services, "SERVICE_TIMEOUT", 0)
    yield service, directory, destination, predecessor, update_id, manager, frozen
    await service.shutdown()


def private_bytes(destination):
    return {name: (destination / name).read_bytes()
            for name in (".env", ".mobkit/state/native.db", "data/ledger.db")}


def install_mocks(monkeypatch, directory, destination, *, failure=None):
    supervisor, events = mock_dependency_runtime(monkeypatch, directory, destination,
        failure=failure, target_version="0.8.47")
    monkeypatch.setattr(Supervisor, "command", type(supervisor).command)
    return events


async def test_stopped_background_refresh_retains_service_and_roles_and_gets_fresh_health(reading_background, monkeypatch):
    service, directory, destination, previous, identity, manager, frozen = reading_background
    unit = Path(frozen["unit_path"]).read_bytes()
    service_host = copy.deepcopy(frozen["host_files"])
    native = (destination / "config/mob.toml").read_bytes()
    old_private = private_bytes(destination)
    events = install_mocks(monkeypatch, directory, destination)
    await services.update_background(directory, manager, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "completed" and receipt["status"] == "running"
    assert events == ["retain", "install", "native", "sdk", "storage", "doctor", "application-check", "native-final"]
    assert receipt["update"].get("native_definition_activation") is None
    assert receipt["update"]["runtime_dependency_update"]["definition_unchanged"] is True
    assert manager.starts == [service.implementation.identity]
    assert receipt["update_service_verification"]["service_instance_id"] != "previous-instance"
    assert Path(frozen["unit_path"]).read_bytes() == unit
    assert services.verify_service(directory, receipt)["host_files"] == service_host
    assert (destination / "config/mob.toml").read_bytes() == native
    assert (destination / ".env").read_bytes() == old_private[".env"]
    for name in (".mobkit/state/native.db", "data/ledger.db"):
        assert (destination / name).read_bytes().startswith(old_private[name])
    assert receipt["package_sha256"] != previous["package_sha256"]


@pytest.mark.parametrize("failure", ["install", "storage", "cancel"])
async def test_background_dependency_failure_is_forward_only_without_replacing_service_or_history(reading_background, monkeypatch, failure):
    _, directory, destination, previous, identity, manager, frozen = reading_background
    unit = Path(frozen["unit_path"]).read_bytes()
    native = (destination / "config/mob.toml").read_bytes()
    private = private_bytes(destination)
    events = install_mocks(monkeypatch, directory, destination, failure=failure)
    with pytest.raises(asyncio.CancelledError if failure == "cancel" else RuntimeError):
        await services.update_background(directory, manager, "update", update_id=identity)
    receipt = read_receipt(directory)
    assert receipt["update"]["status"] == "recovery-needed"
    assert receipt["update"]["rollback_available"] is False
    assert receipt["update"]["runtime_admission_possible"] is True
    assert receipt["build_id"] != previous["build_id"]
    assert not manager.starts and not manager.stops
    assert "install" in events
    assert Path(frozen["unit_path"]).read_bytes() == unit
    assert (destination / "config/mob.toml").read_bytes() == native
    assert (destination / ".env").read_bytes() == private[".env"]
    assert (destination / "data/ledger.db").read_bytes().startswith(private["data/ledger.db"])
    recovered = install_mocks(monkeypatch, directory, destination)
    await services.update_background(directory, manager, "recover-current", update_id=identity)
    assert read_receipt(directory)["update"]["status"] == "completed"
    assert manager.starts == [receipt["build_id"]]
    assert "storage" in recovered


@pytest.mark.parametrize("state", ["running", "cleanup", "unit", "live-manager"])
async def test_background_refresh_refuses_changed_or_live_owner_before_runtime(reading_background, monkeypatch, state):
    _, directory, destination, _, identity, manager, frozen = reading_background
    if state == "running":
        update_receipt(directory, status="running")
        manager.observation = {"loaded": True, "pid": 12345, "state": "running"}
    elif state == "cleanup": update_receipt(directory, service_cleanup_confirmed=False)
    elif state == "unit": Path(frozen["unit_path"]).write_bytes(b"changed service owner")
    else: manager.observation = {"loaded": True, "pid": 12345, "state": "running"}
    before = private_bytes(destination)
    events = install_mocks(monkeypatch, directory, destination)
    with pytest.raises((DraftConflict, RuntimeError)):
        await services.update_background(directory, manager, "update", update_id=identity)
    assert not events and not manager.starts
    assert private_bytes(destination) == before


async def test_completed_047_refresh_keeps_archive_and_qualification_after_later_compiler(reading_background, monkeypatch):
    _, directory, destination, _, identity, manager, _ = reading_background
    install_mocks(monkeypatch, directory, destination)
    await services.update_background(directory, manager, "update", update_id=identity)
    receipt, root, update = updates.transaction(directory, identity)
    retained = {name: (root / name).read_bytes()
                for name in ("update.json", "candidate.zip", "previous.zip", "previous-receipt.json")}
    def forbidden(*args, **kwargs):
        pytest.fail("Closed .47 history must not regenerate or admit a runtime")
    monkeypatch.setattr(compiler, "generate_package", forbidden)
    monkeypatch.setattr(updates, "current_host_framework", forbidden)
    assert updates.transaction(directory, identity)[2] == update
    assert {name: (root / name).read_bytes() for name in retained} == retained
    forged = copy.deepcopy(update)
    forged["runtime_dependency_update"]["compiler_baseline_digest"] = "f" * 64
    updates.write_private(root / "update.json", forged)
    update_receipt(directory, update=forged)
    with pytest.raises(DraftConflict, match="runtime dependency"):
        updates.transaction(directory, identity)
