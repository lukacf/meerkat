"""Exact selected-trial inputs in isolated source-backed offline fixtures.

The signed checker fixture below tests host custody, not native execution,
acceptance, deployment, or live adapter behavior.
"""
import copy
import asyncio
import hashlib
import json
import os
import uuid
from types import SimpleNamespace

import pytest

from launchkit.application_snapshot import write_trial_source
from launchkit.artifact_binding import proposal_hash
from launchkit.blueprint import starter_blueprint
from launchkit.compiler import _runtime
from launchkit.implementation import ImplementationService, update_receipt
from launchkit.implementation_worker import BuildTools
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict, fingerprint
from launchkit.rehearsal import RehearsalService
from launchkit.templates.storage_setup import write_private


PRIVATE_CANARY = "UNRELATED_PRIVATE_FORM_AND_PROVIDER_CANARY"
LONG_MANIFEST = json.dumps({"description": "A fictional school concert. " * 90,
                            "seat": "A12", "route": "step_free"})
LONG_JOB = "Book the exact fictional concert using this seating manifest: " + LONG_MANIFEST
FIXTURES = {"files": {"concert/guest.txt": "Mira requires step-free access.\n"},
            "notes": ["The public test concert has one step-free route."]}
APPLICATION_FIXTURES = {"adapter": "explicit offline adapter", "event_id": "concert-42",
                        "capacity": 120, "nested": {"enabled": True, "budget": 0}}


EVENT_SOURCE = '''import json
class Integration:
    def __init__(self, context, fixtures=None):
        self.context, self.fixtures = context, fixtures or {}
    def tools_for_agent(self, agent):
        if agent["identity"] != self.context.deployment["entrypoint"]:
            return []
        return [
            {"name": "create_event", "description": "Create an offline event.",
             "schema": {"type": "object", "properties": {"event_id": {"type": "string"}}, "required": ["event_id"]}, "handler": self.create},
            {"name": "add_guest", "description": "Reserve an offline guest.",
             "schema": {"type": "object", "properties": {"event_id": {"type": "string"}, "guest": {"type": "string"}}, "required": ["event_id", "guest"]}, "handler": self.add},
            {"name": "assign_seat", "description": "Assign an accessible offline seat.",
             "schema": {"type": "object", "properties": {"event_id": {"type": "string"}, "guest": {"type": "string"}, "manifest": {"type": "string"}}, "required": ["event_id", "guest", "manifest"]}, "handler": self.assign}]
    def read(self):
        path = self.context.root / "data" / "event.json"
        return json.loads(path.read_text()) if path.exists() else None
    def write(self, value):
        (self.context.root / "data" / "event.json").write_text(json.dumps(value))
    def checked(self, event_id):
        event = self.read()
        if not event or event["id"] != event_id:
            raise ValueError("Use the created event's actual scoped identifier")
        return event
    async def create(self, args):
        if args["event_id"] != self.fixtures["event_id"]:
            raise ValueError("Use the explicitly supplied fixture event")
        value = {"id": args["event_id"], "guests": {}, "capacity": self.fixtures["capacity"]}
        self.write(value)
        return {"event_id": value["id"], "capacity": value["capacity"]}
    async def add(self, args):
        value = self.checked(args["event_id"])
        value["guests"][args["guest"]] = {"status": "reserved"}
        self.write(value)
        return {"event_id": value["id"], "guest": args["guest"], "status": "reserved"}
    async def assign(self, args):
        value = self.checked(args["event_id"])
        if args["guest"] not in value["guests"]:
            raise ValueError("Reserve the guest before assigning a seat")
        manifest = json.loads(args["manifest"][:1200])
        value["guests"][args["guest"]] = {"status": "confirmed", "seat": manifest["seat"], "route": manifest["route"]}
        self.write(value)
        return {"event_id": value["id"], "guest": args["guest"], **value["guests"][args["guest"]]}
    async def start(self):
        assert self.context.runtime is None
    async def stop(self):
        pass
def build(context):
    return Integration(context)
def rehearse(context, fixtures):
    if not {"adapter", "event_id", "capacity"} <= set(fixtures):
        raise ValueError("Supply explicit offline event fixtures")
    return Integration(context, fixtures)
'''
REPAIRED_EVENT_SOURCE = EVENT_SOURCE.replace('args["manifest"][:1200]', 'args["manifest"]')


def offline_signed_check(workspace):
    """Synthetic host signing for custody tests, never execution or acceptance."""
    with workspace._locked():
        stored = workspace._state()
        check = {"status": "pass", "tests_run": 1, "ready_to_package": True,
                 "digest": workspace._digest(workspace._files(stored)),
                 "checks": [{"name": "offline custody fixture", "status": "pass",
                             "detail": "Synthetic checker signature; no execution claim."}]}
        check["signature"] = workspace._signed(check)
        stored["last_check"] = check
        workspace._save_state(stored)
        return check


def artifact_bytes(directory):
    return {str(path.relative_to(directory)): path.read_bytes()
            for path in directory.rglob("*") if path.is_file()}


def write_build(fixture, *, source=REPAIRED_EVENT_SOURCE, replay_reference=None):
    build_id = str(uuid.uuid4())
    directory = fixture.implementation._directory(fixture.guide_id, build_id)
    directory.mkdir(parents=True, mode=0o700)
    frozen = {"guide_id": fixture.guide_id, "guide_version": fixture.state["version"],
              "blueprint": copy.deepcopy(fixture.state["proposal"]),
              "brief": copy.deepcopy(fixture.state["brief"]), "messages": [], "requests": []}
    if replay_reference is not None:
        frozen["trial_replay"] = replay_reference
    write_private(directory / "input.json", frozen)
    workspace = ImplementationWorkspace(directory / "workspace", frozen["blueprint"])
    workspace.seed()
    workspace.write_file("application/__init__.py", source)
    check = offline_signed_check(workspace)
    files = workspace.export_files(require_verified=True)
    digest = workspace._digest(files)
    write_private(directory / "receipt.json", {
        "guide_id": fixture.guide_id, "guide_version": fixture.state["version"],
        "build_id": build_id, "blueprint_hash": proposal_hash(frozen["blueprint"]),
        "brief_hash": fingerprint(frozen["brief"]), "status": "completed",
        "cleanup_pending": False, "cleanup_in_progress": False,
        "stop_requested": False, "worker_stop_confirmed": True,
        "receipt": check, "checks": check["checks"], "live_ready": False})
    write_private(directory.parent / "latest.json", {"build_id": build_id})
    source_binding = {"build_id": build_id, "source_digest": digest,
                      "brief_hash": fingerprint(frozen["brief"]),
                      "deployment": json.loads(next(row.content for row in files if row.path == "deployment.json")),
                      "acceptance_status": None}
    return SimpleNamespace(id=build_id, directory=directory, workspace=workspace,
                           source=source_binding, files=files, input=frozen)


@pytest.fixture
def replay_fixture(tmp_path):
    guide_id, trial_id = str(uuid.uuid4()), str(uuid.uuid4())
    state = {"id": guide_id, "version": 3,
             "proposal": starter_blueprint().model_dump(mode="json"),
             "brief": {"mission": "Assign accessible fictional concert seats after confirmation."},
             "messages": [{"role": "user", "text": PRIVATE_CANARY}],
             "connector_forms": {"secret": PRIVATE_CANARY}, "provider_credentials": PRIVATE_CANARY}
    async def get(identity):
        assert identity == guide_id
        return copy.deepcopy(state)
    guide = SimpleNamespace(get=get)
    implementation = ImplementationService(tmp_path, guide)
    rehearsal = RehearsalService(tmp_path, guide, implementation=implementation)
    fixture = SimpleNamespace(guide_id=guide_id, trial_id=trial_id, state=state,
                              guide=guide, implementation=implementation, rehearsal=rehearsal)
    fixture.original = write_build(fixture, source=EVENT_SOURCE)
    directory = rehearsal._directory(guide_id, trial_id)
    directory.mkdir(parents=True, mode=0o700)
    fixture.directory = directory
    frozen = {"guide_id": guide_id, "guide_version": state["version"],
              "blueprint": copy.deepcopy(state["proposal"]), "brief": copy.deepcopy(state["brief"]),
              "mode": "application", "text": LONG_JOB, "flow_id": None, "params": None,
              "fixtures": copy.deepcopy(FIXTURES), "application_fixtures": copy.deepcopy(APPLICATION_FIXTURES),
              "application_source": copy.deepcopy(fixture.original.source), "keep_open": True}
    write_private(directory / "input.json", frozen)
    write_trial_source(directory, fixture.original.files)
    entrypoint = fixture.original.source["deployment"]["entrypoint"]
    arguments = [
        {"event_id": APPLICATION_FIXTURES["event_id"]},
        {"event_id": APPLICATION_FIXTURES["event_id"], "guest": "Mira"},
        {"event_id": APPLICATION_FIXTURES["event_id"], "guest": "Mira", "manifest": LONG_MANIFEST}]
    names = ["create_event", "add_guest", "assign_seat"]
    observations = [{"kind": "application_started", "observation_revision": 1, "catalog": [], "dependencies": [], "unavailable": None}]
    for index, (name, args) in enumerate(zip(names, arguments)):
        observations.append({"kind": "callback_requested", "observation_revision": index * 2 + 2,
                             "identity": entrypoint, "name": name, "arguments": args})
        observations.append({"kind": "callback_failed" if index == 2 else "callback_returned",
                             "observation_revision": index * 2 + 3, "identity": entrypoint, "name": name,
                             **({"error": "ApplicationTrialError", "detail": "Unterminated string in truncated seating manifest"}
                                if index == 2 else {"result": {"event_id": APPLICATION_FIXTURES["event_id"], "mutation": index + 1}})})
    observations.append({"kind": "application_stopped", "observation_revision": 7, "process_stop_confirmed": True, "lifecycle_error": None})
    for row in observations:
        row.update(at="2026-10-03T00:00:00+00:00", scope="offline-application",
                   source_digest=fixture.original.source["source_digest"])
    fixture.observations = observations
    (directory / "application-observations.jsonl").write_text("".join(json.dumps(row) + "\n" for row in observations))
    continuation_texts = ["Reserve Mira using the event's actual identifier.",
                          "Confirm Mira's step-free seat using the full supplied manifest."]
    conversation = "offline-fixture-conversation"
    turns = [{"turn": index + 1, "command_id": "initial" if index == 0 else str(uuid.uuid4()),
              "text": text, "status": "failed" if index == 2 else "settled",
              "native_admission": {"interaction_id": "offline-fixture-interaction-" + str(index + 1),
                                   "conversation_id": conversation},
              "terminal_interaction": {"kind": "interaction_failed" if index == 2 else "interaction_complete",
                                       "interaction_id": "offline-fixture-interaction-" + str(index + 1)}}
             for index, text in enumerate([LONG_JOB, *continuation_texts])]
    fixture.turns = turns
    write_private(directory / "turns.json", {"status": "failed", "turns": turns,
        "turn_count": 2, "current_turn": 3, "conversation_id": conversation, "ended_reason": "native_failed"})
    receipt = {"guide_id": guide_id, "guide_version": state["version"], "rehearsal_id": trial_id,
               "blueprint_hash": proposal_hash(frozen["blueprint"]), "brief_hash": fingerprint(frozen["brief"]),
               "mode": "application", "status": "failed", "current": True,
               "keep_open": True, "worker_stop_confirmed": True, "application_stop_confirmed": True,
               "application_source": copy.deepcopy(fixture.original.source),
               "application_observations": copy.deepcopy(observations), "application_observation_revision": 7,
               "application_trial": {"available": True, "running": False, "dependencies": [], "calls": 3},
               "turns": copy.deepcopy(turns), "turn_count": 2, "current_turn": 3,
               "conversation_id": conversation, "ended_reason": "native_failed", "live_ready": False, "connected": False}
    fixture.receipt = receipt
    write_private(directory / "receipt.json", receipt)
    write_private(directory.parent / "latest.json", {"rehearsal_id": trial_id})
    unrelated = directory.parent / str(uuid.uuid4())
    unrelated.mkdir(mode=0o700)
    write_private(unrelated / "input.json", {"unrelated_trial_private_fixture": PRIVATE_CANARY})
    fixture.unrelated = unrelated
    return fixture


def capture(fixture):
    from launchkit.trial_replay import capture_trial_replay
    return capture_trial_replay(fixture.rehearsal, fixture.guide_id,
                                fixture.state, fixture.receipt)


def attach_repair(fixture):
    from launchkit.trial_replay import save_trial_replay
    capsule = capture(fixture)
    repaired = write_build(fixture)
    reference = save_trial_replay(repaired.directory, capsule)
    repaired.input["trial_replay"] = reference
    previous = {"build_id": fixture.original.id, "source_digest": fixture.original.source["source_digest"]}
    repaired.input["trial_replay_ancestry"] = [previous]
    repaired.input["previous_implementation"] = {
        "build_id": previous["build_id"], "copied_source_digest": previous["source_digest"]}
    write_private(repaired.directory / "input.json", repaired.input)
    repaired.reference, repaired.capsule = reference, capsule
    return repaired


def as_flow_trial(fixture):
    frozen = json.loads((fixture.directory / "input.json").read_text())
    frozen.update(flow_id="briefing", params={"topic": LONG_MANIFEST,
        "requirements": {"accessible": True, "budget": 0, "unknown": None}, "cases": ["Mira", "Ren"]},
        keep_open=False)
    write_private(fixture.directory / "input.json", frozen)
    (fixture.directory / "turns.json").unlink()
    fixture.receipt.update(keep_open=False, turns=[], turn_count=0, current_turn=0)
    write_private(fixture.directory / "receipt.json", fixture.receipt)
    return frozen


def rewrite_observations(fixture, observations):
    fixture.receipt["application_observations"] = copy.deepcopy(observations)
    (fixture.directory / "application-observations.jsonl").write_text(
        "".join(json.dumps(row) + "\n" for row in observations))


def test_capture_keeps_full_job_failed_turn_and_callback_prefix_without_private_history(replay_fixture):
    fixture = replay_fixture
    before = artifact_bytes(fixture.directory)
    unrelated = artifact_bytes(fixture.unrelated)
    capsule = capture(fixture)
    payload = capsule["payload"]
    assert payload["initial"]["text"] == LONG_JOB and len(LONG_JOB) > 1200
    assert payload["initial"]["fixtures"] == FIXTURES
    assert payload["initial"]["application_fixtures"] == APPLICATION_FIXTURES
    assert payload["observations"] == fixture.observations
    assert payload["observations"][-3]["arguments"]["manifest"] == LONG_MANIFEST
    assert len(payload["turns"]) == 3 and payload["turns"][-1]["status"] == "failed"
    assert [row["text"] for row in payload["continuations"]] == [row["text"] for row in fixture.turns[1:]]
    assert capsule["payload_sha256"] == fingerprint(payload)
    assert capsule["captured_evidence"]["input_sha256"] == hashlib.sha256(before["input.json"]).hexdigest()
    assert PRIVATE_CANARY not in json.dumps(capsule)
    assert "live_ready" not in payload and "acceptance" not in payload
    assert artifact_bytes(fixture.directory) == before
    assert artifact_bytes(fixture.unrelated) == unrelated
    payload["initial"]["application_fixtures"]["nested"]["enabled"] = False
    assert json.loads((fixture.directory / "input.json").read_text())["application_fixtures"] == APPLICATION_FIXTURES


@pytest.mark.parametrize("field", ["worker_stop_confirmed", "application_stop_confirmed"])
@pytest.mark.parametrize("value", [None, False, "true", 1])
def test_capture_requires_positive_exact_cleanup(replay_fixture, field, value):
    fixture = replay_fixture
    fixture.receipt[field] = value
    with pytest.raises(DraftConflict):
        capture(fixture)


@pytest.mark.parametrize("owner", ["tasks", "processes"])
def test_capture_rejects_retained_actual_custody_even_with_saved_stop_flags(replay_fixture, owner):
    fixture = replay_fixture
    getattr(fixture.rehearsal, owner)[fixture.trial_id] = object()
    with pytest.raises(DraftConflict):
        capture(fixture)
    assert not (fixture.original.directory / "trial-replay").exists()


@pytest.mark.parametrize("defect", ["missing_input", "input_symlink", "source_changed", "missing_turns",
                                     "turn_gap", "different_conversation", "missing_admission",
                                     "missing_observations", "partial_observation_line", "other_source",
                                     "other_scope", "observation_revision", "callback_result_without_request",
                                     "deep_observation_json"])
def test_capture_refuses_incomplete_or_changed_authoritative_evidence(replay_fixture, defect):
    fixture = replay_fixture
    if defect == "missing_input":
        (fixture.directory / "input.json").unlink()
    elif defect == "input_symlink":
        path = fixture.directory / "input.json"
        body = path.read_bytes()
        path.unlink()
        target = fixture.directory.parent / "outside-input.json"
        target.write_bytes(body)
        path.symlink_to(target)
    elif defect == "source_changed":
        (fixture.directory / "application-source/application/__init__.py").write_text(REPAIRED_EVENT_SOURCE)
    elif defect == "missing_turns":
        (fixture.directory / "turns.json").unlink()
    elif defect in {"turn_gap", "different_conversation", "missing_admission"}:
        ledger = json.loads((fixture.directory / "turns.json").read_text())
        if defect == "turn_gap":
            ledger["turns"][1]["turn"] = 4
        elif defect == "different_conversation":
            ledger["turns"][1]["native_admission"]["conversation_id"] = "offline-unrelated-conversation"
        else:
            ledger["turns"][1].pop("native_admission")
        fixture.receipt["turns"] = copy.deepcopy(ledger["turns"])
        write_private(fixture.directory / "turns.json", ledger)
    elif defect == "missing_observations":
        (fixture.directory / "application-observations.jsonl").unlink()
    elif defect == "partial_observation_line":
        path = fixture.directory / "application-observations.jsonl"
        path.write_bytes(path.read_bytes().rstrip(b"\n"))
    elif defect == "deep_observation_json":
        (fixture.directory / "application-observations.jsonl").write_bytes(
            b'[' * 1500 + b'0' + b']' * 1500 + b'\n')
    else:
        rows = copy.deepcopy(fixture.observations)
        if defect == "other_source":
            rows[1]["source_digest"] = "b" * 64
        elif defect == "other_scope":
            rows[1]["scope"] = "live-production"
        elif defect == "observation_revision":
            rows[3]["observation_revision"] = True
        else:
            rows.pop(1)
        rewrite_observations(fixture, rows)
    with pytest.raises(DraftConflict):
        capture(fixture)


def test_failed_final_requested_turn_without_native_admission_is_retained_without_native_success(replay_fixture):
    fixture = replay_fixture
    ledger = json.loads((fixture.directory / "turns.json").read_text())
    ledger["turns"][-1].pop("native_admission")
    ledger["turns"][-1].pop("terminal_interaction")
    fixture.receipt["turns"] = copy.deepcopy(ledger["turns"])
    write_private(fixture.directory / "turns.json", ledger)
    payload = capture(fixture)["payload"]
    assert payload["turns"][-1]["text"] == fixture.turns[-1]["text"]
    assert payload["turns"][-1]["status"] == "failed"
    assert payload["turns"][-1]["native_admission"] == {}
    assert payload["turns"][-1]["terminal_interaction"] == {}
    assert payload["continuations"][-1]["text"] == fixture.turns[-1]["text"]


def test_flow_capsule_retains_exact_long_parameter_types_without_synthetic_text_turns(replay_fixture):
    fixture = replay_fixture
    frozen = as_flow_trial(fixture)
    payload = capture(fixture)["payload"]
    assert payload["initial"]["flow_id"] == "briefing"
    assert payload["initial"]["params"] == frozen["params"]
    assert payload["initial"]["text"] == LONG_JOB
    assert payload["turns"] == payload["continuations"] == []


async def test_capsule_pages_bind_full_exact_input_and_never_enter_export_or_general_context(replay_fixture):
    from launchkit.trial_replay import read_trial_replay
    fixture = replay_fixture
    original = artifact_bytes(fixture.directory)
    repaired = attach_repair(fixture)
    pieces, offset = [], 0
    while True:
        page = read_trial_replay(repaired.directory, repaired.reference, section="job", offset=offset, limit=113)
        assert page["offset"] == offset and len(page["content"]) <= 113
        assert page["payload_sha256"] == repaired.capsule["payload_sha256"]
        pieces.append(page["content"])
        if page["next_offset"] is None:
            break
        offset = page["next_offset"]
    job = json.loads("".join(pieces))
    assert job == repaired.capsule["payload"]["initial"]
    assert job["text"].endswith(LONG_MANIFEST)
    exported = repaired.workspace.export_files(require_verified=True)
    assert all("trial-replay" not in row.path for row in exported)
    assert PRIVATE_CANARY not in "\n".join(row.content for row in exported)
    tools = BuildTools(repaired.directory)
    context = await tools.context({})
    rendered = json.dumps(context)
    assert PRIVATE_CANARY not in rendered
    assert LONG_MANIFEST not in rendered and LONG_JOB not in rendered
    assert "application-observations.jsonl" not in rendered
    assert (repaired.directory / "trial-replay/capsule.json").stat().st_mode & 0o777 == 0o600
    assert artifact_bytes(fixture.directory) == original


@pytest.mark.parametrize("options", [{"section": "../input.json"}, {"section": []}, {"section": {}},
                                     {"section": None}, {"offset": True}, {"offset": -1},
                                     {"limit": True}, {"limit": 0}, {"limit": 16001}])
def test_capsule_reader_has_bounded_typed_pages_and_no_arbitrary_paths(replay_fixture, options):
    from launchkit.trial_replay import read_trial_replay
    repaired = attach_repair(replay_fixture)
    with pytest.raises(ValueError):
        read_trial_replay(repaired.directory, repaired.reference, **options)


@pytest.mark.parametrize("defect", ["symlink", "hardlink"])
def test_private_capsule_storage_cannot_alias_another_file(replay_fixture, defect):
    from launchkit.trial_replay import read_trial_replay
    repaired = attach_repair(replay_fixture)
    path = repaired.directory / "trial-replay/capsule.json"
    other = repaired.directory.parent / "unrelated-capsule.json"
    other.write_bytes(path.read_bytes())
    path.unlink()
    if defect == "symlink":
        path.symlink_to(other)
    else:
        os.link(other, path)
    with pytest.raises(DraftConflict):
        read_trial_replay(repaired.directory, repaired.reference)


def test_capsule_tamper_is_rejected_before_any_page_or_available_projection(replay_fixture):
    from launchkit.trial_replay import read_trial_replay, trial_replay_projection
    fixture = replay_fixture
    repaired = attach_repair(fixture)
    path = repaired.directory / "trial-replay/capsule.json"
    value = json.loads(path.read_text())
    value["payload"]["initial"]["fixtures"]["notes"].append("Different job")
    value["payload_sha256"] = fingerprint(value["payload"])
    path.write_text(json.dumps(value))
    with pytest.raises(DraftConflict):
        read_trial_replay(repaired.directory, repaired.reference, section="job")
    projected = trial_replay_projection(repaired.directory, repaired.input,
        {"build_id": repaired.id, "source_digest": repaired.source["source_digest"],
         "current": True, "source_verified": True, "status": "completed",
         "cleanup_pending": False, "cleanup_in_progress": False, "worker_stop_confirmed": True}, fixture.state)
    assert projected["available"] is False
    assert PRIVATE_CANARY not in json.dumps(projected)


async def load_repair(fixture, repaired):
    from launchkit.trial_replay import load_trial_replay
    return await load_trial_replay(fixture.implementation, fixture.guide_id, fixture.trial_id,
        fixture.original.id, fixture.original.source["source_digest"], repaired.id,
        repaired.source["source_digest"], state=fixture.state)


async def test_historical_original_is_not_current_but_exact_repair_ancestry_loads(replay_fixture):
    fixture = replay_fixture
    original = artifact_bytes(fixture.directory)
    repaired = attach_repair(fixture)
    old = await fixture.rehearsal.status(fixture.guide_id, fixture.trial_id)
    assert old["current"] is False and old["live_ready"] is False
    loaded = await load_repair(fixture, repaired)
    assert loaded["initial"]["text"] == LONG_JOB
    assert loaded["initial"]["fixtures"] == FIXTURES
    assert loaded["initial"]["application_fixtures"] == APPLICATION_FIXTURES
    assert loaded["observations"] == fixture.observations
    assert loaded["original_turns"][-1]["status"] == "failed"
    assert loaded["reference"]["original_source_digest"] != repaired.source["source_digest"]
    assert artifact_bytes(fixture.directory) == original


@pytest.mark.parametrize("defect", ["selection", "source", "cleanup_pending", "cleanup_in_progress",
                                     "worker_stop_unconfirmed", "scope_change", "missing_ancestry",
                                     "wrong_ancestry", "ancestry_cycle", "previous_source_changed"])
async def test_capsule_load_blocks_changed_selection_source_cleanup_and_scope(replay_fixture, defect):
    fixture = replay_fixture
    repaired = attach_repair(fixture)
    if defect == "selection":
        write_build(fixture)
    elif defect == "source":
        repaired.workspace.write_file("application/changed.py", "CHANGED = True\n")
    elif defect == "scope_change":
        fixture.state["brief"]["mission"] = "An unrelated new job"
    elif defect in {"missing_ancestry", "wrong_ancestry", "ancestry_cycle", "previous_source_changed"}:
        frozen = copy.deepcopy(repaired.input)
        if defect == "missing_ancestry":
            frozen.pop("trial_replay_ancestry")
        elif defect == "wrong_ancestry":
            frozen["trial_replay_ancestry"][0]["source_digest"] = "b" * 64
        elif defect == "ancestry_cycle":
            frozen["trial_replay_ancestry"].append({"build_id": repaired.id,
                                                   "source_digest": repaired.source["source_digest"]})
        else:
            frozen["previous_implementation"]["copied_source_digest"] = "b" * 64
        write_private(repaired.directory / "input.json", frozen)
    else:
        path = repaired.directory / "receipt.json"
        receipt = json.loads(path.read_text())
        receipt["worker_stop_confirmed" if defect == "worker_stop_unconfirmed" else defect] = (
            False if defect == "worker_stop_unconfirmed" else True)
        write_private(path, receipt)
    with pytest.raises(DraftConflict):
        await load_repair(fixture, repaired)


class OfflineWaitingImplementation(ImplementationService):
    """Real build admission only; never starts a native or provider worker."""
    async def _run(self, directory):
        await asyncio.Event().wait()


async def finish_offline_build(service, fixture, build, source=REPAIRED_EVENT_SOURCE):
    task = service.tasks.pop(build["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    directory = service._directory(fixture.guide_id, build["build_id"])
    project = ImplementationWorkspace(directory / "workspace", fixture.state["proposal"])
    project.write_file("application/__init__.py", source)
    check = offline_signed_check(project)
    update_receipt(directory, status="completed", cleanup_pending=False, cleanup_in_progress=False,
                   worker_stop_confirmed=True, stop_requested=False, receipt=check)
    return directory, project, await service.status(fixture.guide_id, state=fixture.state)


async def test_real_repair_admission_retains_capsule_and_source_ancestry_across_continuation(replay_fixture, monkeypatch):
    from launchkit.trial_feedback import repair_trial
    from launchkit.trial_replay import load_trial_replay, read_trial_replay
    fixture = replay_fixture
    fixture.state["messages"] = [{"role": "user", "text": "Keep confirmation and step-free seating."}]
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True})
    service = OfflineWaitingImplementation(fixture.implementation.root.parent, fixture.guide)
    fixture.rehearsal.implementation = service
    original = artifact_bytes(fixture.directory)
    first = await repair_trial(fixture.rehearsal, service, fixture.guide_id, 3, fixture.trial_id,
        fixture.original.id, fixture.original.source["source_digest"], "Repair the full seating manifest.")
    try:
        directory, _, checked = await finish_offline_build(service, fixture, first)
        frozen = json.loads((directory / "input.json").read_text())
        original_pair = {"build_id": fixture.original.id, "source_digest": fixture.original.source["source_digest"]}
        assert frozen["trial_replay_ancestry"] == [original_pair]
        assert frozen["previous_implementation"]["copied_source_digest"] == original_pair["source_digest"]
        assert frozen["trial_replay"]["original_build_id"] == fixture.original.id
        assert "read_trial_replay" in frozen["request"]
        tools = BuildTools(directory)
        context = await tools.context({})
        assert context["messages"] == fixture.state["messages"]
        assert PRIVATE_CANARY not in json.dumps(context)
        assert (await tools.trial_replay({"section": "job", "limit": 16000}))["content"].find("step_free") >= 0
        for illegal in ({"path": "../input.json"}, {"capsule_id": fixture.trial_id}, {"offset": True}):
            value = await tools.trial_replay(illegal)
            assert value["ok"] is False and PRIVATE_CANARY not in json.dumps(value)
        second = await service.submit(fixture.guide_id, 3, "Also support accented guest names.",
                                       expected_selected_build_id=first["build_id"])
        next_directory, next_project, next_checked = await finish_offline_build(service, fixture, second)
        next_input = json.loads((next_directory / "input.json").read_text())
        assert next_input["trial_replay_ancestry"] == [original_pair,
            {"build_id": first["build_id"], "source_digest": checked["source_digest"]}]
        assert next_input["previous_implementation"]["copied_source_digest"] == checked["source_digest"]
        first_page = read_trial_replay(directory, frozen["trial_replay"], section="job", limit=16000)
        next_page = read_trial_replay(next_directory, next_input["trial_replay"], section="job", limit=16000)
        assert first_page["content"] == next_page["content"]
        loaded = await load_trial_replay(service, fixture.guide_id, fixture.trial_id, fixture.original.id,
            fixture.original.source["source_digest"], second["build_id"], next_checked["source_digest"], state=fixture.state)
        assert loaded["initial"]["text"] == LONG_JOB and loaded["initial"]["fixtures"] == FIXTURES
        assert PRIVATE_CANARY not in "\n".join(row.content for row in next_project.export_files(require_verified=True))
        assert next_checked["acceptance_trusted"] is False and next_checked["ready_to_package"] is False
    finally:
        for task in list(service.tasks.values()):
            task.cancel()
        await asyncio.gather(*service.tasks.values(), return_exceptions=True)
    assert artifact_bytes(fixture.directory) == original


@pytest.mark.parametrize("defect", ["deep_json", "missing_blueprint", "missing_initial",
                                     "missing_observations", "initial_empty", "continuation_empty",
                                     "changed_continuation", "invalid_turn_status", "ancestry_at_limit"])
async def test_corrupt_or_full_auxiliary_replay_never_blocks_real_source_continuation(replay_fixture, monkeypatch, defect):
    """Saved replay corruption is isolated from actual checked source custody."""
    fixture = replay_fixture
    repaired = attach_repair(fixture)
    original_source = artifact_bytes(repaired.workspace.project)
    original_trial = artifact_bytes(fixture.directory)
    frozen = copy.deepcopy(repaired.input)
    path = repaired.directory / "trial-replay/capsule.json"
    if defect == "deep_json":
        # Valid bounded JSON, deeper than Python's parser recursion limit.
        body = (b'{"payload":' + b'[' * 1500 + b'0' + b']' * 1500 + b'}')
        assert len(body) < 32_000_000
        path.write_bytes(body)
        frozen["trial_replay"]["capsule_sha256"] = hashlib.sha256(body).hexdigest()
    elif defect.startswith("missing_") or defect in {
            "initial_empty", "continuation_empty", "changed_continuation", "invalid_turn_status"}:
        capsule = copy.deepcopy(repaired.capsule)
        if defect.startswith("missing_"):
            capsule["payload"].pop(defect.removeprefix("missing_"))
        elif defect == "initial_empty":
            capsule["payload"]["initial"] = {}
        elif defect == "continuation_empty":
            capsule["payload"]["continuations"] = [{}]
        elif defect == "changed_continuation":
            capsule["payload"]["continuations"][-1]["text"] = "An unrelated replacement job"
        else:
            capsule["payload"]["turns"][-1]["status"] = {"invalid": "failed"}
        capsule["payload_sha256"] = fingerprint(capsule["payload"])
        body = json.dumps(capsule, ensure_ascii=True, allow_nan=False,
                          sort_keys=True, separators=(",", ":")).encode()
        path.write_bytes(body)
        frozen["trial_replay"].update(capsule_sha256=hashlib.sha256(body).hexdigest(),
                                    payload_sha256=capsule["payload_sha256"])
    else:
        # Synthetic private ancestry exercises its bounded retention threshold.
        chain = [copy.deepcopy(frozen["trial_replay_ancestry"][0]),
                 *[{"build_id": str(uuid.uuid4()), "source_digest": "c" * 64} for _ in range(999)]]
        frozen["trial_replay_ancestry"] = chain
        frozen["previous_implementation"].update(build_id=chain[-1]["build_id"],
                                                  copied_source_digest=chain[-1]["source_digest"])
    write_private(repaired.directory / "input.json", frozen)
    tools = BuildTools(repaired.directory)
    status = await fixture.implementation.status(fixture.guide_id, state=fixture.state)
    assert status["source_verified"] is True and status["source_digest"] == repaired.source["source_digest"]
    if defect != "ancestry_at_limit":
        assert status["trial_replay"]["available"] is False
        value = await tools.trial_replay({"section": "job"})
        assert value["ok"] is False and "error" in value
        assert PRIVATE_CANARY not in json.dumps(value)
    else:
        assert status["trial_replay"]["available"] is True
    fixture.state["messages"] = [{"role": "user", "text": "Keep the authored event source."}]
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True})
    service = OfflineWaitingImplementation(fixture.implementation.root.parent, fixture.guide)
    admitted = await service.submit(fixture.guide_id, 3, "Improve the confirmation wording.",
                                      expected_selected_build_id=repaired.id)
    try:
        directory = service._directory(fixture.guide_id, admitted["build_id"])
        retained = json.loads((directory / "input.json").read_text())
        assert "trial_replay" not in retained and "trial_replay_ancestry" not in retained
        assert retained["trial_replay_unavailable"] == (
            "The previous selected-job evidence is unavailable. Source continuation is preserved.")
        assert retained["previous_implementation"]["build_id"] == repaired.id
        assert retained["previous_implementation"]["copied_source_digest"] == repaired.source["source_digest"]
        project = ImplementationWorkspace(directory / "workspace", fixture.state["proposal"])
        assert project.read_file("application/__init__.py") == REPAIRED_EVENT_SOURCE
        assert not (directory / "trial-replay").exists()
        context = await BuildTools(directory).context({})
        assert PRIVATE_CANARY not in json.dumps(context)
        assert artifact_bytes(repaired.workspace.project) == original_source
        assert artifact_bytes(fixture.directory) == original_trial
    finally:
        for task in list(service.tasks.values()):
            task.cancel()
        await asyncio.gather(*service.tasks.values(), return_exceptions=True)


@pytest.mark.parametrize("defect", ["original_status_dict", "original_kind_list"])
async def test_malformed_original_history_rejects_capture_but_preserves_diagnostic_repair(replay_fixture, monkeypatch, defect):
    from launchkit.trial_feedback import repair_trial
    from launchkit.trial_replay import TrialReplayUnavailable
    fixture = replay_fixture
    if defect == "original_status_dict":
        ledger = json.loads((fixture.directory / "turns.json").read_text())
        ledger["turns"][-1]["status"] = {"malformed": "failed"}
        fixture.receipt["turns"] = copy.deepcopy(ledger["turns"])
        write_private(fixture.directory / "turns.json", ledger)
    else:
        rows = copy.deepcopy(fixture.observations)
        rows[1]["kind"] = ["callback_requested"]
        rewrite_observations(fixture, rows)
    write_private(fixture.directory / "receipt.json", fixture.receipt)
    if defect == "original_status_dict":
        assert json.loads((fixture.directory / "turns.json").read_text())["turns"] == fixture.receipt["turns"]
    else:
        assert [json.loads(line) for line in (fixture.directory / "application-observations.jsonl").read_text().splitlines()] == fixture.receipt["application_observations"]
    original = artifact_bytes(fixture.directory)
    with pytest.raises(TrialReplayUnavailable):
        capture(fixture)
    fixture.state["messages"] = [{"role": "user", "text": "Repair the observed seating failure."}]
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True})
    service = OfflineWaitingImplementation(fixture.implementation.root.parent, fixture.guide)
    fixture.rehearsal.implementation = service
    admitted = await repair_trial(fixture.rehearsal, service, fixture.guide_id, 3, fixture.trial_id,
        fixture.original.id, fixture.original.source["source_digest"], "Repair the observed seating failure.")
    try:
        directory = service._directory(fixture.guide_id, admitted["build_id"])
        frozen = json.loads((directory / "input.json").read_text())
        assert "trial_replay" not in frozen and "trial_replay_ancestry" not in frozen
        assert "complete saved job is unavailable" in frozen["request"]
        assert "do not describe them as an exact replay" in frozen["request"]
        assert frozen["previous_implementation"]["build_id"] == fixture.original.id
        assert frozen["previous_implementation"]["copied_source_digest"] == fixture.original.source["source_digest"]
        project = ImplementationWorkspace(directory / "workspace", fixture.state["proposal"])
        assert project.read_file("application/__init__.py") == EVENT_SOURCE
        assert not (directory / "trial-replay").exists()
        assert artifact_bytes(fixture.directory) == original
    finally:
        for task in list(service.tasks.values()):
            task.cancel()
        await asyncio.gather(*service.tasks.values(), return_exceptions=True)
