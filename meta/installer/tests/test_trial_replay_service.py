"""Host replay controls with a labelled simulated native pipe, never a provider."""
import asyncio
from copy import deepcopy
from dataclasses import replace
import json
from types import SimpleNamespace
import uuid

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.implementation import read_receipt, update_receipt
from launchkit.projects import DraftConflict, fingerprint
from launchkit.rehearsal import (ReplayNotAdmitted, RehearsalService, deadline_after,
    replay_rejection_matches, replay_request_fingerprint, trial_turn_state)
from launchkit.templates.storage_setup import write_private

from test_rehearsal_conversation import native_seam
from test_trial_replay_capsule import attach_repair, artifact_bytes, replay_fixture, write_build
from test_trial_replay_journey import fresh_service, replay

from meta_trial_fixtures import selected, meta_trial_runtime


class NativePipeFixture:
    """A fake owned command pipe that acknowledges real host commands on disk.

    Its receipts are explicitly synthetic. No model, OS child, callback or
    application code is executed. The service's ledger and locks are real.
    """
    def __init__(self, directory):
        self.directory = directory
        self.returncode = None
        self.stdin = self
        self.sent = []
        self.tasks = []
        self.uncertain = False
        self.acknowledge = asyncio.Event()
        self.acknowledge.set()

    def is_closing(self):
        return self.returncode is not None

    def write(self, encoded):
        command = json.loads(encoded)
        turns = trial_turn_state(self.directory)
        assert turns["status"] == "waiting_input"
        assert all(row["status"] == "settled" for row in turns["turns"])
        self.sent.append(command)
        if not self.uncertain:
            self.tasks.append(asyncio.create_task(self._settle(command)))

    async def drain(self):
        if self.uncertain:
            raise ConnectionError("Synthetic uncertain command delivery")

    async def wait(self):
        while self.returncode is None:
            await asyncio.sleep(.001)
        return self.returncode

    async def _settle(self, command):
        await self.acknowledge.wait()
        turns = trial_turn_state(self.directory)
        if command["kind"] == "end":
            turns.update(status="completed", end_command_id=command["id"], ended_reason="user_ended")
            write_private(self.directory / "turns.json", turns)
            update_receipt(self.directory, status="completed", trace_complete=True,
                           worker_stop_confirmed=True, application_stop_confirmed=True)
            self.returncode = 0
            return
        assert command["expected_turn"] == turns["turn_count"]
        row = {"turn": command["turn"], "command_id": command["id"], "text": command["text"],
               "status": "settled", "native_admission": {"conversation_id": "synthetic-conversation",
                   "interaction_id": "synthetic-interaction-" + str(command["turn"])}}
        turns["turns"].append(row)
        turns.update(turn_count=command["turn"], current_turn=command["turn"],
                     status="waiting_input", idle_expires_at=deadline_after(300))
        write_private(self.directory / "turns.json", turns)
        update_receipt(self.directory, status="waiting_input", trace_complete=True)


@pytest.fixture
async def journey(tmp_path, monkeypatch):
    state = {"id": str(uuid.uuid4()), "version": 3,
             "proposal": starter_blueprint().model_dump(mode="json"), "brief": {"mission": "Offline journey"}}
    async def get(identity):
        assert identity == state["id"]
        return deepcopy(state)
    implementation = SimpleNamespace(lock=asyncio.Lock())
    service = RehearsalService(tmp_path, SimpleNamespace(get=get), implementation=implementation)
    directory = service._directory(state["id"], str(uuid.uuid4()))
    directory.mkdir(parents=True)
    source = {"build_id": str(uuid.uuid4()), "source_digest": "a" * 64,
              "brief_hash": fingerprint(state["brief"]), "deployment": {}, "acceptance_status": None}
    continuations = [{"text": "Prepare the draft using the earlier state."},
                     {"text": "Confirm the complete draft and produce its handoff.", "status": "failed"}]
    metadata = {"replay_version": 1, "original_rehearsal_id": str(uuid.uuid4()),
                "original_build_id": str(uuid.uuid4()), "original_source_digest": "b" * 64,
                "build_id": source["build_id"], "source_digest": source["source_digest"],
                "payload_sha256": "c" * 64, "expected_turns": 3, "completed_turns": 0, "status": "queued"}
    frozen = {"guide_id": state["id"], "guide_version": 3, "blueprint": state["proposal"],
              "brief": state["brief"], "text": "Read the exact isolated fixtures first.", "flow_id": None,
              "params": None, "fixtures": {"files": {}, "notes": []}, "application_fixtures": {},
              "application_source": source, "keep_open": True, "expires_at": deadline_after(600),
              "trial_replay": metadata, "replay_continuations": continuations}
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", {"rehearsal_id": directory.name,
        "guide_id": state["id"], "guide_version": 3, "blueprint": state["proposal"],
        "blueprint_hash": fingerprint(state["proposal"]), "brief_hash": fingerprint(state["brief"]),
        "status": "waiting_input", "mode": "application", "application_source": source,
        "keep_open": True, "trial_replay": metadata, "expires_at": frozen["expires_at"],
        "application_observation_revision": None, "worker_stop_confirmed": False})
    write_private(directory / "turns.json", {"status": "waiting_input", "turn_count": 1,
        "current_turn": 1, "conversation_id": "synthetic-conversation", "idle_expires_at": deadline_after(300),
        "turns": [{"turn": 1, "command_id": "initial", "text": frozen["text"], "status": "settled",
                   "native_admission": {"conversation_id": "synthetic-conversation", "interaction_id": "synthetic-initial"}}]})
    process = NativePipeFixture(directory)
    service.processes[directory.name] = process
    owned_task = asyncio.create_task(asyncio.Event().wait())
    service.tasks[directory.name] = owned_task
    checked = []
    async def checked_source(host, guide_id, current, build_id):
        assert host is implementation and guide_id == state["id"] and build_id == source["build_id"]
        assert service.lock.locked() and implementation.lock.locked()
        checked.append(deepcopy(current))
        return deepcopy(source), []
    monkeypatch.setattr("launchkit.application_snapshot.checked_trial_source", checked_source)
    async def status(guide_id, *, state):
        return {"guide_id": guide_id, "guide_version": state["version"], "current_guide_version": state["version"],
                "current": True, "status": "completed", "source_verified": True,
                "source_digest": source["source_digest"], "build_id": source["build_id"]}
    implementation.status = status
    harness = SimpleNamespace(service=service, directory=directory, process=process, state=state,
                              frozen=frozen, continuations=continuations, source=source, checked=checked)
    yield harness
    process.acknowledge.set()
    await asyncio.gather(*process.tasks, return_exceptions=True)
    owned_task.cancel()
    await asyncio.gather(owned_task, return_exceptions=True)
    service.tasks.clear()
    service.processes.clear()


async def test_saved_prefix_runs_in_order_then_closes_without_claiming_identical_model_decisions(journey):
    h = journey
    original_input = (h.directory / "input.json").read_bytes()
    await h.service._drive_replay(h.directory, h.process, h.continuations)
    await asyncio.gather(*h.process.tasks)
    h.service._finish_replay(h.directory)
    assert [row["kind"] for row in h.process.sent] == ["continue", "continue", "end"]
    assert [row["text"] for row in h.process.sent[:-1]] == [row["text"] for row in h.continuations]
    assert [row["expected_turn"] for row in h.process.sent[:-1]] == [1, 2]
    assert [row["turn"] for row in h.process.sent[:-1]] == [2, 3]
    assert len({row["id"] for row in h.process.sent}) == 3
    assert h.process.returncode == 0 and len(h.checked) == 3
    assert read_receipt(h.directory)["trial_replay"]["status"] == "completed"
    assert read_receipt(h.directory)["trial_replay"]["completed_turns"] == 3
    assert (h.directory / "input.json").read_bytes() == original_input


async def test_saved_followup_is_not_duplicated_while_its_acknowledgement_is_pending(journey):
    h = journey
    h.process.acknowledge.clear()
    drive = asyncio.create_task(h.service._drive_replay(h.directory, h.process, h.continuations))
    for _ in range(30000):
        if h.process.sent:
            break
        await asyncio.sleep(.001)
    assert len(h.process.sent) == 1
    await asyncio.sleep(.06)
    assert len(h.process.sent) == 1 and not drive.done()
    h.process.acknowledge.set()
    await drive
    await asyncio.gather(*h.process.tasks)
    assert len(h.process.sent) == 3


async def test_uncertain_delivery_remains_reserved_and_does_not_resend(journey):
    h = journey
    h.process.uncertain = True
    with pytest.raises(DraftConflict, match="delivery is uncertain"):
        await h.service._drive_replay(h.directory, h.process, h.continuations)
    assert len(h.process.sent) == 1
    assert json.loads((h.directory / "control.json").read_text())["status"] == "uncertain"
    with pytest.raises(DraftConflict, match="unresolved"):
        await h.service._drive_replay(h.directory, h.process, h.continuations)
    assert len(h.process.sent) == 1


@pytest.mark.parametrize("change", ["version", "brief", "owner", "idle", "prefix_text", "prefix_status"])
async def test_automatic_followup_requires_current_source_owned_worker_and_exact_settled_prefix(journey, change):
    h = journey
    if change == "version":
        h.state["version"] += 1
    elif change == "brief":
        h.state["brief"]["new_outcome"] = "Changed application scope"
    elif change == "owner":
        h.service.processes.clear()
    else:
        turns = trial_turn_state(h.directory)
        if change == "idle":
            turns["idle_expires_at"] = deadline_after(-1)
        elif change == "prefix_text":
            turns["turns"][0]["text"] = "A different preceding request"
        else:
            turns["turns"][0]["status"] = "admitted"
        write_private(h.directory / "turns.json", turns)
    with pytest.raises(DraftConflict):
        await h.service._drive_replay(h.directory, h.process, h.continuations)
    assert not h.process.sent


async def test_replay_keeps_manual_continuation_disabled(journey):
    h = journey
    status = await h.service.status(h.state["id"], h.directory.name, state=deepcopy(h.state))
    assert status["current"] and status["status"] == "waiting_input" and not status["can_continue"]
    with pytest.raises(DraftConflict, match="automatically"):
        await h.service.continue_trial(h.state["id"], 3, h.directory.name, 1, "Substituted browser request")
    assert not h.process.sent


@pytest.mark.parametrize("execution", ["complete", "source_changed", "delivery_uncertain"])
async def test_host_worker_orchestration_owns_replay_feeder_and_exact_cleanup(journey, monkeypatch, execution):
    """Actual host _run and feeder, simulated process pipe and native ledger."""
    h = journey
    if execution == "source_changed":
        h.state["brief"]["mission"] = "A different source scope"
    elif execution == "delivery_uncertain":
        h.process.uncertain = True
    spawned, stopped = [], []
    async def spawn(*args, **kwargs):
        spawned.append((args, kwargs))
        return h.process, False
    async def stop(process):
        assert process is h.process
        stopped.append(process)
        process.returncode = 0
    monkeypatch.setattr("launchkit.rehearsal.spawn_owned_worker", spawn)
    monkeypatch.setattr(h.service, "_stop_process", stop)
    task = asyncio.create_task(h.service._run(h.directory))
    h.service.tasks[h.directory.name] = task
    await task
    receipt = read_receipt(h.directory)
    assert len(spawned) == len(stopped) == 1
    assert not h.service.tasks and not h.service.processes
    assert receipt["worker_stop_confirmed"] is True
    if execution == "complete":
        assert receipt["status"] == "completed" and receipt["trial_replay"]["status"] == "completed"
        assert receipt["trial_replay"]["completed_turns"] == 3
        assert [row["kind"] for row in h.process.sent] == ["continue", "continue", "end"]
    else:
        assert receipt["status"] == "failed" and receipt["trial_replay"]["status"] == "incomplete"
        assert receipt["trace_complete"] is False
        assert len(h.process.sent) == (1 if execution == "delivery_uncertain" else 0)


async def test_completed_native_first_turn_does_not_pass_a_longer_saved_journey(journey):
    h = journey
    update_receipt(h.directory, status="completed", trace_complete=True)
    h.service._finish_replay(h.directory)
    receipt = read_receipt(h.directory)
    assert receipt["status"] == "failed" and receipt["trial_replay"]["status"] == "incomplete"
    assert receipt["trial_replay"]["completed_turns"] == 1
    assert "not replayed completely" in receipt["detail"]


@pytest.mark.parametrize("mode", ["text", "flow"])
async def test_closed_replay_uses_actual_worker_ledger_semantics(native_seam, monkeypatch, mode):
    """Run production execute/trace wiring with only native I/O replaced."""
    import meerkat_meta
    worker, directory, observed = native_seam
    frozen = json.loads((directory / "input.json").read_text())
    frozen.update(keep_open=False, replay_continuations=[], trial_replay={"expected_turns": 1})
    flow_calls = []
    if mode == "flow":
        frozen["blueprint"]["flows"] = [{"id": "briefing", "name": "Briefing", "steps": [
            {"id": "prepare", "agent": frozen["blueprint"]["topology"]["entrypoint"],
             "prompt": "Prepare this isolated sample."}]}]
        frozen.update(flow_id="briefing", params={"sample": "exact saved parameters"},
                      trial_replay={"expected_turns": 0})
        original_builder = meerkat_meta.Meta.builder
        class Flow:
            async def run_flow(self, flow_id, params):
                flow_calls.append((flow_id, params))
                return "offline-flow-execution"
            async def flow_status(self, run_id):
                return SimpleNamespace(status="completed", to_dict=lambda: {"run_id": run_id, "status": "completed"})
        def builder():
            value = original_builder()
            original_build = value.build
            async def build():
                runtime = await original_build()
                runtime.mob_handle = lambda: Flow()
                return runtime
            value.build = build
            return value
        monkeypatch.setattr(meerkat_meta.Meta, "builder", builder)
    write_private(directory / "input.json", frozen)
    update_receipt(directory, trial_replay=frozen["trial_replay"])
    await worker.execute(directory, asyncio.Event())
    turns = trial_turn_state(directory)
    if mode == "text":
        assert turns["status"] == "completed" and turns["turn_count"] == 1
        assert len(turns["turns"]) == 1 and turns["turns"][0]["status"] == "settled"
        assert observed.calls[0]["content"] == frozen["text"]
    else:
        assert turns["status"] == "completed" and turns["turn_count"] == 0
        assert turns["turns"] == [] and not observed.calls
        assert flow_calls == [("briefing", frozen["params"])]
        assert read_receipt(directory)["flow_run_id"] == "offline-flow-execution"
    service = RehearsalService(directory.parent, SimpleNamespace())
    service._finish_replay(directory)
    receipt = read_receipt(directory)
    assert receipt["status"] == "completed" and receipt["trial_replay"]["status"] == "completed"
    assert receipt["trial_replay"]["completed_turns"] == (1 if mode == "text" else 0)
    assert observed.stops == 1 and observed.shutdowns == 1


@pytest.mark.parametrize("failure", ["missing_capsule", "tampered_capsule", "cleanup", "source", "selection",
                                    "version", "operation", "missing_guide", "provider"])
async def test_pre_reservation_failure_has_durable_negative_replay_witness_when_host_reference_matches(
        replay_fixture, fresh_service, monkeypatch, failure):
    fixture, service = replay_fixture, fresh_service
    repaired = attach_repair(fixture)
    original = artifact_bytes(fixture.directory)
    latest = (fixture.directory.parent / "latest.json").read_bytes()
    if failure == "missing_capsule":
        (repaired.directory / "trial-replay/capsule.json").unlink()
    elif failure == "tampered_capsule":
        (repaired.directory / "trial-replay/capsule.json").write_text("unavailable capsule")
    elif failure == "cleanup":
        update_receipt(repaired.directory, cleanup_pending=True)
    elif failure == "source":
        repaired.workspace.write_file("application/late_change.py", "CHANGED = True\n")
    elif failure == "selection":
        write_build(fixture)
    elif failure == "version":
        fixture.state["version"] += 1
    elif failure == "operation":
        fixture.state["operation"] = {"status": "running"}
    elif failure == "missing_guide":
        async def missing(identity):
            raise LookupError("Guide no longer available")
        monkeypatch.setattr(service, "_guide", missing)
    else:
        monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": False, "reason": "Offline missing provider"})
    action = str(uuid.uuid4())
    arguments = (fixture.guide_id, 3, fixture.trial_id, fixture.original.id,
                 fixture.original.source["source_digest"], repaired.id, repaired.source["source_digest"], action)
    with pytest.raises((LookupError, DraftConflict, ValueError, RuntimeError)) as failed:
        await service.replay(*arguments)
    rejected = getattr(failed.value, "trial_start_rejection", None)
    assert replay_rejection_matches(rejected, *arguments)
    assert rejected.payload_sha256 == repaired.capsule["payload_sha256"]
    assert not service._directory(fixture.guide_id, action).exists()
    assert not service.admitted and not service.tasks and not service.processes
    assert (fixture.directory.parent / "latest.json").read_bytes() == latest
    assert artifact_bytes(fixture.directory) == original
    # Durable rejection recovery never re-enters the guide, source or worker.
    async def forbidden(identity):
        pytest.fail("Durable negative recovery performed another guide lookup")
    monkeypatch.setattr(service, "_guide", forbidden)
    with pytest.raises((LookupError, DraftConflict, ValueError, RuntimeError)) as recovered:
        await service.replay(*arguments)
    assert getattr(recovered.value, "trial_start_rejection", None) == rejected
    assert not service.admitted


@pytest.mark.parametrize("failure", ["missing_input", "symlink_input", "different_reference", "invalid_hash"])
async def test_unknown_host_reference_cannot_fabricate_a_negative_replay_witness(
        replay_fixture, fresh_service, failure):
    fixture, service = replay_fixture, fresh_service
    repaired = attach_repair(fixture)
    path = repaired.directory / "input.json"
    if failure == "missing_input":
        path.unlink()
    elif failure == "symlink_input":
        original_path = repaired.directory / "saved-input.json"
        path.rename(original_path)
        path.symlink_to(original_path)
    else:
        frozen = json.loads(path.read_text())
        if failure == "different_reference":
            frozen["trial_replay"]["rehearsal_id"] = str(uuid.uuid4())
        else:
            frozen["trial_replay"]["payload_sha256"] = "invalid"
        write_private(path, frozen)
    action = str(uuid.uuid4())
    with pytest.raises((LookupError, DraftConflict, ValueError)) as failed:
        await replay(service, fixture, repaired, action)
    assert getattr(failed.value, "trial_start_rejection", None) is None
    assert not service._directory(fixture.guide_id, action).exists() and not service.admitted
    assert not service._rejected_start_path(fixture.guide_id, action).exists()


def witness():
    ids = [str(uuid.uuid4()) for _ in range(5)]
    guide_id, rehearsal_id, original_id, repaired_id, action_id = ids
    original_digest, source_digest, payload = "a" * 64, "b" * 64, "c" * 64
    digest = replay_request_fingerprint(guide_id, 3, rehearsal_id, original_id, original_digest,
                                        repaired_id, source_digest, payload)
    receipt = ReplayNotAdmitted(guide_id=guide_id, guide_version=3, mode="application",
        start_request_id=action_id, start_request_fingerprint=digest, http_status=409,
        original_rehearsal_id=rehearsal_id, original_build_id=original_id,
        original_source_digest=original_digest, build_id=repaired_id, source_digest=source_digest,
        payload_sha256=payload)
    return receipt, (guide_id, 3, rehearsal_id, original_id, original_digest, repaired_id, source_digest, action_id)


def test_replay_rejection_is_bound_to_both_sources_and_exact_host_payload():
    receipt, values = witness()
    assert replay_rejection_matches(receipt, *values)
    projected = receipt.projection()
    assert projected["replay_version"] == 1 and projected["start_request_complete"] is False
    assert "http_status" not in projected and projected["payload_sha256"] == "c" * 64


@pytest.mark.parametrize("change", [{"payload_sha256": "d" * 64}, {"source_digest": "d" * 64},
    {"original_source_digest": "d" * 64}, {"guide_version": True}, {"witness_version": True},
    {"replay_version": True}, {"start_request_complete": True}, {"mode": "design"},
    {"status": "failed"}, {"start_request_id": str(uuid.uuid4())}, {"http_status": 500}])
def test_changed_or_unproven_replay_rejection_never_confirms_non_admission(change):
    receipt, values = witness()
    assert not replay_rejection_matches(replace(receipt, **change), *values)


def test_durable_replay_rejection_is_recovered_without_launching_a_worker(tmp_path):
    receipt, values = witness()
    service = RehearsalService(tmp_path, None)
    path = service._rejected_start_path(receipt.guide_id, receipt.start_request_id)
    path.parent.mkdir(parents=True)
    for folder in (service.root, service.root.parent):
        folder.mkdir(exist_ok=True)
    from dataclasses import asdict
    write_private(path, asdict(receipt))
    binding = {key: value for key, value in asdict(receipt).items()
               if key not in {"http_status", "witness_version", "status", "start_request_complete"}}
    recovered = service._durable_rejected_start(path, binding)
    assert recovered == receipt and replay_rejection_matches(recovered, *values)
    before = path.read_bytes()
    with pytest.raises(DraftConflict) as error:
        service._replay_rejected_start(path, binding)
    assert error.value.trial_start_rejection == receipt and path.read_bytes() == before
    assert not service.tasks and not service.processes
