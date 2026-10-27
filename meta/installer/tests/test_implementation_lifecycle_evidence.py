"""Guide lifecycle eligibility checked against offline production service controls."""
import asyncio
from copy import deepcopy
import json
from types import SimpleNamespace
import uuid

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.implementation import ImplementationService, update_receipt
from launchkit.implementation_evidence import compact_implementation_evidence, project_implementation_evidence
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict, fingerprint
from launchkit.templates.storage_setup import write_private


GATES = ("build_gate_clear", "stop_available", "retry_cleanup_available", "resume_available",
         "rebuild_available", "application_trial_gate_clear")


@pytest.fixture
async def build(tmp_path, monkeypatch):
    state = {"id": str(uuid.uuid4()), "version": 3, "brief": {"mission": "Synthetic offline lifecycle"},
             "proposal": starter_blueprint().model_dump(mode="json"), "messages": []}

    async def get(identity):
        assert identity == state["id"]
        return deepcopy(state)

    service = ImplementationService(tmp_path, SimpleNamespace(get=get))
    identity = str(uuid.uuid4())
    directory = service._directory(state["id"], identity)
    directory.mkdir(parents=True)
    frozen = {"guide_id": state["id"], "guide_version": 3, "blueprint": state["proposal"],
              "brief": state["brief"], "messages": [], "request": ""}
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", {
        "guide_id": state["id"], "guide_version": 3, "build_id": identity,
        "blueprint_hash": fingerprint(state["proposal"]), "brief_hash": fingerprint(state["brief"]),
        "status": "failed", "phase": "implementation", "progress": [], "files": [], "checks": [],
        "unresolved": [], "request": "", "ready_to_package": False,
        "stop_requested": False, "cleanup_pending": False, "cleanup_in_progress": False,
        "worker_stop_confirmed": True})
    write_private(directory.parent / "latest.json", {"build_id": identity})
    ImplementationWorkspace(directory / "workspace", state["proposal"]).seed()
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/offline-fixture-checker")
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True, "provider": "openai"})

    async def no_native_work(_directory):
        await asyncio.Event().wait()

    monkeypatch.setattr(service, "_run", no_native_work)
    yield service, directory, state
    tasks = list(service.tasks.values())
    for task in tasks:
        task.cancel()
    await asyncio.gather(*tasks, return_exceptions=True)


async def observe(build):
    service, _, state = build
    status = await service.status(state["id"], state=deepcopy(state))
    return status, project_implementation_evidence(state, status)


@pytest.mark.asyncio
@pytest.mark.parametrize("status,phase,reason", [
    ("queued", None, "queued"), ("running", "implementation", "building"),
    ("checking", "implementation", "source_check"),
    ("running", "independent_review", "independent_review"),
    ("running", "automatic_repair", "automatic_repair"),
])
async def test_exact_live_host_phase_and_stop_eligibility_survive_a_stale_brief(build, status, phase, reason):
    service, directory, state = build
    task = asyncio.create_task(asyncio.Event().wait())
    service.tasks[directory.name] = task
    update_receipt(directory, status=status, phase=phase, cleanup_pending=True, worker_stop_confirmed=False)
    state["brief"]["new_request"] = "A revised application outcome"
    state["version"] += 1
    _, evidence = await observe(build)
    lifecycle = evidence["lifecycle"]
    assert evidence["freshness"] == "stale" and not evidence["source_verified"]
    assert not evidence["acceptance_current"] and not evidence["ready_to_package"]
    assert lifecycle["availability"] == "available" and not lifecycle["content_current"]
    assert (lifecycle["status"], lifecycle["phase"], lifecycle["reason_code"]) == (status, phase, reason)
    assert lifecycle["guide_version"] == 3 and lifecycle["control_guide_version"] == 4
    assert lifecycle["stop_available"] and not lifecycle["build_gate_clear"]
    assert not lifecycle["resume_available"] and not lifecycle["application_trial_gate_clear"]
    # A stale content version does not prevent stopping the selected old worker.
    stopped = await service.request_stop(state["id"], 4, directory.name)
    assert stopped["stop_requested"]
    with pytest.raises(asyncio.CancelledError):
        await task
    service.tasks.pop(directory.name, None)


@pytest.mark.asyncio
async def test_owned_stale_cleanup_requires_real_retry_before_new_build_admission(build, monkeypatch):
    service, directory, state = build
    child = object()
    service.processes[directory.name] = child
    update_receipt(directory, status="failed", cleanup_pending=True, worker_stop_confirmed=False,
                   stop_requested=True, detail="private-worker-error-sentinel", cleanup_detail="private-cleanup-sentinel")
    state["brief"]["new_request"] = "Changed context"
    state["version"] += 1
    _, evidence = await observe(build)
    lifecycle = evidence["lifecycle"]
    assert lifecycle["retry_cleanup_available"] and not lifecycle["stop_available"]
    assert lifecycle["reason_code"] == "cleanup_retry_needed" and not lifecycle["content_current"]
    assert not lifecycle["build_gate_clear"] and not lifecycle["application_trial_gate_clear"]
    assert "private-" not in json.dumps(lifecycle)
    with pytest.raises(DraftConflict, match="cleanup is not confirmed"):
        await service.submit(state["id"], 4, expected_selected_build_id=directory.name)

    calls = []

    async def stop_owned(observed):
        assert observed is child
        calls.append(observed)

    monkeypatch.setattr(service, "_stop_process", stop_owned)
    await service.request_stop(state["id"], 4, directory.name)
    retry = service.tasks.get(directory.name)
    if retry:
        await retry
    assert calls == [child] and directory.name not in service.processes
    _, recovered = await observe(build)
    assert recovered["lifecycle"]["build_gate_clear"] and recovered["lifecycle"]["rebuild_available"]
    assert not recovered["lifecycle"]["resume_available"]  # Its saved content is still stale.
    admitted = await service.submit(state["id"], 4, expected_selected_build_id=directory.name)
    assert admitted["build_id"] != directory.name and admitted["status"] == "queued"


@pytest.mark.asyncio
async def test_restart_custody_loss_overrides_persisted_cleanup_progress_and_forbids_retry(build):
    service, directory, state = build
    update_receipt(directory, status="running", phase="independent_review", cleanup_pending=True,
                   cleanup_in_progress=True, worker_stop_confirmed=False,
                   detail="secret-provider-sentinel", native_console_url="https://private-host-sentinel")
    state["brief"]["new_request"] = "Changed context"
    state["version"] += 1
    status, evidence = await observe(build)
    assert status["status"] == "interrupted" and status["cleanup_custody_lost"]
    lifecycle = evidence["lifecycle"]
    assert lifecycle["availability"] == "available" and lifecycle["cleanup_in_progress"]
    assert lifecycle["cleanup_custody_lost"] and lifecycle["reason_code"] == "cleanup_custody_lost"
    assert not any(lifecycle[name] for name in GATES)
    with pytest.raises(DraftConflict, match="no longer owns"):
        await service.request_stop(state["id"], 4, directory.name)
    with pytest.raises(DraftConflict, match="cleanup cannot be confirmed"):
        await service.submit(state["id"], 4, expected_selected_build_id=directory.name)
    assert "sentinel" not in json.dumps(lifecycle)


@pytest.mark.asyncio
async def test_stopped_terminal_build_can_resume_after_cleanup_without_old_verification(build):
    service, directory, state = build
    update_receipt(directory, status="interrupted", stop_requested=True, cleanup_pending=False,
                   cleanup_in_progress=False, worker_stop_confirmed=True)
    _, evidence = await observe(build)
    lifecycle = evidence["lifecycle"]
    assert lifecycle["resume_available"] and lifecycle["build_gate_clear"] and lifecycle["stop_requested"]
    assert not lifecycle["stop_available"] and not lifecycle["retry_cleanup_available"]
    assert not lifecycle["application_trial_gate_clear"] and not evidence["ready_to_package"]
    admitted = await service.submit(state["id"], 3, expected_selected_build_id=directory.name)
    assert admitted["build_id"] != directory.name and not admitted["source_verified"]


@pytest.mark.asyncio
async def test_confirmed_missing_build_opens_only_the_first_build_gate_and_actual_admission(build):
    service, _, state = build
    service.root = service.root / "no-existing-builds"
    with pytest.raises(LookupError):
        await service.status(state["id"], state=deepcopy(state))
    evidence = project_implementation_evidence(state)
    lifecycle = evidence["lifecycle"]
    assert lifecycle["availability"] == "missing" and lifecycle["build_gate_clear"]
    assert not any(lifecycle[name] for name in GATES if name != "build_gate_clear")
    assert not evidence["source_verified"] and not evidence["ready_to_package"]
    admitted = await service.submit(state["id"], 3, expected_selected_build_id=None)
    assert admitted["status"] == "queued"


@pytest.mark.asyncio
@pytest.mark.parametrize("change", [
    {"status": "reviewing"}, {"status": ["running"]}, {"status": {"running": True}},
    {"phase": "private-phase-sentinel"}, {"phase": {"name": "implementation"}},
    {"build_id": "../private-path-sentinel"}, {"build_id": "x" * 181},
    {"guide_id": "another-guide"}, {"guide_version": True}, {"guide_version": 0},
    {"stop_requested": "false"}, {"cleanup_pending": 0}, {"cleanup_in_progress": None},
    {"cleanup_custody_lost": "false"}, {"worker_stop_confirmed": None},
    {"cleanup_pending": False, "cleanup_custody_lost": True},
    {"cleanup_pending": False, "cleanup_in_progress": True},
])
async def test_malformed_host_lifecycle_never_authorizes_an_action_or_copies_unknown_values(build, change):
    status, _ = await observe(build)
    status.update(change, detail="private-detail-sentinel", log="private-log-sentinel")
    before = deepcopy(status)
    lifecycle = project_implementation_evidence(build[2], status)["lifecycle"]
    assert lifecycle["availability"] == "invalid" and not any(lifecycle[name] for name in GATES)
    assert lifecycle["status"] is None and lifecycle["build_id"] is None
    assert "sentinel" not in json.dumps(lifecycle) and status == before


@pytest.mark.asyncio
@pytest.mark.parametrize("field", ["stop_requested", "cleanup_pending", "cleanup_in_progress", "cleanup_custody_lost"])
async def test_absent_ownership_flags_remain_unknown_without_a_retry_default(build, field):
    status, _ = await observe(build)
    status.pop(field)
    lifecycle = project_implementation_evidence(build[2], status)["lifecycle"]
    assert lifecycle["availability"] == "invalid" and not any(lifecycle[name] for name in GATES)


@pytest.mark.parametrize("value", [{}, [], "failed", True, 7])
def test_malformed_provider_result_is_not_a_confirmed_missing_build(value):
    state = {"id": "setup", "version": 3, "brief": {}, "proposal": starter_blueprint().model_dump(mode="json")}
    lifecycle = project_implementation_evidence(state, value)["lifecycle"]
    assert lifecycle["availability"] == "invalid" and not any(lifecycle[name] for name in GATES)


def test_unavailable_provider_cannot_borrow_a_missing_build_gate():
    state = {"id": "setup", "version": 3, "brief": {}, "proposal": starter_blueprint().model_dump(mode="json")}
    lifecycle = project_implementation_evidence(state, unavailable=True)["lifecycle"]
    assert lifecycle["availability"] == "unavailable" and not any(lifecycle[name] for name in GATES)


@pytest.mark.asyncio
async def test_compact_lifecycle_preserves_exact_scope_without_worker_prose_or_mutating_source(build):
    status, evidence = await observe(build)
    status.update(detail="private-worker-sentinel", cleanup_detail="private-cleanup-sentinel",
                  request="private-request-sentinel", worker_log="private-log-sentinel",
                  native_console_url="https://private-host-sentinel", files=[{"path": "private-path-sentinel"}],
                  phase="automatic_repair")
    before = deepcopy(status)
    projected = project_implementation_evidence(build[2], status)
    compact = compact_implementation_evidence(projected)
    assert compact["lifecycle"] == projected["lifecycle"]
    assert compact["lifecycle"]["phase"] == "automatic_repair"
    assert compact["lifecycle"]["reason_code"] == "build_failed"
    assert "sentinel" not in json.dumps(compact["lifecycle"])
    assert status == before
    for key, value in evidence.items():
        if key != "lifecycle":
            assert projected[key] == value
