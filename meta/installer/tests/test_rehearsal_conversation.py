from meta_trial_fixtures import selected, meta_trial_runtime
"""Exercise open-trial ownership, real turn bindings and retained offline state."""
import asyncio
import copy
import json
import os
from pathlib import Path
from types import SimpleNamespace
import uuid

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.implementation import read_receipt, update_receipt
from launchkit.projects import DraftConflict
from launchkit.rehearsal import RehearsalService, deadline_after, trial_turn_state
from launchkit.templates.storage_setup import write_private


class Guide:
    def __init__(self):
        self.identity = str(uuid.uuid4())
        self.state = {"id": self.identity, "version": 3,
            "proposal": starter_blueprint().model_dump(mode="json"), "brief": {"requirements": []}}

    async def get(self, identity):
        assert identity == self.identity
        return copy.deepcopy(self.state)


class WaitingService(RehearsalService):
    async def _run(self, directory):
        await asyncio.Event().wait()

    async def _stop_process(self, process):
        await _stop_fixture_process(process)


class Process:
    def __init__(self):
        self.returncode = None
        self.sent = []
        self.closed = asyncio.Event()
        self.stdin = self
        self.block = None

    def is_closing(self):
        return self.closed.is_set()

    def write(self, value):
        self.sent.append(json.loads(value))

    async def drain(self):
        if self.block:
            await self.block.wait()

    def close(self):
        self.returncode = 0
        self.closed.set()

    async def wait(self):
        await self.closed.wait()
        return self.returncode


async def _stop_fixture_process(process):
    """Model this caller's owned pipe closure, not OS PID/group cleanup.

    Exact-process cleanup is covered separately in test_owned_worker.py.
    Never send a synthetic conversation handle through the real stop helper.
    """
    assert isinstance(process, Process)
    process.stdin.close()
    assert await process.wait() == 0


@pytest.fixture
async def opened(tmp_path, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-provider-fixture")
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": True})
    service = WaitingService(tmp_path, Guide())
    receipt = await service.submit(service.guide.identity, 3, "Read the fixture.", keep_open=True)
    directory = service._directory(service.guide.identity, receipt["rehearsal_id"])
    process = Process()
    service.processes[directory.name] = process
    update_receipt(directory, status="waiting_input", trace_complete=True)
    write_private(directory / "turns.json", {"status": "waiting_input", "turn_count": 1, "current_turn": 1,
        "conversation_id": "returned-native-conversation", "idle_expires_at": deadline_after(300),
        "turns": [{"turn": 1, "command_id": "initial", "status": "settled", "text": "Read the fixture."}]})
    yield service, directory, process
    await service.shutdown()


async def test_open_opt_in_cannot_attach_to_a_flow_before_worker_admission(tmp_path, monkeypatch):
    service = WaitingService(tmp_path, Guide())
    with pytest.raises(ValueError, match="text trial"):
        await service.submit(service.guide.identity, 3, "Flow", flow_id="not-a-flow", keep_open=True)
    with pytest.raises(ValueError, match="text trial"):
        await service.submit(service.guide.identity, 3, "Text", keep_open=1)
    assert not service.tasks and not list(tmp_path.rglob("receipt.json"))


@pytest.fixture
def legacy_completed_trial(tmp_path):
    from launchkit.artifact_binding import inputs
    guide = Guide()
    source = {"build_id": str(uuid.uuid4()), "source_digest": "a" * 64}
    class Implementation:
        async def status(self, identity, *, state):
            assert identity == guide.identity and state == guide.state
            return {**source, "guide_id": identity, "guide_version": state["version"],
                    "current": True, "status": "completed", "source_verified": True}
    service = RehearsalService(tmp_path, guide, implementation=Implementation())
    identity = str(uuid.uuid4())
    directory = service._directory(guide.identity, identity)
    directory.mkdir(parents=True)
    write_private(directory.parent / "latest.json", {"rehearsal_id": identity})
    write_private(directory / "input.json", {"guide_id": guide.identity, "guide_version": 3,
        "blueprint": guide.state["proposal"], "brief": guide.state["brief"], "application_source": source})
    write_private(directory / "receipt.json", {"guide_id": guide.identity, "guide_version": 3,
        "rehearsal_id": identity, **inputs(guide.state), "mode": "application", "application_source": source,
        "status": "completed", "keep_open": False, "trace_complete": True,
        "worker_stop_confirmed": True, "application_stop_confirmed": True})
    write_private(directory / "turns.json", {"status": "running", "turn_count": 1, "current_turn": 1,
        "conversation_id": "desk:main", "idle_expires_at": None, "turns": [{"turn": 1,
            "command_id": "initial", "text": "Corroborate the saved synthetic maintenance release.",
            "status": "settled", "native_admission": {"interaction_id": "completed-interaction"},
            "terminal_interaction": {"interaction_id": "completed-interaction",
                                     "kind": "interaction_complete", "status": "completed"}}]})
    return service, directory


async def test_legacy_one_shot_status_reconciles_completion_without_rewriting_history(legacy_completed_trial):
    service, directory = legacy_completed_trial
    paths = [directory / name for name in ("input.json", "receipt.json", "turns.json")]
    paths.append(directory.parent / "latest.json")
    before = {path: path.read_bytes() for path in paths}

    for _ in range(2):
        result = await service.status(service.guide.identity)
        assert result["current"] is True
        assert result["status"] == result["session_status"] == "completed"
        assert result["can_continue"] is False and not result["live_ready"] and not result["connected"]
        assert result["turn_count"] == 1 and result["turns"][0]["status"] == "settled"
        assert result["turns"][0]["terminal_interaction"]["interaction_id"] == "completed-interaction"
    assert {path: path.read_bytes() for path in paths} == before
    assert trial_turn_state(directory)["status"] == "running"
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("file,key,value", [
    ("receipt.json", "status", "failed"), ("receipt.json", "status", "interrupted"),
    ("receipt.json", "status", "unavailable"),
    ("receipt.json", "keep_open", True), ("receipt.json", "keep_open", None),
    ("receipt.json", "trace_complete", False), ("receipt.json", "trace_complete", None),
    ("receipt.json", "worker_stop_confirmed", False), ("receipt.json", "worker_stop_confirmed", None),
    ("receipt.json", "application_stop_confirmed", False), ("receipt.json", "application_stop_confirmed", None),
    ("turns.json", "status", "failed"), ("turns.json", "status", "interrupted"),
    ("turns.json", "status", "completed"), ("turns.json", "status", "waiting_input"),
    ("turns.json", "status", None),
])
async def test_legacy_completion_projection_preserves_other_states_and_unconfirmed_cleanup(
        legacy_completed_trial, file, key, value):
    service, directory = legacy_completed_trial
    path = directory / file
    changed = json.loads(path.read_text())
    if value is None:
        changed.pop(key)
    else:
        changed[key] = value
    write_private(path, changed)
    before = {name: (directory / name).read_bytes() for name in ("receipt.json", "turns.json")}
    raw_receipt, raw_turns = read_receipt(directory), trial_turn_state(directory)

    result = await service.status(service.guide.identity)

    assert result["status"] == raw_receipt["status"]
    assert result["session_status"] == raw_turns.get("status")
    assert result["can_continue"] is False
    assert {name: (directory / name).read_bytes() for name in before} == before


async def test_serial_follow_up_admission_reserves_one_exact_turn_and_never_rewrites_frozen_input(opened):
    service, directory, process = opened
    original = (directory / "input.json").read_bytes()
    before = await service.status(service.guide.identity, directory.name)
    assert before["can_continue"] and before["turn_count"] == 1
    process.block = asyncio.Event()
    first = asyncio.create_task(service.continue_trial(service.guide.identity, 3, directory.name, 1, "What did it say?"))
    while not process.sent:
        await asyncio.sleep(0)
    second = asyncio.create_task(service.continue_trial(service.guide.identity, 3, directory.name, 1, "Duplicate admission"))
    process.block.set()
    result = await first
    with pytest.raises(DraftConflict):
        await second
    assert result["status"] == "running" and not result["can_continue"]
    assert result["turn_command"]["status"] == "sent" and result["turn_command"]["turn"] == 2
    assert len(process.sent) == 1 and process.sent[0]["text"] == "What did it say?"
    assert (directory / "input.json").read_bytes() == original
    # A concurrent worker receipt update cannot erase the host reservation.
    update_receipt(directory, status="waiting_input", frame_count=55)
    with pytest.raises(DraftConflict):
        await service.continue_trial(service.guide.identity, 3, directory.name, 1, "Again")
    assert len(process.sent) == 1


@pytest.mark.parametrize("changed", ["version", "brief", "proposal", "idle", "lifetime", "owner", "turn"])
async def test_follow_up_requires_current_owned_unexpired_exact_artifacts(opened, changed):
    service, directory, process = opened
    if changed == "version": service.guide.state["version"] += 1
    elif changed == "brief": service.guide.state["brief"]["requirements"] = [{"id": "changed"}]
    elif changed == "proposal": service.guide.state["proposal"]["agent_classes"][0]["model"] = "gpt-6-sol"
    elif changed == "idle":
        state = trial_turn_state(directory)
        state["idle_expires_at"] = deadline_after(-1)
        write_private(directory / "turns.json", state)
    elif changed == "lifetime": update_receipt(directory, expires_at=deadline_after(-1))
    elif changed == "owner": service.processes.pop(directory.name)
    with pytest.raises(DraftConflict):
        await service.continue_trial(service.guide.identity, 3, directory.name, 2 if changed == "turn" else 1, "Next")
    assert not process.sent
    if changed == "owner": service.processes[directory.name] = process


async def test_ambiguous_host_write_cannot_admit_a_second_paid_turn(opened):
    service, directory, process = opened
    async def failed(): raise ConnectionError("private transport diagnostic")
    process.drain = failed
    with pytest.raises(DraftConflict, match="uncertain"):
        await service.continue_trial(service.guide.identity, 3, directory.name, 1, "Next")
    result = await service.status(service.guide.identity, directory.name)
    assert result["turn_command"]["status"] == "uncertain" and not result["can_continue"]
    with pytest.raises(DraftConflict):
        await service.continue_trial(service.guide.identity, 3, directory.name, 1, "Retry with a new key")
    assert len(process.sent) == 1


async def test_maximum_multilingual_text_fits_the_bounded_utf8_host_wire(opened):
    service, directory, process = opened
    text = "å漢🙂" * 2000
    assert len(text) == 6000
    result = await service.continue_trial(service.guide.identity, 3, directory.name, 1, text)
    assert process.sent[0]["text"] == text and result["turn_command"]["status"] == "sent"
    assert len(json.dumps(process.sent[0], ensure_ascii=False).encode()) < 32000


async def test_checked_application_source_is_revalidated_and_admission_lease_is_held(opened):
    service, directory, process = opened
    from launchkit.projects import fingerprint
    from launchkit.artifact_binding import proposal_hash
    state = service.guide.state
    digest = "a" * 64
    update_receipt(directory, mode="application", application_source={"build_id": "build", "source_digest": digest})
    class Implementation:
        lock = asyncio.Lock()
        async def status(self, guide_id, *, state):
            assert self.lock.locked()
            return {"guide_id": guide_id, "guide_version": state["version"], "current": True,
                "current_guide_version": state["version"], "blueprint_hash": proposal_hash(state["proposal"]),
                "brief_hash": fingerprint(state["brief"]), "build_id": "build", "source_digest": "b" * 64,
                "status": "completed", "source_verified": True}
    service.implementation = Implementation()
    with pytest.raises(DraftConflict):
        await service.continue_trial(service.guide.identity, 3, directory.name, 1, "Read it again")
    assert not process.sent and not service.implementation.lock.locked()


async def test_last_fresh_guide_read_fences_a_change_during_source_status(opened):
    service, directory, process = opened
    original = service._guide
    reads = 0
    async def changing(identity):
        nonlocal reads
        reads += 1
        if reads == 2: service.guide.state["version"] += 1
        return await original(identity)
    service._guide = changing
    with pytest.raises(DraftConflict, match="before follow-up"):
        await service.continue_trial(service.guide.identity, 3, directory.name, 1, "Next")
    assert not process.sent


async def test_end_cleans_the_explicit_stale_trial_without_admitting_more_work(opened):
    service, directory, process = opened
    service.guide.state["version"] += 1
    result = await service.end(service.guide.identity, directory.name)
    assert result["status"] == "ending" and not result["can_continue"]
    assert process.sent == [{"kind": "end", "id": result["turn_command"]["id"]}]
    await service.end(service.guide.identity, directory.name)
    assert len(process.sent) == 1
    cancelled = await service.cancel(service.guide.identity, directory.name)
    assert cancelled["worker_stop_confirmed"] and cancelled["status"] == "interrupted"


async def test_unconfirmed_application_cleanup_blocks_new_trial_even_after_outer_worker_exit(opened):
    service, directory, process = opened
    await service.cancel(service.guide.identity, directory.name)
    update_receipt(directory, application_stop_confirmed=False, worker_stop_confirmed=True)
    with pytest.raises(DraftConflict, match="sandbox cleanup is unconfirmed"):
        await service.submit(service.guide.identity, 3, "Another first job", keep_open=True)
    assert len(list(directory.parent.glob("*/input.json"))) == 1


async def test_lost_host_cannot_claim_open_worker_reaped_or_promote_it_by_cancellation(opened):
    from launchkit.architect import ArchitectUnavailable
    service, directory, process = opened
    restarted = RehearsalService(service.root.parent, service.guide)
    result = await restarted.status(service.guide.identity, directory.name)
    assert result["status"] == "interrupted" and result["worker_stop_confirmed"] is False
    assert not result["trace_complete"] and not result["can_continue"]
    with pytest.raises(ArchitectUnavailable, match="ownership was lost"):
        await restarted.cancel(service.guide.identity, directory.name)
    assert read_receipt(directory)["worker_stop_confirmed"] is False and process.returncode is None
    with pytest.raises(DraftConflict, match="worker cleanup is unconfirmed"):
        await restarted.submit(service.guide.identity, 3, "Another trial", keep_open=True)
    assert len(list(directory.parent.glob("*/input.json"))) == 1


@pytest.mark.parametrize("confirmed", [None, False])
async def test_new_open_application_needs_positive_child_cleanup_even_after_outer_reaping(opened, confirmed):
    service, directory, _ = opened
    await service.cancel(service.guide.identity, directory.name)
    update_receipt(directory, mode="application", application_stop_confirmed=confirmed)
    with pytest.raises(DraftConflict, match="sandbox cleanup is unconfirmed"):
        await service.submit(service.guide.identity, 3, "Another trial")
    assert read_receipt(directory)["worker_stop_confirmed"] is True


@pytest.mark.parametrize("action", ["cancel", "shutdown"])
async def test_locally_cancelled_prestart_task_proves_no_application_child_was_admitted(tmp_path, monkeypatch, action):
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-provider-fixture")
    monkeypatch.setattr("launchkit.rehearsal.planner_capability", lambda: {"available": True})
    service = RehearsalService(tmp_path, Guide())
    async def missing_build(*args, **kwargs):
        raise LookupError("This ownership fixture has no checked application build.")
    service.implementation = SimpleNamespace(status=missing_build)
    receipt = await service.submit(service.guide.identity, 3, "Fixture", keep_open=True)
    directory = service._directory(service.guide.identity, receipt["rehearsal_id"])
    update_receipt(directory, mode="application", application_stop_confirmed=None)
    # submit has scheduled _run, but this task has not yielded to it yet.
    assert not service.processes and read_receipt(directory)["status"] == "queued"
    if action == "cancel":
        await service.cancel(service.guide.identity, directory.name)
    else:
        await service.shutdown()
    result = read_receipt(directory)
    assert result["status"] == "interrupted" and result["worker_stop_confirmed"] is True
    assert result["application_stop_confirmed"] is True and not service.processes


async def test_unexpected_outer_exit_invalidates_a_previously_complete_waiting_trace(tmp_path, monkeypatch):
    import launchkit.rehearsal as module
    monkeypatch.setenv("OPENAI_API_KEY", "isolated-provider-fixture")
    monkeypatch.setattr(module, "planner_capability", lambda: {"available": True})
    service = RehearsalService(tmp_path, Guide())
    monkeypatch.setattr(service, "_stop_process", _stop_fixture_process)
    async def create(*args, **kwargs):
        directory = Path(args[3])
        update_receipt(directory, status="waiting_input", trace_complete=True)
        process = Process()
        process.close()
        return process
    monkeypatch.setattr(module.asyncio, "create_subprocess_exec", create)
    receipt = await service.submit(service.guide.identity, 3, "Fixture", keep_open=True)
    task = service.tasks[receipt["rehearsal_id"]]
    await task
    directory = service._directory(service.guide.identity, receipt["rehearsal_id"])
    result = read_receipt(directory)
    assert result["status"] == "failed" and result["worker_stop_confirmed"] is True
    assert result["trace_complete"] is False and not service.tasks and not service.processes


@pytest.fixture
def native_seam(tmp_path, monkeypatch):
    """Actual execute/trace/tool wiring, with only native I/O replaced."""
    import httpx
    import meerkat_meta
    from meerkat_meta.identity_first_models import DurableAgentSpec, ManagedPeerEdge
    import launchkit.rehearsal_worker as worker
    from launchkit.application_preview import sanitize_application_preview
    spec = starter_blueprint()
    frozen = {"guide_id": str(uuid.uuid4()), "guide_version": 3,
        "blueprint": spec.model_dump(mode="json"), "text": "Remember the fictional item.",
        "flow_id": None, "params": None, "fixtures": {"files": {}, "notes": []}, "keep_open": True,
        "mode": "application", "application_source": {"build_id": str(uuid.uuid4()),
            "source_digest": "a" * 64, "deployment": {}},
        "application_fixtures": {}}
    write_private(tmp_path / "input.json", frozen)
    write_private(tmp_path / "receipt.json", {"status": "queued"})
    observed = SimpleNamespace(builds=0, starts=0, stops=0, shutdowns=0, prepares=0,
                              calls=[], traces=[], drafts=[], count=0, policy_change=None)
    class Application:
        def __init__(self, *args): pass
        async def start(self):
            observed.starts += 1
            return [{"identity": spec.topology.entrypoint, "tools": [{"name": "read_sample", "description": "Read real isolated state", "schema": {"type": "object"}}]}]
        def native_policy(self):
            compiled = worker._runtime(spec)
            classes = {row.id: row for row in spec.agent_classes}
            policy = {"roster": [DurableAgentSpec(identity=row["identity"], profile=row["profile"],
                display_name=row["name"], addressability="addressable", runtime_mode_override="turn_driven",
                labels={"class": row["classId"], "role": row["role"], "console_group": classes[row["classId"]].name}).to_dict()
                for row in compiled["agents"]], "edges": [ManagedPeerEdge(a=a, b=b).to_dict() for a,b in compiled["connections"]]}
            if observed.policy_change:
                observed.policy_change(policy)
            return policy
        async def call(self, identity, name, args):
            observed.count += 1
            return {"count": observed.count}
        async def stop(self): observed.stops += 1; return True
        def status(self): return {"available": True, "running": not observed.stops}
        def observations(self): return [{"kind": "callback_returned", "result": {"count": observed.count}}]
        @property
        def observation_revision(self): return 1 + observed.count * 2
        def application_preview(self):
            return {**sanitize_application_preview({"html": f"<main>Fictional items: {observed.count}</main>"}),
                "source_digest": frozen["application_source"]["source_digest"],
                "observation_revision": self.observation_revision}
    class Runtime:
        rust_http_base_url = "http://native-fixture"
        async def identity_bootstrap_status(self): return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self): observed.shutdowns += 1
    class Builder:
        def __getattr__(self, name):
            def configure(*args, **kwargs):
                if name == "session_service": self.sessions = args[0]
                return self
            return configure
        async def build(self):
            observed.builds += 1
            for agent in worker._runtime(spec)["agents"]:
                class Draft:
                    profile_name, resume_session_id = agent["profile"], None
                    def __init__(self): self.callbacks = {}; self.additional_instructions = []
                    def register_tool(self, name, callback, **kwargs): self.callbacks[name] = callback
                draft = Draft()
                await self.sessions.build_agent(draft)
                observed.drafts.append(draft)
            return Runtime()
    class Client:
        def __init__(self, **kwargs): assert kwargs["trust_env"] is False
        async def __aenter__(self): return self
        async def __aexit__(self, *args): pass
    async def capture(trace, client, base):
        observed.traces.append(trace); trace.ready.set(); await asyncio.Event().wait()
    async def prepare(*args): observed.prepares += 1
    async def request(client, method, url, **kwargs):
        payload = kwargs["json"]
        observed.calls.append(payload)
        draft = next(row for row in observed.drafts if "read_sample" in row.callbacks)
        result = await draft.callbacks["read_sample"]({})
        if len(observed.calls) == 1:
            draft.callbacks["notes_save"]({"content": "First turn stored this fictional note."})
        else:
            assert draft.callbacks["notes_search"]({"query": "fictional"})["notes"]
        number = len(observed.calls)
        frame = {"id": f"terminal-{number}", "cursor": f"console:{number}", "identity": spec.topology.entrypoint,
            "conversation_id": "actual-returned-conversation", "session_id": "actual-native-session",
            "interaction_id": f"actual-interaction-{number}", "kind": "interaction_complete", "status": "completed", "payload": {}}
        observed.traces[0].record({"type": "console_frame", "frame": frame})
        return SimpleNamespace(json=lambda: {"identity": spec.topology.entrypoint,
            "conversation_id": "actual-returned-conversation", "session_id": "actual-native-session",
            "interaction_id": frame["interaction_id"], "input_frame_id": f"input-{number}", "cursor": frame["cursor"]})
    monkeypatch.setattr("launchkit.application_trial.SandboxApplicationTrial", Application)
    monkeypatch.setattr(meerkat_meta.Meta, "builder", Builder)
    monkeypatch.setattr(worker, "QUIET_SECONDS", 0)
    monkeypatch.setattr(worker.NativeTrace, "capture", capture)
    monkeypatch.setattr(worker, "_native_request", request)
    monkeypatch.setattr(httpx, "AsyncClient", Client)
    return worker, tmp_path, observed


async def wait_for_worker(directory, status):
    for _ in range(3000):
        if read_receipt(directory).get("status") == status: return
        await asyncio.sleep(.01)
    pytest.fail(f"Worker did not reach {status}")


@pytest.mark.parametrize("change", [
    lambda policy: policy["roster"][0].update(profile="foreign-profile"),
    lambda policy: policy["edges"].append({"a": policy["roster"][0]["identity"], "b": "foreign:main"}),
    lambda policy: policy["roster"][0].update(placement="other-host"),
])
async def test_invalid_application_policy_stops_before_native_bootstrap(native_seam, change):
    from launchkit.application_trial import ApplicationTrialError
    worker, directory, observed = native_seam
    observed.policy_change = change
    with pytest.raises(ApplicationTrialError):
        await worker.execute(directory, asyncio.Event(), asyncio.Queue())
    assert observed.starts == 1 and observed.stops >= 1
    assert observed.prepares == observed.builds == observed.shutdowns == 0
    assert not observed.calls and not observed.traces and not observed.drafts
    assert read_receipt(directory)["application_stop_confirmed"] is True


async def test_two_native_turns_reuse_actual_conversation_application_and_fixture_state(native_seam):
    worker, directory, observed = native_seam
    commands = asyncio.Queue()
    task = asyncio.create_task(worker.execute(directory, asyncio.Event(), commands))
    await wait_for_worker(directory, "waiting_input")
    first = trial_turn_state(directory)
    assert first["turn_count"] == 1 and first["conversation_id"] == "actual-returned-conversation"
    assert observed.stops == 0 and observed.shutdowns == 0
    assert read_receipt(directory)["application_observations"][0]["result"]["count"] == 1
    first_receipt = read_receipt(directory)
    first_preview = first_receipt["application_preview"]
    frozen = json.loads((directory / "input.json").read_text())
    assert first_preview["observation_revision"] == first_receipt["application_observation_revision"] == 3
    assert first_preview["trial_id"] == directory.name and first_preview["guide_version"] == frozen["guide_version"]
    assert first_preview["build_id"] == frozen["application_source"]["build_id"]
    assert first_preview["source_digest"] == frozen["application_source"]["source_digest"]
    assert "Fictional items: 1" in first_preview["html"]
    await commands.put({"kind": "continue", "id": str(uuid.uuid4()), "turn": 2, "expected_turn": 1, "text": "Read back that same item."})
    while trial_turn_state(directory).get("turn_count") != 2:
        await asyncio.sleep(.01)
    await commands.put({"kind": "end", "id": str(uuid.uuid4())})
    await asyncio.wait_for(task, 2)
    final = trial_turn_state(directory)
    assert final["turn_count"] == 2 and final["ended_reason"] == "user_ended"
    assert [row["native_admission"]["conversation_id"] for row in final["turns"]] == ["actual-returned-conversation"] * 2
    assert [row["terminal_interaction"]["interaction_id"] for row in final["turns"]] == ["actual-interaction-1", "actual-interaction-2"]
    assert observed.builds == observed.starts == observed.stops == observed.shutdowns == 1
    assert observed.count == 2 and read_receipt(directory)["application_stop_confirmed"]
    final_receipt = read_receipt(directory)
    assert final_receipt["application_observation_revision"] == final_receipt["application_preview"]["observation_revision"] == 5
    assert "Fictional items: 2" in final_receipt["application_preview"]["html"]
    assert first_preview["document_sha256"] != final_receipt["application_preview"]["document_sha256"]
    assert "Fictional items: 1" in first_preview["html"]
    assert observed.calls[0]["identity"] == observed.calls[1]["identity"]
    assert observed.calls[0]["idempotency_key"] != observed.calls[1]["idempotency_key"]
    assert all("conversation_id" not in row for row in observed.calls)


async def test_one_shot_native_turn_finishes_saved_session_after_cleanup(native_seam, monkeypatch):
    worker, directory, observed = native_seam
    frozen = json.loads((directory / "input.json").read_text())
    frozen["keep_open"] = False
    write_private(directory / "input.json", frozen)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)

    await asyncio.wait_for(worker.run(directory), 2)

    receipt, turns = read_receipt(directory), trial_turn_state(directory)
    assert receipt["status"] == turns["status"] == "completed" and receipt["trace_complete"]
    assert turns["turn_count"] == 1 and turns["idle_expires_at"] is None
    assert turns["turns"][0]["status"] == "settled"
    assert turns["turns"][0]["terminal_interaction"]["interaction_id"] == "actual-interaction-1"
    assert turns["conversation_id"] == "actual-returned-conversation"
    assert len(observed.calls) == observed.count == 1
    assert observed.builds == observed.starts == observed.stops == observed.shutdowns == 1
    assert receipt["application_stop_confirmed"] is True and receipt["native_console_url"] is None

    service = RehearsalService(directory / "host-state", Guide())
    monkeypatch.setattr(service, "_directory", lambda *args: directory)
    projected = await service.status(frozen["guide_id"], directory.name, state={
        "id": frozen["guide_id"], "version": frozen["guide_version"], "proposal": frozen["blueprint"]})
    assert projected["status"] == projected["session_status"] == "completed"
    assert projected["can_continue"] is False


@pytest.mark.parametrize("cleanup", ["native", "application"])
async def test_one_shot_cleanup_failure_keeps_saved_session_failed(native_seam, monkeypatch, cleanup):
    import meerkat_meta
    from launchkit.application_trial import SandboxApplicationTrial
    worker, directory, observed = native_seam
    frozen = json.loads((directory / "input.json").read_text())
    frozen["keep_open"] = False
    write_private(directory / "input.json", frozen)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    if cleanup == "application":
        async def stop(application):
            observed.stops += 1
            raise RuntimeError("Application cleanup failed")
        monkeypatch.setattr(SandboxApplicationTrial, "stop", stop)
    else:
        builder = meerkat_meta.Meta.builder
        class FailingCleanupBuilder(builder):
            async def build(self):
                runtime = await super().build()
                async def shutdown():
                    observed.shutdowns += 1
                    raise RuntimeError("Native cleanup failed")
                runtime.shutdown = shutdown
                return runtime
        monkeypatch.setattr(meerkat_meta.Meta, "builder", FailingCleanupBuilder)

    await asyncio.wait_for(worker.run(directory), 2)

    receipt, turns = read_receipt(directory), trial_turn_state(directory)
    assert receipt["status"] == turns["status"] == "failed" and not receipt["trace_complete"]
    assert turns["turn_count"] == 1 and turns["turns"][0]["status"] == "settled"
    assert turns["turns"][0]["terminal_interaction"]["interaction_id"] == "actual-interaction-1"
    assert len(observed.calls) == observed.count == 1
    assert observed.stops == observed.shutdowns == 1
    assert receipt["application_stop_confirmed"] is (cleanup != "application")


async def test_idle_expiry_ends_without_new_native_work_and_reaps_application(native_seam, monkeypatch):
    worker, directory, observed = native_seam
    monkeypatch.setattr(worker, "REHEARSAL_IDLE_SECONDS", .03)
    await asyncio.wait_for(worker.execute(directory, asyncio.Event(), asyncio.Queue()), 2)
    assert trial_turn_state(directory)["ended_reason"] == "idle_timeout"
    assert len(observed.calls) == 1 and observed.stops == observed.shutdowns == 1
    assert read_receipt(directory)["status"] == "completed"


async def test_end_received_before_first_native_admission_does_not_send_a_paid_turn(native_seam):
    worker, directory, observed = native_seam
    commands = asyncio.Queue()
    await commands.put({"kind": "end", "id": str(uuid.uuid4())})
    await worker.execute(directory, asyncio.Event(), commands)
    assert not observed.calls and trial_turn_state(directory)["turn_count"] == 0
    assert trial_turn_state(directory)["ended_reason"] == "user_ended"
    assert observed.stops == observed.shutdowns == 1


@pytest.mark.parametrize("ending", ["end", "idle"])
async def test_late_native_work_must_settle_before_end_or_idle_claims_completion(native_seam, monkeypatch, ending):
    worker, directory, observed = native_seam
    monkeypatch.setattr(worker, "REHEARSAL_IDLE_SECONDS", .03 if ending == "idle" else 300)
    commands = asyncio.Queue()
    task = asyncio.create_task(worker.execute(directory, asyncio.Event(), commands))
    await wait_for_worker(directory, "waiting_input")
    trace = observed.traces[0]
    identity = observed.calls[0]["identity"]
    trace.record({"type": "console_frame", "frame": {"id": "late-start", "cursor": "console:10", "kind": "run_started",
        "identity": identity, "run_id": "late-real-run", "payload": {}}})
    if ending == "end": await commands.put({"kind": "end", "id": str(uuid.uuid4())})
    await asyncio.sleep(.05)
    assert not task.done() and observed.stops == 0
    assert read_receipt(directory)["status"] == "running" and not read_receipt(directory)["trace_complete"]
    trace.record({"type": "console_frame", "frame": {"id": "late-finish", "cursor": "console:11", "kind": "run_completed",
        "identity": identity, "run_id": "late-real-run", "payload": {}}})
    await asyncio.wait_for(task, 2)
    assert observed.stops == observed.shutdowns == 1 and read_receipt(directory)["trace_complete"]


async def test_failed_final_settlement_never_publishes_a_completed_session(native_seam, monkeypatch):
    worker, directory, observed = native_seam
    commands = asyncio.Queue()
    execute = worker.execute
    settle = worker.NativeTrace.settle
    async def use_commands(path, stop, unused):
        return await execute(path, stop, commands)
    async def fail_final(trace, *args):
        if trial_turn_state(directory).get("status") == "ending":
            raise RuntimeError("Native capture failed after End")
        return await settle(trace, *args)
    monkeypatch.setattr(worker, "execute", use_commands)
    monkeypatch.setattr(worker.NativeTrace, "settle", fail_final)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    task = asyncio.create_task(worker.run(directory))
    await wait_for_worker(directory, "waiting_input")
    await commands.put({"kind": "end", "id": str(uuid.uuid4())})
    await asyncio.wait_for(task, 2)
    receipt, turns = read_receipt(directory), trial_turn_state(directory)
    assert receipt["status"] == turns["status"] == "failed" and not receipt["trace_complete"]
    assert turns["ended_reason"] == "user_ended" and turns["turn_count"] == 1
    assert turns["turns"][0]["terminal_interaction"]["interaction_id"] == "actual-interaction-1"
    assert receipt["application_stop_confirmed"] is True and observed.stops == observed.shutdowns == 1


async def test_ten_turn_cap_never_waits_or_admits_an_eleventh_request(native_seam, monkeypatch):
    worker, directory, observed = native_seam
    commands = asyncio.Queue()
    task = asyncio.create_task(worker.execute(directory, asyncio.Event(), commands))
    for turn in range(1, 10):
        while trial_turn_state(directory).get("turn_count") != turn:
            await asyncio.sleep(.005)
        await commands.put({"kind": "continue", "id": str(uuid.uuid4()), "turn": turn + 1, "expected_turn": turn, "text": f"Follow-up {turn}"})
    await asyncio.wait_for(task, 2)
    assert len(observed.calls) == 10 and trial_turn_state(directory)["ended_reason"] == "turn_limit"
    assert observed.stops == observed.starts == observed.shutdowns == 1


@pytest.mark.parametrize("late_work", [False, True])
async def test_total_lifetime_is_complete_only_if_the_actual_native_trace_is_settled(native_seam, monkeypatch, late_work):
    from launchkit.owned_worker import owned_job_deadline
    worker, directory, observed = native_seam
    # Reaching waiting_input takes tens of milliseconds, so a short lifetime
    # could expire before that state, or right after it and unseen by the
    # poll. The row ends the trial's lifetime itself once its turn is waiting.
    monkeypatch.setattr(worker, "REHEARSAL_SECONDS", 30)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    deadlines, request = [], worker._native_request
    async def admitted(*args, **kwargs):
        deadlines.append(owned_job_deadline.get())
        return await request(*args, **kwargs)
    monkeypatch.setattr(worker, "_native_request", admitted)
    task = asyncio.create_task(worker.run(directory))
    await wait_for_worker(directory, "waiting_input")
    if late_work:
        observed.traces[0].record({"type": "console_frame", "frame": {"id": "late-start", "cursor": "console:10",
            "kind": "run_started", "identity": observed.calls[0]["identity"], "run_id": "unfinished-native-run", "payload": {}}})
    deadlines[0].reschedule(asyncio.get_running_loop().time())
    await asyncio.wait_for(task, 2)
    assert deadlines[0].expired()
    receipt = read_receipt(directory)
    assert observed.stops == observed.shutdowns == 1 and len(observed.calls) == 1
    if late_work:
        assert receipt["status"] == "failed" and not receipt["trace_complete"]
        assert not trial_turn_state(directory)["stop_trace_settled"]
    else:
        assert receipt["status"] == "completed" and receipt["trace_complete"]
        assert trial_turn_state(directory)["ended_reason"] == "lifetime_limit"


async def test_cancel_during_admitted_native_work_retains_actual_admission_and_partial_trace(native_seam, monkeypatch):
    worker, directory, observed = native_seam
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    entered = asyncio.Event()
    async def admitted(*args, **kwargs):
        entered.set()
        return SimpleNamespace(json=lambda: {"interaction_id": "actual-unfinished-interaction", "identity": "lead:main",
            "conversation_id": "actual-conversation", "input_frame_id": "actual-input", "cursor": "console:1"})
    monkeypatch.setattr(worker, "_native_request", admitted)
    task = asyncio.create_task(worker.run(directory))
    await entered.wait()
    await asyncio.sleep(0)
    task.cancel()
    await task
    receipt, turns = read_receipt(directory), trial_turn_state(directory)
    assert receipt["status"] == "interrupted" and not receipt["trace_complete"]
    assert turns["turns"][0]["native_admission"]["interaction_id"] == "actual-unfinished-interaction"
    assert turns["turns"][0]["status"] == "interrupted" and not turns["turns"][0].get("terminal_interaction")
    assert observed.stops == observed.shutdowns == 1


async def test_changed_native_conversation_rejects_continuation_without_a_third_turn(native_seam, monkeypatch):
    worker, directory, observed = native_seam
    original = worker._native_request
    async def changed(*args, **kwargs):
        response = await original(*args, **kwargs)
        result = response.json()
        if len(observed.calls) == 2: result["conversation_id"] = "different-native-conversation"
        return SimpleNamespace(json=lambda: result)
    monkeypatch.setattr(worker, "_native_request", changed)
    commands = asyncio.Queue()
    task = asyncio.create_task(worker.execute(directory, asyncio.Event(), commands))
    await wait_for_worker(directory, "waiting_input")
    await commands.put({"kind": "continue", "id": str(uuid.uuid4()), "turn": 2, "expected_turn": 1, "text": "Next"})
    with pytest.raises(RuntimeError, match="conversation binding"):
        await task
    assert trial_turn_state(directory)["turns"][1]["status"] == "failed"
    assert observed.stops == observed.shutdowns == 1 and len(observed.calls) == 2


async def test_owned_stdin_parses_a_bounded_command_then_true_eof_cancels_waiting_worker(tmp_path, monkeypatch):
    import launchkit.rehearsal_worker as worker
    write_private(tmp_path / "input.json", {"keep_open": True})
    write_private(tmp_path / "receipt.json", {"status": "waiting_input"})
    received, stopped = asyncio.Event(), asyncio.Event()
    async def execute(directory, stop, commands):
        assert (await commands.get())["text"] == "A genuine follow-up"
        received.set()
        try: await asyncio.Event().wait()
        finally:
            assert stop.is_set(); stopped.set()
    read_fd, write_fd = os.pipe()
    monkeypatch.setattr(worker.sys, "stdin", SimpleNamespace(fileno=lambda: read_fd))
    monkeypatch.setattr(worker, "execute", execute)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    task = asyncio.create_task(worker.run(tmp_path, owned_stdin=True))
    try:
        os.write(write_fd, (json.dumps({"kind": "continue", "id": "owned-command", "turn": 2,
            "expected_turn": 1, "text": "A genuine follow-up"}) + "\n").encode())
        await asyncio.wait_for(received.wait(), 30)
        os.close(write_fd); write_fd = None
        await asyncio.wait_for(task, 2)
        assert stopped.is_set() and read_receipt(tmp_path)["status"] == "interrupted"
    finally:
        if write_fd is not None: os.close(write_fd)
        os.close(read_fd)
        if not task.done(): task.cancel(); await asyncio.gather(task, return_exceptions=True)
