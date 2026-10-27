from meta_trial_fixtures import selected, meta_trial_runtime
import asyncio
import copy
import json
import os
import stat
from pathlib import Path
from types import SimpleNamespace
import tomllib
from unittest.mock import AsyncMock
import uuid

import pytest

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.compiler import mob_toml, resolved_blueprint
from launchkit.implementation import read_receipt, update_receipt
from launchkit.projects import DraftConflict
from launchkit.rehearsal import (RehearsalService, masked_capabilities, trace_cursor,
                                StartNotAdmitted, start_request_fingerprint, validate_trial, worker_environment)
from launchkit.rehearsal_worker import FixtureTools, NativeTrace, rehearsal_mob_toml
from launchkit.templates.storage_setup import write_private


class Guide:
    def __init__(self):
        self.identity = str(uuid.uuid4())
        self.state = {"id": self.identity, "version": 3,
                      "proposal": starter_blueprint().model_dump(mode="json"),
                      "brief": {"facts": {}, "requirements": []}}

    async def get(self, identity):
        if identity != self.identity:
            raise LookupError("Unknown setup")
        return copy.deepcopy(self.state)


class WaitingService(RehearsalService):
    async def _run(self, directory):
        await asyncio.Event().wait()


@pytest.fixture
def service(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-provider-key")
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": True})
    return WaitingService(tmp_path, Guide())


async def submit(service, **values):
    return await service.submit(service.guide.identity, service.guide.state["version"],
                                values.pop("text", "Compare the sample evidence with your domain peers."), **values)


class CompletedFixtureService(RehearsalService):
    """Real host admission tasks, bounded fixture work, no native/provider calls."""
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.admitted = []
        self.drop_response = False
        self.wait_next = False

    async def _run(self, directory):
        frozen = json.loads((directory / "input.json").read_text())
        self.admitted.append((directory.name, copy.deepcopy(frozen)))
        if self.wait_next:
            await asyncio.Event().wait()
        update_receipt(directory, status="completed", worker_stop_confirmed=True,
                       application_stop_confirmed=True, trace_complete=True,
                       detail="The isolated fixture worker finished.")
        self.tasks.pop(directory.name, None)

    async def status(self, guide_id, rehearsal_id=None, **kwargs):
        if self.drop_response:
            self.drop_response = False
            await self.tasks[rehearsal_id]
            raise ConnectionError("Fixture response lost after admission and terminal write")
        return await super().status(guide_id, rehearsal_id, **kwargs)


async def test_lost_start_response_replays_terminal_action_without_repeating_fixture_work(service):
    completed = CompletedFixtureService(service.root.parent, service.guide)
    action = str(uuid.uuid4())
    fixtures = {"files": {"job.txt": "The original isolated fixture"}, "notes": []}
    completed.drop_response = True
    with pytest.raises(ConnectionError, match="response lost"):
        await submit(completed, text="  Original job  ", fixtures=fixtures, start_request_id=action)
    directory = completed._directory(completed.guide.identity, action)
    before = {name: (directory / name).read_bytes() for name in ("input.json", "receipt.json")}
    latest = (directory.parent / "latest.json").read_bytes()
    result = await submit(completed, text="  Original job  ", fixtures=fixtures, start_request_id=action)
    assert result["rehearsal_id"] == result["start_request_id"] == action
    assert result["status"] == "completed" and result["start_request_complete"] is True
    assert result["worker_stop_confirmed"] is True and not result["live_ready"]
    assert completed.admitted[0][1]["fixtures"] == fixtures and len(completed.admitted) == 1
    assert not completed.tasks and (directory.parent / "latest.json").read_bytes() == latest
    assert {name: (directory / name).read_bytes() for name in before} == before
    assert len(result["start_request_fingerprint"]) == 64


async def test_exact_rejected_start_is_durable_and_never_admitted_after_host_recovers(service, monkeypatch):
    from launchkit.architect import ArchitectUnavailable
    action = str(uuid.uuid4())
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": False, "reason": "Host unavailable"})
    with pytest.raises(ArchitectUnavailable) as first:
        await submit(service, text="Exact unadmitted job", start_request_id=action)
    witness = first.value.trial_start_rejection
    assert isinstance(witness, StartNotAdmitted)
    assert witness.start_request_complete is False and witness.status == "rejected"
    assert witness.start_request_fingerprint == start_request_fingerprint(service.guide.identity, 3, "Exact unadmitted job")
    path = service._rejected_start_path(service.guide.identity, action)
    before = path.read_bytes()
    assert b"Exact unadmitted job" not in before and b"Host unavailable" not in before
    assert not service._directory(service.guide.identity, action).exists()
    assert not (service.root / service.guide.identity).exists() and not service.tasks and not service.processes
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": True})
    recovered = WaitingService(service.root.parent, service.guide)
    with pytest.raises(ArchitectUnavailable) as repeated:
        await submit(recovered, text="Exact unadmitted job", start_request_id=action)
    assert repeated.value.trial_start_rejection == witness and path.read_bytes() == before
    with pytest.raises(DraftConflict) as changed:
        await submit(recovered, text="Corrected job", start_request_id=action)
    assert not hasattr(changed.value, "trial_start_rejection") and path.read_bytes() == before
    assert not recovered.tasks and not service._directory(service.guide.identity, action).exists()
    new_action = str(uuid.uuid4())
    admitted = await submit(recovered, text="Corrected job", start_request_id=new_action)
    assert admitted["start_request_complete"] is True and admitted["rehearsal_id"] == new_action
    assert path.read_bytes() == before
    await recovered.cancel(service.guide.identity, new_action)


async def test_input_rejection_preserves_prior_receipt_and_latest_without_a_trial_directory(service):
    first = await submit(service, start_request_id=str(uuid.uuid4()))
    directory = service._directory(service.guide.identity, first["rehearsal_id"])
    original = {path: path.read_bytes() for path in [directory / "input.json", directory / "receipt.json", directory.parent / "latest.json"]}
    rejected_id = str(uuid.uuid4())
    with pytest.raises(ValueError, match="200 KB") as rejected:
        await submit(service, fixtures={"files": {"sample-context.txt": "界" * 80_000}}, start_request_id=rejected_id)
    assert rejected.value.trial_start_rejection.start_request_id == rejected_id
    assert {path: path.read_bytes() for path in original} == original
    assert list(service.tasks) == [first["rehearsal_id"]]
    assert not service._directory(service.guide.identity, rejected_id).exists()
    await service.cancel(service.guide.identity, first["rehearsal_id"])


async def test_rejection_ledger_write_failure_stays_ambiguous_and_does_not_mutate_trial_state(service, monkeypatch):
    action = str(uuid.uuid4())
    def denied(*args):
        raise OSError("Fixture disk unavailable")
    monkeypatch.setattr("launchkit.rehearsal.write_private", denied)
    with pytest.raises(ValueError, match="200 KB") as failure:
        await submit(service, fixtures={"files": {"sample-context.txt": "界" * 80_000}}, start_request_id=action)
    assert not hasattr(failure.value, "trial_start_rejection")
    assert not service._rejected_start_path(service.guide.identity, action).exists()
    assert not (service.root / service.guide.identity).exists() and not service.tasks


async def test_leftover_rejection_file_requires_successful_durable_replay_before_a_witness(service, monkeypatch):
    from launchkit.architect import ArchitectUnavailable
    action = str(uuid.uuid4())
    real_fsync = os.fsync
    flushes = []
    def unavailable_directory_flush(descriptor):
        directory = stat.S_ISDIR(os.fstat(descriptor).st_mode)
        flushes.append(directory)
        if directory:
            raise OSError("Fixture failure after atomic replacement, before directory durability")
        return real_fsync(descriptor)
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": False, "reason": "Fixture host unavailable"})
    monkeypatch.setattr("launchkit.rehearsal.os.fsync", unavailable_directory_flush)
    with pytest.raises(ArchitectUnavailable) as initial:
        await submit(service, text="Public synthetic unchanged request", start_request_id=action)
    assert not hasattr(initial.value, "trial_start_rejection")
    path = service._rejected_start_path(service.guide.identity, action)
    before = path.read_bytes()
    assert flushes == [False, True] and json.loads(before)["start_request_id"] == action
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": True})
    restarted = WaitingService(service.root.parent, service.guide)
    for _ in range(2):
        with pytest.raises(DraftConflict, match="unverified durability") as replay:
            await submit(restarted, text="Public synthetic unchanged request", start_request_id=action)
        assert not hasattr(replay.value, "trial_start_rejection")
        assert path.read_bytes() == before and not restarted.tasks
        assert not restarted._directory(service.guide.identity, action).exists()
    assert flushes == [False, True, True, True]
    durable_flushes = []
    def available_flush(descriptor):
        durable_flushes.append(stat.S_ISDIR(os.fstat(descriptor).st_mode))
        return real_fsync(descriptor)
    monkeypatch.setattr("launchkit.rehearsal.os.fsync", available_flush)
    with pytest.raises(ArchitectUnavailable) as recovered:
        await submit(restarted, text="Public synthetic unchanged request", start_request_id=action)
    assert recovered.value.trial_start_rejection == StartNotAdmitted(**json.loads(before))
    assert durable_flushes == [True] * 4
    assert path.read_bytes() == before and not restarted.tasks
    assert not (service.root / service.guide.identity).exists()


async def test_post_reservation_failure_never_claims_non_admission_and_existing_reservation_wins(service, monkeypatch):
    action = str(uuid.uuid4())
    original_write = write_private
    def fail_reserved_input(path, value):
        if path.name == "input.json":
            raise ValueError("Fixture failure after reservation")
        original_write(path, value)
    monkeypatch.setattr("launchkit.rehearsal.write_private", fail_reserved_input)
    with pytest.raises(ValueError, match="after reservation") as failure:
        await submit(service, start_request_id=action)
    assert not hasattr(failure.value, "trial_start_rejection")
    assert service._directory(service.guide.identity, action).exists()
    assert not service._rejected_start_path(service.guide.identity, action).exists()
    with pytest.raises(DraftConflict) as replay:
        await submit(service, start_request_id=action)
    assert not hasattr(replay.value, "trial_start_rejection") and not service.tasks


async def test_rejected_action_bound_never_forgets_old_fences_or_admits_another_key(service, monkeypatch):
    monkeypatch.setattr("launchkit.rehearsal.MAX_REJECTED_STARTS", 1)
    action = str(uuid.uuid4())
    with pytest.raises(ValueError):
        await submit(service, text="x" * 6001, start_request_id=action)
    path = service._rejected_start_path(service.guide.identity, action)
    before = path.read_bytes()
    with pytest.raises(DraftConflict, match="history is full") as full:
        await submit(service, start_request_id=str(uuid.uuid4()))
    assert not hasattr(full.value, "trial_start_rejection")
    with pytest.raises(ValueError) as repeat:
        await submit(service, text="x" * 6001, start_request_id=action)
    assert repeat.value.trial_start_rejection.start_request_id == action
    assert path.read_bytes() == before and not service.tasks
    assert not (service.root / service.guide.identity).exists()


async def test_replay_after_current_guide_changes_keeps_original_history_and_new_latest(service, monkeypatch):
    completed = CompletedFixtureService(service.root.parent, service.guide)
    action = str(uuid.uuid4())
    first = await submit(completed, start_request_id=action)
    await completed.tasks[action]
    completed.guide.state["version"] += 1
    completed.guide.state["brief"]["requirements"].append({"id": "new", "outcome": "Changed requirements"})
    completed.wait_next = True
    second = await submit(completed, text="A distinct latest trial", start_request_id=str(uuid.uuid4()))
    directory = completed._directory(first["guide_id"], action)
    original = (directory / "input.json").read_bytes()
    latest = (directory.parent / "latest.json").read_bytes()
    tasks = dict(completed.tasks)
    monkeypatch.delenv("OPENAI_API_KEY")
    completed.guide.state["operation"] = {"status": "running"}
    replay = await completed.submit(first["guide_id"], 3,
        "Compare the sample evidence with your domain peers.", start_request_id=action)
    assert replay["rehearsal_id"] == action and replay["status"] == "completed"
    assert replay["current"] is False and replay["current_guide_version"] is None
    assert (directory.parent / "latest.json").read_bytes() == latest
    assert json.loads(latest)["rehearsal_id"] == second["rehearsal_id"]
    assert completed.tasks == tasks and (directory / "input.json").read_bytes() == original
    await completed.cancel(second["guide_id"], second["rehearsal_id"])


async def test_application_replay_uses_frozen_source_after_selected_build_changes(service, monkeypatch):
    from launchkit.generator import PackageFile
    source = {"build_id": "build-a", "source_digest": "a" * 64, "guide_version": 3}
    checked = AsyncMock(return_value=(source, [PackageFile("application/__init__.py", "fixture = True\n")]))
    monkeypatch.setattr("launchkit.application_snapshot.checked_trial_source", checked)
    build = {"guide_id": service.guide.identity, "guide_version": 3, "current": True,
             "source_verified": True, "status": "completed", "build_id": "build-a", "source_digest": "a" * 64}
    implementation = SimpleNamespace(status=AsyncMock(return_value=build))
    completed = CompletedFixtureService(service.root.parent, service.guide, implementation=implementation)
    action = str(uuid.uuid4())
    args = {"mode": "application", "build_id": "build-a", "application_fixtures": {"items": [1]},
            "keep_open": True, "start_request_id": action}
    first = await submit(completed, **args)
    await completed.tasks[action]
    directory = completed._directory(first["guide_id"], action)
    original = (directory / "input.json").read_bytes()
    checks = checked.await_count
    build.update(build_id="build-b", source_digest="b" * 64, status="failed")
    replay = await submit(completed, **args)
    assert replay["application_source"] == source and replay["current"] is False
    assert replay["start_request_complete"] is True and replay["rehearsal_id"] == action
    assert len(completed.admitted) == 1 and checked.await_count == checks
    assert (directory / "input.json").read_bytes() == original


@pytest.mark.parametrize("change", [{"version": 4}, {"text": "Original job "}, {"mode": "application"},
    {"flow_id": "other"}, {"params": {}}, {"fixtures": {}}, {"build_id": "other"},
    {"application_fixtures": {}}, {"keep_open": True}])
async def test_start_action_payload_changes_conflict_before_new_admission(service, change):
    action = str(uuid.uuid4())
    first = await submit(service, text="Original job", start_request_id=action)
    directory = service._directory(first["guide_id"], action)
    before = (directory / "input.json").read_bytes(), (directory / "receipt.json").read_bytes()
    body = {"version": 3, "text": "Original job", **change}
    with pytest.raises(DraftConflict, match="different or incomplete"):
        await service.submit(first["guide_id"], body.pop("version"), body.pop("text"), start_request_id=action, **body)
    assert len(service.tasks) == 1
    assert ((directory / "input.json").read_bytes(), (directory / "receipt.json").read_bytes()) == before
    await service.cancel(first["guide_id"], action)


def test_start_fingerprint_preserves_json_types_full_payload_and_input():
    body = {"params": {"nested": {"value": 1}}, "fixtures": {"notes": ["one", "two"]},
            "application_fixtures": {"items": [False, 1.0]}, "text": "Job"}
    before = copy.deepcopy(body)
    original = start_request_fingerprint("guide", 3, **body)
    assert body == before
    for value in (True, 1.0, "1"):
        changed = copy.deepcopy(body)
        changed["params"]["nested"]["value"] = value
        assert start_request_fingerprint("guide", 3, **changed) != original
    assert start_request_fingerprint("other-guide", 3, **body) != original
    assert start_request_fingerprint("guide", 3, **{**body, "text": " Job"}) != original
    reordered = {key: body[key] for key in reversed(body)}
    assert start_request_fingerprint("guide", 3, **reordered) == original
    with pytest.raises(ValueError, match="finite JSON"):
        start_request_fingerprint("guide", 3, "Job", params={"private": float("nan")})


async def test_concurrent_same_action_replays_one_admission_and_new_action_is_distinct(service):
    action = str(uuid.uuid4())
    first, replay = await asyncio.gather(submit(service, start_request_id=action), submit(service, start_request_id=action))
    assert first["rehearsal_id"] == replay["rehearsal_id"] == action and len(service.tasks) == 1
    with pytest.raises(DraftConflict, match="already running"):
        await submit(service, start_request_id=str(uuid.uuid4()))
    await service.cancel(first["guide_id"], action)
    fresh = str(uuid.uuid4())
    second = await submit(service, start_request_id=fresh)
    assert second["rehearsal_id"] == fresh != action
    assert service._directory(first["guide_id"], action).exists()
    await service.cancel(second["guide_id"], fresh)


async def test_partial_start_reservation_is_unavailable_without_latest_or_native_work(service):
    previous = await submit(service)
    await service.cancel(previous["guide_id"], previous["rehearsal_id"])
    previous_latest = (service.root / service.guide.identity / "latest.json").read_bytes()
    action = str(uuid.uuid4())
    original = service.guide.get
    reads = 0
    async def changed(identity):
        nonlocal reads
        reads += 1
        if reads == 2:
            service.guide.state["version"] += 1
            service.guide.state["brief"]["requirements"].append({"id": "changed", "outcome": "Different plan"})
        return await original(identity)
    service.guide.get = changed
    with pytest.raises(DraftConflict, match="before rehearsal admission"):
        await service.submit(service.guide.identity, 3, "Original job", keep_open=True, start_request_id=action)
    directory = service._directory(service.guide.identity, action)
    frozen = (directory / "input.json").read_bytes()
    result = await service.submit(service.guide.identity, 3, "Original job", keep_open=True, start_request_id=action)
    assert result["status"] == "unavailable" and result["start_request_complete"] is False
    assert result["worker_stop_confirmed"] is None and result["native_admission"] is None
    assert result["current"] is False and not result["live_ready"]
    assert not service.tasks and (directory.parent / "latest.json").read_bytes() == previous_latest
    assert (directory / "input.json").read_bytes() == frozen


@pytest.mark.parametrize("action", ["", "not-a-uuid", "../escape", True, 12,
    "D8377671-ECCC-4F39-937A-4108A5E43594", "d8377671eccc4f39937a4108a5e43594"])
async def test_invalid_start_action_never_creates_or_admits_a_trial(service, action):
    with pytest.raises(ValueError, match="canonical UUID"):
        await submit(service, start_request_id=action)
    assert not service.tasks and not service.root.exists()


async def test_incomplete_or_legacy_action_directory_cannot_be_reused(service):
    action = str(uuid.uuid4())
    directory = service._directory(service.guide.identity, action)
    directory.mkdir(parents=True)
    for value in (None, {"guide_id": service.guide.identity, "guide_version": 3}):
        if value:
            write_private(directory / "input.json", value)
        before = {path.name: path.read_bytes() for path in directory.iterdir()}
        with pytest.raises(DraftConflict, match="incomplete saved input"):
            await submit(service, start_request_id=action)
        assert {path.name: path.read_bytes() for path in directory.iterdir()} == before
        assert not service.tasks and not (directory.parent / "latest.json").exists()


async def test_keyed_action_does_not_adopt_a_preexisting_legacy_trial(service):
    legacy = await submit(service)
    directory = service._directory(legacy["guide_id"], legacy["rehearsal_id"])
    original = (directory / "input.json").read_bytes(), (directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict, match="different or incomplete"):
        await submit(service, start_request_id=legacy["rehearsal_id"])
    assert ((directory / "input.json").read_bytes(), (directory / "receipt.json").read_bytes()) == original
    assert len(service.tasks) == 1
    await service.cancel(legacy["guide_id"], legacy["rehearsal_id"])


async def test_keyed_request_captures_nested_input_before_awaited_guide_reads(service):
    action = str(uuid.uuid4())
    fixtures = {"files": {"job.txt": "Original context"}, "notes": []}
    original = copy.deepcopy(fixtures)
    read = service.guide.get
    async def changed(identity):
        fixtures["files"]["job.txt"] = "Changed after the request was captured"
        return await read(identity)
    service.guide.get = changed
    result = await submit(service, fixtures=fixtures, start_request_id=action)
    frozen = json.loads((service._directory(result["guide_id"], action) / "input.json").read_text())
    assert frozen["fixtures"] == original and fixtures != original
    replay = await submit(service, fixtures=original, start_request_id=action)
    assert replay["rehearsal_id"] == action and len(service.tasks) == 1
    await service.cancel(result["guide_id"], action)


async def test_submission_freezes_actual_roles_graph_models_and_bounded_fixtures(service):
    fixtures = {"files": {"observations/night.txt": "Clouds cover the western sky."}, "notes": ["A user-supplied observation"]}
    result = await submit(service, fixtures=fixtures)
    root = service._directory(result["guide_id"], result["rehearsal_id"])
    frozen = json.loads((root / "input.json").read_text())
    assert frozen["blueprint"] == service.guide.state["proposal"]
    assert frozen["fixtures"] == fixtures and frozen["guide_version"] == 3
    assert result["identities"] == resolved_blueprint(frozen["blueprint"])["identities"]
    assert result["edges"] == resolved_blueprint(frozen["blueprint"])["edges"]
    assert result["current"] and not result["live_ready"] and not result["connected"]
    assert "Not full application" in result["scope"]
    fixtures["files"].clear()
    assert json.loads((root / "input.json").read_text())["fixtures"]["files"]
    assert os.stat(root / "input.json").st_mode & 0o777 == 0o600
    await service.shutdown()


async def test_versions_operation_fences_and_single_active_trial_precede_worker_admission(service):
    with pytest.raises(DraftConflict, match="changed"):
        await service.submit(service.guide.identity, 2, "Example")
    service.guide.state["operation"] = {"status": "running"}
    with pytest.raises(DraftConflict, match="current revision"):
        await submit(service)
    service.guide.state.pop("operation")
    first = await submit(service)
    with pytest.raises(DraftConflict, match="already running"):
        await submit(service, text="Another trial")
    assert len(service.tasks) == 1
    await service.cancel(first["guide_id"], first["rehearsal_id"])


@pytest.mark.parametrize("changed", ["proposal", "brief"])
async def test_old_native_trace_never_counts_for_changed_specification(service, changed):
    first = await submit(service)
    if changed == "version":
        service.guide.state["version"] += 1
    elif changed == "proposal":
        service.guide.state["proposal"]["project"]["name"] = "Revised design"
    else:
        service.guide.state["brief"]["requirements"].append({"id": "custom-ui", "outcome": "Add a second channel"})
    result = await service.status(first["guide_id"], first["rehearsal_id"])
    assert not result["current"] and not result["live_ready"]
    await service.shutdown()


@pytest.mark.parametrize("fixtures", [
    {"files": {"../host.txt": "x"}}, {"files": {"/host.txt": "x"}},
    {"files": {".env": "x"}}, {"files": {"data/.hidden": "x"}},
    {"files": {"a/./b": "x"}}, {"files": {"a//b": "x"}},
    {"files": {"a\\b": "x"}}, {"files": {"a": "x", "a/b": "x"}},
    {"files": {"x": "x" * 200001}}, {"files": {str(i): "x" for i in range(33)}},
    {"files": {str(i): "x" * 200000 for i in range(3)}},
    {"notes": ["x"] * 33}, {"notes": ["x" * 10001]}, {"notes": [""]},
    {"shell": "arbitrary command"}, {"files": {"x": b"binary"}},
])
async def test_invalid_fixtures_are_rejected_before_paid_worker_admission(service, fixtures):
    with pytest.raises(ValueError):
        await submit(service, fixtures=fixtures)
    assert not service.tasks and not service.root.exists()


async def test_missing_selected_provider_does_not_silently_replace_the_proposed_model(service, monkeypatch):
    monkeypatch.delenv("OPENAI_API_KEY")
    service.guide.state["proposal"]["agent_classes"][0]["model"] = "gpt-fixture-different-model"
    from launchkit.architect import ArchitectUnavailable
    with pytest.raises(ArchitectUnavailable, match="selected model providers"):
        await submit(service)
    assert not service.tasks


async def test_actual_flow_selection_preserves_params_and_default_sample_is_explicit(service):
    bp = service.guide.state["proposal"]
    bp["flows"] = [{"id": "compare", "name": "Compare observations", "steps": [
        {"id": "read", "agent": bp["topology"]["entrypoint"], "prompt": "Compare the provided observations."}]}]
    with pytest.raises(ValueError, match="actual flow"):
        await submit(service, flow_id="invented")
    with pytest.raises(ValueError, match="require a selected flow"):
        await submit(service, params={"sample": "x"})
    result = await submit(service, flow_id="compare", params={"observations": ["Clouds", "Moonlight"]})
    assert result["flow_id"] == "compare" and result["params"] == {"observations": ["Clouds", "Moonlight"]}
    await service.cancel(result["guide_id"], result["rehearsal_id"])
    result = await submit(service, flow_id="compare", text="The user's edited sample")
    assert result["params"] == {"sample": "The user's edited sample"}
    await service.shutdown()


async def test_cancel_and_shutdown_before_worker_first_instruction_leave_terminal_receipts(service):
    first = await submit(service)
    result = await service.cancel(first["guide_id"], first["rehearsal_id"])
    assert result["status"] == "interrupted" and not service.tasks
    second = await submit(service)
    await service.shutdown()
    assert read_receipt(service._directory(second["guide_id"], second["rehearsal_id"]))["status"] == "interrupted"
    assert not service.tasks


async def test_project_scope_and_uuid_paths_do_not_read_another_trial(service):
    first = await submit(service)
    for identity in ("../escape", "not-an-id", str(uuid.uuid4())):
        with pytest.raises(LookupError):
            await service.status(identity, first["rehearsal_id"])
    with pytest.raises(LookupError):
        await service.status(first["guide_id"], "../../escape")
    await service.shutdown()


def test_native_definition_keeps_exact_skills_members_profiles_and_flow_dependencies():
    spec = starter_blueprint().model_dump(mode="json")
    first = spec["agent_classes"][0]
    first.update(instances=2, members=[
        {"key": "alice", "name": "Alice", "instructions": "Own Alice's private context and preferences."},
        {"key": "bob", "name": "Bob", "instructions": "Own Bob's private context and preferences."}])
    spec["topology"]["entrypoint"] = first["id"] + ":alice"
    first["tools"].update(builtins=True, shell=True, schedule=True, workgraph=True)
    spec["flows"] = [{"id": "compare", "name": "Compare", "steps": [
        {"id": "first", "agent": first["id"] + ":alice", "prompt": "Read this person's sample."},
        {"id": "second", "agent": first["id"] + ":bob", "prompt": "Review this person's sample.", "depends_on": ["first"]}]}]
    spec = Blueprint.model_validate(spec)
    original = tomllib.loads(mob_toml(spec))
    trial = tomllib.loads(rehearsal_mob_toml(spec))
    assert trial["skills"] == original["skills"] and trial["flows"] == original["flows"]
    assert set(trial["profiles"]) == set(original["profiles"])
    for name, profile in original["profiles"].items():
        for key in ("model", "provider", "skills", "peer_description", "runtime_mode"):
            assert trial["profiles"][name][key] == profile[key]
        tools = trial["profiles"][name]["tools"]
        assert tools["comms"] and tools["memory"] == profile["tools"]["memory"]
        assert all(not tools[key] for key in ("mob", "builtins", "shell", "workgraph", "schedule"))
    assert masked_capabilities(spec)[0]["masked"]


def test_trial_environment_excludes_user_folder_connector_secrets_and_arbitrary_provider_hooks(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "host-selected-model-provider")
    monkeypatch.setenv("FILES_ROOT", "/private/user/files")
    monkeypatch.setenv("ELEPHANT_TOKEN", "private-knowledge-token")
    monkeypatch.setenv("TELEGRAM_BOT_TOKEN", "private-channel-token")
    monkeypatch.setenv("PYTHONSTARTUP", "/private/hook.py")
    environment = worker_environment(tmp_path, starter_blueprint())
    assert environment["OPENAI_API_KEY"] == "host-selected-model-provider"
    assert environment["HOME"] == str(tmp_path / "home")
    assert all(name not in environment for name in ("FILES_ROOT", "ELEPHANT_TOKEN", "TELEGRAM_BOT_TOKEN", "PYTHONSTARTUP"))


def test_fixture_callbacks_never_open_host_files_or_fabricate_external_success(tmp_path, monkeypatch):
    monkeypatch.setenv("FILES_ROOT", "/private/real-user-folder")
    tools = FixtureTools(tmp_path, {"night.txt": "Actual copied fixture"}, ["Trial observation"])
    assert tools.files_read({"path": "night.txt"})["content"] == "Actual copied fixture"
    for path in ("../.env", "/etc/passwd", "/private/real-user-folder/night.txt", "missing.txt"):
        assert tools.files_read({"path": path})["ok"] is False
    assert tools.notes_search({"query": "observation"})["notes"][0]["content"] == "Trial observation"
    assert tools.notes_save({"content": "A new isolated note"})["saved"]
    assert tools.notes_search({"query": "new isolated"})["notes"]


def native_frame(seq, identity="assistant:main", kind="text_delta", **extra):
    return {"id": f"native-{seq}", "cursor": f"console:{seq}", "identity": identity,
            "kind": kind, "timestamp_ms": 1234 + seq, "payload": {"text": "Native output"},
            "frame_version": 1, **extra}


def trace_root(tmp_path):
    write_private(tmp_path / "receipt.json", {"status": "running", "native_console_url": "http://native/console"})
    return NativeTrace(tmp_path)


def test_actual_all_agent_frames_and_same_cursor_version_updates_are_replayable(tmp_path):
    trace = trace_root(tmp_path)
    owner = native_frame(1, interaction_id="native-turn")
    peer = native_frame(2, "weather:main", "tool_call", payload={"name": "notes_search", "args": {"query": "cloud"}})
    trace.record({"type": "console_frame", "frame": owner})
    trace.record({"type": "console_frame", "frame": peer})
    updated = {**peer, "frame_version": 2, "updated_at_ms": 9999, "status": "complete"}
    trace.record({"type": "frame_updated", "frame": updated})
    trace.record({"type": "frame_updated", "frame": updated})
    rows = [json.loads(line) for line in (tmp_path / "timeline.jsonl").read_text().splitlines()]
    assert len(rows) == 3 and [row["rehearsal_seq"] for row in rows] == [1, 2, 3]
    assert rows[1]["cursor"] == rows[2]["cursor"] == "console:2"
    assert rows[2]["frame_version"] == 2 and rows[2]["updated_at_ms"] == 9999
    assert rows[2]["native_event"] == "frame_updated" and rows[2]["identity"] == "weather:main"
    for original, observed in ((owner, rows[0]), (peer, rows[1]), (updated, rows[2])):
        assert all(observed[key] == value for key, value in original.items())


def test_late_native_frame_updates_do_not_resurrect_completed_runs(tmp_path):
    trace = trace_root(tmp_path)
    start = native_frame(1, kind="run_started", run_id="run-1", interaction_id="turn-1")
    trace.record({"type": "console_frame", "frame": start})
    trace.record({"type": "console_frame", "frame": native_frame(2, kind="run_completed", run_id="run-1", interaction_id="turn-1")})
    trace.record({"type": "frame_updated", "frame": {**start, "frame_version": 2}})
    assert not trace.active_runs
    trace.record({"type": "console_frame", "frame": native_frame(3, kind="interaction_started", interaction_id="turn-1")})
    trace.record({"type": "console_frame", "frame": native_frame(4, kind="interaction_complete", interaction_id="turn-1")})
    trace.record({"type": "frame_updated", "frame": native_frame(3, kind="interaction_started", interaction_id="turn-1", frame_version=2)})
    assert not trace.active_interactions and "turn-1" in trace.completed


@pytest.mark.parametrize("started_run", [None, "actual-start-run-id"])
async def test_actual_native_run_completed_projection_clears_matching_active_run(tmp_path, started_run):
    trace = trace_root(tmp_path)
    started = native_frame(1, kind="run_started", interaction_id="native-admitted-interaction",
                           payload={"source_event_type": "run_started", "type": "run_started"})
    if started_run:
        started["run_id"] = started_run
    trace.record({"type": "console_frame", "frame": started})
    assert trace.active_runs
    terminal = native_frame(2, kind="interaction_complete", interaction_id="native-admitted-interaction",
                            payload={"source_event_type": "run_completed", "type": "run_completed", "result": "Actual native answer"})
    trace.record({"type": "console_frame", "frame": terminal})
    assert not trace.active_runs and "native-admitted-interaction" in trace.completed
    trace.record({"type": "frame_updated", "frame": {**started, "frame_version": 2}})
    assert not trace.active_runs


def test_sequential_no_id_peer_runs_track_distinct_actual_start_events(tmp_path):
    trace = trace_root(tmp_path)
    first = native_frame(1, "lead:main", "run_started")
    first_terminal = native_frame(2, "lead:main", "interaction_complete",
                                  payload={"source_event_type": "run_completed", "type": "run_completed"})
    second = native_frame(3, "lead:main", "run_started")
    trace.record({"type": "console_frame", "frame": first})
    trace.record({"type": "console_frame", "frame": first_terminal})
    assert not trace.active_runs
    trace.record({"type": "console_frame", "frame": second})
    assert trace.active_runs == {("lead:main", "native-3")}
    # Replay/frame versions of the earlier generation cannot either resurrect
    # its start or clear the distinct new generation that is actually running.
    trace.record({"type": "frame_updated", "frame": {**first, "frame_version": 2}})
    trace.record({"type": "frame_updated", "frame": {**first_terminal, "frame_version": 2}})
    assert trace.active_runs == {("lead:main", "native-3")}
    trace.record({"type": "console_frame", "frame": native_frame(4, "lead:main", "interaction_complete")})
    assert not trace.active_runs
    trace.record({"type": "frame_updated", "frame": {**second, "frame_version": 2}})
    assert not trace.active_runs


async def test_unfinished_sequential_peer_continuation_prevents_trace_settlement(tmp_path, monkeypatch):
    import launchkit.rehearsal_worker as worker
    trace = trace_root(tmp_path)
    for frame in (native_frame(1, "lead:main", "run_started"),
                  native_frame(2, "lead:main", "interaction_complete"),
                  native_frame(3, "lead:main", "run_started"),
                  native_frame(4, "lead:main", "turn_started")):
        trace.record({"type": "console_frame", "frame": frame})
    monkeypatch.setattr(worker, "QUIET_SECONDS", 0)
    capture, stop = asyncio.create_task(asyncio.Event().wait()), asyncio.Event()
    try:
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(trace.settle(capture, stop), .02)
        assert trace.active_runs == {("lead:main", "native-3")}
        trace.record({"type": "console_frame", "frame": native_frame(5, "lead:main", "interaction_complete")})
        await asyncio.wait_for(trace.settle(capture, stop), .2)
    finally:
        capture.cancel()
        await asyncio.gather(capture, return_exceptions=True)


@pytest.mark.parametrize("kind", ["run_failed", "interaction_failed", "message_delivery_failed", "interaction_canceled"])
def test_any_real_native_agent_failure_prevents_successful_rehearsal(tmp_path, kind):
    trace = trace_root(tmp_path)
    trace.record({"type": "console_frame", "frame": native_frame(1, "specialist:main", kind,
        interaction_id="peer-turn", payload={"error": "Actual native failure"})})
    assert "Actual native failure" in trace.failure


async def test_replay_uses_trial_cursor_not_repeated_native_cursor_and_preserves_scope(service):
    first = await submit(service)
    directory = service._directory(first["guide_id"], first["rehearsal_id"])
    trace = NativeTrace(directory)
    frame = native_frame(1)
    trace.record({"type": "console_frame", "frame": frame})
    trace.record({"type": "frame_updated", "frame": {**frame, "frame_version": 2}})
    page = await service.timeline(first["guide_id"], "console:0", 1, first["rehearsal_id"])
    assert page["next_cursor"] == "rehearsal:1" and page["latest_cursor"] == "rehearsal:2"
    page = await service.timeline(first["guide_id"], page["next_cursor"], 1, first["rehearsal_id"])
    assert page["frames"][0]["frame_version"] == 2 and page["frames"][0]["cursor"] == "console:1"
    assert "Not full application" in page["scope"]
    with pytest.raises(ValueError, match="beyond"):
        await service.timeline(first["guide_id"], "rehearsal:99", rehearsal_id=first["rehearsal_id"])
    await service.shutdown()


@pytest.mark.parametrize("cursor", ["console:2", "rehearsal:-1", "rehearsal:x", 2, True])
def test_native_and_unrelated_cursors_cannot_skip_trial_versions(cursor):
    with pytest.raises(ValueError):
        trace_cursor(cursor)


async def test_host_eof_cancels_even_an_awaited_native_admission(tmp_path, monkeypatch):
    import launchkit.rehearsal_worker as worker
    write_private(tmp_path / "receipt.json", {"status": "queued", "native_console_url": None})
    read_fd, write_fd = os.pipe()
    waiting, cancelled = asyncio.Event(), asyncio.Event()
    async def pending(directory, stop):
        waiting.set()
        try:
            await asyncio.Event().wait()
        finally:
            assert stop.is_set()
            cancelled.set()
    monkeypatch.setattr(worker, "execute", pending)
    monkeypatch.setattr(worker.sys, "stdin", SimpleNamespace(fileno=lambda: read_fd))
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    task = asyncio.create_task(worker.run(tmp_path, owned_stdin=True))
    try:
        await waiting.wait()
        os.close(write_fd)
        write_fd = None
        await asyncio.wait_for(task, 1)
        assert cancelled.is_set()
        assert read_receipt(tmp_path)["status"] == "interrupted"
    finally:
        if write_fd is not None:
            os.close(write_fd)
        os.close(read_fd)


async def test_worker_timeout_marks_partial_trace_and_stops_native_work(tmp_path, monkeypatch):
    import launchkit.rehearsal_worker as worker
    write_private(tmp_path / "receipt.json", {"status": "queued", "native_console_url": None})
    stopped = asyncio.Event()
    async def pending(directory, stop):
        try:
            await asyncio.Event().wait()
        finally:
            stopped.set()
    monkeypatch.setattr(worker, "execute", pending)
    monkeypatch.setattr(worker, "REHEARSAL_SECONDS", .01)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    await asyncio.wait_for(worker.run(tmp_path), 1)
    assert stopped.is_set() and read_receipt(tmp_path)["status"] == "failed"
    assert not read_receipt(tmp_path)["trace_complete"]


async def test_cancellation_during_subprocess_creation_reaps_the_actual_child(tmp_path, monkeypatch):
    import launchkit.rehearsal as module
    guide = Guide()
    service = RehearsalService(tmp_path, guide)
    root = service._directory(guide.identity, str(uuid.uuid4()))
    root.mkdir(parents=True)
    write_private(root / "input.json", {"blueprint": guide.state["proposal"]})
    write_private(root / "receipt.json", {"status": "queued", "native_console_url": None})
    spawning, release, stopped = asyncio.Event(), asyncio.Event(), asyncio.Event()
    class Child:
        returncode = None
        async def wait(self):
            await stopped.wait()
            self.returncode = 0
            return 0
        def terminate(self):
            stopped.set()
        def kill(self):
            stopped.set()
        stdin = SimpleNamespace(close=lambda: stopped.set())
    child = Child()
    async def create(*args, **kwargs):
        spawning.set()
        await release.wait()
        return child
    monkeypatch.setattr(module.asyncio, "create_subprocess_exec", create)
    async def confirmed_stop(process):
        assert process is child
        process.stdin.close()
        await process.wait()
    # This caller regression tests spawn custody; real group cleanup has
    # its separate exact-process tests in test_owned_worker.py.
    monkeypatch.setattr(service, "_stop_process", confirmed_stop)
    task = asyncio.create_task(service._run(root))
    await spawning.wait()
    task.cancel()
    release.set()
    await asyncio.wait_for(asyncio.gather(task, return_exceptions=True), 1)
    assert child.returncode == 0 and stopped.is_set()
    assert read_receipt(root)["status"] == "interrupted"


async def test_unconfirmed_worker_termination_is_not_claimed_stopped_and_can_be_retried(tmp_path, monkeypatch):
    import launchkit.rehearsal as module
    from launchkit.architect import ArchitectUnavailable
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-provider-key")
    monkeypatch.setattr(module, "planner_capability", lambda: {"available": True})
    service = RehearsalService(tmp_path, Guide())
    launched = asyncio.Event()
    class Child:
        returncode = None
        stdin = SimpleNamespace(close=lambda: None)
        async def wait(self):
            await asyncio.Event().wait()
    child = Child()
    async def create(*args, **kwargs):
        launched.set()
        return child
    async def denied(process):
        raise PermissionError("Cannot terminate this actual child")
    monkeypatch.setattr(module.asyncio, "create_subprocess_exec", create)
    monkeypatch.setattr(service, "_stop_process", denied)
    first = await submit(service)
    await launched.wait()
    await asyncio.sleep(0)
    with pytest.raises(ArchitectUnavailable, match="termination could not be confirmed"):
        await service.cancel(first["guide_id"], first["rehearsal_id"])
    root = service._directory(first["guide_id"], first["rehearsal_id"])
    receipt = read_receipt(root)
    assert receipt["status"] == "failed" and receipt["worker_stop_confirmed"] is False
    assert service.processes[first["rehearsal_id"]] is child and child.returncode is None
    with pytest.raises(DraftConflict, match="has not stopped"):
        await submit(service)
    with pytest.raises(RuntimeError, match="could not confirm termination"):
        await service.shutdown()
    assert service.processes[first["rehearsal_id"]] is child
    async def confirmed(process):
        process.returncode = 0
    monkeypatch.setattr(service, "_stop_process", confirmed)
    result = await service.cancel(first["guide_id"], first["rehearsal_id"])
    assert result["status"] == "interrupted" and result["worker_stop_confirmed"] is True
    assert not service.processes


@pytest.mark.parametrize("racing_signal", ["terminate", "kill"])
async def test_process_exit_race_still_requires_observed_reaping(tmp_path, monkeypatch, racing_signal):
    import signal
    from launchkit import owned_worker
    service = RehearsalService(tmp_path, Guide())
    present = racing_signal == "kill"
    class Child:
        pid, returncode, stdin, waits = 7777777, None, None, 0
        async def wait(self):
            self.waits += 1
            return self.returncode
        def terminate(self):
            self.returncode = 0  # Simulate the owned child watcher's observation.
            if racing_signal == "terminate":
                raise ProcessLookupError
    child = Child()
    def killpg(pid, selected):
        nonlocal present
        assert pid == child.pid
        if not present:
            raise ProcessLookupError
        if selected == signal.SIGKILL:
            present = False
            raise ProcessLookupError  # Exit raced the exact owned group signal.
    monkeypatch.setattr(owned_worker.os, "killpg", killpg)
    monkeypatch.setattr(owned_worker, "WORKER_STOP_TIMEOUT", .05)
    monkeypatch.setattr(owned_worker, "PROCESS_FORCE_REAP_TIMEOUT", .05)
    await service._stop_process(child)
    assert child.returncode == 0 and child.waits == 1 and not present


@pytest.mark.parametrize("mode", ["message", "flow"])
async def test_worker_admits_actual_entrypoint_or_flow_and_builds_same_identity_topology(tmp_path, monkeypatch, mode):
    import launchkit.rehearsal_worker as worker
    import meerkat_meta
    import httpx
    spec = starter_blueprint()
    if mode == "flow":
        value = spec.model_dump(mode="json")
        value["flows"] = [{"id": "compare", "name": "Compare", "steps": [
            {"id": "read", "agent": spec.topology.entrypoint, "prompt": "Interpret the actual sample parameters."}]}]
        spec = Blueprint.model_validate(value)
    frozen = {"blueprint": spec.model_dump(mode="json"), "text": "User-edited sample",
              "flow_id": "compare" if mode == "flow" else None,
              "params": {"observations": ["A", "B"]} if mode == "flow" else None,
              "fixtures": {"files": {"sample.txt": "Actual fixture evidence"}, "notes": []}}
    write_private(tmp_path / "input.json", frozen)
    write_private(tmp_path / "receipt.json", {"status": "queued", "native_console_url": None})
    calls, drafts, traces = [], [], []
    class Runtime:
        rust_http_base_url = "http://isolated-native"
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            calls.append("shutdown")
        def mob_handle(self):
            return self
        async def run_flow(self, flow_id, params):
            calls.append(("flow", flow_id, params))
            return "actual-native-flow-id"
        async def flow_status(self, run_id):
            return SimpleNamespace(status="completed", to_dict=lambda: {"run_id": run_id, "status": "completed"})
    runtime = Runtime()
    class Builder:
        def __getattr__(self, name):
            def configure(*args, **kwargs):
                if name in {"roster", "topology_provider", "session_service", "mob_inline", "agent_memory"}:
                    setattr(self, name + "_value", args[0])
                return self
            return configure
        async def build(self):
            roster = await self.roster_value.roster(None)
            calls.append(("roster", [row.identity for row in roster]))
            edges = await self.topology_provider_value.compute_edges([row.identity for row in roster], None)
            calls.append(("edges", [(edge.a, edge.b) for edge in edges]))
            definition = tomllib.loads(self.mob_inline_value)
            # Profiles describe reusable roles; only the desired roster is
            # admitted as native identities by the production runtime.
            for member in roster:
                profile = member.profile
                assert profile in definition["profiles"]
                class Draft:
                    profile_name, resume_session_id = profile, None
                    additional_instructions = []
                    callbacks = {}
                    def register_tool(self, name, handler, **kwargs):
                        self.callbacks[name] = handler
                draft = Draft()
                await self.session_service_value.build_agent(draft)
                drafts.append(draft)
            return runtime
    builder = Builder()
    monkeypatch.setattr(meerkat_meta.Meta, "builder", lambda: builder)
    async def prepared(root, build):
        return None
    monkeypatch.setattr(worker, "QUIET_SECONDS", 0)
    class Client:
        def __init__(self, **kwargs):
            assert kwargs["trust_env"] is False
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            return None
    monkeypatch.setattr(httpx, "AsyncClient", Client)
    async def capture(trace, client, base):
        traces.append(trace)
        trace.ready.set()
        await asyncio.Event().wait()
    monkeypatch.setattr(worker.NativeTrace, "capture", capture)
    async def request(client, method, url, **values):
        calls.append(("send", values["json"]))
        traces[0].record({"type": "console_frame", "frame": native_frame(1, spec.topology.entrypoint,
            "interaction_complete", interaction_id="actual-native-turn")})
        return SimpleNamespace(json=lambda: {"interaction_id": "actual-native-turn", "cursor": "console:1"})
    monkeypatch.setattr(worker, "_native_request", request)
    await worker.execute(tmp_path, asyncio.Event())
    expected = resolved_blueprint(spec)
    assert ("roster", [row["identity"] for row in expected["identities"]]) in calls
    assert ("edges", [(row["a"], row["b"]) for row in expected["edges"]]) in calls
    assert builder.agent_memory_value["distiller"] == {"enabled": False}
    assert builder.agent_memory_value["steward"] == {"enabled": False}
    for draft in drafts:
        assert "rehearsal_capabilities" in draft.callbacks
        assert "unavailable" in draft.additional_instructions[0]
        assert not any(name.startswith("mcp_") for name in draft.callbacks)
    receipt = read_receipt(tmp_path)
    assert receipt["status"] == "completed" and receipt["trace_complete"]
    assert receipt["native_console_url"] is None and "shutdown" in calls
    if mode == "message":
        request = next(value for kind, value in [row for row in calls if isinstance(row, tuple) and row[0] == "send"])
        assert request["identity"] == spec.topology.entrypoint and request["content"] == "User-edited sample"
    else:
        assert ("flow", "compare", {"observations": ["A", "B"]}) in calls
        assert receipt["flow_run_id"] == "actual-native-flow-id"
        assert not any(isinstance(row, tuple) and row[0] == "send" for row in calls)


async def test_application_without_available_callbacks_stops_before_native_agent_admission(tmp_path, monkeypatch):
    import meerkat_meta
    from launchkit.rehearsal_worker import execute
    directory = tmp_path / 'trial'
    directory.mkdir()
    write_private(directory / 'receipt.json', {})
    write_private(directory / 'input.json', {'blueprint': starter_blueprint().model_dump(mode='json'),
        'text': 'Example request', 'flow_id': None, 'params': None, 'fixtures': {'files': {}, 'notes': []},
        'mode': 'application', 'application_source': {'source_digest': 'checked', 'deployment': {}},
        'application_fixtures': {}})
    class UnavailableApplication:
        def __init__(self, *args): self.stopped = False
        async def start(self): return []
        async def stop(self): self.stopped = True; return True
        def status(self): return {'available': False, 'running': not self.stopped, 'detail': 'Missing trial hook'}
        def observations(self): return [{'kind': 'application_stopped', 'process_stop_confirmed': self.stopped}]
    monkeypatch.setattr('launchkit.application_trial.SandboxApplicationTrial', UnavailableApplication)
    monkeypatch.setattr(meerkat_meta.Meta, 'builder', lambda: pytest.fail('Unavailable application started native agent work'))
    await execute(directory, asyncio.Event())
    receipt = read_receipt(directory)
    assert receipt['status'] == 'unavailable' and not receipt['trace_complete']
    assert receipt['application_stop_confirmed'] and receipt['application_trial']['available'] is False
    assert 'no native agent work' in receipt['detail']
    assert not (directory / 'timeline.jsonl').exists()


async def test_harmless_interview_turn_retains_trace_without_rewriting_trial_inputs(service):
    first = await submit(service)
    root = service._directory(first["guide_id"], first["rehearsal_id"])
    frozen = (root / "input.json").read_bytes()
    service.guide.state["version"] += 1
    service.guide.state["messages"] = [{"role": "user", "text": "What did this trial observe?"}]
    current = await service.status(first["guide_id"], first["rehearsal_id"])
    assert current["current"] and current["guide_version"] == 3 and current["current_guide_version"] == 4
    assert not current["live_ready"] and not current["connected"]
    assert (root / "input.json").read_bytes() == frozen
    await service.shutdown()


@pytest.mark.parametrize("changed", ["version", "brief", "proposal", "operation"])
async def test_scope_change_during_trial_preparation_never_publishes_admission(service, changed):
    original = service.guide.get
    calls = 0
    async def read(identity):
        nonlocal calls
        calls += 1
        if calls == 2:
            if changed == "version": service.guide.state["version"] += 1
            elif changed == "brief": service.guide.state["brief"]["requirements"].append({"id": "recurrence", "outcome": "Run each day"})
            elif changed == "proposal": service.guide.state["proposal"]["agent_classes"][0]["model"] = "gpt-6-sol"
            else: service.guide.state["operation"] = {"status": "running"}
        return await original(identity)
    service.guide.get = read
    with pytest.raises(DraftConflict, match="before rehearsal admission"):
        await submit(service)
    assert not service.tasks and not (service.root / service.guide.identity / "latest.json").exists()


@pytest.mark.parametrize("trigger", ["signal", "timeout"])
async def test_worker_reawaits_sdk_startup_drain_after_cancellation(tmp_path, monkeypatch, trigger):
    from contextvars import copy_context
    import signal
    mode = "message"
    import launchkit.rehearsal_worker as worker
    import meerkat_meta
    import httpx
    spec = starter_blueprint()
    if mode == "flow":
        value = spec.model_dump(mode="json")
        value["flows"] = [{"id": "compare", "name": "Compare", "steps": [
            {"id": "read", "agent": spec.topology.entrypoint, "prompt": "Interpret the actual sample parameters."}]}]
        spec = Blueprint.model_validate(value)
    frozen = {"blueprint": spec.model_dump(mode="json"), "text": "User-edited sample",
              "flow_id": "compare" if mode == "flow" else None,
              "params": {"observations": ["A", "B"]} if mode == "flow" else None,
              "fixtures": {"files": {"sample.txt": "Actual fixture evidence"}, "notes": []}}
    write_private(tmp_path / "input.json", frozen)
    write_private(tmp_path / "receipt.json", {"status": "queued", "native_console_url": None})
    calls, drafts, traces = [], [], []
    draining, release = asyncio.Event(), asyncio.Event()
    loop, callbacks = asyncio.get_running_loop(), {}
    def register(sig, callback):
        callbacks[sig] = (callback, copy_context())
    monkeypatch.setattr(loop, "add_signal_handler", register)
    # Reaching the SDK's startup drain takes tens of milliseconds, so a short
    # job limit could expire before the stop this row sends. The row ends the
    # job itself, once the original cleanup is draining.
    monkeypatch.setattr(worker, "REHEARSAL_SECONDS", 30)
    from launchkit.owned_worker import owned_job_deadline
    class Runtime:
        rust_http_base_url = "http://isolated-native"
        async def identity_bootstrap_status(self):
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        cleanup_task = None
        cleanup_finished = False
        shutdown_calls = 0
        deadline = None
        async def shutdown(self):
            self.shutdown_calls += 1
            if self.cleanup_task is None:
                self.deadline = owned_job_deadline.get()
                async def drain():
                    draining.set()
                    await release.wait()
                    self.cleanup_finished = True
                self.cleanup_task = asyncio.create_task(drain())
            await asyncio.shield(self.cleanup_task)
            calls.append("shutdown")
        def mob_handle(self):
            return self
        async def run_flow(self, flow_id, params):
            calls.append(("flow", flow_id, params))
            return "actual-native-flow-id"
        async def flow_status(self, run_id):
            return SimpleNamespace(status="completed", to_dict=lambda: {"run_id": run_id, "status": "completed"})
    runtime = Runtime()
    class Builder:
        def __getattr__(self, name):
            def configure(*args, **kwargs):
                if name in {"roster", "topology_provider", "session_service", "mob_inline", "agent_memory"}:
                    setattr(self, name + "_value", args[0])
                return self
            return configure
        async def build(self):
            roster = await self.roster_value.roster(None)
            calls.append(("roster", [row.identity for row in roster]))
            edges = await self.topology_provider_value.compute_edges([row.identity for row in roster], None)
            calls.append(("edges", [(edge.a, edge.b) for edge in edges]))
            definition = tomllib.loads(self.mob_inline_value)
            # Profiles describe reusable roles; only the desired roster is
            # admitted as native identities by the production runtime.
            for member in roster:
                profile = member.profile
                assert profile in definition["profiles"]
                class Draft:
                    profile_name, resume_session_id = profile, None
                    additional_instructions = []
                    callbacks = {}
                    def register_tool(self, name, handler, **kwargs):
                        self.callbacks[name] = handler
                draft = Draft()
                await self.session_service_value.build_agent(draft)
                drafts.append(draft)
            # Model the SDK retaining cleanup when native init has not returned
            # a runtime and cancellation interrupts its first stop attempt.
            try:
                await runtime.shutdown()
            except asyncio.CancelledError as error:
                error.gateway_cleanup = runtime
                raise
            pytest.fail("Cancelled SDK initialization cannot admit the job")
    builder = Builder()
    monkeypatch.setattr(meerkat_meta.Meta, "builder", lambda: builder)
    monkeypatch.setattr(worker, "QUIET_SECONDS", 0)
    class Client:
        def __init__(self, **kwargs):
            assert kwargs["trust_env"] is False
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            return None
    monkeypatch.setattr(httpx, "AsyncClient", Client)
    async def capture(trace, client, base):
        traces.append(trace)
        trace.ready.set()
        await asyncio.Event().wait()
    monkeypatch.setattr(worker.NativeTrace, "capture", capture)
    async def request(client, method, url, **values):
        calls.append(("send", values["json"]))
        traces[0].record({"type": "console_frame", "frame": native_frame(1, spec.topology.entrypoint,
            "interaction_complete", interaction_id="actual-native-turn")})
        return SimpleNamespace(json=lambda: {"interaction_id": "actual-native-turn", "cursor": "console:1"})
    monkeypatch.setattr(worker, "_native_request", request)
    task = asyncio.create_task(worker.run(tmp_path))
    try:
        await asyncio.wait_for(draining.wait(), 30)
        if trigger == "signal":
            callback, captured = callbacks[signal.SIGTERM]
            loop.call_soon(callback, context=captured)
        else:
            runtime.deadline.reschedule(loop.time())
        await asyncio.sleep(.08)
        assert not task.done() and not runtime.cleanup_finished
        if trigger == "signal":
            assert runtime.deadline.when() is None  # The stop disarmed the job limit.
        else:
            assert runtime.deadline.expired()
        release.set()
        await asyncio.wait_for(task, 1)
        assert runtime.cleanup_finished and runtime.shutdown_calls == 2
        assert read_receipt(tmp_path)["status"] == ("failed" if trigger == "timeout" else "interrupted")
        assert not any(isinstance(row, tuple) and row[0] == "send" for row in calls)
        assert read_receipt(tmp_path)["native_console_url"] is None
    finally:
        release.set()
        if not task.done():
            task.cancel()
        await asyncio.gather(task, return_exceptions=True)


async def test_reaped_worker_keeps_custody_when_group_cleanup_is_unconfirmed(tmp_path, monkeypatch):
    import launchkit.rehearsal as module
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-provider-key")
    monkeypatch.setattr(module, "planner_capability", lambda: {"available": True})
    service = RehearsalService(tmp_path, Guide())
    class Child:
        pid, returncode, stdin = 7777777, 0, None
        async def wait(self):
            return 0
    child = Child()
    async def admitted(*args, **kwargs):
        return child, False
    async def unconfirmed(process):
        raise PermissionError("Synthetic residual group failure")
    monkeypatch.setattr(module, "spawn_owned_worker", admitted)
    monkeypatch.setattr(service, "_stop_process", unconfirmed)
    first = await submit(service)
    owner = service.tasks[first["rehearsal_id"]]
    result = await asyncio.gather(owner, return_exceptions=True)
    assert isinstance(result[0], PermissionError)
    directory = service._directory(first["guide_id"], first["rehearsal_id"])
    assert read_receipt(directory)["worker_stop_confirmed"] is False
    assert service.processes[first["rehearsal_id"]] is child
    assert first["rehearsal_id"] not in service.tasks
    with pytest.raises(DraftConflict, match="has not stopped"):
        await submit(service)
    async def confirmed(process):
        assert process is child and process.returncode == 0
    monkeypatch.setattr(service, "_stop_process", confirmed)
    stopped = await service.cancel(first["guide_id"], first["rehearsal_id"])
    assert stopped["worker_stop_confirmed"] is True and not service.processes
    assert stopped.get("application_stop_confirmed") is not True


async def test_direct_finite_trial_admission_rejects_lost_worker_cleanup(tmp_path, monkeypatch):
    import launchkit.rehearsal as module
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-test-provider-key")
    monkeypatch.setattr(module, "planner_capability", lambda: {"available": True})
    first_host = WaitingService(tmp_path, Guide())
    first = await submit(first_host)
    task = first_host.tasks.pop(first["rehearsal_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    directory = first_host._directory(first["guide_id"], first["rehearsal_id"])
    update_receipt(directory, status="running", keep_open=False, worker_stop_confirmed=None)
    fresh_host = RehearsalService(tmp_path, first_host.guide)
    with pytest.raises(DraftConflict, match="worker cleanup is unconfirmed"):
        await submit(fresh_host)
    assert not fresh_host.tasks and not fresh_host.processes
