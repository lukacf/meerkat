"""Metadata-only build selection recovery from retained, owned source."""
import ast
from copy import deepcopy
import json
import logging
import os
from pathlib import Path
import stat
from types import SimpleNamespace
from unittest.mock import AsyncMock
import uuid

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient
from pydantic import BaseModel, ConfigDict, Field
import pytest

from launchkit import deployment, implementation
from launchkit import implementation_selection
from launchkit.implementation import BuildSelectionUnavailable, ImplementationService, read_receipt
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates.storage_setup import write_private
from test_implementation import Guide


def disk_bytes(root):
    return {path.relative_to(root): path.read_bytes() for path in root.rglob("*") if path.is_file()}


@pytest.fixture
def saved(tmp_path, monkeypatch):
    guide = Guide()
    service = ImplementationService(tmp_path, guide)
    service.root.mkdir(mode=0o700)
    parent = service.root / guide.identity
    parent.mkdir(mode=0o700)
    build_id = str(uuid.uuid4())
    directory = service._directory(guide.identity, build_id)
    directory.mkdir(mode=0o700)
    workspace = ImplementationWorkspace(directory / "workspace", guide.state["proposal"])
    workspace.seed()
    workspace.write_file("application/recovery_fixture.py", "# retained-source-private-canary\n")
    frozen = {"guide_id": guide.identity, "guide_version": 3, "blueprint": deepcopy(guide.state["proposal"]),
        "acceptance_policy_version": implementation.ACCEPTANCE_POLICY_VERSION,
        "brief": deepcopy(guide.state["brief"]), "request": "Keep the retained application source",
        "messages": deepcopy(guide.state["messages"]), "implementation_requests": [],
        "refresh_framework": False, "preserve_design": False, "previous_implementation": None,
        "framework": workspace.framework_provenance(), "application_contract": implementation.APPLICATION_CONTRACT}
    receipt = {"guide_id": guide.identity, "guide_version": 3, "build_id": build_id,
        "blueprint_hash": fingerprint(frozen["blueprint"]), "brief_hash": fingerprint(frozen["brief"]),
        "acceptance_policy_version": implementation.ACCEPTANCE_POLICY_VERSION,
        "request": frozen["request"], "refresh_framework": False, "preserve_design": False,
        "status": "failed", "summary": "Retained failed build", "detail": "Original saved diagnostic",
        "files": workspace.list_files(), "checks": [], "unresolved": [], "ready_to_package": False,
        "native_console_url": None, "framework": frozen["framework"], "receipt": None,
        "stop_requested": False, "cleanup_pending": True, "cleanup_in_progress": False,
        "worker_stop_confirmed": False, "progress": [],
        "created_at": "2026-10-11T01:00:00+00:00", "updated_at": "2026-10-11T01:00:00+00:00"}
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", receipt)
    (directory / ".receipt.lock").touch(mode=0o600)
    def no_worker(*args, **kwargs):
        pytest.fail("Selection recovery must not admit a worker")
    monkeypatch.setattr(implementation, "spawn_owned_worker", no_worker)
    monkeypatch.setattr(ImplementationWorkspace, "seed", no_worker)
    monkeypatch.setattr(service, "status", AsyncMock(side_effect=AssertionError("Recovery must not inspect or mutate status")))
    monkeypatch.setattr(service, "_run", AsyncMock(side_effect=AssertionError("Recovery must not start a build")))
    return service, directory, workspace, frozen, receipt


async def candidate(saved):
    service, directory, workspace, frozen, receipt = saved
    result = await service.saved_selection(service.guide.identity)
    assert result["guide_id"] == service.guide.identity and result["version"] == service.guide.state["version"]
    assert len(result["candidates"]) == 1 and result["unavailable_count"] == 0
    return result["candidates"][0]


async def restore(saved, choice, **changes):
    service, directory, workspace, frozen, receipt = saved
    selected = {"version": service.guide.state["version"], "build_id": directory.name,
                "selection_token": choice["selection_token"]}
    selected.update(changes)
    return await service.restore_saved_selection(service.guide.identity, **selected)


@pytest.mark.parametrize("operation", ["write", "batch"])
async def test_nested_source_written_with_group_umask_remains_recoverable(saved, operation):
    service, directory, workspace, frozen, receipt = saved
    name = "application/new-parent/nested/retained.py"
    content = "# Retained inert source, never executed.\n"
    previous_mask = os.umask(0o002)
    try:
        if operation == "write":
            workspace.write_file(name, content)
        else:
            workspace.apply_batch([{"kind": "write", "path": name,
                                    "content": content, "expected_sha256": None}])
    finally:
        os.umask(previous_mask)
    receipt["files"] = workspace.list_files()
    write_private(directory / "receipt.json", receipt)
    before = disk_bytes(directory)
    choice = await candidate(saved)
    assert choice["build_id"] == directory.name
    result = await restore(saved, choice)
    assert result["build_id"] == directory.name
    assert workspace.read_file(name) == content
    assert disk_bytes(directory) == before


async def test_saved_selection_still_refuses_group_writable_nested_source(saved):
    service, directory, workspace, frozen, receipt = saved
    workspace.write_file("application/new-parent/nested/retained.py", "# Inert source.\n")
    parent = workspace.project / "application/new-parent"
    parent.chmod(0o770)
    result = await service.saved_selection(service.guide.identity)
    assert result["candidates"] == [] and result["unavailable_count"] == 1
    assert stat.S_IMODE(parent.stat().st_mode) == 0o770
    assert not (directory.parent / "latest.json").exists()


async def test_original_saved_source_can_be_selected_after_current_guide_changes_without_granting_cleanup(saved):
    service, directory, workspace, frozen, receipt = saved
    service.guide.state["version"] = 8
    service.guide.state["proposal"]["project"]["name"] = "A newer unbuilt design"
    service.guide.state["brief"] = {"mission": "A newer requested scope"}
    before = disk_bytes(service.root)
    choice = await candidate(saved)
    assert choice["build_id"] == directory.name and choice["guide_version"] == 3
    assert choice["created_at"] == receipt["created_at"] and choice["status"] == "failed"
    assert choice["cleanup_pending"] is True and choice["cleanup_custody_lost"] is True
    assert "retained-source-private-canary" not in json.dumps(choice)
    assert disk_bytes(service.root) == before
    result = await restore(saved, choice)
    assert result == {"guide_id": service.guide.identity, "version": 8, "build_id": directory.name,
                      "selection_restored": True}
    after = disk_bytes(service.root)
    latest = (directory.parent / "latest.json").relative_to(service.root)
    assert json.loads(after.pop(latest)) == {"build_id": directory.name}
    assert after == before
    assert (directory.parent / "latest.json").stat().st_mode & 0o777 == 0o600
    assert read_receipt(directory)["cleanup_pending"] and not read_receipt(directory)["worker_stop_confirmed"]
    assert not service.tasks and not service.processes
    service.status.assert_not_awaited()
    service._run.assert_not_awaited()


async def test_receipt_first_queued_crash_get_creates_nothing_and_restore_only_creates_original_lock_and_selection(saved):
    service, directory, workspace, frozen, receipt = saved
    write_private(directory / "receipt.json", {**receipt, "status": "queued"})
    lock = directory / ".receipt.lock"
    lock.unlink()
    before = disk_bytes(service.root)
    choice = await candidate(saved)
    assert choice["status"] == "queued" and choice["cleanup_pending"] and choice["cleanup_custody_lost"]
    assert not lock.exists() and disk_bytes(service.root) == before
    await restore(saved, choice)
    after = disk_bytes(service.root)
    latest = (directory.parent / "latest.json").relative_to(service.root)
    assert json.loads(after.pop(latest)) == {"build_id": directory.name}
    assert after.pop(lock.relative_to(service.root)) == b"" and after == before
    info = lock.stat()
    assert stat.S_ISREG(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
    assert info.st_uid == os.getuid() and info.st_nlink == 1
    original = read_receipt(directory)
    assert original["status"] == "queued" and original["cleanup_pending"] is True
    assert original["worker_stop_confirmed"] is False
    assert not service.tasks and not service.processes
    service.status.assert_not_awaited()
    service._run.assert_not_awaited()


async def test_partial_saved_directories_are_unavailable_and_do_not_hide_complete_original_source(saved):
    service, directory, workspace, frozen, receipt = saved
    partial = directory.parent / str(uuid.uuid4())
    partial.mkdir(mode=0o700)
    write_private(partial / "receipt.json", {"build_id": partial.name, "guide_id": service.guide.identity})
    before = disk_bytes(service.root)
    result = await service.saved_selection(service.guide.identity)
    assert [item["build_id"] for item in result["candidates"]] == [directory.name]
    assert result["unavailable_count"] == 1 and disk_bytes(service.root) == before
    with pytest.raises((DraftConflict, LookupError, ValueError)):
        await restore(saved, result["candidates"][0], build_id=partial.name)
    assert not (directory.parent / "latest.json").exists()


@pytest.mark.parametrize("change", ["token", "source", "receipt", "input"])
async def test_restore_refuses_changed_token_or_retained_bytes_without_rewriting_source(saved, change):
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    changes = {}
    if change == "token":
        changes["selection_token"] = "0" * 64
    elif change == "source":
        workspace.write_file("application/recovery_fixture.py", "# changed retained source\n")
    else:
        path = directory / ("receipt.json" if change == "receipt" else "input.json")
        path.write_bytes(path.read_bytes() + b"\n")
    before = disk_bytes(service.root)
    with pytest.raises((DraftConflict, ValueError)):
        await restore(saved, choice, **changes)
    assert disk_bytes(service.root) == before and not (directory.parent / "latest.json").exists()


@pytest.mark.parametrize("change", ["request-build", "receipt-build", "receipt-guide", "input-guide", "input-version", "request-uuid"])
async def test_restore_requires_original_uuid_guide_and_build_identity(saved, change):
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    changes = {}
    if change in {"request-build", "request-uuid"}:
        changes["build_id"] = str(uuid.uuid4()) if change == "request-build" else "../private-build-marker"
    else:
        path = directory / ("receipt.json" if change.startswith("receipt") else "input.json")
        value = json.loads(path.read_text())
        key = "build_id" if change == "receipt-build" else "guide_version" if change == "input-version" else "guide_id"
        value[key] = 4 if key == "guide_version" else str(uuid.uuid4())
        write_private(path, value)
    before = disk_bytes(service.root)
    with pytest.raises((DraftConflict, LookupError, ValueError)):
        await restore(saved, choice, **changes)
    assert disk_bytes(service.root) == before and not (directory.parent / "latest.json").exists()


@pytest.mark.parametrize("kind", ["valid", "corrupt", "unreadable", "symlink", "fifo", "hardlink"])
async def test_existing_latest_pointer_always_refuses_recovery(saved, monkeypatch, kind):
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    latest = directory.parent / "latest.json"
    if kind == "corrupt":
        latest.write_bytes(b"invalid private selection marker")
    elif kind == "fifo":
        os.mkfifo(latest, mode=0o600)
    elif kind in {"symlink", "hardlink"}:
        original = directory.parent / "retained-selection.json"
        write_private(original, {"build_id": directory.name})
        latest.symlink_to(original) if kind == "symlink" else os.link(original, latest)
    else:
        write_private(latest, {"build_id": directory.name})
        if kind == "unreadable":
            original_open = os.open
            def refuse_latest(path, flags, *args, **kwargs):
                if Path(path) == latest or path == latest.name:
                    raise PermissionError("private selection read denied")
                return original_open(path, flags, *args, **kwargs)
            monkeypatch.setattr(os, "open", refuse_latest)
    before = disk_bytes(service.root)
    with pytest.raises(DraftConflict):
        await service.saved_selection(service.guide.identity)
    with pytest.raises(DraftConflict):
        await restore(saved, choice)
    assert disk_bytes(service.root) == before
    assert latest.is_symlink() if kind == "symlink" else latest.exists()
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("target", ["input", "receipt", "source"])
@pytest.mark.parametrize("change", ["symlink", "fifo", "hardlink", "cross-owner"])
async def test_unsafe_saved_files_are_unavailable_and_cannot_be_selected(saved, monkeypatch, target, change):
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    path = workspace.project / "application/recovery_fixture.py" if target == "source" else directory / f"{target}.json"
    if change == "cross-owner":
        identity = path.stat()
        original_fstat = os.fstat
        def foreign_file(descriptor):
            value = original_fstat(descriptor)
            if (value.st_dev, value.st_ino) == (identity.st_dev, identity.st_ino):
                attributes = {name: getattr(value, name) for name in (
                    "st_mode", "st_uid", "st_gid", "st_nlink", "st_size", "st_dev", "st_ino",
                    "st_atime", "st_mtime", "st_ctime", "st_atime_ns", "st_mtime_ns", "st_ctime_ns")}
                return SimpleNamespace(**{**attributes, "st_uid": value.st_uid + 1})
            return value
        monkeypatch.setattr(os, "fstat", foreign_file)
    else:
        original = path.with_name(path.name + ".retained")
        path.rename(original)
        if change == "symlink":
            path.symlink_to(original)
        elif change == "fifo":
            os.mkfifo(path, mode=0o600)
        else:
            os.link(original, path)
    before = disk_bytes(service.root)
    if change == "fifo":
        original_open = os.open
        parent_identity = path.parent.stat()
        def bounded_open(name, flags, *args, **kwargs):
            parent_fd = kwargs.get("dir_fd")
            selected = Path(name) == path
            if name == path.name and parent_fd is not None:
                parent = os.fstat(parent_fd)
                selected = (parent.st_dev, parent.st_ino) == (parent_identity.st_dev, parent_identity.st_ino)
            if selected:
                assert flags & os.O_NONBLOCK and flags & os.O_NOFOLLOW
            return original_open(name, flags, *args, **kwargs)
        monkeypatch.setattr(os, "open", bounded_open)
    result = await service.saved_selection(service.guide.identity)
    assert result["candidates"] == [] and result["unavailable_count"] == 1
    with pytest.raises((DraftConflict, ValueError)):
        await restore(saved, choice)
    assert disk_bytes(service.root) == before and not (directory.parent / "latest.json").exists()
    if change == "fifo":
        assert stat.S_ISFIFO(path.lstat().st_mode)


@pytest.mark.parametrize("status", ["failed", "queued"])
async def test_restore_preserves_legacy_lost_cleanup_custody_and_normal_controls_stay_refused(saved, status):
    service, directory, workspace, frozen, receipt = saved
    value = {**receipt, "status": status, "cleanup_custody_lost": True, "stop_requested": True}
    write_private(directory / "receipt.json", value)
    choice = await candidate(saved)
    original_receipt = (directory / "receipt.json").read_bytes()
    await restore(saved, choice)
    before = disk_bytes(service.root)
    with pytest.raises(DraftConflict, match="cleanup"):
        await service.submit(service.guide.identity, 3, "Continue saved work", expected_selected_build_id=directory.name)
    with pytest.raises(DraftConflict, match="cleanup"):
        await service.request_stop(service.guide.identity, 3, directory.name)
    assert (directory / "receipt.json").read_bytes() == original_receipt and disk_bytes(service.root) == before
    original = read_receipt(directory)
    assert original["status"] == status and original["cleanup_pending"] is True
    assert original["worker_stop_confirmed"] is False and original["cleanup_custody_lost"] is True
    assert not service.tasks and not service.processes
    service.status.assert_not_awaited()
    service._run.assert_not_awaited()


async def test_restore_requires_the_current_guide_version(saved):
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    service.guide.state["version"] += 1
    before = disk_bytes(service.root)
    with pytest.raises(DraftConflict):
        await restore(saved, choice, version=3)
    assert disk_bytes(service.root) == before and not (directory.parent / "latest.json").exists()


@pytest.mark.parametrize("action", ["list", "restore"])
async def test_guide_revision_during_awaited_validation_refuses_recovery(saved, monkeypatch, action):
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    changed = False
    if action == "list":
        original_validation = deployment.await_validation
        async def changed_after_validation(function, *args, **kwargs):
            nonlocal changed
            result = await original_validation(function, *args, **kwargs)
            service.guide.state["version"] += 1
            changed = True
            return result
        monkeypatch.setattr(deployment, "await_validation", changed_after_validation)
    else:
        original_current = implementation_selection.Candidate.require_current
        def changed_during_validation(held):
            nonlocal changed
            original_current(held)
            if not changed:
                service.guide.state["version"] += 1
                changed = True
        monkeypatch.setattr(implementation_selection.Candidate, "require_current", changed_during_validation)
    before = disk_bytes(service.root)
    with pytest.raises(DraftConflict):
        if action == "list":
            await service.saved_selection(service.guide.identity)
        else:
            await restore(saved, choice)
    assert changed and disk_bytes(service.root) == before and not (directory.parent / "latest.json").exists()


async def test_concurrent_selection_publication_preserves_the_other_callers_exact_pointer(saved, monkeypatch):
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    before = disk_bytes(service.root)
    latest = directory.parent / "latest.json"
    competing = {"build_id": str(uuid.uuid4())}
    original_publish = deployment._rename_new_source
    def another_publication(stage, parent, name):
        assert name == "latest.json"
        write_private(latest, competing)
        return original_publish(stage, parent, name)
    monkeypatch.setattr(deployment, "_rename_new_source", another_publication)
    with pytest.raises(DraftConflict):
        await restore(saved, choice)
    after = disk_bytes(service.root)
    assert json.loads(after.pop(latest.relative_to(service.root))) == competing
    assert after == before and not list(directory.parent.glob(".forge-source-*"))
    assert not service.tasks and not service.processes


@pytest.fixture
def api():
    calls = []
    build_id = str(uuid.uuid4())
    class SavedBuilds:
        async def saved_selection(self, *args):
            calls.append(("saved", args))
            if args[0] == "unavailable":
                raise BuildSelectionUnavailable("The saved history cannot establish a selected build.")
            return {"guide_id": args[0], "version": 8, "candidates": [{"build_id": build_id,
                "guide_version": 3, "created_at": "2026-10-11T01:00:00+00:00", "status": "failed",
                "cleanup_pending": True, "cleanup_custody_lost": True, "selection_token": "a" * 64}],
                "unavailable_count": 1}
        async def restore_saved_selection(self, *args):
            calls.append(("restore", args))
            return {"guide_id": args[0], "version": args[1], "build_id": args[2], "selection_restored": True}
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    tree = ast.parse(source.read_text())
    selected = {"ImplementationSelectionRequest", "guide_result", "saved_implementation_selection",
                "restore_implementation_selection"}
    body = [node for node in tree.body if isinstance(node, (ast.ClassDef, ast.AsyncFunctionDef)) and node.name in selected]
    assert len(body) == 4
    app = FastAPI()
    namespace = {"app": app, "implementation": SavedBuilds(), "BaseModel": BaseModel,
        "ConfigDict": ConfigDict, "Field": Field, "HTTPException": HTTPException, "JSONResponse": JSONResponse,
        "DraftConflict": DraftConflict, "BuildSelectionUnavailable": BuildSelectionUnavailable,
        "ArchitectUnavailable": type("Unavailable", (RuntimeError,), {}),
        "InstalledFeedbackNotAdmitted": type("FeedbackNotAdmitted", (RuntimeError,), {}),
        "InstalledSuccessNotAdmitted": type("SuccessNotAdmitted", (RuntimeError,), {}), "LOG": logging.getLogger(__name__)}
    exec(compile(ast.Module(body=body, type_ignores=[]), str(source), "exec"), namespace)
    return TestClient(app), calls, build_id


def test_saved_selection_api_forwards_exact_current_version_original_build_and_token(api):
    client, calls, build_id = api
    path = "/api/guide/owned/implementation/saved-selection"
    response = client.get(path)
    assert response.status_code == 200 and response.json()["unavailable_count"] == 1
    choice = response.json()["candidates"][0]
    assert choice["cleanup_pending"] and choice["cleanup_custody_lost"]
    response = client.post(path, json={"version": 8, "build_id": build_id, "selection_token": choice["selection_token"]})
    assert response.status_code == 200
    assert response.json() == {"guide_id": "owned", "version": 8, "build_id": build_id, "selection_restored": True}
    assert calls == [("saved", ("owned",)), ("restore", ("owned", 8, build_id, "a" * 64))]


def test_saved_selection_api_refuses_incomplete_or_ambiguous_restoration_before_delegation(api):
    client, calls, build_id = api
    body = {"version": 8, "build_id": build_id, "selection_token": "a" * 64}
    for changed in ({"version": True}, {"version": "8"}, {"build_id": "bad"},
                    {"selection_token": "unbound"}, {"force": True}):
        response = client.post("/api/guide/owned/implementation/saved-selection", json={**body, **changed})
        assert response.status_code == 422
    for missing in body:
        response = client.post("/api/guide/owned/implementation/saved-selection",
            json={key: value for key, value in body.items() if key != missing})
        assert response.status_code == 422
    assert calls == []


def test_saved_selection_api_reports_unavailable_selection_as_recoverable_conflict(api):
    client, calls, build_id = api
    response = client.get("/api/guide/unavailable/implementation/saved-selection")
    assert response.status_code == 409
    assert response.json() == {"detail": "The saved history cannot establish a selected build.",
                               "code": "build_selection_unavailable"}
    assert calls == [("saved", ("unavailable",))]


@pytest.mark.parametrize("status", ["queued", "running", "checking"])
@pytest.mark.parametrize("task_state", ["absent", "done"])
async def test_recovered_unrequested_stop_without_cleanup_custody_refuses_before_prerequisites_or_source_work(saved, monkeypatch, status, task_state):
    service, directory, workspace, frozen, receipt = saved
    value = {**receipt, "status": status, "stop_requested": False,
             "cleanup_pending": True, "cleanup_in_progress": False, "worker_stop_confirmed": False}
    write_private(directory / "receipt.json", value)
    choice = await candidate(saved)
    assert choice["status"] == status and choice["cleanup_pending"] and choice["cleanup_custody_lost"]
    await restore(saved, choice)
    assert not service.tasks and not service.processes
    if task_state == "done":
        service.tasks[directory.name] = SimpleNamespace(done=lambda: True)
    original_tasks = service.tasks.copy()
    original_receipt = (directory / "receipt.json").read_bytes()
    before = disk_bytes(service.root)
    def no_prerequisite_or_source_work(*args, **kwargs):
        pytest.fail("Lost cleanup custody must refuse before prerequisites or retained source work")
    monkeypatch.setattr("launchkit.compiler.package_availability", no_prerequisite_or_source_work)
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", no_prerequisite_or_source_work)
    monkeypatch.setattr("launchkit.planner.planner_capability", no_prerequisite_or_source_work)
    monkeypatch.setattr(service, "_retained_runtime_candidate", no_prerequisite_or_source_work)
    monkeypatch.setattr(service, "_accepted_framework", no_prerequisite_or_source_work)
    with pytest.raises(DraftConflict, match="cleanup"):
        await service.submit(service.guide.identity, service.guide.state["version"], "Continue the recovered build",
                             expected_selected_build_id=directory.name)
    assert (directory / "receipt.json").read_bytes() == original_receipt and disk_bytes(service.root) == before
    original = read_receipt(directory)
    assert original["status"] == status and original["stop_requested"] is False
    assert original["cleanup_pending"] is True and original["worker_stop_confirmed"] is False
    assert service.tasks == original_tasks and not service.processes
    service.status.assert_not_awaited()
    service._run.assert_not_awaited()


async def test_retained_live_task_same_request_returns_original_build_without_new_work(saved, monkeypatch):
    service, directory, workspace, frozen, receipt = saved
    value = {**receipt, "status": "queued", "stop_requested": False,
             "cleanup_pending": True, "cleanup_in_progress": False, "worker_stop_confirmed": False}
    write_private(directory / "receipt.json", value)
    await restore(saved, await candidate(saved))
    live_task = SimpleNamespace(done=lambda: False)
    service.tasks[directory.name] = live_task
    before = disk_bytes(service.root)
    expected = {**value, "cleanup_custody_lost": False}
    monkeypatch.setattr(service, "status", AsyncMock(return_value=expected))
    monkeypatch.setattr("launchkit.compiler.package_availability", lambda proposal: {"available": True})
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "inert-sandbox")
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True})
    retained_reads = []
    def inert_retained_candidate(original_directory, original_input):
        assert original_directory == directory and original_input == frozen
        retained_reads.append(original_directory)
        return None
    def no_new_framework(*args, **kwargs):
        pytest.fail("Retrying a live task must not prepare another framework")
    monkeypatch.setattr(service, "_retained_runtime_candidate", inert_retained_candidate)
    monkeypatch.setattr(service, "_accepted_framework", no_new_framework)
    result = await service.submit(service.guide.identity, service.guide.state["version"], frozen["request"],
                                  expected_selected_build_id=directory.name)
    assert result is expected and result["build_id"] == directory.name
    assert retained_reads == [directory] and disk_bytes(service.root) == before
    assert service.tasks == {directory.name: live_task} and not service.processes
    service.status.assert_awaited_once_with(service.guide.identity)
    service._run.assert_not_awaited()


async def test_recovery_releases_its_locks_when_the_loop_waits_for_them(saved, monkeypatch):
    # The recovery thread checks the guide on the event loop while it holds
    # the build's selection locks. If the loop is itself waiting for one of
    # those locks, the thread must give up within a bound and release them.
    import fcntl
    import time
    service, directory, workspace, frozen, receipt = saved
    choice = await candidate(saved)
    monkeypatch.setattr(implementation, "SELECTION_FENCE_TIMEOUT", 0.5, raising=False)
    original, calls, observed = service._guide, [], {}

    async def guide(identity):
        calls.append(identity)
        if len(calls) > 1 and not observed:
            # A synchronous on-loop wait for the receipt lock, bounded here
            # so that a wedge fails this row instead of hanging it.
            descriptor = os.open(directory / ".receipt.lock", os.O_RDWR)
            try:
                deadline = time.monotonic() + 5
                while not observed:
                    try:
                        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                        observed["acquired"] = True
                    except BlockingIOError:
                        if time.monotonic() > deadline:
                            observed["acquired"] = False
                        time.sleep(0.05)
            finally:
                os.close(descriptor)
        return await original(identity)

    monkeypatch.setattr(service, "_guide", guide)
    with pytest.raises(DraftConflict):
        await restore(saved, choice)
    assert observed == {"acquired": True}
    assert not (directory.parent / "latest.json").exists()
