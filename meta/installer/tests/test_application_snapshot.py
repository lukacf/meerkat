import asyncio
import copy
import json
import os
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from launchkit.application_snapshot import checked_trial_source, trial_build_current, write_trial_source
from launchkit.generator import PackageFile
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict, fingerprint
from launchkit.rehearsal import RehearsalService


STATE = {"id": "guide", "version": 3, "brief": {"mission": "User outcome"}}


@pytest.fixture
def source(tmp_path, monkeypatch):
    files = [PackageFile("deployment.json", '{}\n'), PackageFile("application/__init__.py", "def build(context): return None\n"),
             PackageFile("setup.sh", "#!/bin/sh\nexit 0\n", 0o755)]
    files.sort(key=lambda item: item.path)
    status = {"build_id": "build", "guide_id": "guide", "guide_version": 3, "current": True,
              "status": "failed", "source_verified": True, "source_digest": ImplementationWorkspace._digest(files),
              "acceptance": {"status": "needs_repair"}}
    (tmp_path / "input.json").write_text(json.dumps({"brief": STATE["brief"], "blueprint": {}}))
    monkeypatch.setattr(ImplementationWorkspace, "__init__", lambda self, *args: None)
    monkeypatch.setattr(ImplementationWorkspace, "export_files", lambda self, require_verified: files)
    service = SimpleNamespace(status=AsyncMock(return_value=status), _directory=lambda *args: tmp_path)
    return service, status, files


async def test_trial_allows_checked_source_with_failed_independent_acceptance_and_keeps_exact_modes(source, tmp_path):
    service, status, files = source
    descriptor, frozen = await checked_trial_source(service, "guide", STATE, "build")
    assert descriptor["acceptance_status"] == "needs_repair"
    target = tmp_path / "trial"
    target.mkdir()
    write_trial_source(target, frozen)
    from launchkit.application_trial import source_files
    assert ImplementationWorkspace._digest(source_files(target / "application-source")) == status["source_digest"]
    assert os.stat(target / "application-source/setup.sh").st_mode & 0o777 == 0o755
    assert os.stat(target / "application-source").st_mode & 0o777 == 0o700


@pytest.mark.parametrize("changes", [{"current": False}, {"source_verified": False}, {"status": "running"},
    {"guide_version": 2}, {"build_id": "other"}, {"source_digest": None}, {"guide_id": "other"},
    {"cleanup_pending": True}])
async def test_trial_rejects_stale_mutable_or_unverified_source_before_execution(source, changes):
    service, status, _ = source
    status.update(changes)
    with pytest.raises(DraftConflict):
        await checked_trial_source(service, "guide", STATE, "build")


@pytest.mark.parametrize("terminal", ["completed", "failed", "interrupted"])
async def test_pending_cleanup_blocks_new_trial_before_source_access_and_preserves_evidence(source, terminal):
    service, status, files = source
    status.update(status=terminal, cleanup_pending=True, acceptance={"status": "pass"})
    before = copy.deepcopy(status)
    source_access = []
    directory = service._directory
    service._directory = lambda *args: source_access.append(args) or directory(*args)
    with pytest.raises(DraftConflict, match="Review cleanup in the builder"):
        await checked_trial_source(service, "guide", STATE, "build")
    assert not source_access
    assert status == before
    assert trial_build_current(status, STATE, "build")

    status["cleanup_pending"] = False
    descriptor, exported = await checked_trial_source(service, "guide", STATE, "build")
    assert descriptor["build_id"] == before["build_id"]
    assert descriptor["source_digest"] == before["source_digest"]
    assert descriptor["acceptance_status"] == "pass"
    assert exported == files
    assert source_access == [("guide", "build")]
    assert {key: value for key, value in status.items() if key != "cleanup_pending"} == {
        key: value for key, value in before.items() if key != "cleanup_pending"}


@pytest.mark.parametrize(("changes", "guidance"), [
    ({"status": "running", "cleanup_in_progress": False}, "builder is still working"),
    ({"status": "checking", "cleanup_in_progress": True}, "Follow cleanup in the builder"),
    ({"status": "completed", "cleanup_in_progress": True}, "Follow cleanup in the builder"),
    ({"status": "failed", "cleanup_in_progress": False}, "Review cleanup in the builder"),
    ({"status": "running", "cleanup_in_progress": False, "stop_requested": True}, "Follow cleanup in the builder"),
])
async def test_new_trial_rejection_explains_building_closing_or_unresolved_cleanup(source, changes, guidance):
    service, status, _ = source
    status.update(cleanup_pending=True, **changes)
    before = copy.deepcopy(status)

    def unexpected_source_access(*args):
        pytest.fail("A pending build cannot export source for a new application trial.")

    service._directory = unexpected_source_access
    with pytest.raises(DraftConflict, match=guidance):
        await checked_trial_source(service, "guide", STATE, "build")
    assert status == before


async def test_export_race_and_brief_change_invalidate_the_trial(source):
    service, status, files = source
    files.append(PackageFile("application/changed.py", "changed = True\n"))
    with pytest.raises(DraftConflict, match="source changed"):
        await checked_trial_source(service, "guide", STATE, "build")
    state = copy.deepcopy(STATE)
    state["brief"]["mission"] = "A later outcome"
    with pytest.raises(DraftConflict, match="brief changed"):
        await checked_trial_source(service, "guide", state, "build")


async def test_missing_build_and_invalid_trial_modes_never_admit_worker_work(tmp_path):
    with pytest.raises(ValueError, match="Choose a checked"):
        await checked_trial_source(None, "guide", STATE, None)
    service = RehearsalService(tmp_path, None)
    for values in [{"mode": "arbitrary"}, {"mode": "design", "build_id": "build"},
                   {"mode": "design", "application_fixtures": {}}]:
        with pytest.raises(ValueError):
            await service.submit("guide", 3, "Example", **values)
    assert not service.tasks and not service.root.exists()


async def test_current_host_projection_retains_original_build_version_for_harmless_turn(source):
    service, status, _ = source
    status["current_guide_version"] = 4
    state = {**STATE, "version": 4}
    descriptor, _ = await checked_trial_source(service, "guide", state, "build")
    assert descriptor["build_id"] == "build" and status["guide_version"] == 3
    status["current_guide_version"] = 3
    assert not trial_build_current(status, state, "build")
