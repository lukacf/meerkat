"""Fresh host re-admission and real sandbox callback-prefix regression.

Native admission fixtures are explicitly offline. Actual application callbacks
run only in the existing isolated sandbox, with fake I/O and no native model,
provider, deployment, or acceptance claim.
"""
import asyncio
import copy
import json
import uuid

import pytest

from launchkit.application_trial import ApplicationTrialError, SandboxApplicationTrial
from launchkit.implementation import read_receipt, update_receipt
from launchkit.implementation_workspace import _sandbox_executable
from launchkit.projects import DraftConflict
from launchkit.rehearsal import RehearsalService, replay_request_fingerprint
from launchkit.templates.storage_setup import write_private

from test_trial_replay_capsule import (
    APPLICATION_FIXTURES, FIXTURES, LONG_JOB, LONG_MANIFEST, PRIVATE_CANARY, REPAIRED_EVENT_SOURCE,
    artifact_bytes, as_flow_trial, attach_repair, load_repair, replay_fixture, write_build,
)

from meta_trial_fixtures import selected, meta_trial_runtime


class OfflineTerminalReplayService(RehearsalService):
    """Real host admission with labelled offline worker rows, no native calls."""
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.admitted = []
        self.drop_response = False

    async def _run(self, directory):
        frozen = json.loads((directory / "input.json").read_text())
        self.admitted.append(copy.deepcopy(frozen))
        texts = ([] if frozen.get("flow_id") is not None else
                 [frozen["text"], *[row["text"] for row in frozen.get("replay_continuations", [])]])
        conversation = "offline-fresh-conversation-" + directory.name
        turns = [{"turn": index, "command_id": "offline-command-" + str(index), "text": text,
                  "status": "settled", "native_admission": {
                      "interaction_id": "offline-fresh-interaction-" + str(index),
                      "conversation_id": conversation}, "terminal_interaction": {
                      "kind": "interaction_complete", "interaction_id": "offline-fresh-interaction-" + str(index)}}
                 for index, text in enumerate(texts, 1)]
        write_private(directory / "turns.json", {"status": "ending", "turns": turns,
            "turn_count": len(turns), "current_turn": len(turns), "conversation_id": conversation,
            "ended_reason": "replay_completed"})
        update_receipt(directory, status="completed", worker_stop_confirmed=True,
                       application_stop_confirmed=True, trace_complete=True,
                       detail="Offline native fixture only; no application outcome or acceptance claim.")
        if frozen.get("trial_replay"):
            self._finish_replay(directory)
        self.tasks.pop(directory.name, None)

    async def status(self, guide_id, rehearsal_id=None, **kwargs):
        if self.drop_response:
            self.drop_response = False
            await self.tasks[rehearsal_id]
            raise ConnectionError("Offline fixture lost response after durable fresh admission")
        return await super().status(guide_id, rehearsal_id, **kwargs)


@pytest.fixture
def fresh_service(replay_fixture, monkeypatch, selected):
    monkeypatch.setenv("OPENAI_API_KEY", "offline-provider-fixture-never-used")
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": True})
    fixture = replay_fixture
    service = OfflineTerminalReplayService(fixture.rehearsal.root.parent, fixture.guide,
                                            implementation=fixture.implementation)
    return service


async def replay(service, fixture, repaired, action):
    return await service.replay(fixture.guide_id, fixture.state["version"], fixture.trial_id,
        fixture.original.id, fixture.original.source["source_digest"], repaired.id,
        repaired.source["source_digest"], action)


async def settle(service, trial_id):
    task = service.tasks.get(trial_id)
    if task:
        await task


async def test_fresh_replay_preserves_exact_inputs_and_both_source_generations(replay_fixture, fresh_service):
    fixture, service = replay_fixture, fresh_service
    original = artifact_bytes(fixture.directory)
    repaired = attach_repair(fixture)
    action = str(uuid.uuid4())
    value = await replay(service, fixture, repaired, action)
    await settle(service, action)
    value = await service.status(fixture.guide_id, action)
    directory = service._directory(fixture.guide_id, action)
    frozen = json.loads((directory / "input.json").read_text())
    binding = frozen["trial_replay"]
    assert value["rehearsal_id"] == action != fixture.trial_id
    assert frozen["text"] == LONG_JOB and frozen["fixtures"] == FIXTURES
    assert frozen["application_fixtures"] == APPLICATION_FIXTURES
    assert [row["text"] for row in frozen["replay_continuations"]] == [row["text"] for row in fixture.turns[1:]]
    assert binding["original_rehearsal_id"] == fixture.trial_id
    assert binding["original_build_id"] == fixture.original.id
    assert binding["original_source_digest"] == fixture.original.source["source_digest"]
    assert binding["build_id"] == repaired.id
    assert binding["source_digest"] == repaired.source["source_digest"]
    assert binding["payload_sha256"] == repaired.capsule["payload_sha256"]
    assert frozen["start_request_fingerprint"] == replay_request_fingerprint(
        fixture.guide_id, fixture.state["version"], fixture.trial_id, fixture.original.id,
        fixture.original.source["source_digest"], repaired.id, repaired.source["source_digest"], binding["payload_sha256"])
    assert value["trial_replay"]["status"] == "completed"
    assert len(value["turns"]) == 3 and value["turns"][-1]["text"] == fixture.turns[-1]["text"]
    assert value["live_ready"] is False and value["connected"] is False
    assert value.get("application_outcomes_verified") is not True and value.get("acceptance") is not True
    assert value["can_continue"] is False and PRIVATE_CANARY not in json.dumps(frozen)
    assert artifact_bytes(fixture.directory) == original
    assert len(service.admitted) == 1


async def test_lost_response_same_action_recovers_once_even_after_selection_and_capsule_change(replay_fixture, fresh_service):
    fixture, service = replay_fixture, fresh_service
    repaired = attach_repair(fixture)
    action = str(uuid.uuid4())
    service.drop_response = True
    with pytest.raises(ConnectionError, match="lost response"):
        await replay(service, fixture, repaired, action)
    directory = service._directory(fixture.guide_id, action)
    before = artifact_bytes(directory)
    latest = (directory.parent / "latest.json").read_bytes()
    (repaired.directory / "trial-replay/capsule.json").write_text("unavailable after original admission")
    write_build(fixture)
    value = await replay(service, fixture, repaired, action)
    assert value["rehearsal_id"] == action and len(service.admitted) == 1
    assert value["current"] is False and value["live_ready"] is False
    assert artifact_bytes(directory) == before
    assert (directory.parent / "latest.json").read_bytes() == latest


async def test_concurrent_same_uuid_admits_one_fresh_trial_and_new_uuid_is_distinct(replay_fixture, fresh_service):
    fixture, service = replay_fixture, fresh_service
    repaired = attach_repair(fixture)
    action = str(uuid.uuid4())
    first, second = await asyncio.gather(replay(service, fixture, repaired, action),
                                          replay(service, fixture, repaired, action))
    await settle(service, action)
    assert first["rehearsal_id"] == second["rehearsal_id"] == action
    assert len(service.admitted) == 1
    distinct = str(uuid.uuid4())
    third = await replay(service, fixture, repaired, distinct)
    await settle(service, distinct)
    assert third["rehearsal_id"] == distinct and len(service.admitted) == 2


async def test_same_action_cannot_change_any_public_replay_binding(replay_fixture, fresh_service):
    fixture, service = replay_fixture, fresh_service
    repaired = attach_repair(fixture)
    action = str(uuid.uuid4())
    await replay(service, fixture, repaired, action)
    await settle(service, action)
    before = artifact_bytes(service._directory(fixture.guide_id, action))
    with pytest.raises(DraftConflict):
        await service.replay(fixture.guide_id, 3, fixture.trial_id, fixture.original.id,
            fixture.original.source["source_digest"], repaired.id, "b" * 64, action)
    assert len(service.admitted) == 1
    assert artifact_bytes(service._directory(fixture.guide_id, action)) == before


async def test_fresh_flow_admission_keeps_exact_host_saved_parameters_and_fixtures(replay_fixture, fresh_service):
    fixture, service = replay_fixture, fresh_service
    flow_input = as_flow_trial(fixture)
    repaired = attach_repair(fixture)
    action = str(uuid.uuid4())
    await replay(service, fixture, repaired, action)
    await settle(service, action)
    frozen = json.loads((service._directory(fixture.guide_id, action) / "input.json").read_text())
    assert frozen["flow_id"] == "briefing" and frozen["params"] == flow_input["params"]
    assert frozen["fixtures"] == FIXTURES and frozen["application_fixtures"] == APPLICATION_FIXTURES
    assert frozen["keep_open"] is False and frozen["replay_continuations"] == []
    assert PRIVATE_CANARY not in json.dumps(frozen)


@pytest.mark.parametrize("race", ["selection", "cleanup", "source", "guide"])
async def test_change_during_frozen_source_copy_prevents_new_worker_admission(replay_fixture, fresh_service, monkeypatch, race):
    import launchkit.application_snapshot as snapshot
    fixture, service = replay_fixture, fresh_service
    repaired = attach_repair(fixture)
    original = artifact_bytes(fixture.directory)
    checked, seen = snapshot.checked_trial_source, []
    async def source_check(*args, **kwargs):
        assert fixture.implementation.lock.locked()
        result = await checked(*args, **kwargs)
        seen.append(True)
        if len(seen) == 1:
            if race == "selection":
                write_build(fixture)
            elif race == "cleanup":
                update_receipt(repaired.directory, cleanup_pending=True)
            elif race == "source":
                repaired.workspace.write_file("application/late_change.py", "CHANGED = True\n")
            else:
                fixture.state["version"] += 1
        return result
    monkeypatch.setattr(snapshot, "checked_trial_source", source_check)
    action = str(uuid.uuid4())
    with pytest.raises(DraftConflict):
        await replay(service, fixture, repaired, action)
    assert seen and not service.admitted and not service.tasks
    assert json.loads((fixture.directory.parent / "latest.json").read_text())["rehearsal_id"] == fixture.trial_id
    assert artifact_bytes(fixture.directory) == original
    reserved = service._directory(fixture.guide_id, action)
    if reserved.exists():
        assert read_receipt(reserved)["start_request_complete"] is False
        assert read_receipt(reserved)["status"] == "unavailable"


async def test_actual_sandbox_replays_full_prefix_and_repairs_long_manifest_with_accumulated_state(replay_fixture):
    if not _sandbox_executable():
        pytest.skip("Actual application callbacks require the isolated sandbox; no unsandboxed fallback.")
    fixture = replay_fixture
    (fixture.directory / "application-observations.jsonl").unlink()
    source = fixture.original.source
    original_app = SandboxApplicationTrial(fixture.directory, fixture.directory / "application-source",
        source["source_digest"], source["deployment"], APPLICATION_FIXTURES)
    identity = source["deployment"]["entrypoint"]
    try:
        await original_app.start()
        assert await original_app.call(identity, "create_event", {"event_id": "concert-42"}) == {
            "event_id": "concert-42", "capacity": 120}
        await original_app.call(identity, "add_guest", {"event_id": "concert-42", "guest": "Mira"})
        with pytest.raises(ApplicationTrialError):
            await original_app.call(identity, "assign_seat", {
                "event_id": "concert-42", "guest": "Mira", "manifest": LONG_MANIFEST})
    finally:
        assert await original_app.stop() is True
    fixture.observations = original_app.observations()
    fixture.receipt["application_observations"] = copy.deepcopy(fixture.observations)
    fixture.receipt["application_observation_revision"] = original_app.observation_revision
    fixture.receipt["application_trial"] = original_app.status()
    write_private(fixture.directory / "receipt.json", fixture.receipt)
    original = artifact_bytes(fixture.directory)
    repaired = attach_repair(fixture)
    selected = await load_repair(fixture, repaired)
    assert selected["initial"]["text"] == LONG_JOB
    requests = [row for row in selected["observations"] if row["kind"] == "callback_requested"]
    assert [row["name"] for row in requests] == ["create_event", "add_guest", "assign_seat"]
    assert requests[-1]["arguments"]["manifest"] == LONG_MANIFEST
    assert LONG_MANIFEST.index('"seat"') > 1200
    root = repaired.directory / "regression-source"
    for row in repaired.files:
        path = root / row.path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(row.content)
        path.chmod(row.mode)
    app = SandboxApplicationTrial(repaired.directory / "callback-regression", root,
        repaired.source["source_digest"], repaired.source["deployment"], selected["initial"]["application_fixtures"])
    try:
        await app.start()
        with pytest.raises(ApplicationTrialError, match="actual scoped identifier"):
            await app.call(identity, "assign_seat", requests[-1]["arguments"])
        await app.call(identity, "create_event", requests[0]["arguments"])
        with pytest.raises(ApplicationTrialError, match="Reserve the guest"):
            await app.call(identity, "assign_seat", requests[-1]["arguments"])
        with pytest.raises(ApplicationTrialError, match="unavailable for the selected native identity"):
            await app.call("ungranted-offline-identity", "create_event", requests[0]["arguments"])
        assert app.calls == 3
        returned = [await app.call(row["identity"], row["name"], row["arguments"]) for row in requests]
        assert returned[-1] == {"event_id": "concert-42", "guest": "Mira", "status": "confirmed",
                                "seat": "A12", "route": "step_free"}
        with pytest.raises(ApplicationTrialError, match="actual scoped identifier"):
            await app.call(identity, "assign_seat", {**requests[-1]["arguments"], "event_id": "different-event"})
        snapshot = json.loads((app.root / "project/data/event.json").read_text())
        assert snapshot["guests"]["Mira"] == {"status": "confirmed", "seat": "A12", "route": "step_free"}
        assert app.status()["live_ready"] is False
    finally:
        assert await app.stop() is True
    assert artifact_bytes(fixture.directory) == original
    assert source["source_digest"] != repaired.source["source_digest"]


@pytest.mark.parametrize("defect", ["callback_removed", "fixture_dependency_missing"])
async def test_actual_sandbox_prefix_unavailable_dependencies_never_become_success(replay_fixture, defect):
    if not _sandbox_executable():
        pytest.skip("Actual callbacks require the isolated sandbox; no unsandboxed fallback.")
    fixture = replay_fixture
    repaired = attach_repair(fixture)
    source = REPAIRED_EVENT_SOURCE
    if defect == "callback_removed":
        source = source.replace('"name": "assign_seat"', '"name": "assign_seat_v2"')
    build = write_build(fixture, source=source)
    root = build.directory / "callback-source"
    for row in build.files:
        path = root / row.path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(row.content)
        path.chmod(row.mode)
    fixtures = copy.deepcopy(APPLICATION_FIXTURES)
    if defect == "fixture_dependency_missing":
        fixtures.pop("adapter")
    app = SandboxApplicationTrial(build.directory / "dependency-regression", root,
        build.source["source_digest"], build.source["deployment"], fixtures)
    try:
        if defect == "fixture_dependency_missing":
            with pytest.raises(ApplicationTrialError, match="explicit offline event fixtures"):
                await app.start()
            assert app.calls == 0
        else:
            await app.start()
            requests = [row for row in repaired.capsule["payload"]["observations"] if row["kind"] == "callback_requested"]
            await app.call(requests[0]["identity"], requests[0]["name"], requests[0]["arguments"])
            await app.call(requests[1]["identity"], requests[1]["name"], requests[1]["arguments"])
            with pytest.raises(ApplicationTrialError, match="unavailable"):
                await app.call(requests[-1]["identity"], requests[-1]["name"], requests[-1]["arguments"])
            assert app.calls == 2
        assert app.status()["live_ready"] is False
        assert not any(row["kind"] == "callback_returned" and row.get("name") == "assign_seat"
                       for row in app.observations())
    finally:
        assert await app.stop() is True
