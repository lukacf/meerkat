"""Installed private settings remain independent of newer candidate acceptance."""
from copy import deepcopy
import json
import os
from types import SimpleNamespace
import uuid
from unittest.mock import AsyncMock

import pytest

from launchkit import deployment
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, Service, submit


PRIVATE = "saved-private-settings-canary"


async def prepared(tmp_path, *, viewer_files=None, private_settings=None):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    implementation.application_configuration = {"fields": [
        {"key": "OPTIONAL_TOKEN", "label": "Optional token", "secret": True, "required": False},
        {"key": "DISPLAY_THEME", "label": "Theme", "secret": False, "required": False, "default": "source-theme"},
    ]}
    # An automatic generated field remains host-managed, including for removal.
    implementation.definition["environment"].append({
        "key": "AUTOMATIC_TOKEN", "label": "Automatic token", "secret": True,
        "required": False, "automatic": True})
    if viewer_files is not None:
        implementation.application_configuration = json.loads(viewer_files["application/configuration.json"])
        implementation.extra_files = {name: content for name, content in viewer_files.items()
            if name != "application/configuration.json"}
    service = Service(tmp_path / "server", guide, implementation)
    destination = tmp_path / "installed"
    initial = await submit(service, destination, settings={
        "OPENAI_API_KEY": PRIVATE, "OPTIONAL_TOKEN": PRIVATE, "DISPLAY_THEME": "chosen-theme",
        **(private_settings or {})})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None, process_live=False, runtime_pid=None,
                   health="ready", startup_attempt_id="old-attempt", startup_console_health={"category": "authentication"})
    (destination / "data").mkdir()
    (destination / "data/records.json").write_text('{"keep":"domain record"}')
    (destination / ".mobkit/state").mkdir()
    (destination / ".mobkit/state/native.json").write_text('{"keep":"native record"}')
    with project_lock(destination):
        pass
    implementation.identity = str(uuid.uuid4())
    implementation.accepted = False
    implementation.digest = "b" * 64
    implementation.definition["environment"] = []
    implementation.application_configuration = {"fields": []}
    implementation.package = AsyncMock(side_effect=AssertionError("selected candidate must not supply installed settings"))
    implementation.status = AsyncMock(return_value={"build_id": implementation.identity, "status": "failed",
        "current": True, "ready_to_package": False, "acceptance_current": False, "acceptance_trusted": False})
    guide.state["version"] += 1
    guide.state["proposal"]["project"]["name"] = "New unaccepted design"
    return service, directory, destination, initial


def body(service, receipt):
    return {"version": service.guide.state["version"], "deployment_id": receipt["deployment_id"],
            "build_id": receipt["build_id"], "source_digest": "a" * 64,
            "package_sha256": receipt["package_sha256"]}


async def test_stopped_installed_a_settings_ignore_failed_unaccepted_b_and_preserve_source_state(tmp_path):
    service, directory, destination, initial = await prepared(tmp_path)
    before = {path: path.read_bytes() for path in destination.rglob("*") if path.is_file() and path.name != ".env"}
    saved_package = (directory / "package.zip").read_bytes()
    candidate = deepcopy(await service.implementation.status(service.guide.identity))
    options = await service.settings_options(service.guide.identity, initial["deployment_id"])
    assert options["version"] == 5 and options["guide_version"] == 4
    assert options["build_id"] == initial["build_id"] != service.implementation.identity
    assert options["source_digest"] == "a" * 64 and options["source_verified"] is True
    assert options["scope"] == "verified-installed-private-configuration" and options["requires_stopped"] is True
    fields = {field["key"]: field for field in options["fields"]}
    assert fields["OPTIONAL_TOKEN"]["configured"] is True and fields["OPTIONAL_TOKEN"]["removable"] is True
    assert fields["OPENAI_API_KEY"]["removable"] is False and fields["AUTOMATIC_TOKEN"]["removable"] is False
    assert PRIVATE not in json.dumps(options) and ".env" not in json.dumps(options)
    service._launch = lambda *args: pytest.fail("saving settings must not launch a worker")
    result = await service.configure(service.guide.identity, **body(service, initial),
        settings={"OPENAI_API_KEY": "", "OPTIONAL_TOKEN": "", "DISPLAY_THEME": ""},
        remove_fields=["OPTIONAL_TOKEN", "DISPLAY_THEME"])
    values = deployment.private_configuration_snapshot(destination)[0]
    assert values["OPENAI_API_KEY"] == PRIVATE and "OPTIONAL_TOKEN" not in values and "DISPLAY_THEME" not in values
    assert result["status"] == "stopped" and result["health"] == "unknown"
    assert result["startup_attempt_id"] is None and result["startup_console_health"] is None
    assert (destination / ".env").stat().st_mode & 0o777 == 0o600
    assert all(path.read_bytes() == content for path, content in before.items())
    assert (directory / "package.zip").read_bytes() == saved_package
    assert await service.implementation.status(service.guide.identity) == candidate
    service.implementation.package.assert_not_awaited()
    assert PRIVATE not in json.dumps(result)
    assert all(PRIVATE not in path.read_text() for path in directory.rglob("*.json"))


async def test_blank_keep_is_exact_noop_and_does_not_restore_removed_defaults(tmp_path):
    service, directory, destination, initial = await prepared(tmp_path)
    before = {path: (path.read_bytes(), path.stat().st_ino) for path in [destination / ".env", directory / "receipt.json",
        destination / "data/records.json", destination / ".mobkit/state/native.json"]}
    result = await service.configure(service.guide.identity, **body(service, initial),
        settings={"OPENAI_API_KEY": "", "OPTIONAL_TOKEN": "", "DISPLAY_THEME": ""})
    assert result == read_receipt(directory)
    assert all((path.read_bytes(), path.stat().st_ino) == value for path, value in before.items())
    assert result["health"] == "ready" and result["startup_attempt_id"] == "old-attempt"
    await service.configure(service.guide.identity, **body(service, initial), remove_fields=["DISPLAY_THEME"])
    options = await service.settings_options(service.guide.identity, initial["deployment_id"])
    assert next(field for field in options["fields"] if field["key"] == "DISPLAY_THEME")["configured"] is False
    after_removal = (destination / ".env").read_bytes(), (destination / ".env").stat().st_ino
    receipt_after = (directory / "receipt.json").read_bytes()
    await service.configure(service.guide.identity, **body(service, initial), settings={"DISPLAY_THEME": ""})
    assert ((destination / ".env").read_bytes(), (destination / ".env").stat().st_ino) == after_removal
    assert (directory / "receipt.json").read_bytes() == receipt_after
    assert "DISPLAY_THEME" not in deployment.private_configuration_snapshot(destination)[0]
    await service.configure(service.guide.identity, **body(service, initial), settings={"MOBKIT_PORT": "18089"})
    assert deployment.private_configuration_snapshot(destination)[0]["MOBKIT_PORT"] == "18089"
    assert "DISPLAY_THEME" not in deployment.private_configuration_snapshot(destination)[0]


async def test_same_package_blank_install_form_keeps_explicitly_removed_optional_default(tmp_path):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    implementation.application_configuration = {"fields": [
        {"key": "DISPLAY_THEME", "label": "Theme", "secret": False,
         "required": False, "default": "source-theme"},
    ]}
    service = Service(tmp_path / "server", guide, implementation)
    destination = tmp_path / "installed"
    initial = await submit(service, destination, settings={"OPENAI_API_KEY": PRIVATE})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None,
                   process_live=False, runtime_pid=None)
    assert deployment.private_configuration_snapshot(destination)[0]["DISPLAY_THEME"] == "source-theme"
    (destination / "data").mkdir()
    (destination / "data/records.json").write_text('{"keep":"domain record"}')
    (destination / ".mobkit/state").mkdir()
    (destination / ".mobkit/state/native.json").write_text('{"keep":"native record"}')
    await service.configure(guide.identity, **body(service, initial), remove_fields=["DISPLAY_THEME"])
    retained_values = deployment.private_configuration_snapshot(destination)[0]
    assert "DISPLAY_THEME" not in retained_values
    retained_files = {path: path.read_bytes() for path in destination.rglob("*")
                      if path.is_file() and path.name != ".env"}
    package = (directory / "package.zip").read_bytes()
    implementation.package = AsyncMock(return_value=(package, "test-team"))
    admissions = []
    original_launch = service._launch
    def admitted(directory, mode):
        admissions.append((directory, mode))
        original_launch(directory, mode)
    service._launch = admitted
    try:
        result = await submit(service, destination, settings={"OPENAI_API_KEY": "", "DISPLAY_THEME": ""})
        assert deployment.private_configuration_snapshot(destination)[0] == retained_values
        assert result["deployment_id"] == initial["deployment_id"] and result["build_id"] == initial["build_id"]
        assert result["package_sha256"] == initial["package_sha256"] and result["status"] == "queued"
        assert admissions == [(directory, "install")]
        assert all(path.read_bytes() == content for path, content in retained_files.items())
        assert (directory / "package.zip").read_bytes() == package
        assert (destination / ".env").stat().st_mode & 0o777 == 0o600
        assert PRIVATE not in json.dumps(result)
    finally:
        await service.shutdown()


async def background_submit_prepared(tmp_path, monkeypatch, *, service_created=True):
    from launchkit.deployment_service import prepare_service
    from test_deployment_service import Manager
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    os.makedirs(os.environ["HOME"])
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    implementation.application_configuration = {"fields": [
        {"key": "OPTIONAL_TOKEN", "label": "Optional token", "secret": True, "required": False},
        {"key": "DISPLAY_THEME", "label": "Theme", "secret": False,
         "required": False, "default": "source-theme"},
    ]}
    manager = Manager()
    service = Service(tmp_path / "server", guide, implementation, service_manager=manager)
    destination = tmp_path / "installed"
    initial = await submit(service, destination, execution_mode="background", settings={
        "OPENAI_API_KEY": PRIVATE, "OPTIONAL_TOKEN": "previous-private-value"})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", process_live=False, runtime_pid=None)
    if service_created:
        prepare_service(directory, read_receipt(directory))
        update_receipt(directory, service_supervisor_pid=12345,
                       service_instance_id=str(uuid.uuid4()), service_cleanup_confirmed=True)
    (destination / "data").mkdir()
    (destination / "data/records.json").write_text('{"keep":"domain record"}')
    (destination / ".mobkit/state").mkdir()
    (destination / ".mobkit/state/native.json").write_text('{"keep":"native record"}')
    with project_lock(destination):
        pass
    implementation.package = AsyncMock(return_value=((directory / "package.zip").read_bytes(), "test-team"))
    return service, manager, directory, destination, initial


@pytest.mark.parametrize("transition", [
    "manager-live", "manager-waiting", "instance", "native-cleanup", "frozen-host",
    "source", "missing-source", "package", "private", "lease",
])
async def test_background_same_package_submit_rechecks_prepared_service_source_and_private_snapshot(
        tmp_path, monkeypatch, transition):
    # Exercise real owned package, frozen service verification and project lease
    # with a deterministic manager, without running a native app or OS service.
    import fcntl
    service, manager, directory, destination, initial = await background_submit_prepared(tmp_path, monkeypatch)
    original = deployment.prepare_private_settings
    held = []
    expected = {}
    def transitioned(*args, **kwargs):
        values = original(*args, **kwargs)
        if transition == "manager-live":
            manager.observation = {"loaded": True, "state": "running", "pid": 23456}
        elif transition == "manager-waiting":
            manager.observation = {"loaded": True, "state": "waiting", "pid": None}
        elif transition == "instance":
            update_receipt(directory, service_instance_id=str(uuid.uuid4()))
        elif transition == "native-cleanup":
            update_receipt(directory, service_cleanup_confirmed=False)
        elif transition == "frozen-host":
            (directory / "service-host/launchkit/deployment_worker.py").write_text("changed frozen host")
        elif transition == "source":
            (destination / "app.py").write_text("changed installed source")
        elif transition == "missing-source":
            (destination / "app.py").unlink()
        elif transition == "package":
            (directory / "package.zip").write_bytes(b"changed saved package")
        elif transition == "private":
            (destination / ".env").write_text('OPENAI_API_KEY="newer-private-value"\nMOBKIT_PORT="18088"\n')
        elif transition == "lease":
            descriptor = os.open(destination / ".mobkit/launch.lock", os.O_RDWR)
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            held.append(descriptor)
        expected["private"] = (destination / ".env").read_bytes(), (destination / ".env").stat().st_ino
        expected["files"] = {path: path.read_bytes() for path in destination.rglob("*")
                             if path.is_file() and path.name != ".env"}
        return values
    monkeypatch.setattr(deployment, "prepare_private_settings", transitioned)
    writes = []
    def forbidden_write(*args, **kwargs):
        writes.append((args, kwargs))
        pytest.fail("a changed service, source, private file or lease must not save values")
    monkeypatch.setattr(deployment, "write_private_settings", forbidden_write)
    monkeypatch.setattr(deployment, "replace_private_configuration", forbidden_write)
    admissions = []
    service._launch = lambda *args: admissions.append(args)
    try:
        with pytest.raises((DraftConflict, RuntimeError)):
            await submit(service, destination, execution_mode="background", settings={"OPTIONAL_TOKEN": "replacement"})
        assert writes == [] and admissions == [] and service.tasks == {}
        assert ((destination / ".env").read_bytes(), (destination / ".env").stat().st_ino) == expected["private"]
        assert all(path.read_bytes() == content for path, content in expected["files"].items())
        assert read_receipt(directory)["status"] == "stopped"
        assert not any(name in {"start", "stop"} for name, _ in manager.calls)
    finally:
        for descriptor in held:
            os.close(descriptor)
        await service.shutdown()


async def test_background_same_package_stopped_submit_holds_lease_during_final_manager_check(tmp_path, monkeypatch):
    service, manager, directory, destination, initial = await background_submit_prepared(tmp_path, monkeypatch)
    before = {path: path.read_bytes() for path in destination.rglob("*")
              if path.is_file() and path.name != ".env"}
    frozen = {path: path.read_bytes() for path in (directory / "service-host").rglob("*") if path.is_file()}
    original_observe = manager.observe
    locked_checks = []
    async def observed(plan):
        try:
            with project_lock(destination):
                locked_checks.append(False)
        except RuntimeError:
            locked_checks.append(True)
        return await original_observe(plan)
    manager.observe = observed
    admissions = []
    original_launch = service._launch
    def admitted(root, mode):
        admissions.append((root, mode))
        original_launch(root, mode)
    service._launch = admitted
    try:
        result = await submit(service, destination, execution_mode="background", settings={"OPTIONAL_TOKEN": "replacement"})
        assert any(locked_checks) and admissions == [(directory, "install")]
        assert result["deployment_id"] == initial["deployment_id"]
        assert read_receipt(directory)["status"] == "queued" and directory.name in service.tasks
        assert deployment.private_configuration_snapshot(destination)[0]["OPTIONAL_TOKEN"] == "replacement"
        assert all(path.read_bytes() == content for path, content in before.items())
        assert all(path.read_bytes() == content for path, content in frozen.items())
        assert not any(name in {"start", "stop"} for name, _ in manager.calls)
        assert PRIVATE not in json.dumps(result)
    finally:
        await service.shutdown()


@pytest.mark.parametrize("private_present", [True, False])
async def test_background_same_package_partial_install_without_service_keeps_retry_path(tmp_path, monkeypatch, private_present):
    service, manager, directory, destination, initial = await background_submit_prepared(
        tmp_path, monkeypatch, service_created=False)
    (destination / "app.py").unlink()
    if not private_present:
        (destination / ".env").unlink()
    update_receipt(directory, status="failed")
    package = (directory / "package.zip").read_bytes()
    records = {path: path.read_bytes() for path in [
        destination / "data/records.json", destination / ".mobkit/state/native.json"]}
    try:
        result = await submit(service, destination, execution_mode="background", settings={
            "OPENAI_API_KEY": "" if private_present else PRIVATE, "OPTIONAL_TOKEN": "replacement"})
        assert result["deployment_id"] == initial["deployment_id"] and result["status"] == "queued"
        assert directory.name in service.tasks and not read_receipt(directory).get("service")
        assert deployment.private_configuration_snapshot(destination)[0]["OPTIONAL_TOKEN"] == "replacement"
        assert deployment.private_configuration_snapshot(destination)[0]["DISPLAY_THEME"] == "source-theme"
        assert not (destination / "app.py").exists() and (directory / "package.zip").read_bytes() == package
        assert all(path.read_bytes() == content for path, content in records.items())
        materialize(directory)
        assert (destination / "app.py").read_text() == "print('native test fixture')\n"
        assert not any(name in {"start", "stop"} for name, _ in manager.calls)
    finally:
        await service.shutdown()


@pytest.mark.parametrize("transition", ["guide", "candidate", "acceptance", "source", "private"])
async def test_background_same_package_final_service_observation_rechecks_authority(tmp_path, monkeypatch, transition):
    import asyncio
    service, manager, directory, destination, _ = await background_submit_prepared(tmp_path, monkeypatch)
    original_observe = manager.observe
    count = 0
    injected = False
    expected = {}
    async def transitioned(plan):
        nonlocal count, injected
        count += 1
        try:
            with project_lock(destination):
                lease_held = False
        except RuntimeError:
            lease_held = True
        if lease_held and not injected:
            injected = True
            if transition == "guide":
                service.guide.state["version"] += 1
            elif transition == "candidate":
                service.implementation.identity = str(uuid.uuid4())
            elif transition == "acceptance":
                service.implementation.accepted = False
            elif transition == "source":
                (destination / "app.py").write_text("source changed during the manager observation")
            elif transition == "private":
                (destination / ".env").write_text('OPENAI_API_KEY="newer-value"\nMOBKIT_PORT="18088"\n')
            expected["private"] = (destination / ".env").read_bytes(), (destination / ".env").stat().st_ino
            await asyncio.sleep(0)
        return await original_observe(plan)
    manager.observe = transitioned
    admissions = []
    service._launch = lambda *args: admissions.append(args)
    try:
        with pytest.raises((DraftConflict, ValueError)):
            await submit(service, destination, execution_mode="background", settings={"OPTIONAL_TOKEN": "replacement"})
        assert count > 0 and injected and admissions == [] and service.tasks == {}
        assert ((destination / ".env").read_bytes(), (destination / ".env").stat().st_ino) == expected["private"]
        assert read_receipt(directory)["status"] == "stopped"
        assert not any(name in {"start", "stop"} for name, _ in manager.calls)
    finally:
        await service.shutdown()


@pytest.mark.parametrize("status", ["queued", "running", "checking", "admitting", "stopping", "unknown"])
async def test_active_or_ambiguous_current_candidate_blocks_private_settings(tmp_path, status):
    service, _, destination, initial = await prepared(tmp_path)
    service.implementation.status.return_value["status"] = status
    before = (destination / ".env").read_bytes()
    with pytest.raises(DraftConflict, match="builder to stop"):
        await service.configure(service.guide.identity, **body(service, initial), settings={"OPTIONAL_TOKEN": "replacement"})
    assert (destination / ".env").read_bytes() == before


@pytest.mark.parametrize("status", ["completed", "failed", "interrupted"])
async def test_terminal_unaccepted_current_candidate_does_not_block_installed_options(tmp_path, status):
    service, _, _, initial = await prepared(tmp_path)
    service.implementation.status.return_value["status"] = status
    assert (await service.settings_options(service.guide.identity, initial["deployment_id"]))["source_verified"] is True


async def test_absent_current_candidate_is_allowed_but_ambiguous_read_and_cleanup_are_not(tmp_path):
    service, _, _, initial = await prepared(tmp_path)
    service.implementation.status.side_effect = LookupError("No current build")
    service.implementation.selected_build_id = lambda guide_id: (_ for _ in ()).throw(LookupError("No current build"))
    assert (await service.settings_options(service.guide.identity, initial["deployment_id"]))["build_id"] == initial["build_id"]
    service.implementation.status.side_effect = ValueError(PRIVATE)
    with pytest.raises(DraftConflict, match="could not be verified") as error:
        await service.settings_options(service.guide.identity, initial["deployment_id"])
    assert PRIVATE not in str(error.value)
    service.implementation.status.side_effect = None
    service.implementation.status.return_value["cleanup_confirmed"] = False
    with pytest.raises(DraftConflict, match="builder to stop"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])
    service.implementation.status.return_value["cleanup_confirmed"] = None
    with pytest.raises(DraftConflict, match="builder to stop"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])


@pytest.mark.parametrize("changes", [{"version": 4}, {"deployment_id": str(uuid.uuid4())},
    {"build_id": str(uuid.uuid4())}, {"source_digest": "c" * 64}, {"package_sha256": "d" * 64}])
async def test_exact_installed_and_current_guide_bindings_reject_stale_save(tmp_path, changes):
    service, _, destination, initial = await prepared(tmp_path)
    before = (destination / ".env").read_bytes()
    with pytest.raises(DraftConflict):
        await service.configure(service.guide.identity, **{**body(service, initial), **changes}, settings={"OPTIONAL_TOKEN": "new"})
    assert (destination / ".env").read_bytes() == before


@pytest.mark.parametrize("removed", [["OPENAI_API_KEY"], ["AUTOMATIC_TOKEN"], ["MOBKIT_PORT"], ["UNDECLARED"],
                                   ["OPTIONAL_TOKEN", "OPTIONAL_TOKEN"], [False]])
async def test_required_automatic_port_unknown_and_duplicate_removals_are_refused(tmp_path, removed):
    service, _, destination, initial = await prepared(tmp_path)
    before = (destination / ".env").read_bytes()
    with pytest.raises(ValueError):
        await service.configure(service.guide.identity, **body(service, initial), remove_fields=removed)
    assert (destination / ".env").read_bytes() == before


@pytest.mark.parametrize("settings,removed", [({"OPTIONAL_TOKEN": "replacement"}, ["OPTIONAL_TOKEN"]),
                                           ({"AUTOMATIC_TOKEN": "replacement"}, []), ({"UNDECLARED": "value"}, [])])
async def test_replacement_removal_conflicts_and_unrequested_writes_are_refused(tmp_path, settings, removed):
    service, _, destination, initial = await prepared(tmp_path)
    before = (destination / ".env").read_bytes()
    with pytest.raises(ValueError):
        await service.configure(service.guide.identity, **body(service, initial), settings=settings, remove_fields=removed)
    assert (destination / ".env").read_bytes() == before


async def test_live_project_lease_and_worker_handle_prevent_reconfiguration(tmp_path):
    service, directory, destination, initial = await prepared(tmp_path)
    with project_lock(destination):
        with pytest.raises(RuntimeError, match="already running"):
            await service.configure(service.guide.identity, **body(service, initial))
    service.processes[directory.name] = SimpleNamespace(returncode=None)
    with pytest.raises(DraftConflict, match="Stop this installed"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])


@pytest.mark.parametrize("kind", ["source", "package", "env-link", "env-hardlink", "env-public", "env-size", "lock-link"])
async def test_source_private_and_lock_integrity_fail_closed_without_secret_error(tmp_path, kind):
    service, directory, destination, initial = await prepared(tmp_path)
    path = destination / ".env"
    if kind == "source":
        (destination / "app.py").write_text("modified")
    elif kind == "package":
        (directory / "package.zip").write_bytes(b"modified")
    elif kind == "env-link":
        path.rename(destination / "private-backup")
        path.symlink_to(destination / "private-backup")
    elif kind == "env-hardlink":
        os.link(path, destination / "private-backup")
    elif kind == "env-public":
        path.chmod(0o644)
    elif kind == "env-size":
        path.write_bytes(b"X" * (256 * 1024 + 1))
    else:
        (destination / ".mobkit/launch.lock").unlink()
        (destination / ".mobkit/launch.lock").symlink_to(path)
    before = path.read_bytes()
    with pytest.raises(DraftConflict) as error:
        await service.configure(service.guide.identity, **body(service, initial), settings={"OPTIONAL_TOKEN": "new"})
    assert PRIVATE not in str(error.value) and str(destination) not in str(error.value)
    assert path.read_bytes() == before


async def test_current_guide_or_selected_candidate_race_before_final_write_is_rejected(tmp_path):
    service, _, destination, initial = await prepared(tmp_path)
    before = (destination / ".env").read_bytes()
    original = service.guide.get
    calls = 0
    async def changing(identity):
        nonlocal calls
        calls += 1
        if calls == 2:
            service.implementation.identity = str(uuid.uuid4())
        return await original(identity)
    service.guide.get = changing
    with pytest.raises(DraftConflict, match="changed while preparing"):
        await service.configure(service.guide.identity, **body(service, initial))
    assert (destination / ".env").read_bytes() == before
    calls = 0
    async def guide_changing(identity):
        nonlocal calls
        calls += 1
        if calls == 2:
            service.guide.state["version"] += 1
        return await original(identity)
    service.guide.get = guide_changing
    service.implementation.status.return_value["build_id"] = service.implementation.identity
    with pytest.raises(DraftConflict, match="changed while preparing"):
        await service.configure(service.guide.identity, **body(service, initial))
    assert (destination / ".env").read_bytes() == before


async def test_concurrent_private_edit_and_same_bytes_inode_replacement_are_preserved(tmp_path, monkeypatch):
    service, _, destination, initial = await prepared(tmp_path)
    original = deployment.prepare_private_settings
    path = destination / ".env"
    newer = b'OPENAI_API_KEY="newer-private-canary"\nMOBKIT_PORT="18088"\n'
    def concurrent(*args, **kwargs):
        values = original(*args, **kwargs)
        path.write_bytes(newer)
        return values
    monkeypatch.setattr(deployment, "prepare_private_settings", concurrent)
    with pytest.raises(DraftConflict, match="newer values were preserved"):
        await service.configure(service.guide.identity, **body(service, initial))
    assert path.read_bytes() == newer and not list(destination.glob(".env.*.pending"))
    def replaced(*args, **kwargs):
        values = original(*args, **kwargs)
        old = path.read_bytes()
        path.rename(destination / "old-private")
        path.write_bytes(old)
        path.chmod(0o600)
        return values
    monkeypatch.setattr(deployment, "prepare_private_settings", replaced)
    with pytest.raises(DraftConflict, match="newer values were preserved"):
        await service.configure(service.guide.identity, **body(service, initial))
    assert path.read_bytes() == newer


async def test_background_save_requires_actual_owned_stopped_service_and_cleanup(tmp_path, monkeypatch):
    service, directory, destination, initial = await prepared(tmp_path)
    from launchkit import deployment_service
    calls = []
    async def stopped(owned_directory, manager):
        calls.append((owned_directory, manager))
        raise DraftConflict("native cleanup is unconfirmed")
    monkeypatch.setattr(deployment_service, "require_stopped_service", stopped)
    update_receipt(directory, execution_mode="background")
    before = (destination / ".env").read_bytes()
    with pytest.raises(DraftConflict, match="unconfirmed"):
        await service.configure(service.guide.identity, **body(service, initial))
    assert calls == [(directory, service.service_manager)] and (destination / ".env").read_bytes() == before
    async def changed(owned_directory, manager):
        update_receipt(owned_directory, service_instance_id="different-instance")
    monkeypatch.setattr(deployment_service, "require_stopped_service", changed)
    with pytest.raises(DraftConflict, match="changed while preparing"):
        await service.configure(service.guide.identity, **body(service, initial))
    assert (destination / ".env").read_bytes() == before


@pytest.mark.parametrize("problem", [None, "native-cleanup", "manager-live", "frozen-host"])
async def test_background_settings_use_real_frozen_service_verification_without_manager_mutation(tmp_path, monkeypatch, problem):
    # Only isolated unit/host files and deterministic manager observations are
    # used here. This is not a live OS-service or native health proof.
    from launchkit.deployment_service import prepare_service
    from test_deployment_service import Manager
    monkeypatch.setenv("HOME", str(tmp_path / "isolated-home"))
    os.makedirs(os.environ["HOME"])
    service, directory, destination, initial = await prepared(tmp_path)
    manager = Manager()
    service.service_manager = manager
    update_receipt(directory, execution_mode="background")
    plan = prepare_service(directory, read_receipt(directory))
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None, service_supervisor_pid=12345, service_cleanup_confirmed=True)
    if problem == "native-cleanup":
        update_receipt(directory, service_cleanup_confirmed=False)
    elif problem == "manager-live":
        manager.observation = {"loaded": True, "state": "running", "pid": 23456}
    elif problem == "frozen-host":
        (directory / "service-host/launchkit/deployment_worker.py").write_text("changed host")
    before = (destination / ".env").read_bytes()
    if problem:
        with pytest.raises(DraftConflict):
            await service.configure(service.guide.identity, **body(service, initial), settings={"OPTIONAL_TOKEN": "replacement"})
        assert (destination / ".env").read_bytes() == before
    else:
        result = await service.configure(service.guide.identity, **body(service, initial), settings={"OPTIONAL_TOKEN": "replacement"})
        assert result["status"] == "stopped" and result["service"]["unit_sha256"] == plan["unit_sha256"]
        assert deployment.private_configuration_snapshot(destination)[0]["OPTIONAL_TOKEN"] == "replacement"
    assert not any(name in {"start", "stop"} for name, _ in manager.calls)


async def test_recovery_needed_and_guide_busy_are_not_private_setup_shortcuts(tmp_path):
    service, directory, destination, initial = await prepared(tmp_path)
    before = (destination / ".env").read_bytes()
    update_receipt(directory, update={"status": "recovery-needed"})
    with pytest.raises(DraftConflict, match="recover"):
        await service.settings_options(service.guide.identity, initial["deployment_id"])
    update_receipt(directory, update=None)
    service.guide.state["operation"] = {"status": "running"}
    with pytest.raises(DraftConflict, match="busy"):
        await service.configure(service.guide.identity, **body(service, initial))
    assert (destination / ".env").read_bytes() == before
