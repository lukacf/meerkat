"""A macOS background login item cannot read protected folders.

launchd runs a background installation with Forge's own Python and reads
Forge's saved deployment files and the installation folder. macOS privacy
protection stops a login item from reading Desktop, Documents, Downloads,
iCloud Drive and other volumes, so such a service stalls in Python start-up
with no message. Options and submit say so before anything is installed.

These rows use a fake home and a fake launchd manager. Nothing here reads a
real protected folder or changes a privacy setting.
"""
import asyncio
import os
from pathlib import Path
import sys
import uuid

import pytest

import launchkit.deployment_service as deployment_service
from launchkit.deployment import DeploymentService, materialize, read_receipt, update_receipt
from launchkit.projects import DraftConflict
from test_deployment import Guide, Implementation, Service, submit

# Third-party cloud folders (~/Library/CloudStorage) are refused too, failing closed.
PROTECTED = ("Desktop", "Documents", "Downloads", "Library/Mobile Documents", "Library/CloudStorage")
PROTECTED_IDS = ("Desktop", "Documents", "Downloads", "iCloud-Drive", "CloudStorage")
FOLDER_REFUSAL = ("macOS does not let a background login item read Desktop, Documents, Downloads, iCloud Drive or "
    "cloud storage folders. Move the Forge folder and the installation folder to a folder such as ~/MetaForge, or "
    "choose 'While Forge is open'.")
# An installed unit is not moved: moving its folders would break its installation.
EARLIER_REFUSAL = ("macOS does not let a background login item read Desktop, Documents, Downloads, iCloud Drive or "
    "cloud storage folders. Stop this service and install the application again in a folder such as ~/MetaForge, "
    "or install it to run while Forge is open.")
FOLDER_NOTE = ("macOS does not let a background login item read Desktop, Documents, Downloads, iCloud Drive or "
    "cloud storage folders. This service runs only while this computer lets background login items read them, "
    "for example with Full Disk Access.")
LEGACY_NOTE = FOLDER_NOTE + (" Forge cannot confirm that this older service can read them. If it does not start, "
    "stop it and install the application again in a folder such as ~/MetaForge, or install it to run while Forge "
    "is open.")
VOLUME_REFUSAL = ("macOS does not let a background login item read external or network volumes, which may also be "
    "unavailable at login. Move the Forge folder and the installation folder to a folder such as ~/MetaForge, "
    "or choose 'While Forge is open'.")


class Manager:
    """A logged-in user's service manager that never runs anything."""

    def __init__(self, manager="launchd"):
        self.manager = manager
        self.calls = []

    async def capability(self):
        return {"available": True, "manager": self.manager, "persistence": "Runs after you log in."}

    async def observe(self, service):
        self.calls.append("observe")
        return {"loaded": False, "pid": None, "state": "stopped"}

    async def start(self, service):
        self.calls.append("start")
        return {"loaded": True, "pid": 4567, "state": "running"}

    async def stop(self, service):
        self.calls.append("stop")
        return {"loaded": False, "pid": None, "state": "stopped"}


@pytest.fixture
def home(tmp_path, monkeypatch):
    home = tmp_path / "home"
    for folder in PROTECTED:
        (home / folder).mkdir(parents=True)
    monkeypatch.setenv("HOME", str(home))
    # Another volume is a folder under this fake /Volumes; the startup disk
    # is the link macOS keeps there.
    volumes = tmp_path / "Volumes"
    (volumes / "External").mkdir(parents=True)
    (volumes / "Startup Disk").symlink_to("/")
    monkeypatch.setattr(deployment_service, "MACOS_VOLUMES", str(volumes), raising=False)
    return home


def forge(root, manager="launchd"):
    guide = Guide()
    return Service(root, guide, Implementation(guide), service_manager=Manager(manager))


def mode(options, identity):
    return next(value for value in options["execution_modes"] if value["id"] == identity)


async def read_options(service):
    try:
        return await service.options(service.guide.identity)
    finally:
        await service.shutdown()


async def refused_background(service, destination):
    try:
        with pytest.raises(ValueError) as refused:
            await submit(service, destination, execution_mode="background")
        # Nothing was admitted: no receipt, worker, service or destination.
        with pytest.raises(LookupError):
            service._directory(service.guide.identity)
        assert not service.tasks and not service.service_manager.calls
        assert not destination.exists()
    finally:
        await service.shutdown()
    return str(refused.value)


@pytest.mark.parametrize("folder", PROTECTED, ids=PROTECTED_IDS)
async def test_options_refuse_background_when_forge_python_is_in_a_protected_folder(home, monkeypatch, folder):
    monkeypatch.setattr(sys, "executable", str(home / folder / "MetaForge/.venv/bin/python"))
    options = await read_options(forge(home / "MetaForge/.studio"))
    background = mode(options, "background")
    assert background["available"] is False and background["reason"] == FOLDER_REFUSAL
    assert mode(options, "interactive")["available"] is True


@pytest.mark.parametrize("folder", PROTECTED, ids=PROTECTED_IDS)
async def test_options_refuse_background_when_forge_state_is_in_a_protected_folder(home, folder):
    options = await read_options(forge(home / folder / "MetaForge/.studio"))
    background = mode(options, "background")
    assert background["available"] is False and background["reason"] == FOLDER_REFUSAL
    assert mode(options, "interactive")["available"] is True


async def test_options_offer_background_outside_protected_folders(home):
    # A folder with a protected name deeper in another folder is ordinary.
    options = await read_options(forge(home / "MetaForge/Documents/.studio"))
    background = mode(options, "background")
    assert background["available"] is True and background["reason"] is None


async def test_options_keep_background_for_a_linux_user_service(home):
    options = await read_options(forge(home / "Documents/MetaForge/.studio", manager="systemd-user"))
    assert mode(options, "background")["available"] is True


@pytest.mark.parametrize("folder", PROTECTED, ids=PROTECTED_IDS)
@pytest.mark.parametrize("where", ["python", "state", "destination"])
async def test_background_submit_refuses_protected_folders(home, monkeypatch, folder, where):
    if where == "python":
        monkeypatch.setattr(sys, "executable", str(home / folder / "MetaForge/.venv/bin/python"))
    root = home / (folder if where == "state" else "MetaForge") / "forge/.studio"
    destination = home / (folder if where == "destination" else "Projects") / "team"
    assert await refused_background(forge(root), destination) == FOLDER_REFUSAL


async def test_background_submit_outside_protected_folders_is_admitted(home):
    service = forge(home / "MetaForge/.studio")
    try:
        receipt = await submit(service, home / "Projects/team", execution_mode="background")
        assert receipt["execution_mode"] == "background" and receipt["status"] == "queued"
    finally:
        await service.shutdown()


async def test_interactive_submit_may_use_a_protected_folder(home):
    service = forge(home / "Documents/MetaForge/.studio")
    try:
        receipt = await submit(service, home / "Documents/team")
        assert receipt["execution_mode"] == "interactive" and receipt["status"] == "queued"
    finally:
        await service.shutdown()


async def test_background_refuses_another_volume(home, tmp_path):
    volume = tmp_path / "Volumes/External"
    options = await read_options(forge(volume / "MetaForge/.studio"))
    assert mode(options, "background")["reason"] == VOLUME_REFUSAL
    assert mode(options, "background")["available"] is False
    refusal = await refused_background(forge(home / "MetaForge/.studio"), volume / "team")
    assert refusal == VOLUME_REFUSAL


async def test_background_follows_the_startup_disk_link_in_volumes(home, tmp_path):
    # The startup disk's entry in /Volumes links to /, so a path through it
    # names an ordinary folder.
    root = tmp_path / "Volumes/Startup Disk" / home.relative_to("/") / "MetaForge/.studio"
    options = await read_options(forge(root))
    assert mode(options, "background")["available"] is True


async def test_background_folder_names_ignore_case(home):
    # macOS folders are case-insensitive by default.
    options = await read_options(forge(home / "downloads/MetaForge/.studio"))
    assert mode(options, "background")["reason"] == FOLDER_REFUSAL


async def test_background_follows_links_without_looking_inside_protected_folders(home, monkeypatch):
    # ~/MetaForge links into Documents. The check reads the link, then stops
    # at Documents: looking inside it could show a privacy prompt.
    (home / "Documents/MetaForge/.studio").mkdir(parents=True)
    (home / "MetaForge").symlink_to(home / "Documents/MetaForge")
    service = forge(home / "MetaForge/.studio")
    lookups = []
    for name in ("stat", "lstat", "readlink", "scandir", "listdir"):
        original = getattr(os, name)

        def record(*args, _original=original, **kwargs):
            if args and isinstance(args[0], (str, bytes, os.PathLike)):
                lookups.append(Path(os.fsdecode(args[0])))
            return _original(*args, **kwargs)
        monkeypatch.setattr(os, name, record)
    refusal = await refused_background(service, home / "Projects/team")
    monkeypatch.undo()
    assert refusal == FOLDER_REFUSAL
    assert home / "MetaForge" in lookups, "The check reads the link where it is."
    protected = home / "Documents"
    assert not [path for path in lookups if protected in path.parents], lookups


async def test_options_and_submit_walk_the_folders_off_the_event_loop(home, monkeypatch):
    # The walk looks up path parts on disk; like the setup card's check, it
    # runs in a worker thread so a slow disk cannot hold the event loop.
    import threading
    threads = []
    original = deployment_service.launchd_folder_refusal

    def record(*args, **kwargs):
        threads.append(threading.current_thread() is threading.main_thread())
        return original(*args, **kwargs)
    monkeypatch.setattr(deployment_service, "launchd_folder_refusal", record)
    await read_options(forge(home / "MetaForge/.studio"))
    service = forge(home / "MetaForge/forge/.studio")
    try:
        await submit(service, home / "Projects/team", execution_mode="background")
    finally:
        await service.shutdown()
    assert threads == [False, False]


@pytest.fixture(params=["darwin"])
async def earlier_service(request, home, tmp_path, monkeypatch):
    """A service installed before this preflight, with its Python in Documents.

    No migration: the unit keeps the interpreter its definition names.
    """
    from test_deployment_service import Manager as ServiceManager
    plan = deployment_service.service_plan
    monkeypatch.setattr(deployment_service, "service_plan", lambda identity, root, **options:
        plan(identity, root, **{**options, "platform": request.param, "home": Path.home()}))
    guide = Guide()
    manager = ServiceManager()
    service = Service(tmp_path / "private", guide, Implementation(guide), service_manager=manager)
    with monkeypatch.context() as earlier:
        # No preflight existed when this service was installed.
        earlier.setattr(DeploymentService, "_background_folder_refusal", lambda self, *args, **kwargs: None)
        receipt = await submit(service, tmp_path / "application", execution_mode="background")
        directory = service._directory(guide.identity)
        materialize(directory)
        service.tasks[receipt["deployment_id"]].cancel()
        await asyncio.gather(*service.tasks.values(), return_exceptions=True)
        service.tasks.clear()
        earlier.setattr(sys, "executable", str(home / "Documents/MetaForge/.venv/bin/python"))
        unit = deployment_service.prepare_service(directory, read_receipt(directory))
    assert unit["interpreter"].startswith(str(home / "Documents"))
    yield service, manager, directory
    await service.shutdown()


def waiting_in_start_up(directory, manager):
    # The unit's Python is waiting on a protected folder and never registered.
    update_receipt(directory, status="starting", phase="service", service_cleanup_confirmed=False)
    manager.observation = {"loaded": True, "pid": 98110, "state": "running"}


def no_manager_effect(manager):
    return not [call for call in manager.calls if call[0] in {"observe", "start", "stop"}]


@pytest.mark.parametrize("action", ["restart", "retry"])
async def test_earlier_service_in_a_protected_folder_refuses_a_start_before_any_manager_call(
        earlier_service, action):
    service, manager, directory = earlier_service
    waiting_in_start_up(directory, manager)
    before = read_receipt(directory)
    manager.calls.clear()
    with pytest.raises(DraftConflict) as refused:
        await service.action(service.guide.identity, directory.name, action)
    assert str(refused.value) == EARLIER_REFUSAL
    assert no_manager_effect(manager)
    after = read_receipt(directory)
    assert after["status"] == before["status"]
    assert deployment_service.service_binding(after["service"]) == deployment_service.service_binding(before["service"])


@pytest.mark.parametrize("action", ["prepare-support", "apply-support"])
async def test_earlier_service_in_a_protected_folder_refuses_a_support_refresh_before_any_manager_call(
        earlier_service, action):
    service, manager, directory = earlier_service
    update_receipt(directory, status="stopped", service_cleanup_confirmed=True)
    manager.calls.clear()
    support = ({"expected_service_digest": "a" * 64} if action == "prepare-support"
               else {"revision": str(uuid.uuid4()), "descriptor_sha256": "b" * 64})
    with pytest.raises(Exception) as refused:
        await service.action(service.guide.identity, directory.name, action,
                             action_request_id=str(uuid.uuid4()), **support)
    assert (type(refused.value), str(refused.value)) == (DraftConflict, EARLIER_REFUSAL)
    assert no_manager_effect(manager)
    assert not (directory / "supervisor-refresh").exists()


async def test_earlier_service_in_a_protected_folder_can_still_stop(earlier_service, monkeypatch):
    from test_deployment_service import ended_pid
    service, manager, directory = earlier_service
    monkeypatch.setattr(deployment_service, "SERVICE_TIMEOUT", 2)
    waiting_in_start_up(directory, manager)
    manager.observation = {"loaded": True, "pid": ended_pid(), "state": "running"}
    stopped = await service.action(service.guide.identity, directory.name, "stop")
    assert stopped["status"] == "stopped"


@pytest.mark.parametrize("earlier_service", ["linux"], indirect=True)
async def test_a_linux_user_service_with_its_python_in_documents_still_restarts(earlier_service):
    service, manager, directory = earlier_service
    manager.cleanup = directory
    update_receipt(directory, status="running", service_supervisor_pid=4567, service_instance_id="generation-one",
                   service_cleanup_confirmed=False)
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    restarted = await service.action(service.guide.identity, directory.name, "restart")
    assert restarted["status"] == "starting"
    assert [call[0] for call in manager.calls if call[0] != "observe"] == ["stop", "start"]


async def test_status_names_the_folder_an_earlier_service_waits_on(earlier_service):
    _, manager, directory = earlier_service
    waiting_in_start_up(directory, manager)
    status = await deployment_service.background_status(directory, manager)
    assert (status["status"], status["detail"]) == ("starting", EARLIER_REFUSAL)
    assert read_receipt(directory)["detail"] != EARLIER_REFUSAL, "The reason is a status projection only."


async def test_status_of_an_earlier_service_running_in_a_protected_folder_is_unchanged(earlier_service):
    # A lifetime registered there, so this computer lets it read the folder.
    _, manager, directory = earlier_service
    update_receipt(directory, status="running", service_supervisor_pid=4567,
                   service_instance_id=str(uuid.uuid4()), service_cleanup_confirmed=False)
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}
    status = await deployment_service.background_status(directory, manager)
    assert status["detail"] != EARLIER_REFUSAL


def ready_evidence(directory, **changes):
    """The record this unit's registered instance writes when it reaches ready."""
    receipt = read_receipt(directory)
    service = receipt["service"]
    evidence = {"instance_id": receipt["service_instance_id"],
                **{key: service[key] for key in ("manager", "domain", "label", "unit_path", "interpreter",
                                                 "receipt_directory")},
                "destination": receipt["destination"]}
    return {**evidence, **changes}


async def attempt(service, directory, action):
    """The refusal of an action, or None when it was admitted."""
    try:
        await service.action(service.guide.identity, directory.name, action)
    except DraftConflict as error:
        return str(error)
    return None


def registered(directory, manager, *, ready=True, **changes):
    update_receipt(directory, status="running", phase="run", service_supervisor_pid=4567,
                   service_instance_id=str(uuid.uuid4()), service_cleanup_confirmed=False)
    if ready:
        update_receipt(directory, service_folder_evidence=ready_evidence(directory, **changes))
    manager.observation = {"loaded": True, "pid": 4567, "state": "running"}


async def test_a_registered_unit_without_ready_evidence_is_refused(earlier_service):
    service, manager, directory = earlier_service
    registered(directory, manager, ready=False)
    manager.calls.clear()
    with pytest.raises(DraftConflict) as refused:
        await service.action(service.guide.identity, directory.name, "restart")
    assert str(refused.value) == EARLIER_REFUSAL
    assert no_manager_effect(manager)


async def test_ready_evidence_lets_restart_reach_the_manager_and_the_status_notes_the_folder(earlier_service):
    # This exact unit reached ready there, so this computer lets it read the folder.
    service, manager, directory = earlier_service
    registered(directory, manager)
    status = await deployment_service.background_status(directory, manager)
    assert status.get("protected_folder") == FOLDER_NOTE and status["detail"] != EARLIER_REFUSAL
    manager.cleanup = directory
    manager.calls.clear()
    assert await attempt(service, directory, "restart") is None
    assert read_receipt(directory)["status"] == "starting"
    assert [call[0] for call in manager.calls if call[0] != "observe"] == ["stop", "start"]


@pytest.mark.parametrize("action", ["restart", "retry"])
async def test_a_start_that_never_registers_clears_the_evidence(earlier_service, action):
    service, manager, directory = earlier_service
    registered(directory, manager)
    manager.cleanup = directory
    assert await attempt(service, directory, action) is None
    # The fake unit never registers a new lifetime, so it never reaches ready.
    assert read_receipt(directory).get("service_folder_evidence") is None
    for task in list(service.tasks.values()):
        task.cancel()
    await asyncio.gather(*service.tasks.values(), return_exceptions=True)
    service.tasks.clear()
    stopped = await service.action(service.guide.identity, directory.name, "stop")
    assert stopped["status"] == "stopped"
    manager.calls.clear()
    with pytest.raises(DraftConflict) as refused:
        await service.action(service.guide.identity, directory.name, "restart")
    assert str(refused.value) == EARLIER_REFUSAL
    assert no_manager_effect(manager)


@pytest.mark.parametrize("field,value", [
    ("interpreter", "/Users/someone/Documents/OtherForge/.venv/bin/python"),
    ("receipt_directory", "/Users/someone/Documents/OtherForge/.studio/deployments/moved"),
    ("destination", "/Users/someone/Documents/other-application"),
    ("instance_id", "an-earlier-lifetime"),
    ("label", "ai.mobkit.launch.another-unit"),
])
async def test_ready_evidence_does_not_transfer(earlier_service, field, value):
    service, manager, directory = earlier_service
    registered(directory, manager, **{field: value})
    manager.calls.clear()
    with pytest.raises(DraftConflict) as refused:
        await service.action(service.guide.identity, directory.name, "restart")
    assert str(refused.value) == EARLIER_REFUSAL
    assert no_manager_effect(manager)


async def test_installation_support_follows_the_ready_evidence(earlier_service):
    service, manager, directory = earlier_service
    registered(directory, manager)
    update_receipt(directory, status="stopped", service_cleanup_confirmed=True)
    manager.observation = {"loaded": False, "pid": None, "state": "stopped"}
    manager.calls.clear()
    try:
        await service.action(service.guide.identity, directory.name, "prepare-support",
                             action_request_id=str(uuid.uuid4()), expected_service_digest="a" * 64)
    except DraftConflict as error:
        # A synthetic digest may be refused later; never for the folder.
        assert str(error) != EARLIER_REFUSAL
    assert any(call[0] == "observe" for call in manager.calls), "Support reached the service manager."


def legacy(directory):
    # Its supervisor predates the ready record, so it can never write one.
    receipt = read_receipt(directory)
    service = {key: value for key, value in receipt["service"].items() if key != "folder_evidence_protocol"}
    update_receipt(directory, service=service)


async def test_a_new_unit_records_that_its_supervisor_writes_folder_evidence(earlier_service):
    _, _, directory = earlier_service
    assert read_receipt(directory)["service"].get("folder_evidence_protocol") == 1


async def test_a_legacy_unit_without_a_record_may_restart_and_its_status_says_access_is_unverified(earlier_service):
    # A wrong allow costs one stalled start: Stop settles and the status says why.
    service, manager, directory = earlier_service
    legacy(directory)
    registered(directory, manager, ready=False)
    status = await deployment_service.background_status(directory, manager)
    assert status.get("protected_folder") == LEGACY_NOTE
    manager.cleanup = directory
    manager.calls.clear()
    assert await attempt(service, directory, "restart") is None
    assert [call[0] for call in manager.calls if call[0] != "observe"] == ["stop", "start"]


async def test_a_legacy_unit_whose_start_then_fails_still_stops(earlier_service, monkeypatch):
    from test_deployment_service import ended_pid
    service, manager, directory = earlier_service
    monkeypatch.setattr(deployment_service, "SERVICE_TIMEOUT", 2)
    legacy(directory)
    registered(directory, manager, ready=False)
    manager.cleanup = directory
    assert await attempt(service, directory, "restart") is None
    # Its Python stalls on the folder and never registers a new lifetime.
    manager.cleanup = None
    manager.observation = {"loaded": True, "pid": ended_pid(), "state": "running"}
    stopped = await service.action(service.guide.identity, directory.name, "stop")
    assert stopped["status"] == "stopped"
