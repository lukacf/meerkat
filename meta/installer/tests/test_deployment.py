import asyncio
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import time
import uuid
import zipfile

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.deployment import (DeploymentService, configuration_fields, materialize, package_files,
    deployment_handoff, prepare_private_settings, private_settings, read_receipt, update_receipt, verify_destination)
from launchkit.deployment_worker import Redactor, clean_environment, readiness
from launchkit.deployment_worker import Supervisor, read_settings
from launchkit.deployment_worker import knowledge_container_owned
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS


class Guide:
    def __init__(self):
        self.identity = str(uuid.uuid4())
        proposal = starter_blueprint().model_dump(mode="json")
        proposal["target"] = "mac" if os.sys.platform == "darwin" else "linux"
        self.state = {"id": self.identity, "version": 4, "proposal": proposal}

    async def get(self, identity):
        if identity != self.identity:
            raise LookupError("Unknown guide")
        return deepcopy(self.state)


class Implementation:
    def __init__(self, guide):
        self.guide = guide
        self.identity = str(uuid.uuid4())
        self.original = deepcopy(guide.state)
        self.accepted = True
        self.digest = "checked-source-digest"
        self.application_configuration = None
        self.extra_files = {}
        self.install_script = "exit 0\n"
        self.definition = {"port": 18088, "target": guide.state["proposal"]["target"], "slug": "test-team",
            "environment": [{"key": "OPENAI_API_KEY", "label": "OpenAI key", "secret": True, "required": True}],
            "agents": [{"identity": "lead:main"}], "classes": [{"tools": {"mcp": [], "knowledge": False}}],
            "knowledge": {"mode": "none"}}

    async def status(self, identity):
        await self.guide.get(identity)
        return {"guide_id": identity, "guide_version": self.original["version"],
            "current_guide_version": self.guide.state["version"], "build_id": self.identity,
            "current": True, "ready_to_package": True, "status": "completed", "source_digest": self.digest,
            "acceptance_current": self.accepted, "acceptance_trusted": self.accepted,
            "acceptance": {"status": "pass"} if self.accepted else None}

    def selected_build_id(self, identity):
        if identity != self.guide.identity:
            raise LookupError("Unknown guide")
        return self.identity

    async def package(self, identity, build_id):
        if build_id != self.identity:
            raise DraftConflict("Another build selected")
        await self.guide.get(identity)
        result = io.BytesIO()
        report = {"build_id": build_id, "guide_version": self.original["version"],
            "brief_hash": fingerprint(self.guide.state.get("brief") or {}),
            "blueprint_hash": fingerprint(self.guide.state["proposal"]), "verification": {"digest": self.digest}, "unresolved": ["Actual external data not verified"]}
        if self.original.get("organization") is not None:
            report["organization_hash"] = fingerprint(self.original["organization"])
        with zipfile.ZipFile(result, "w") as archive:
            for path, content in {"deployment.json": json.dumps(self.definition), "implementation-report.json": json.dumps(report),
                "app.py": "print('native test fixture')\n", "install.sh": self.install_script, "application/__init__.py": ""}.items():
                archive.writestr(zipfile.ZipInfo(path), content)
            if self.application_configuration is not None:
                archive.writestr(zipfile.ZipInfo("application/configuration.json"), json.dumps(self.application_configuration))
            for path, content in self.extra_files.items():
                archive.writestr(zipfile.ZipInfo(path), content)
        return result.getvalue(), "test-team"


class Service(DeploymentService):
    async def _run(self, directory, mode, *, update_selection=None):
        try:
            await asyncio.Event().wait()
        finally:
            self.tasks.pop(directory.name, None)


@pytest.fixture
def service(tmp_path):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    return Service(tmp_path / "private-server", guide, implementation)


async def submit(service, destination, **changes):
    values = {"version": service.guide.state["version"], "build_id": service.implementation.identity,
        "destination": str(destination), "settings": {"OPENAI_API_KEY": "private-api-key-value"}}
    return await service.submit(service.guide.identity, **{**values, **changes})


async def test_detached_deployment_evidence_does_not_reenter_guide_and_verifies_exact_installed_source(tmp_path):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "b" * 64
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None)
    state = {**guide.state, "id": guide.identity}
    async def recursive_guide(_): raise AssertionError("detached evidence must not read the guide")
    guide.get = recursive_guide
    receipt = await service.status(guide.identity, state=state)
    assert receipt["current"] is True
    evidence = await service.guide_status(guide.identity, state=state)
    assert evidence["source_verified"] is True and evidence["source_digest"] == "b" * 64
    harmless = await service.status(guide.identity, state={**state, "version": 5})
    assert harmless["current"] is True and harmless["current_guide_version"] == 5
    assert harmless["guide_version"] == 4
    assert (await service.status(guide.identity, state={**state, "brief": {"outcome": "New required UI"}}))["current"] is False
    with pytest.raises(ValueError, match="matching detached"):
        await service.guide_status(guide.identity, state={**state, "id": "another guide"})
    (destination / "application/__init__.py").write_text("modified installed source")
    unverified = await service.guide_status(guide.identity, state=state)
    assert unverified["source_verified"] is False and unverified["source_digest"] is None


async def test_explicit_install_claims_owned_empty_folder_and_secrets_never_enter_journal(service, tmp_path):
    target = tmp_path / "team"
    receipt = await submit(service, target)
    try:
        assert receipt["status"] == "queued"
        assert receipt["build_id"] == service.implementation.identity
        assert receipt["package_sha256"] == hashlib.sha256((service._directory(service.guide.identity) / "package.zip").read_bytes()).hexdigest()
        assert json.loads((target / ".mobkit/launch-deployment-owner.json").read_text())["deployment_id"] == receipt["deployment_id"]
        assert "private-api-key-value" in (target / ".env").read_text()
        assert (target / ".env").stat().st_mode & 0o777 == 0o600
        assert "private-api-key-value" not in json.dumps(receipt)
        for file in service.root.rglob("*.json"):
            assert "private-api-key-value" not in file.read_text()
        materialize(service._directory(service.guide.identity))
        assert (target / "app.py").is_file()
    finally:
        await service.shutdown()


async def test_no_implicit_host_provider_copy_and_only_explicit_selected_keys(service, tmp_path, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "host-private-key")
    monkeypatch.setenv("ANTHROPIC_API_KEY", "unselected-host-key")
    options = await service.options(service.guide.identity, service.implementation.identity)
    assert options["host_provider_keys"] == ["OPENAI_API_KEY"]
    assert options["fresh_install"] == {"available": True, "reason": None}
    assert "host-private-key" not in json.dumps(options)
    with pytest.raises(ValueError, match="OPENAI_API_KEY"):
        await submit(service, tmp_path / "team", settings={})
    receipt = await submit(service, tmp_path / "team", settings={}, use_host_provider_credentials=True)
    try:
        text = (tmp_path / "team/.env").read_text()
        assert "host-private-key" in text
        assert "unselected-host-key" not in text
        assert "host-private-key" not in json.dumps(receipt)
    finally:
        await service.shutdown()


async def test_private_presence_options_bind_the_exact_selected_same_source_folder_without_values(service, tmp_path):
    service.implementation.application_configuration = {"fields": [
        {"key": "APP_TOKEN", "label": "Application token", "secret": True, "required": False}]}
    empty = await service.options(service.guide.identity)
    assert empty["configured_deployment_id"] is None and empty["configured_destination"] is None
    assert not any(field["configured"] for field in empty["fields"])
    first = await submit(service, tmp_path / "first-folder", settings={
        "OPENAI_API_KEY": "first-private-provider", "APP_TOKEN": "first-private-token"})
    first_options = await service.options(service.guide.identity)
    assert first_options["configured_deployment_id"] == first["deployment_id"]
    assert first_options["configured_destination"] == first["destination"]
    assert next(field["configured"] for field in first_options["fields"] if field["key"] == "APP_TOKEN")
    await service.shutdown()
    second = await submit(service, tmp_path / "second-folder", settings={"OPENAI_API_KEY": "second-private-provider"})
    try:
        first_private = (tmp_path / "first-folder/.env").read_bytes()
        second_private = (tmp_path / "second-folder/.env").read_bytes()
        # A different tab still has the first receipt, while server selection
        # and this same-build options response belong to the second folder.
        options = await service.options(service.guide.identity)
        assert first["build_id"] == second["build_id"] == options["build_id"]
        assert options["configured_deployment_id"] == second["deployment_id"] != first["deployment_id"]
        assert options["configured_destination"] == second["destination"] != first["destination"]
        assert next(field["configured"] for field in options["fields"] if field["key"] == "OPENAI_API_KEY")
        assert not next(field["configured"] for field in options["fields"] if field["key"] == "APP_TOKEN")
        assert "first-private" not in json.dumps(options) and "second-private" not in json.dumps(options)
        assert (tmp_path / "first-folder/.env").read_bytes() == first_private
        assert (tmp_path / "second-folder/.env").read_bytes() == second_private
    finally:
        await service.shutdown()


@pytest.mark.parametrize("change", [{"build_id": "older-build"}, {"guide_version": 3}, {"guide_version": True}])
async def test_private_presence_options_require_same_build_and_immutable_original_version(service, tmp_path, change):
    await submit(service, tmp_path / "team")
    try:
        update_receipt(service._directory(service.guide.identity), **change)
        options = await service.options(service.guide.identity)
        assert options["configured_deployment_id"] is None and options["configured_destination"] is None
        assert not any(field["configured"] for field in options["fields"])
        assert "private-api-key-value" not in json.dumps(options)
    finally:
        await service.shutdown()


@pytest.mark.parametrize("settings", [
    {}, [],
    {"OPENAI_API_KEY": 42},
    {"OPENAI_API_KEY": "private-key", "UNDECLARED_TOKEN": "private-unrequested-value"},
    {"OPENAI_API_KEY": "private-key", "MOBKIT_PORT": "outside-the-port-range"},
])
async def test_rejected_initial_settings_never_claim_a_folder_or_select_a_queued_deployment(service, tmp_path, settings):
    target = tmp_path / "team"
    with pytest.raises(ValueError):
        await submit(service, target, settings=settings)
    assert not target.exists()
    assert not service.root.exists()
    assert not service.tasks
    with pytest.raises(LookupError, match="No local deployment"):
        await service.status(service.guide.identity)
    # The same destination remains usable through the ordinary setup form.
    receipt = await submit(service, target)
    assert receipt["status"] == "queued"
    await service.shutdown()


async def test_local_knowledge_missing_extraction_key_is_rejected_before_install_admission(service, tmp_path, monkeypatch):
    from launchkit import deployment
    from launchkit.compiler import generate_files

    # This fixture checks key validation with a dummy worker that never listens.
    # Keep host port availability separate from this synthetic install boundary;
    # real bind/rebind behavior is checked by the dedicated owned socket proof.
    def available_port(port, **_):
        return {"scope": "local-console-port-observation", "port": port,
                "status": "available", "observed_at": "2026-10-03T00:00:00+00:00"}
    port_checks = []
    monkeypatch.setattr(deployment, "observe_local_console_port", available_port)
    monkeypatch.setattr(deployment, "require_available_console_port",
                        lambda port: port_checks.append(port) or available_port(port))

    data = deepcopy(service.guide.state["proposal"])
    data.update(provider="anthropic", model="claude-sonnet-4-6")
    data["knowledge"]["mode"] = "local"
    compiled = generate_files(data)
    service.guide.state["proposal"] = data
    service.implementation.original = deepcopy(service.guide.state)
    service.implementation.definition = json.loads(compiled["deployment.json"])
    service.implementation.extra_files["knowledge/settings.json"] = compiled["knowledge/settings.json"]
    options = await service.options(service.guide.identity)
    fields = {row["key"]: row for row in options["fields"]}
    assert fields["ANTHROPIC_API_KEY"]["required"] is True
    assert fields["ANTHROPIC_API_KEY"]["secret"] is True and "default" not in fields["ANTHROPIC_API_KEY"]
    target = tmp_path / "local-knowledge-team"
    with pytest.raises(ValueError, match="Anthropic API key for knowledge extraction"):
        await submit(service, target)
    assert not target.exists() and not service.root.exists() and not service.tasks
    assert port_checks == []
    with pytest.raises(LookupError, match="No local deployment"):
        await service.status(service.guide.identity)

    values = {"OPENAI_API_KEY": "private-agent-and-embedding-key",
              "ANTHROPIC_API_KEY": "private-extraction-key"}
    try:
        receipt = await submit(service, target, settings=values)
        assert receipt["status"] == "queued"
        assert port_checks == [service.implementation.definition["port"]]
        assert {key: read_settings(target)[key] for key in values} == values
        public = json.dumps(receipt) + json.dumps(options)
        assert all(value not in public for value in values.values())
    finally:
        await service.shutdown()


@pytest.mark.parametrize("reuse_destination", [False, True])
async def test_rejected_settings_preserve_selected_owned_deployment_and_private_values(service, tmp_path, reuse_destination):
    original = tmp_path / "installed"
    receipt = await submit(service, original)
    materialize(service._directory(service.guide.identity))
    await service.shutdown()
    directory = service._directory(service.guide.identity)
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None)
    retained = {path: path.read_bytes() for path in [
        directory.parent / "latest.json", directory / "receipt.json",
        original / ".env", original / ".mobkit/launch-deployment-owner.json"]}
    target = original if reuse_destination else tmp_path / "replacement"
    with pytest.raises(ValueError, match="Console port"):
        await submit(service, target, settings={"OPENAI_API_KEY": "replacement-private-value", "MOBKIT_PORT": "0"})
    assert all(path.read_bytes() == content for path, content in retained.items())
    assert (await service.status(service.guide.identity))["deployment_id"] == receipt["deployment_id"]
    assert (await service.status(service.guide.identity))["status"] == "stopped"
    assert len(list(directory.parent.glob("*/receipt.json"))) == 1
    assert reuse_destination or not target.exists()
    assert not service.tasks


async def test_owned_destination_reuse_does_not_depend_on_the_package_clock(service, tmp_path, monkeypatch):
    # The same build packaged a second later must stay the same package, or
    # reusing its owned destination is refused as a foreign non-empty folder.
    original = tmp_path / "installed"
    await submit(service, original)
    materialize(service._directory(service.guide.identity))
    await service.shutdown()
    update_receipt(service._directory(service.guide.identity), status="stopped",
        interactive_lifecycle_id=None, interactive_cleanup=None)

    class LaterClock:
        def __getattr__(self, name):
            return getattr(time, name)

        def time(self):
            return time.time() + 2

    monkeypatch.setattr(zipfile, "time", LaterClock())
    with pytest.raises(ValueError, match="Console port"):
        await submit(service, original, settings={"OPENAI_API_KEY": "replacement-private-value", "MOBKIT_PORT": "0"})


@pytest.mark.parametrize("has_previous", [False, True])
async def test_private_write_failure_does_not_select_a_queued_candidate_and_form_can_retry(service, tmp_path, monkeypatch, has_previous):
    from launchkit import deployment
    retained = {}
    if has_previous:
        await submit(service, tmp_path / "installed")
        await service.shutdown()
        directory = service._directory(service.guide.identity)
        update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None)
        retained = {path: path.read_bytes() for path in [directory.parent / "latest.json", directory / "receipt.json"]}
    target = tmp_path / "candidate"
    original_writer = deployment.write_private_settings
    def failed_write(*args, **kwargs):
        raise PermissionError("synthetic private storage failure")
    monkeypatch.setattr(deployment, "write_private_settings", failed_write)
    with pytest.raises(ValueError, match="Private setup could not be saved"):
        await submit(service, target)
    assert all(path.read_bytes() == content for path, content in retained.items())
    assert list(target.iterdir()) == []
    assert len(list(service.root.rglob("package.zip"))) == int(has_previous)
    assert not service.tasks
    if not has_previous:
        with pytest.raises(LookupError, match="No local deployment"):
            await service.status(service.guide.identity)
    monkeypatch.setattr(deployment, "write_private_settings", original_writer)
    receipt = await submit(service, target)
    assert receipt["status"] == "queued"
    await service.shutdown()


@pytest.mark.parametrize("stage", ["receipt", "owner", "package", "journal", "latest"])
async def test_partial_candidate_publication_preserves_previous_selection_and_can_retry(service, tmp_path, monkeypatch, stage):
    from launchkit import deployment
    await submit(service, tmp_path / "installed")
    await service.shutdown()
    directory = service._directory(service.guide.identity)
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None)
    retained = {path: path.read_bytes() for path in [
        directory.parent / "latest.json", directory / "receipt.json", tmp_path / "installed/.env"]}
    original_private, original_open = deployment.write_private, deployment.os.open
    receipt_writes = 0
    failed = False
    def interrupted_private(path, value):
        nonlocal receipt_writes, failed
        if path.name == "receipt.json":
            receipt_writes += 1
        if ((stage == "receipt" and path.name == "receipt.json" and receipt_writes == 1)
                or (stage == "journal" and path.name == "receipt.json" and receipt_writes == 2)
                or (stage == "owner" and path.name == "launch-deployment-owner.json")
                or (stage == "latest" and path.name == "latest.json")):
            failed = True
            path.with_suffix(".pending").write_text("synthetic interrupted metadata write")
            raise PermissionError("synthetic publication failure")
        return original_private(path, value)
    def interrupted_open(path, flags, *args, **kwargs):
        nonlocal failed
        if stage == "package" and Path(path).name == "package.zip":
            failed = True
            Path(path).write_bytes(b"synthetic incomplete package")
            raise PermissionError("synthetic package write failure")
        return original_open(path, flags, *args, **kwargs)
    monkeypatch.setattr(deployment, "write_private", interrupted_private)
    monkeypatch.setattr(deployment.os, "open", interrupted_open)
    target = tmp_path / "candidate"
    with pytest.raises(ValueError, match="Private setup could not be saved"):
        await submit(service, target)
    assert failed
    assert all(path.read_bytes() == content for path, content in retained.items())
    assert (not target.exists()) or list(target.iterdir()) == []
    assert set(directory.parent.iterdir()) == {directory, directory.parent / "latest.json"}
    assert not service.tasks
    monkeypatch.setattr(deployment, "write_private", original_private)
    monkeypatch.setattr(deployment.os, "open", original_open)
    receipt = await submit(service, target)
    assert receipt["status"] == "queued"
    await service.shutdown()


async def test_design_revision_during_final_status_check_rejects_before_claiming_destination(service, tmp_path):
    original_status = service.implementation.status
    calls = 0
    async def changing_status(identity):
        nonlocal calls
        calls += 1
        if calls == 2:
            service.guide.state["version"] += 1
        return await original_status(identity)
    service.implementation.status = changing_status
    with pytest.raises(DraftConflict, match="proposal changed before deployment admission"):
        await submit(service, tmp_path / "team")
    assert calls == 2
    assert not (tmp_path / "team").exists()
    assert not service.root.exists()
    assert not service.tasks


async def test_new_build_selected_during_final_guide_read_rejects_before_claiming_destination(service, tmp_path):
    original_guide = service._guide
    calls = 0
    async def changing_selection(identity):
        nonlocal calls
        calls += 1
        state = await original_guide(identity)
        if calls == 2:
            service.implementation.identity = str(uuid.uuid4())
        return state
    service._guide = changing_selection
    with pytest.raises(DraftConflict, match="proposal changed before deployment admission"):
        await submit(service, tmp_path / "team")
    assert calls == 2
    assert not (tmp_path / "team").exists()
    assert not service.root.exists()
    assert not service.tasks


async def test_user_file_created_during_private_preparation_is_preserved_and_not_claimed(service, tmp_path, monkeypatch):
    from launchkit import deployment
    target = tmp_path / "team"
    original_prepare = deployment.prepare_private_settings
    def destination_changed(*args, **kwargs):
        values = original_prepare(*args, **kwargs)
        target.mkdir()
        (target / "user-file.txt").write_text("independently owned file")
        return values
    monkeypatch.setattr(deployment, "prepare_private_settings", destination_changed)
    with pytest.raises(DraftConflict, match="not empty"):
        await submit(service, target)
    assert (target / "user-file.txt").read_text() == "independently owned file"
    assert list(target.iterdir()) == [target / "user-file.txt"]
    assert not service.root.exists()
    assert not service.tasks


async def test_existing_private_write_failure_preserves_configuration_and_can_retry(service, tmp_path, monkeypatch):
    from launchkit import deployment
    target = tmp_path / "team"
    receipt = await submit(service, target)
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None, health="ready", health_checked_at="old-check",
                   startup_attempt_id="prior-attempt", startup_console_health={"status": "fail", "category": "authentication"})
    (target / ".mobkit/state").mkdir()
    history = target / ".mobkit/state/retained-history"
    history.write_text("native history retained")
    retained = {path: path.read_bytes() for path in [
        target / ".env", target / ".mobkit/launch-deployment-owner.json", history,
        directory / "package.zip", directory.parent / "latest.json"]}
    original_writer = deployment.write_private_settings
    def failed_write(*args, **kwargs):
        raise PermissionError("synthetic private storage failure")
    monkeypatch.setattr(deployment, "write_private_settings", failed_write)
    with pytest.raises(ValueError, match="Private setup could not be saved"):
        await submit(service, target, settings={"OPENAI_API_KEY": "replacement-private-value"})
    failed = await service.status(service.guide.identity)
    assert failed["deployment_id"] == receipt["deployment_id"]
    assert failed["status"] == "stopped" and failed["phase"] == "configure"
    assert failed["health"] == "failed" and failed["health_checked_at"] is None
    assert failed["process_live"] is False and failed["runtime_pid"] is None
    assert all(path.read_bytes() == content for path, content in retained.items())
    assert "replacement-private-value" not in json.dumps(failed)
    assert not service.tasks
    monkeypatch.setattr(deployment, "write_private_settings", original_writer)
    retried = await submit(service, target)
    assert retried["deployment_id"] == receipt["deployment_id"]
    assert retried["status"] == "queued" and retried["health"] == "unknown"
    assert retried["startup_attempt_id"] is None and retried["startup_console_health"] is None
    await service.shutdown()


def test_private_preparation_is_read_only_and_partial_write_preserves_existing_values(tmp_path, monkeypatch):
    from launchkit import deployment
    target = tmp_path / "team"
    fields = [{"key": "PRIVATE_TOKEN", "label": "Private token", "required": True, "secret": True}]
    prepared = prepare_private_settings(target, fields, {"PRIVATE_TOKEN": "new-private-value"}, False, port=18088)
    assert not target.exists()
    target.mkdir()
    private_settings(target, fields, {"PRIVATE_TOKEN": "retained-private-value"}, False, port=18088)
    retained = (target / ".env").read_bytes()
    def interrupted_flush(*args, **kwargs):
        raise OSError("synthetic flush failure")
    monkeypatch.setattr(deployment.os, "fsync", interrupted_flush)
    with pytest.raises(OSError, match="synthetic flush failure"):
        deployment.write_private_settings(target, prepared)
    assert (target / ".env").read_bytes() == retained
    assert not (target / ".env.launch-pending").exists()
    assert (target / ".env").stat().st_mode & 0o777 == 0o600


async def test_nonempty_and_symlink_destinations_are_never_overwritten(service, tmp_path):
    target = tmp_path / "existing"
    target.mkdir()
    (target / "keep.txt").write_text("user-owned")
    with pytest.raises(DraftConflict, match="not empty"):
        await submit(service, target)
    assert list(target.iterdir()) == [target / "keep.txt"]
    linked = tmp_path / "linked"
    linked.symlink_to(target, target_is_directory=True)
    with pytest.raises(ValueError, match="symbolic links"):
        await submit(service, linked / "child")
    assert (target / "keep.txt").read_text() == "user-owned"


async def test_version_build_acceptance_and_target_are_admission_fences(service, tmp_path):
    with pytest.raises(DraftConflict, match="guide is changing"):
        await submit(service, tmp_path / "team", version=3)
    with pytest.raises(DraftConflict, match="newer application build"):
        await submit(service, tmp_path / "team", build_id=str(uuid.uuid4()))
    service.implementation.accepted = False
    with pytest.raises(ValueError, match="independent acceptance"):
        await submit(service, tmp_path / "team")
    service.implementation.accepted = True
    service.guide.state["proposal"]["target"] = "gcp"
    with pytest.raises(ValueError, match="another host"):
        await submit(service, tmp_path / "team")
    assert not (tmp_path / "team").exists()


@pytest.mark.parametrize("resume_action", ["restart", "retry"])
async def test_server_restart_is_interrupted_and_retry_retains_state_but_rejects_changed_source(service, tmp_path, resume_action):
    target = tmp_path / "team"
    receipt = await submit(service, target)
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    fresh = Service(service.root.parent, service.guide, service.implementation)
    assert (await fresh.status(service.guide.identity))["status"] == "interrupted"
    (target / ".mobkit/state").mkdir()
    history = target / ".mobkit/state/preserved-history"
    history.write_text("native-owned continuity")
    old_attempt = str(uuid.uuid4())
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None, startup_attempt_id=old_attempt,
        health="ready", health_checked_at="previous-observation", process_live=False,
        startup_console_health={"attempt_id": old_attempt, "status": "fail", "category": "authentication", "attempts": 1})
    restarted = await fresh.action(service.guide.identity, receipt["deployment_id"], resume_action)
    try:
        assert restarted["status"] == "queued"
        assert restarted["startup_attempt_id"] is None and restarted["startup_console_health"] is None
        assert restarted["health"] == "unknown" and restarted["health_checked_at"] is None
        assert restarted["process_live"] is False
        assert history.read_text() == "native-owned continuity"
    finally:
        await fresh.shutdown()
    # Isolate installed-byte authority from the separate unknown-cleanup fence.
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None, process_live=False, runtime_pid=None)
    (target / "app.py").write_text("changed source")
    with pytest.raises(DraftConflict, match="Installed source differs"):
        await fresh.action(service.guide.identity, receipt["deployment_id"], "retry")
    assert (target / "app.py").read_text() == "changed source"


async def test_ownership_and_package_tampering_never_start_a_process(service, tmp_path):
    receipt = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    fresh = Service(service.root.parent, service.guide, service.implementation)
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None, process_live=False, runtime_pid=None)
    (directory / "package.zip").write_bytes(b"different package")
    with pytest.raises(DraftConflict, match="saved checked package changed"):
        await fresh.action(service.guide.identity, receipt["deployment_id"], "restart")
    assert not fresh.tasks


@pytest.mark.parametrize("real_supervisor", [False, True])
async def test_shutdown_before_worker_coroutine_first_instruction_is_durable(service, tmp_path, monkeypatch, real_supervisor):
    spawned = []
    receipt = await submit(service, tmp_path / "team")
    if real_supervisor:
        await service.shutdown()
        service = DeploymentService(service.root.parent, service.guide, service.implementation)
        async def unexpected_spawn(*args, **kwargs):
            spawned.append(args)
            raise AssertionError("Shutdown must cancel this queued worker before subprocess admission.")
        monkeypatch.setattr(asyncio, "create_subprocess_exec", unexpected_spawn)
        directory = service._directory(service.guide.identity)
        update_receipt(directory, status="queued")
        service._launch(directory, "install")
    # The service task has not been yielded to after admission.
    await service.shutdown()
    assert not service.tasks
    stored = read_receipt(service._directory(service.guide.identity))
    assert stored["status"] == "interrupted"
    assert stored["runtime_pid"] is None
    assert stored["deployment_id"] == receipt["deployment_id"]
    assert not spawned


async def test_shutdown_serializes_restart_and_preserves_the_later_admission(service, tmp_path):
    receipt = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    entered, stopping, release, restart_entered = (asyncio.Event() for _ in range(4))

    class PausedShutdown(Service):
        runs = 0
        async def _action_locked(self, *args, **kwargs):
            restart_entered.set()
            return await super()._action_locked(*args, **kwargs)

        async def _run(self, directory, mode):
            self.runs += 1
            first = self.runs == 1
            if first:
                entered.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                if first:
                    stopping.set()
                    await release.wait()
                    # This control isolates admission locking from native
                    # teardown, which has separate source-bound controls.
                    update_receipt(directory, status="stopped", interactive_lifecycle_id=None,
                                   interactive_cleanup=None, process_live=False, runtime_pid=None)
                raise
            finally:
                self.tasks.pop(directory.name, None)

    fresh = PausedShutdown(service.root.parent, service.guide, service.implementation)
    update_receipt(directory, status="running", interactive_lifecycle_id=None, interactive_cleanup=None)
    old_task = asyncio.create_task(fresh._run(directory, "start"))
    fresh.tasks[directory.name] = old_task
    await asyncio.wait_for(entered.wait(), 30)
    shutting_down = asyncio.create_task(fresh.shutdown())
    await asyncio.wait_for(stopping.wait(), 30)
    restarting = asyncio.create_task(fresh.action(service.guide.identity, receipt["deployment_id"], "restart"))
    await asyncio.sleep(0)
    try:
        assert fresh.lock.locked() and not restarting.done() and not restart_entered.is_set()
        assert fresh.tasks[directory.name] is old_task
        release.set()
        await asyncio.wait_for(shutting_down, 1)
        restarted = await asyncio.wait_for(restarting, 1)
        new_task = fresh.tasks[directory.name]
        assert restarted["status"] == "queued" and new_task is not old_task
        assert old_task.cancelled() and not new_task.done()
        assert read_receipt(directory)["interactive_lifecycle_id"] != receipt["interactive_lifecycle_id"]
    finally:
        release.set()
        await asyncio.gather(shutting_down, restarting, return_exceptions=True)
        await fresh.shutdown()


async def test_cancelled_shutdown_drains_its_original_join_and_final_receipt_pass(service, tmp_path):
    receipt = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.shutdown()
    entered, stopping, release = asyncio.Event(), asyncio.Event(), asyncio.Event()

    class PausedShutdown(Service):
        async def _run(self, directory, mode):
            entered.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                stopping.set()
                await release.wait()
                raise
            finally:
                self.tasks.pop(directory.name, None)

    fresh = PausedShutdown(service.root.parent, service.guide, service.implementation)
    update_receipt(directory, status="queued", console_url="http://127.0.0.1:18088/", runtime_pid=1234567)
    task = asyncio.create_task(fresh._run(directory, "install"))
    fresh.tasks[directory.name] = task
    await asyncio.wait_for(entered.wait(), 30)
    shutting_down = asyncio.create_task(fresh.shutdown())
    await asyncio.wait_for(stopping.wait(), 30)
    try:
        for _ in range(3):
            shutting_down.cancel()
            await asyncio.sleep(0)
        assert not shutting_down.done() and not task.done()
        assert fresh.lock.locked() and fresh.tasks[directory.name] is task
        assert read_receipt(directory)["status"] == "queued"
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(shutting_down, 1)
        stored = read_receipt(directory)
        assert task.cancelled() and not fresh.tasks and not fresh.lock.locked()
        assert stored["status"] == "interrupted" and stored["console_url"] is None
        assert stored["runtime_pid"] is None and stored["process_live"] is False
        assert stored["deployment_id"] == receipt["deployment_id"]
    finally:
        release.set()
        await asyncio.gather(shutting_down, return_exceptions=True)
        await fresh.shutdown()


async def test_shutdown_cancellation_while_waiting_for_admission_lock_still_finishes(service, tmp_path):
    receipt = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.lock.acquire()
    shutting_down = asyncio.create_task(service.shutdown())
    try:
        await asyncio.sleep(0)
        for _ in range(3):
            shutting_down.cancel()
            await asyncio.sleep(0)
        assert not shutting_down.done() and service.tasks[directory.name] is not None
        service.lock.release()
        with pytest.raises(asyncio.CancelledError):
            await asyncio.wait_for(shutting_down, 1)
        assert not service.tasks and not service.lock.locked()
        assert read_receipt(directory)["status"] == "interrupted"
        assert read_receipt(directory)["deployment_id"] == receipt["deployment_id"]
    finally:
        if service.lock.locked() and not shutting_down.done():
            service.lock.release()
        await asyncio.gather(shutting_down, return_exceptions=True)
        await service.shutdown()


@pytest.mark.parametrize("cancel_shutdown", [False, True])
async def test_shutdown_at_unlocked_lock_handoff_preserves_custody_before_yield(
        service, tmp_path, monkeypatch, cancel_shutdown):
    receipt = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.shutdown()
    fresh = DeploymentService(service.root.parent, service.guide, service.implementation)
    queued, entered, release, exited = (asyncio.Event() for _ in range(4))
    spawned = []

    class Input:
        def close(self):
            exited.set()

    class Process:
        returncode = None
        stdin = Input()
        async def wait(self):
            await exited.wait()
            self.returncode = 0
            return 0

    process = Process()
    async def observing_spawn(*args, **kwargs):
        spawned.append(args)
        return process

    monkeypatch.setattr(asyncio, "create_subprocess_exec", observing_spawn)
    monkeypatch.setattr(os, "killpg", lambda *args: pytest.fail("This control must never signal a process."))
    await fresh.lock.acquire()

    async def earlier_action():
        queued.set()
        async with fresh.lock:
            entered.set()
            await release.wait()

    prior_action = asyncio.create_task(earlier_action())
    await asyncio.wait_for(queued.wait(), 30)

    async def shutdown_at_release():
        update_receipt(directory, status="queued")
        fresh._launch(directory, "install")
        # release() wakes the already queued action but does not resume it.
        # locked() is false here while a fair acquire() must still yield.
        fresh.lock.release()
        assert not fresh.lock.locked()
        await fresh.shutdown()

    shutting_down = asyncio.create_task(shutdown_at_release())
    try:
        await asyncio.wait_for(entered.wait(), 30)
        if cancel_shutdown:
            for _ in range(3):
                shutting_down.cancel()
                await asyncio.sleep(0)
            assert not shutting_down.done(), "Cancellation must not abandon the pending admission handoff."
        else:
            assert not spawned, "Shutdown must cancel the queued worker before its first spawn instruction."
        release.set()
        if cancel_shutdown:
            with pytest.raises(asyncio.CancelledError):
                await asyncio.wait_for(shutting_down, 1)
        else:
            await asyncio.wait_for(shutting_down, 1)
        stored = read_receipt(directory)
        assert not spawned and not fresh.tasks and not fresh.processes
        assert not fresh.lock.locked() and prior_action.done()
        assert stored["status"] == "interrupted" and stored["runtime_pid"] is None
        assert stored["deployment_id"] == receipt["deployment_id"]
    finally:
        release.set()
        await asyncio.gather(shutting_down, prior_action, return_exceptions=True)
        await fresh.shutdown()


async def test_stop_before_supervisor_admission_is_terminal_and_does_not_start(service, tmp_path):
    target = tmp_path / "team"
    receipt = await submit(service, target)
    stopped = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    assert stopped["status"] == "stopped"
    assert stopped["interactive_cleanup"]["boundary"] == "worker-not-admitted"
    assert not (target / "app.py").exists()
    assert not service.tasks
    with pytest.raises(ValueError, match="Installed source"):
        await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    with pytest.raises(ValueError, match="Installed source"):
        await service.settings_options(service.guide.identity, receipt["deployment_id"])
    retried = await service.action(service.guide.identity, receipt["deployment_id"], "retry")
    assert retried["status"] == "queued"
    assert retried["interactive_cleanup"] is None
    assert retried["interactive_lifecycle_id"] != stopped["interactive_lifecycle_id"]
    await service.shutdown()


@pytest.mark.parametrize("change", ["version", "brief", "proposal"])
async def test_guide_change_during_private_preparation_never_claims_a_folder_or_starts_a_process(service, tmp_path, monkeypatch, change):
    from launchkit import deployment
    original = deployment.prepare_private_settings
    def changed(*args, **kwargs):
        values = original(*args, **kwargs)
        if change == "version": service.guide.state["version"] += 1
        elif change == "brief": service.guide.state["brief"] = {"requirements": [{"id": "new-ui", "outcome": "Add a second application UI"}]}
        else: service.guide.state["proposal"]["agent_classes"][0]["instructions"] += " A different domain authority"
        return values
    monkeypatch.setattr(deployment, "prepare_private_settings", changed)
    with pytest.raises(DraftConflict, match="proposal changed before deployment admission"):
        await submit(service, tmp_path / "team")
    assert not service.tasks
    assert not (tmp_path / "team").exists()
    assert not service.root.exists()


async def test_application_fields_require_private_input_and_keep_defaults_source_bound(service, tmp_path):
    service.implementation.application_configuration = {"fields": [
        {"key": "READING_SERVICE_TOKEN", "label": "Reading service token", "description": "Create a token in your reading service account.",
         "secret": True, "required": True},
        {"key": "READING_COLLECTION", "label": "Collection", "secret": False, "required": True, "default": "Reading inbox"},
    ]}
    options = await service.options(service.guide.identity, service.implementation.identity)
    token = next(field for field in options["fields"] if field["key"] == "READING_SERVICE_TOKEN")
    assert token["description"] == "Create a token in your reading service account."
    assert token["secret"] and token["required"] and "default" not in token
    with pytest.raises(ValueError, match="Reading service token"):
        await submit(service, tmp_path / "team")
    assert not service.tasks
    receipt = await submit(service, tmp_path / "team", settings={
        "OPENAI_API_KEY": "private-api-key-value", "READING_SERVICE_TOKEN": "private-custom-token"})
    try:
        values = read_settings(tmp_path / "team")
        assert values["READING_SERVICE_TOKEN"] == "private-custom-token"
        assert values["READING_COLLECTION"] == "Reading inbox"
        assert (tmp_path / "team/.env").stat().st_mode & 0o777 == 0o600
        assert "private-custom-token" not in json.dumps(receipt)
        assert all("private-custom-token" not in file.read_text() for file in service.root.rglob("*.json"))
        assert "private-custom-token" not in (service._directory(service.guide.identity) / "package.zip").read_bytes().decode("latin1")
    finally:
        await service.shutdown()


@pytest.mark.parametrize("changed,error_key", [
    ({"TEAM_PASSWORD": "tiny-marker"}, "TEAM_PASSWORD"),
    ({"TEAM_NAME": " \t "}, "TEAM_NAME"),
    ({"TEAM_PORT": "65536"}, "TEAM_PORT"),
])
async def test_application_value_constraints_reject_before_local_deployment_admission(service, tmp_path, changed, error_key):
    service.implementation.application_configuration = {"fields": [
        {"key": "TEAM_PASSWORD", "label": "Staff password", "secret": True, "required": True, "min_length": 12},
        {"key": "TEAM_NAME", "label": "Staff name", "secret": False, "required": True, "max_length": 80},
        {"key": "TEAM_PORT", "label": "Staff listener", "secret": False, "required": False,
         "type": "integer", "minimum": 1024, "maximum": 65535, "default": "4111"},
    ]}
    destination = tmp_path / "new-team"
    values = {"OPENAI_API_KEY": "private-provider-marker", "TEAM_PASSWORD": "valid-private-password",
              "TEAM_NAME": "Coordinator", **changed}
    with pytest.raises(ValueError, match=error_key) as failure:
        await submit(service, destination, settings=values)
    assert "tiny-marker" not in str(failure.value) and "private-provider-marker" not in str(failure.value)
    assert not destination.exists() and not service.root.exists() and not service.tasks


def test_merged_private_configuration_constraints_preserve_existing_bytes_on_rejection(tmp_path):
    rows = configuration_fields({"environment": [], "knowledge": {"mode": "none"}}, {"fields": [
        {"key": "TEAM_PASSWORD", "label": "Staff password", "secret": True, "required": True,
         "min_length": 12, "max_length": 80},
    ]})
    path = tmp_path / ".env"
    before = b'TEAM_PASSWORD="tiny-marker"\nMOBKIT_PORT="18088"\n'
    path.write_bytes(before)
    with pytest.raises(ValueError, match="TEAM_PASSWORD"):
        private_settings(tmp_path, rows, {}, False, port=18088)
    assert path.read_bytes() == before and not list(tmp_path.glob(".env.*"))
    exact = " \tvalid private password\t "
    private_settings(tmp_path, rows, {"TEAM_PASSWORD": exact}, False, port=18088)
    assert read_settings(tmp_path)["TEAM_PASSWORD"] == exact


@pytest.mark.parametrize("fields", [
    [{"key": "OPENAI_API_KEY", "label": "Duplicate provider", "secret": True, "required": True}],
    [{"key": "CUSTOM_VALUE", "label": "First", "secret": False, "required": False},
     {"key": "CUSTOM_VALUE", "label": "Duplicate", "secret": False, "required": False}],
    [{"key": "PYTHONPATH", "label": "Loader", "secret": False, "required": False}],
    [{"key": "MOBKIT_PORT", "label": "Host port", "secret": False, "required": False}],
    [{"key": "CUSTOM_SECRET", "label": "Private default", "secret": True, "required": True, "default": "not-allowed"}],
])
async def test_application_fields_refuse_duplicates_reserved_keys_and_secret_defaults(service, tmp_path, fields):
    service.implementation.application_configuration = {"fields": fields}
    with pytest.raises(ValueError):
        await service.options(service.guide.identity)
    with pytest.raises(ValueError):
        await submit(service, tmp_path / "team")
    assert not (tmp_path / "team").exists()
    assert not service.tasks


def test_application_configuration_cannot_collide_with_mcp_or_unselected_provider_fields():
    definition = {"environment": [{"key": "MCP_ACCOUNT_TOKEN", "label": "MCP token", "secret": True, "required": True}],
                  "knowledge": {"mode": "none"}}
    for key in ["MCP_ACCOUNT_TOKEN", "ANTHROPIC_API_KEY"]:
        with pytest.raises(ValueError, match="distinct|reserved"):
            configuration_fields(definition, [{"key": key, "label": "Collision", "secret": True, "required": True}])


@pytest.mark.parametrize("key", ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"])
def test_shared_application_configuration_reserves_all_provider_keys(key):
    from launchkit.templates_v2.application_configuration import validate_application_fields
    with pytest.raises(ValueError, match="reserved"):
        validate_application_fields({"fields": [{"key": key, "label": "Provider override", "secret": True, "required": True}]})


async def test_application_metadata_is_fenced_by_checked_source_at_admission(service, tmp_path, monkeypatch):
    from launchkit import deployment
    service.implementation.application_configuration = {"fields": [
        {"key": "COLLECTION", "label": "Collection", "secret": False, "required": True, "default": "Frozen collection"}]}
    original = deployment.prepare_private_settings
    prepared = []
    def changed(*args, **kwargs):
        values = original(*args, **kwargs)
        prepared.append(values)
        service.implementation.digest = "new-source-digest"
        service.implementation.application_configuration["fields"][0]["default"] = "Changed collection"
        return values
    monkeypatch.setattr(deployment, "prepare_private_settings", changed)
    with pytest.raises(DraftConflict, match="proposal changed before deployment admission"):
        await submit(service, tmp_path / "team")
    assert prepared[0]["COLLECTION"] == "Frozen collection"
    assert not (tmp_path / "team").exists()
    assert not service.root.exists()
    assert not service.tasks


async def test_supervisor_admission_cancellation_recovers_and_stops_its_child(service, tmp_path, monkeypatch):
    receipt = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.shutdown()
    admitted, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
    class Input:
        closed = False
        def close(self):
            self.closed = True
            finished.set()
    class Process:
        returncode = None
        stdin = Input()
        async def wait(self):
            await finished.wait()
            self.returncode = 0
            return 0
    process = Process()
    async def spawn(*args, **kwargs):
        admitted.set()
        await release.wait()
        return process
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    fresh = DeploymentService(service.root.parent, service.guide, service.implementation)
    task = asyncio.create_task(fresh._run(directory, "install"))
    await admitted.wait()
    task.cancel()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert process.stdin.closed and process.returncode == 0
    assert not fresh.processes
    assert read_receipt(directory)["status"] == "interrupted"
    assert read_receipt(directory)["deployment_id"] == receipt["deployment_id"]


async def test_stop_then_shutdown_joins_the_original_pending_worker_admission(service, tmp_path, monkeypatch):
    await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.shutdown()
    admitted, release, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
    spawn_calls, spawn_cancelled = [], []

    class Input:
        closed = False
        def close(self):
            self.closed = True
            finished.set()

    class Process:
        returncode = None
        stdin = Input()
        async def wait(self):
            await finished.wait()
            self.returncode = 0
            return 0

    process = Process()

    async def spawn(*args, **kwargs):
        spawn_calls.append(args)
        admitted.set()
        try:
            await release.wait()
        except asyncio.CancelledError:
            spawn_cancelled.append(True)
            raise
        return process

    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    fresh = DeploymentService(service.root.parent, service.guide, service.implementation)
    task = asyncio.create_task(fresh._run(directory, "install"))
    fresh.tasks[directory.name] = task
    await asyncio.wait_for(admitted.wait(), 30)
    task.cancel()  # Stop arrives before the admitted handle is delivered.
    await asyncio.sleep(0)
    shutting_down = asyncio.create_task(fresh.shutdown())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    assert not shutting_down.done() and not task.done()
    assert not spawn_cancelled and len(spawn_calls) == 1
    release.set()
    await asyncio.wait_for(shutting_down, 1)
    assert task.cancelled() and process.stdin.closed and process.returncode == 0
    assert not fresh.processes and not fresh.tasks
    assert read_receipt(directory)["status"] == "interrupted"


async def test_shutdown_during_worker_teardown_joins_same_wait_without_losing_handle(service, tmp_path, monkeypatch):
    await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.shutdown()
    entered, stopping, finished = asyncio.Event(), asyncio.Event(), asyncio.Event()
    wait_calls, wait_cancelled = [], []

    class Input:
        closes = 0
        def close(self):
            self.closes += 1

    class Process:
        returncode = None
        stdin = Input()
        async def wait(self):
            wait_calls.append(True)
            (entered if len(wait_calls) == 1 else stopping).set()
            try:
                await finished.wait()
            except asyncio.CancelledError:
                wait_cancelled.append(True)
                raise
            self.returncode = 0
            return 0

    process = Process()

    async def spawn(*args, **kwargs):
        return process

    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    fresh = DeploymentService(service.root.parent, service.guide, service.implementation)
    task = asyncio.create_task(fresh._run(directory, "install"))
    fresh.tasks[directory.name] = task
    await asyncio.wait_for(entered.wait(), 30)
    task.cancel()
    await asyncio.wait_for(stopping.wait(), 30)
    shutting_down = asyncio.create_task(fresh.shutdown())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    assert not shutting_down.done() and not task.done()
    assert fresh.processes[directory.name] is process and process.returncode is None
    assert process.stdin.closes == 1 and len(wait_calls) == 2 and len(wait_cancelled) == 1
    finished.set()
    await asyncio.wait_for(shutting_down, 1)
    assert task.cancelled() and process.returncode == 0
    assert not fresh.processes and not fresh.tasks
    assert len(wait_calls) == 2 and len(wait_cancelled) == 1


async def test_worker_force_exit_race_still_reaps_original_handle(service, tmp_path, monkeypatch):
    from launchkit import deployment
    await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.shutdown()
    entered = asyncio.Event()
    waits, signals = [], []

    class Input:
        def close(self):
            pass

    class Process:
        pid = 1234567
        returncode = None
        stdin = Input()
        async def wait(self):
            waits.append(True)
            entered.set()
            if len(waits) < 3:
                await asyncio.Event().wait()
            self.returncode = 0
            return 0

    process = Process()

    async def spawn(*args, **kwargs):
        return process

    def already_exited(pid, sig):
        signals.append((pid, sig))
        raise ProcessLookupError

    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    monkeypatch.setattr(deployment, "WORKER_STOP_TIMEOUT", 0.01)
    monkeypatch.setattr(deployment.os, "killpg", already_exited)
    fresh = DeploymentService(service.root.parent, service.guide, service.implementation)
    task = asyncio.create_task(fresh._run(directory, "install"))
    await asyncio.wait_for(entered.wait(), 30)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await asyncio.wait_for(task, 1)
    assert signals == [(process.pid, 9)] and len(waits) == 3
    assert process.returncode == 0 and not fresh.processes


@pytest.mark.parametrize("cancel_shutdown", [False, True])
async def test_shutdown_force_reap_failure_preserves_live_worker_and_reports_cleanup_error(service, tmp_path, monkeypatch, cancel_shutdown):
    from launchkit import deployment
    await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    await service.shutdown()
    entered, stopping = asyncio.Event(), asyncio.Event()
    signals = []

    class Input:
        def close(self):
            stopping.set()

    class Process:
        pid = 1234567
        returncode = None
        stdin = Input()
        async def wait(self):
            entered.set()
            await asyncio.Event().wait()

    process = Process()

    async def spawn(*args, **kwargs):
        return process

    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    monkeypatch.setattr(deployment, "WORKER_STOP_TIMEOUT", 0.01)
    monkeypatch.setattr(deployment, "PROCESS_FORCE_REAP_TIMEOUT", 0.01)
    monkeypatch.setattr(deployment.os, "killpg", lambda pid, sig: signals.append((pid, sig)))
    fresh = DeploymentService(service.root.parent, service.guide, service.implementation)
    task = asyncio.create_task(fresh._run(directory, "install"))
    fresh.tasks[directory.name] = task
    await asyncio.wait_for(entered.wait(), 30)
    shutting_down = asyncio.create_task(fresh.shutdown())
    if cancel_shutdown:
        await asyncio.wait_for(stopping.wait(), 30)
        for _ in range(3):
            shutting_down.cancel()
            await asyncio.sleep(0)
        with pytest.raises(BaseExceptionGroup, match="owned cleanup failed") as failure:
            await asyncio.wait_for(shutting_down, 1)
        assert isinstance(failure.value.exceptions[0], asyncio.CancelledError)
        assert isinstance(failure.value.exceptions[1], RuntimeError)
        assert "did not confirm exit during Forge shutdown" in str(failure.value.exceptions[1])
    else:
        with pytest.raises(RuntimeError, match="did not confirm exit during Forge shutdown"):
            await asyncio.wait_for(shutting_down, 1)
    assert isinstance(task.exception(), RuntimeError)
    assert fresh.processes[directory.name] is process and not fresh.tasks
    assert process.returncode is None and signals == [(process.pid, 9)]
    receipt = read_receipt(directory)
    assert receipt["status"] == "interrupted" and receipt["process_live"] is True
    assert receipt["console_url"] is None and receipt["runtime_pid"] is None
    assert receipt["interactive_cleanup"] is None and receipt["interactive_cleanup_attempt_id"] is None
    with pytest.raises(RuntimeError, match="did not confirm exit during Forge shutdown"):
        await fresh.shutdown()
    assert fresh.processes[directory.name] is process and signals == [(process.pid, 9)]


async def test_startup_spawn_failure_closes_readiness_pipe(service, tmp_path, monkeypatch):
    await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    supervisor = Supervisor(directory)
    original = os.pipe
    descriptors = []
    def pipe():
        pair = original()
        descriptors.extend(pair)
        return pair
    async def spawn(*args, **kwargs):
        raise OSError("Fixture spawn failure")
    monkeypatch.setattr(os, "pipe", pipe)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    with pytest.raises(OSError, match="Fixture"):
        await supervisor.start(service.implementation.definition)
    await asyncio.sleep(0)
    for descriptor in descriptors:
        with pytest.raises(OSError):
            os.fstat(descriptor)


async def test_startup_channel_eof_keeps_failure_clear_and_stops_owned_child(service, tmp_path):
    import sys
    destination = tmp_path / "team"
    await submit(service, destination)
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    previous_attempt = str(uuid.uuid4())
    update_receipt(directory, startup_attempt_id=previous_attempt,
        startup_console_health={"attempt_id": previous_attempt, "status": "fail", "category": "authentication", "attempts": 1})
    interpreter = destination / ".venv/bin/python"
    interpreter.parent.mkdir(parents=True)
    interpreter.symlink_to(sys.executable)
    # The child closes only its readiness channel and keeps running, reproducing
    # the race where EOF arrives before Process.returncode is available.
    (destination / "app.py").write_text("import os,time\nos.close(int(os.environ['MOBKIT_LAUNCH_READY_FD']))\nprint('Native startup fixture failed',flush=True)\ntime.sleep(60)\n")
    supervisor = Supervisor(directory)
    with pytest.raises(RuntimeError, match="closed its startup verification channel"):
        await asyncio.wait_for(supervisor.start(service.implementation.definition), 5)
    assert supervisor.child is None
    assert any("Native startup fixture failed" in row["text"] for row in supervisor.logs)
    stored = read_receipt(directory)
    assert str(uuid.UUID(stored["startup_attempt_id"])) == stored["startup_attempt_id"]
    assert stored["startup_attempt_id"] != previous_attempt
    assert stored["startup_console_health"] is None


async def test_owned_process_group_is_cleaned_when_installer_parent_exits(service, tmp_path):
    import sys
    receipt = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    supervisor = Supervisor(directory)
    heartbeat = tmp_path / "owned-child-heartbeat"
    child = f"import time,pathlib; p=pathlib.Path({str(heartbeat)!r});\nwhile True: p.write_text(str(time.monotonic())); time.sleep(.02)"
    parent = f"import subprocess,sys,time; subprocess.Popen([sys.executable,'-c',{child!r}]); time.sleep(.15)"
    await supervisor.command([sys.executable, "-c", parent], "Fixture installer", 5)
    before = heartbeat.read_text()
    await asyncio.sleep(.15)
    assert heartbeat.read_text() == before


def test_private_settings_reader_bounds_and_parses_generated_single_quoted_values(tmp_path):
    (tmp_path / ".env").write_text("MOBKIT_PORT='18088'\nTOKEN='known-secret'\n")
    assert read_settings(tmp_path) == {"MOBKIT_PORT": "18088", "TOKEN": "known-secret"}
    (tmp_path / ".env").write_text('TOKEN={"bad":"type"}\n')
    assert read_settings(tmp_path)["TOKEN"] == '{"bad":"type"}'
    (tmp_path / ".env").write_text('TOKEN="' + "x" * 33000 + '"\n')
    with pytest.raises(ValueError, match="bounded"):
        read_settings(tmp_path)


async def test_installed_mcp_checks_run_only_for_selected_grants_and_failures_remain_evidence(service, tmp_path):
    await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    supervisor = Supervisor(directory)
    calls = []
    async def command(args, phase, timeout, **kwargs):
        calls.append(args)
        if args[-1] == "--connections-check":
            raise RuntimeError("Actual handshake failed")
    supervisor.command = command
    definition = deepcopy(service.implementation.definition)
    definition["classes"][0]["tools"]["mcp"] = [{"connection_id": "selected"}]
    with pytest.raises(RuntimeError, match="handshake failed"):
        await supervisor.install(definition)
    assert calls[-1][-1] == "--connections-check"
    checks = read_receipt(directory)["checks"]
    assert [(item["name"], item["status"]) for item in checks] == [("Local prerequisites", "pass"), ("Application contract", "pass"), ("MCP connections", "fail")]


async def test_installer_uses_owned_python_when_path_selects_an_older_interpreter(service, tmp_path, monkeypatch):
    from launchkit.compiler import generate_files
    files = generate_files(service.guide.state["proposal"])
    service.implementation.install_script = files["install.sh"]
    service.implementation.extra_files["storage_setup.py"] = files["storage_setup.py"]
    target = tmp_path / "team"
    await submit(service, target)
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    # Exercise the existing sealed installer's selection contract without pip,
    # gateway downloads or native startup. Its venv commands are fixture stubs.
    bin_directory = tmp_path / "old-system-bin"
    bin_directory.mkdir()
    old_python = bin_directory / "python3"
    old_python.write_text("#!/bin/sh\necho 'OLD_SYSTEM_PYTHON_SELECTED'\nexit 42\n")
    old_python.chmod(0o755)
    venv_python = target / ".venv/bin/python"
    venv_python.parent.mkdir(parents=True)
    venv_python.write_text("#!/bin/sh\necho 'FIXTURE_DEPENDENCY_OR_CHECK_COMMAND'\nexit 0\n")
    venv_python.chmod(0o755)
    monkeypatch.setenv("PYTHON_BIN", str(old_python))
    monkeypatch.setenv("OPENAI_API_KEY", "should-not-enter-command-environment")
    supervisor = Supervisor(directory)
    supervisor.environment["PATH"] = str(bin_directory) + os.pathsep + "/usr/bin:/bin"
    assert "PYTHON_BIN" not in supervisor.environment and "OPENAI_API_KEY" not in supervisor.environment
    await supervisor.install(service.implementation.definition)
    stored = read_receipt(directory)
    logs = "\n".join(item["text"] for item in stored["logs"])
    assert "FIXTURE_DEPENDENCY_OR_CHECK_COMMAND" in logs
    assert "OLD_SYSTEM_PYTHON_SELECTED" not in logs
    assert "PYTHON_BIN" not in supervisor.environment
    assert "should-not-enter-command-environment" not in json.dumps(stored)
    assert (target / "install.sh").read_text() == service.implementation.install_script


def test_local_knowledge_ownership_requires_this_exact_compose_folder(tmp_path):
    labels = {"com.docker.compose.project.working_dir": str(tmp_path / "knowledge"),
              "com.docker.compose.project.config_files": str(tmp_path / "knowledge/compose.json")}
    assert knowledge_container_owned(labels, tmp_path)
    assert not knowledge_container_owned(labels, tmp_path / "another")
    assert not knowledge_container_owned({**labels, "com.docker.compose.project.config_files": "/other/compose.json"}, tmp_path)


def test_package_rejects_links_duplicates_traversal_and_private_state():
    for name in ["../outside", "/absolute", ".mobkit/state", ".env", "nested/.env"]:
        result = io.BytesIO()
        with zipfile.ZipFile(result, "w") as archive:
            archive.writestr(name, "bad")
        with pytest.raises(ValueError, match="unsafe"):
            package_files(result.getvalue())


def test_configuration_names_values_and_symlinks_are_data_only(tmp_path, monkeypatch):
    fields = [{"key": "API_TOKEN", "label": "Token", "required": True, "secret": True}]
    with pytest.raises(ValueError, match="not requested"):
        private_settings(tmp_path, fields, {"PYTHONPATH": "host-code", "API_TOKEN": "secret"}, False, port=18088)
    secret = '$(touch SHOULD_NOT_EXIST); `echo nope` "exact"'
    private_settings(tmp_path, fields, {"API_TOKEN": secret}, False, port=18088)
    assert json.loads((tmp_path / ".env").read_text().split("API_TOKEN=", 1)[1].splitlines()[0]) == secret
    assert not (tmp_path / "SHOULD_NOT_EXIST").exists()
    monkeypatch.setenv("OPENAI_API_KEY", "host-secret")
    monkeypatch.setenv("PYTHONPATH", "danger")
    monkeypatch.setenv("HTTPS_PROXY", "private-proxy")
    assert not {"OPENAI_API_KEY", "PYTHONPATH", "HTTPS_PROXY"}.intersection(clean_environment())


def test_readiness_requires_native_bootstrap_owned_pid_identities_and_private_console():
    definition = {"agents": [{"identity": "lead:main"}, {"identity": "weather:main"}]}
    bootstrap = {"complete": True, "ready": True, "counts": {"broken": 0},
        "identities": {"lead:main": {}, "weather:main": {}}}
    payload = {"pid": 42, "bootstrap": bootstrap, "console_url": "http://127.0.0.1:18088/console"}
    assert readiness(payload, definition, 42, 18088) == (bootstrap, payload["console_url"])
    for changes in [{"pid": 43}, {"console_url": "http://example.org:18088/console"},
        {"bootstrap": {**bootstrap, "complete": False}}, {"bootstrap": {**bootstrap, "counts": {"broken": 1}}},
        {"bootstrap": {**bootstrap, "identities": {"lead:main": {}}}}]:
        with pytest.raises(ValueError):
            readiness({**payload, **changes}, definition, 42, 18088)


def test_redaction_preserves_useful_output_without_secret_values():
    redact = Redactor(["private-key", 'quoted"value'])
    text = redact('install passed private-key quoted"value quoted\\"value token=other-secret Bearer fresh-token')
    assert "install passed" in text
    assert all(value not in text for value in ["private-key", 'quoted"value', "other-secret", "fresh-token"])


@pytest.mark.parametrize("target", ["mac", "linux", "gcp"])
async def test_remote_handoff_identifies_script_folder_for_flat_application_zip(service, monkeypatch, tmp_path, target):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "linux" if target == "mac" else "darwin")
    proposal = service.guide.state["proposal"]
    proposal["target"] = target
    proposal["gcp"]["project"] = "accepted-project"
    service.implementation.definition["target"] = target
    service.implementation.extra_files = {path: "# checked package script\n" for path in [
        "setup.py", "start.sh", "remote_operations.py", "deploy/deploy-gcp.sh", "deploy/install-service.sh", "deploy/tunnel-gcp.sh"]}
    payload, slug = await service.implementation.package(service.guide.identity, service.implementation.identity)
    extraction = tmp_path / "chosen extraction folder"
    extraction.mkdir()
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        archive.extractall(extraction)
    files = {name: value[0] for name, value in package_files(payload).items()}
    _, _, handoff = deployment_handoff(service.implementation.definition, files, proposal,
                                     service.guide.identity, service.implementation.identity)
    step = handoff["steps"][0 if target == "gcp" else 1]
    assert "project folder containing install.sh and start.sh" in step["description"]
    assert "may be an inner folder created by the ZIP" in step["description"]
    assert "Open a terminal there" in step["description"]
    assert not (extraction / slug).exists()
    for name in ("install.sh", "setup.py", "start.sh"):
        assert (extraction / name).is_file()
    assert handoff["mode"] == "remote-package" and handoff["status"] == "not-connected"
    assert not service.root.exists() and not service.tasks and not service.processes


@pytest.mark.parametrize("target", ["linux", "gcp"])
async def test_remote_handoff_uses_current_checked_package_without_local_admission(service, monkeypatch, tmp_path, target):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    service.guide.state["proposal"]["target"] = target
    service.guide.state["proposal"]["gcp"]["project"] = "accepted-project"
    service.implementation.definition["target"] = target
    service.implementation.extra_files = {path: "# checked package script\n" for path in [
        "setup.py", "start.sh", "remote_operations.py", "deploy/deploy-gcp.sh", "deploy/install-service.sh", "deploy/tunnel-gcp.sh"]}

    async def no_local_service_probe():
        raise AssertionError("A remote handoff must not prepare the local service manager")

    def no_implicit_local_port_probe(*_, **__):
        raise AssertionError("A remote handoff must not implicitly check a local Console port")

    monkeypatch.setattr(service.service_manager, "capability", no_local_service_probe)
    monkeypatch.setattr("launchkit.deployment.observe_local_console_port", no_implicit_local_port_probe)
    options = await service.options(service.guide.identity, service.implementation.identity)
    assert options["target"] == target and options["host_target"] == "mac"
    assert not options["supported"] and not options["can_use_host_provider_credentials"]
    assert options["fresh_install"] == {"available": False, "reason": None}
    assert options["host_provider_keys"] == []
    assert "console_port_observation" not in options
    handoff = options["handoff"]
    assert handoff["mode"] == "remote-package" and handoff["status"] == "not-connected"
    assert handoff["package_url"].endswith("build_id=" + service.implementation.identity)
    commands = [command for step in handoff["steps"] for command in step["commands"]]
    assert handoff["steps"][0]["where"] == "this computer"
    assert "bash start.sh --doctor" in commands
    if target == "gcp":
        assert "bash deploy/deploy-gcp.sh --dry-run" in commands
        assert "gcloud compute ssh test-team --project accepted-project --zone europe-west1-b --tunnel-through-iap" in commands
        assert ".venv/bin/python setup.py" in commands
        assert "bash deploy/tunnel-gcp.sh" in commands
    else:
        assert handoff["steps"][1]["where"] == "destination computer"
        assert "Transfer the downloaded ZIP" in handoff["steps"][1]["description"]
        assert "new empty folder you own" in handoff["steps"][1]["description"]
        assert "bash install.sh" in commands and "bash deploy/install-service.sh start" in commands
        assert "bash start.sh" not in commands
        assert any(command.startswith("ssh -N -L ") for command in commands)
        assert not any("gcloud" in command for command in commands)
    assert not service.root.exists()
    with pytest.raises(ValueError, match="another host"):
        await submit(service, tmp_path / "must-not-be-created")
    assert not (tmp_path / "must-not-be-created").exists()


@pytest.mark.parametrize("target", ["linux", "gcp"])
@pytest.mark.parametrize("port_status", ["available", "occupied", "unknown"])
async def test_explicit_remote_console_port_check_observes_this_computer_without_local_admission(
        service, monkeypatch, target, port_status):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    monkeypatch.setenv("OPENAI_API_KEY", "host-private-provider-key")
    service.guide.state["proposal"]["target"] = target
    service.implementation.definition["target"] = target
    observation = {"scope": "local-console-port-observation", "port": 24173,
        "status": port_status, "observed_at": "2026-10-06T16:00:00+00:00"}
    checks = []

    def observe(port, *, suggest_alternate):
        checks.append((port, suggest_alternate))
        return observation

    async def no_local_service_probe():
        raise AssertionError("A requested port observation must not admit a local service")

    monkeypatch.setattr("launchkit.deployment.observe_local_console_port", observe)
    monkeypatch.setattr(service.service_manager, "capability", no_local_service_probe)
    options = await service.options(service.guide.identity, service.implementation.identity,
                                    console_port=24173)
    assert checks == [(24173, True)]
    assert options["console_port_observation"] == observation
    assert options["console_port"] == 18088
    assert options["build_id"] == service.implementation.identity
    assert options["source_digest"] == service.implementation.digest
    assert options["current"] is True and options["target"] == target and options["host_target"] == "mac"
    assert options["supported"] is False and options["can_use_host_provider_credentials"] is False
    assert options["host_provider_keys"] == []
    assert all(mode["available"] is False for mode in options["execution_modes"])
    assert not service.root.exists() and not service.tasks and not service.processes


@pytest.mark.parametrize("target", ["linux", "gcp"])
@pytest.mark.parametrize("drift", ["requested-build", "unaccepted", "latest-source", "latest-build", "proposal"])
async def test_explicit_remote_console_port_check_retains_accepted_source_fences(service, monkeypatch, target, drift):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    service.guide.state["proposal"]["target"] = target
    service.implementation.definition["target"] = target
    selected = service.implementation.identity
    calls = []

    def no_port_probe(*_, **__):
        calls.append("port")
        raise AssertionError("A refused source must not produce a local port observation")

    monkeypatch.setattr("launchkit.deployment.observe_local_console_port", no_port_probe)
    status = service.implementation.status
    status_calls = 0

    async def changed_status(identity):
        nonlocal status_calls
        value = await status(identity)
        status_calls += 1
        if status_calls == 2:
            if drift == "latest-source":
                value["source_digest"] = "b" * 64
            elif drift == "latest-build":
                value["build_id"] = str(uuid.uuid4())
        return value

    monkeypatch.setattr(service.implementation, "status", changed_status)
    if drift == "requested-build":
        selected = str(uuid.uuid4())
    elif drift == "unaccepted":
        service.implementation.accepted = False
    elif drift == "proposal":
        package = service.implementation.package

        async def changed_proposal(identity, build_id):
            result = await package(identity, build_id)
            service.guide.state["brief"] = {"outcome": "A changed application requirement"}
            return result

        monkeypatch.setattr(service.implementation, "package", changed_proposal)
    error = ValueError if drift == "unaccepted" else DraftConflict
    message = "independent acceptance" if drift == "unaccepted" else (
        "newer application build" if drift == "requested-build" else "changed while preparing private setup")
    with pytest.raises(error, match=message):
        await service.options(service.guide.identity, selected, console_port=24173)
    assert calls == []
    assert not service.root.exists() and not service.tasks and not service.processes


async def test_handoff_never_invents_missing_scripts_or_uses_another_package_destination(service, monkeypatch):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    # GCP differs from either real localhost OS used by the service fixture.
    service.guide.state["proposal"]["target"] = "gcp"
    with pytest.raises(DraftConflict, match="destination differs"):
        await service.options(service.guide.identity)
    service.implementation.definition["target"] = "gcp"
    options = await service.options(service.guide.identity)
    assert options["handoff"]["steps"] == []
    assert any("lacks the required handoff scripts" in item for item in options["handoff"]["limitations"])


@pytest.mark.parametrize("service_files", [[], ["deploy/install-service.sh"], ["remote_operations.py"],
    ["deploy/install-service.sh", "remote_operations.py"]])
def test_linux_handoff_offers_only_source_present_service_operations_and_terminal_fallback(monkeypatch, service_files):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    definition = {"target": "linux", "port": 18088, "knowledge": {"mode": "none"}}
    files = {path: b"checked source" for path in ["setup.py", "start.sh", "install.sh", *service_files]}
    original = deepcopy(files)
    _, _, handoff = deployment_handoff(definition, files, {"target": "linux"}, "owned-guide", "checked-build")
    commands = [command for step in handoff["steps"] for command in step["commands"]]
    operations = [command for operation in handoff["operations"] for command in operation["commands"]]
    start = next(step for step in handoff["steps"] if "Verify, then" in step["title"])
    if len(service_files) == 2:
        assert commands[-2] == "bash deploy/install-service.sh start"
        assert "bash start.sh" not in commands
        for action in ["status", "stop", "restart", "remove-service", "setup"]:
            assert f"bash deploy/install-service.sh {action}" in operations
        assert "bash start.sh" in operations
        assert "logout or reboot" in start["description"]
        assert any("sudo" in limitation and "No unattended boot" in limitation for limitation in handoff["limitations"])
    else:
        assert "bash start.sh" in commands
        assert not any("install-service.sh" in command for command in commands + operations)
        assert operations == []
        assert "terminal open" in start["description"] and "Control-C" in start["description"]
    assert handoff["mode"] == "remote-package" and handoff["status"] == "not-connected"
    assert files == original


def test_remote_handoff_places_optional_tunnel_in_visible_local_step_not_destination_commands(monkeypatch):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    definition = {"target": "linux", "port": 18088, "knowledge": {"mode": "none"}}
    files = dict.fromkeys(["setup.py", "start.sh", "install.sh"], b"checked source")
    _, _, handoff = deployment_handoff(definition, files, {"target": "linux"}, "owned-guide", "checked-build")
    assert handoff["steps"][0]["commands"] == handoff["steps"][1]["commands"] == []
    assert "does not upload" in handoff["steps"][0]["description"]
    tunnel = next(step for step in handoff["steps"] if step["commands"] == ["ssh -N -L 18088:127.0.0.1:18088 USER@HOST"])
    assert "not the destination SSH session" in tunnel["where"]
    assert "Skip this step" in tunnel["description"] and "replace USER@HOST" in tunnel["description"]
    assert "tunnel terminal open" in tunnel["description"] and "Control-C" in tunnel["description"]
    console = handoff["steps"][-1]
    assert console["where"] == "your computer with the browser"
    assert "http://127.0.0.1:18088/console" in console["description"]
    assert "remote health is not tracked" in console["description"]
    assert console["commands"] == []


@pytest.mark.parametrize("missing", ["install.sh", "setup.py", "start.sh"])
def test_missing_required_install_script_never_produces_a_run_command(monkeypatch, missing):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    definition = {"target": "linux", "port": 18088}
    files = dict.fromkeys({"setup.py", "start.sh", "install.sh"} - {missing}, b"checked source")
    _, _, handoff = deployment_handoff(definition, files, {"target": "linux"}, "owned-guide", "checked-build")
    assert handoff["steps"] == [] and handoff["operations"] == []
    assert any(missing in item for item in handoff["limitations"])


def test_cloud_service_handoff_requires_its_actual_host_helper(monkeypatch):
    monkeypatch.setattr("launchkit.deployment.sys.platform", "darwin")
    definition = {"target": "gcp", "slug": "owned-team", "port": 18088}
    files = dict.fromkeys(["setup.py", "start.sh", "install.sh", "deploy/deploy-gcp.sh",
        "deploy/install-service.sh", "deploy/tunnel-gcp.sh"], b"checked source")
    proposal = {"target": "gcp", "gcp": {"project": "owned-project", "zone": "europe-west1-b"}}
    _, _, handoff = deployment_handoff(definition, files, proposal, "owned-guide", "checked-build")
    assert handoff["steps"] == [] and handoff["operations"] == []
    assert any("remote_operations.py" in item for item in handoff["limitations"])


async def test_console_scope_is_source_declared_and_does_not_claim_custom_interface_health(service):
    service.guide.state["brief"] = {"facts": {"interfaces": {"summary": "Custom reading UI plus native admin Console",
        "status": "confirmed", "source_message_ids": ["user-message"]}}}
    service.implementation.extra_files["application/console.json"] = json.dumps({"authentication": "local-password"})
    service.implementation.application_configuration = {"fields": [
        {"key": "CONSOLE_USER_ID", "label": "Console user", "secret": False, "required": True},
        {"key": "CONSOLE_LOGIN_PASSWORD", "label": "Console password", "secret": True, "required": True}]}
    options = await service.options(service.guide.identity)
    assert options["console_scope"] == {"authentication": "local-password", "health_scope": "native-bootstrap-and-console",
        "requested_interfaces": "Custom reading UI plus native admin Console", "custom_interfaces_verified": False}
    service.implementation.application_configuration["fields"].pop()
    with pytest.raises(ValueError, match="required private setup fields"):
        await service.options(service.guide.identity)


@pytest.mark.parametrize("changed,key", [
    ({"CONSOLE_USER_ID": "private principal-marker"}, "CONSOLE_USER_ID"),
    ({"CONSOLE_LOGIN_PASSWORD": "tiny-marker"}, "CONSOLE_LOGIN_PASSWORD"),
    ({"CONSOLE_LOGIN_PASSWORD": "x" * 4097}, "CONSOLE_LOGIN_PASSWORD"),
    ({"CONSOLE_LOGIN_PASSWORD": "private-surrogate-marker-\ud800"}, "CONSOLE_LOGIN_PASSWORD"),
])
async def test_declared_console_rejects_intrinsic_configuration_before_local_admission(service, tmp_path, changed, key):
    service.implementation.extra_files["application/console.json"] = '{"authentication":"local-password"}'
    service.implementation.application_configuration = {"fields": list(CONSOLE_FIELDS)}
    settings = {"OPENAI_API_KEY": "private-provider-marker", "CONSOLE_USER_ID": "owner@example.test",
                "CONSOLE_LOGIN_PASSWORD": "valid-password-123", **changed}
    destination = tmp_path / "intrinsic-console"
    with pytest.raises(ValueError, match=key) as failure:
        await submit(service, destination, settings=settings)
    assert "tiny-marker" not in str(failure.value) and "principal-marker" not in str(failure.value)
    assert "surrogate-marker" not in str(failure.value)
    assert not destination.exists() and not service.root.exists()
    assert not service.tasks and not service.processes


async def test_declared_console_retains_exact_password_and_restart_refuses_invalid_retained_settings(service, tmp_path):
    service.implementation.extra_files["application/console.json"] = '{"authentication":"local-password"}'
    service.implementation.application_configuration = {"fields": list(CONSOLE_FIELDS)}
    exact = " \tvalid private password\t "
    destination = tmp_path / "intrinsic-console"
    receipt = await submit(service, destination, settings={"OPENAI_API_KEY": "private-provider-marker",
        "CONSOLE_USER_ID": "owner@example.test", "CONSOLE_LOGIN_PASSWORD": exact})
    assert read_settings(destination)["CONSOLE_LOGIN_PASSWORD"] == exact
    materialize(service._directory(service.guide.identity))
    await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    path = destination / ".env"
    before = path.read_bytes().replace(json.dumps(exact).encode(), b'"tiny-marker"')
    path.write_bytes(before)
    directory = service._directory(service.guide.identity)
    before_receipt = (directory / "receipt.json").read_bytes()
    with pytest.raises(ValueError, match="CONSOLE_LOGIN_PASSWORD"):
        await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    assert path.read_bytes() == before and (directory / "receipt.json").read_bytes() == before_receipt
    assert not service.tasks and not service.processes


def test_console_field_names_without_metadata_have_no_intrinsic_host_limits(tmp_path):
    values = prepare_private_settings(tmp_path, list(CONSOLE_FIELDS), {
        "CONSOLE_USER_ID": "custom identity with spaces", "CONSOLE_LOGIN_PASSWORD": "short",
    }, False, port=18088)
    assert values["CONSOLE_USER_ID"] == "custom identity with spaces" and values["CONSOLE_LOGIN_PASSWORD"] == "short"
    assert not (tmp_path / ".env").exists()


async def test_harmless_turn_retains_owned_install_and_private_options_without_rebinding_package(service, tmp_path):
    first = await submit(service, tmp_path / "team")
    directory = service._directory(service.guide.identity)
    original_receipt = read_receipt(directory)
    original_package = (directory / "package.zip").read_bytes()
    service.guide.state["version"] += 1
    service.guide.state["messages"] = [{"role": "user", "text": "Where is my installed application?"}]
    current = await service.status(service.guide.identity)
    assert current["current"] and current["current_guide_version"] == 5
    assert current["guide_version"] == 4 and current["build_id"] == first["build_id"]
    assert current["package_sha256"] == original_receipt["package_sha256"]
    assert (directory / "package.zip").read_bytes() == original_package
    options = await service.options(service.guide.identity, service.implementation.identity)
    assert options["current"] and options["current_guide_version"] == 5 and options["guide_version"] == 4
    assert options["configured_deployment_id"] == first["deployment_id"]
    assert options["configured_destination"] == first["destination"]
    assert read_receipt(directory)["guide_version"] == 4
    # A detached status must still invalidate newly requested private/config UI.
    revised = {**service.guide.state, "brief": {"requirements": [{"id": "custom-ui", "outcome": "Add a custom UI"}]}}
    assert not (await service.status(service.guide.identity, state=revised))["current"]
    await service.shutdown()


async def test_retry_admission_checks_leave_the_event_loop_free(service, tmp_path, monkeypatch):
    from launchkit import deployment
    receipt = await submit(service, tmp_path / "team")
    stopped = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    assert stopped["interactive_cleanup"]["boundary"] == "worker-not-admitted"
    original = deployment.package_files
    def slowed(payload):
        time.sleep(1.0)  # A large package on a small host.
        return original(payload)
    monkeypatch.setattr(deployment, "package_files", slowed)
    stalls, done = [], asyncio.Event()
    async def other_request():
        while not done.is_set():
            started = time.monotonic()
            await asyncio.sleep(0.01)
            stalls.append(time.monotonic() - started)
    probe = asyncio.create_task(other_request())
    await asyncio.sleep(0)  # The other request is waiting on the loop.
    try:
        retried = await service.action(service.guide.identity, receipt["deployment_id"], "retry")
    finally:
        done.set()
        await probe
    assert retried["status"] == "queued" and stalls and max(stalls) < 0.5
    await service.shutdown()


async def test_installed_source_evidence_leaves_the_event_loop_free(tmp_path, monkeypatch):
    from launchkit import deployment
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "b" * 64
    service = Service(tmp_path / "private-server", guide, implementation)
    await submit(service, tmp_path / "application")
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None)
    state = {**guide.state, "id": guide.identity}
    original = deployment.verify_destination
    def slowed(*args, **kwargs):
        time.sleep(1.0)  # A large installed copy on a small host.
        return original(*args, **kwargs)
    monkeypatch.setattr(deployment, "verify_destination", slowed)
    stalls, done = [], asyncio.Event()
    async def other_request():
        while not done.is_set():
            started = time.monotonic()
            await asyncio.sleep(0.01)
            stalls.append(time.monotonic() - started)
    probe = asyncio.create_task(other_request())
    await asyncio.sleep(0)  # The other request is waiting on the loop.
    try:
        evidence = await service.guide_status(guide.identity, state=state)
    finally:
        done.set()
        await probe
    assert evidence["source_verified"] is True and evidence["source_digest"] == "b" * 64
    assert stalls and max(stalls) < 0.5
