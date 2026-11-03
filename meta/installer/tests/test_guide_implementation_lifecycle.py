"""Offline guide turns use current build custody, not saved worker claims.

All guide/build stores below are synthetic temporary fixtures. No native worker,
model provider, executable checker, browser or production store is used.
"""
import asyncio
from copy import deepcopy
import json
from types import SimpleNamespace
import uuid

import pytest

from launchkit.artifact_binding import proposal_hash
from launchkit.architect import ArchitectService
from launchkit.blueprint import starter_blueprint
from launchkit.guide import GuideService, compact_guidance_context
from launchkit.guide_models import DesignBrief, conversation_message, default_question, review_readiness
from launchkit.guide_store import GuideStore
from launchkit.implementation import ACCEPTANCE_POLICY_VERSION, ImplementationService, NoApplicationBuild, update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict, fingerprint
from launchkit.server import GuideResponse
from launchkit.templates.storage_setup import write_private


PRIVATE_PID = 912340987
PRIVATE_VALUES = ("private-worker-detail-sentinel", "private-cleanup-detail-sentinel",
                  "private-progress-sentinel", "private-build-request-sentinel",
                  "/private-fixture/source-path-sentinel", "private-check-output-sentinel",
                  "https://private-console-sentinel.invalid", str(PRIVATE_PID),
                  "private-provider-error-sentinel", "detached-provider-mutation-sentinel")
SCOPE_KEYS = ("brief", "proposal", "blueprint", "stage", "status", "readiness", "answers",
              "manual_edits", "fact_history", "reopened_topics", "completed_stages")


class RecordingArchitect:
    def __init__(self):
        self.calls = []
        self.guide = None

    async def message(self, prompt, blueprint, *, guide_context):
        self.calls.append({"prompt": prompt, "blueprint": deepcopy(blueprint),
                           "context": deepcopy(guide_context),
                           "guide_turn_running": self.guide.lock.locked()})
        # A missing proposal requires a real question. A saved proposal can
        # receive operational guidance even while application work remains open.
        question = None if guide_context["saved_plan_available"] else default_question("purpose")
        return {"guide": {"message": "The application design is preserved. Live jobs and installation still need separate verification.",
                          "question": question, "brief": deepcopy(guide_context["brief"]),
                          "design_changed": False, "summary": "Review the host's current build status.",
                          "warnings": []}, "proposal": None}


@pytest.fixture
async def lifecycle_guide(tmp_path, monkeypatch):
    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {
        "available": True, "provider": "openai", "reason": "Synthetic offline host capability"})
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {
        "available": True, "provider": "openai", "reason": "Synthetic offline host capability"})
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/offline-fixture-checker")
    architect = RecordingArchitect()
    guide = GuideService(GuideStore(tmp_path / "guide"), architect)
    architect.guide = guide
    state = await guide.create()
    request = conversation_message("user", "Keep the admin Console, staff Slack and a custom staff UI for incident coordination.",
                                   stage="purpose", action="chat")
    state["messages"].append(request)
    fact = lambda summary: {"summary": summary, "status": "confirmed", "source_message_ids": [request["id"]]}
    # The fixture's lifecycle controls start after context discovery. Defaults
    # remain proposals, not confirmations attributed to the interface request.
    proposed = lambda summary: {"summary": summary, "status": "proposed", "source_message_ids": []}
    state.update(proposal=starter_blueprint().model_dump(mode="json"), fact_history=[],
                 brief=DesignBrief.model_validate({"facts": {
                     "interfaces": fact("An admin Console, staff Slack and a custom staff UI; keep all three."),
                     "outcome": fact("Coordinate incidents without losing staff context."),
                     "audience": proposed("An internal incident-response staff team."),
                     "ownership": proposed("An operator contact retains incident context; domain specialists prepare evidence and an independent reviewer challenges it."),
                     "workflow": proposed("Triage an incident, gather relevant evidence, review the proposed response and return a staff brief."),
                     "inputs": proposed("Staff messages and operator-provided incident documents; no external account access is assumed."),
                     "knowledge": proposed("Local incident notes with explicit sources and corrections."),
                     "authority": proposed("Prepare recommendations and drafts; external writes require a later explicit operator decision."),
                     "destination": proposed("A local Mac development installation for the first version.")},
                     "requirements": [{"id": identity, "outcome": outcome, "disposition": "open",
                         "extension": "Implement and independently check the application behavior.",
                         "source_message_ids": [request["id"]]}
                         for identity, outcome in (("admin", "Keep the admin Console"),
                             ("staff_slack", "Staff use Slack"), ("staff_ui", "Staff use a custom UI"))]
                 }).model_dump(mode="json"))
    # Match the store's canonical key order before deriving ordered topic lists.
    state["brief"]["facts"] = dict(sorted(state["brief"]["facts"].items()))
    state["readiness"] = review_readiness(state["brief"])
    assert not state["readiness"]["decision_blockers"]
    assert len(state["readiness"]["implementation_blockers"]) == 3
    state = guide.store.save(state, state["version"])
    implementation = ImplementationService(tmp_path / "builds", guide)
    directory = implementation._directory(state["id"], str(uuid.uuid4()))
    directory.mkdir(parents=True)
    frozen = {"guide_id": state["id"], "guide_version": state["version"],
              "blueprint": state["proposal"], "brief": state["brief"], "messages": state["messages"],
              "request": PRIVATE_VALUES[3], "acceptance_policy_version": ACCEPTANCE_POLICY_VERSION}
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", {
        "guide_id": state["id"], "guide_version": state["version"], "build_id": directory.name,
        "blueprint_hash": proposal_hash(state["proposal"]), "brief_hash": fingerprint(state["brief"]),
        "status": "failed", "phase": "implementation", "stop_requested": False,
        "cleanup_pending": False, "cleanup_in_progress": False, "worker_stop_confirmed": True,
        "detail": PRIVATE_VALUES[0], "cleanup_detail": PRIVATE_VALUES[1],
        "progress": [{"message": PRIVATE_VALUES[2]}], "request": PRIVATE_VALUES[3],
        "files": [{"path": PRIVATE_VALUES[4]}], "checks": [{"detail": PRIVATE_VALUES[5]}],
        "native_console_url": PRIVATE_VALUES[6], "worker_pid": PRIVATE_PID,
        # Saved claims are deliberately exaggerated. Real status must replace
        # these with current host verification before the guide can see them.
        "ready_to_package": True, "source_verified": True, "source_digest": "a" * 64,
        "acceptance_trusted": True, "acceptance_current": True,
        "acceptance": {"status": "pass", "issuer": "implementation-agent"},
        "connected": True, "running": True, "live_ready": True, "unresolved": []})
    write_private(directory.parent / "latest.json", {"build_id": directory.name})
    workspace = ImplementationWorkspace(directory / "workspace", state["proposal"])
    workspace.seed()
    harness = SimpleNamespace(guide=guide, architect=architect, implementation=implementation,
                              directory=directory, workspace=workspace, state=state,
                              provider_calls=[], status_snapshots=[], tasks=[], provider_error=False)

    async def evidence(detached):
        assert "implementation_evidence" not in detached
        harness.provider_calls.append(deepcopy(detached))
        status = await implementation.status(detached["id"], state=detached)
        harness.status_snapshots.append(deepcopy(status))
        # A provider must not get a mutable reference to the guide's canonical
        # application scope. This mutation occurs after status has bound it.
        detached["brief"]["facts"]["outcome"]["summary"] = PRIVATE_VALUES[-1]
        if harness.provider_error:
            raise RuntimeError(PRIVATE_VALUES[-2])
        return status

    guide.implementation_evidence_provider = evidence
    yield harness
    tasks = set(harness.tasks) | set(implementation.tasks.values())
    for task in tasks:
        task.cancel()
    await asyncio.gather(*tasks, return_exceptions=True)


def keep_task(harness):
    task = asyncio.create_task(asyncio.Event().wait())
    harness.implementation.tasks[harness.directory.name] = task
    harness.tasks.append(task)
    return task


def record_source_check(harness):
    """Create signed synthetic check metadata; do not execute application code."""
    with harness.workspace._locked():
        state = harness.workspace._state()
        receipt = {"status": "pass", "tests_run": 1, "ready_to_package": True,
                   "digest": harness.workspace._digest(harness.workspace._files(state)),
                   "checks": [{"name": "synthetic source-check fixture", "status": "pass",
                               "detail": "Fixture metadata only; no application execution."}]}
        receipt["signature"] = harness.workspace._signed(receipt)
        state["last_check"] = receipt
        harness.workspace._save_state(state)


async def operational_turn(harness):
    before = deepcopy(harness.guide.store.get(harness.state["id"]))
    result = await harness.guide.respond(before["id"], GuideResponse(
        version=before["version"], action="chat", text="What is the current build status and next available action?",
        selected=[], blueprint=None, stage=None))
    call = harness.architect.calls[-1]
    context = call["context"]
    prompt_context = json.loads(call["prompt"].split("\n\nCurrent setup turn:\n", 1)[1])
    assert prompt_context["current_user_message"] == compact_guidance_context(context)["current_user_message"]
    assert not {"implementation_evidence", "implementation_actions", "trial_evidence", "deployment_evidence"} & prompt_context.keys()
    observer = ArchitectService(harness.guide.store.root / "inspection")
    observer.set_context(call["blueprint"])
    observer.guide_context = context
    observer.guide_edit_turn = {"token": context["edit_base_token"]}
    inspected = (await observer.inspect({"include_schema": False}))["guided_setup"]
    assert inspected == compact_guidance_context(context)
    assert inspected["implementation_evidence"]["lifecycle"] == context["implementation_evidence"]["lifecycle"]
    for key in ("freshness", "source_verified", "acceptance_current", "ready_to_package", "live_ready"):
        assert inspected["implementation_evidence"][key] == context["implementation_evidence"][key]
    assert inspected["implementation_actions"] == context["implementation_actions"]
    assert call["guide_turn_running"]  # Available actions describe post-save UI choices.
    assert context["brief"] == before["brief"]
    if before["proposal"] or before["blueprint"]:
        assert call["blueprint"] == (before["proposal"] or before["blueprint"])
    for key in SCOPE_KEYS:
        assert result.get(key) == before.get(key), key
    saved = harness.guide.store.get(before["id"])
    assert saved["version"] == before["version"] + 1
    assert saved["messages"][:-2] == before["messages"]
    assert saved["messages"][-1]["text"] == "The application design is preserved. Live jobs and installation still need separate verification."
    assert not {"implementation_evidence", "implementation_actions", "trial_evidence", "deployment_evidence"} & saved.keys()
    serialized = json.dumps({"prompt": call["prompt"], "context": context, "saved": saved}, sort_keys=True)
    assert str(harness.directory) not in serialized
    for private in PRIVATE_VALUES:
        assert private not in serialized, private
    evidence = context["implementation_evidence"]
    assert evidence["scope"] == "offline"
    assert not evidence["connected"] and not evidence["running"] and not evidence["live_ready"]
    assert not evidence["acceptance_current"] and not evidence["ready_to_package"]
    assert all(not row["implemented"] and not row["offline_tested"]
               and not row["connected"] and not row["running"] for row in evidence["requirements"])
    assert not result["readiness"]["ready"]
    assert context["implementation_actions"]["build_requires_feedback"] is False
    assert "after this design turn is saved" in inspected["implementation_actions"]["admission"]
    assert "Interrupted or failed status takes" in call["prompt"]
    assert "cleanup custody is lost" in call["prompt"]
    assert "separate trial" in call["prompt"]
    assert "only prepare a handoff" in call["prompt"]
    return context["implementation_evidence"], context["implementation_actions"]


@pytest.mark.asyncio
@pytest.mark.parametrize("phase,reason", [("independent_review", "independent_review"),
                                         ("automatic_repair", "automatic_repair")])
async def test_owned_live_review_or_repair_reaches_actual_prompt_without_readiness_inflation(lifecycle_guide, phase, reason):
    h = lifecycle_guide
    keep_task(h)
    update_receipt(h.directory, status="running", phase=phase, cleanup_pending=True, worker_stop_confirmed=False)
    evidence, actions = await operational_turn(h)
    lifecycle = evidence["lifecycle"]
    assert (lifecycle["status"], lifecycle["phase"], lifecycle["reason_code"]) == ("running", phase, reason)
    assert lifecycle["content_current"] and lifecycle["control_guide_version"] == h.state["version"]
    assert actions["host_build_capable"] and actions["saved_proposal_available"]
    assert actions["stop_available"] and not actions["build_available"]
    assert not actions["retry_cleanup_available"] and not actions["resume_available"]
    assert not actions["application_trial_source_available"] and actions["unavailable_reason"] == lifecycle["detail"]
    assert any(PRIVATE_VALUES[2] in json.dumps(raw) for raw in h.status_snapshots)


@pytest.mark.asyncio
async def test_orphaned_review_is_interrupted_before_guide_selects_resume(lifecycle_guide):
    h = lifecycle_guide
    update_receipt(h.directory, status="running", phase="independent_review")
    evidence, actions = await operational_turn(h)
    assert (evidence["lifecycle"]["status"], evidence["lifecycle"]["phase"], evidence["lifecycle"]["reason_code"]) == (
        "interrupted", "independent_review", "build_interrupted")
    assert actions["build_available"] and actions["resume_available"]
    assert actions["build_action"] == "Resume saved build" and actions["unavailable_reason"] is None
    assert not actions["stop_available"] and not actions["retry_cleanup_available"]


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["brief", "proposal"])
async def test_stale_active_build_retains_exact_stop_control_without_claiming_current_source(lifecycle_guide, changed):
    h = lifecycle_guide
    keep_task(h)
    update_receipt(h.directory, status="running", phase="implementation", cleanup_pending=True, worker_stop_confirmed=False)
    state = deepcopy(h.state)
    if changed == "brief":
        state["brief"]["facts"]["outcome"]["summary"] += " Include incident severity in the report."
    else:
        state["proposal"]["project"]["name"] = "Revised incident coordination"
    h.guide.store.save(state, state["version"])
    evidence, actions = await operational_turn(h)
    lifecycle = evidence["lifecycle"]
    assert evidence["freshness"] == "stale" and not evidence["source_verified"]
    assert lifecycle["build_id"] == h.directory.name and lifecycle["guide_version"] == h.state["version"]
    assert lifecycle["control_guide_version"] == h.state["version"] + 1 and not lifecycle["content_current"]
    assert actions["stop_available"] and not actions["build_available"] and not actions["resume_available"]
    assert actions["build_action"] == "Build updated proposal"


@pytest.mark.asyncio
@pytest.mark.parametrize("status", ["failed", "interrupted", "completed"])
async def test_terminal_owned_cleanup_offers_retry_even_after_content_changes(lifecycle_guide, status):
    h = lifecycle_guide
    h.implementation.processes[h.directory.name] = SimpleNamespace(pid=PRIVATE_PID)
    update_receipt(h.directory, status=status, cleanup_pending=True, worker_stop_confirmed=False)
    state = deepcopy(h.state)
    state["proposal"]["project"]["name"] = "Updated proposal while cleanup is pending"
    h.guide.store.save(state, state["version"])
    evidence, actions = await operational_turn(h)
    assert evidence["lifecycle"]["reason_code"] == "cleanup_retry_needed"
    assert not evidence["lifecycle"]["content_current"]
    assert actions["retry_cleanup_available"] and not actions["stop_available"]
    assert not actions["build_available"] and not actions["resume_available"]
    assert not actions["application_trial_source_available"]
    assert actions["retry_cleanup_action"] == "Retry cleanup"


@pytest.mark.asyncio
async def test_lost_custody_overrides_saved_review_and_cleanup_progress(lifecycle_guide):
    h = lifecycle_guide
    update_receipt(h.directory, status="running", phase="independent_review", cleanup_pending=True,
                   cleanup_in_progress=True, worker_stop_confirmed=False)
    evidence, actions = await operational_turn(h)
    lifecycle = evidence["lifecycle"]
    assert lifecycle["status"] == "interrupted" and lifecycle["phase"] == "independent_review"
    assert lifecycle["cleanup_custody_lost"] and lifecycle["cleanup_in_progress"]
    assert lifecycle["reason_code"] == "cleanup_custody_lost"
    assert not any(actions[key] for key in ("build_available", "stop_available", "retry_cleanup_available",
                                          "resume_available", "application_trial_source_available"))
    assert "no longer owns" in actions["unavailable_reason"]


@pytest.mark.asyncio
@pytest.mark.parametrize("flags,reason", [({"stop_requested": True}, "stop_in_progress"),
    ({"stop_requested": True, "cleanup_in_progress": True}, "cleanup_in_progress")])
async def test_admitted_stop_or_cleanup_does_not_offer_a_duplicate_action(lifecycle_guide, flags, reason):
    h = lifecycle_guide
    keep_task(h)
    update_receipt(h.directory, status="running", phase="implementation", cleanup_pending=True,
                   worker_stop_confirmed=False, **flags)
    evidence, actions = await operational_turn(h)
    assert evidence["lifecycle"]["reason_code"] == reason
    assert not any(actions[key] for key in ("build_available", "stop_available", "retry_cleanup_available",
                                          "resume_available", "application_trial_source_available"))


@pytest.mark.asyncio
@pytest.mark.parametrize("saved_pending", [True, False])
async def test_real_retained_handle_blocks_build_until_exact_offline_cleanup_then_allows_resume(lifecycle_guide, monkeypatch, saved_pending):
    h = lifecycle_guide
    child = SimpleNamespace(pid=PRIVATE_PID)
    h.implementation.processes[h.directory.name] = child
    update_receipt(h.directory, status="failed", cleanup_pending=saved_pending, worker_stop_confirmed=False)
    evidence, actions = await operational_turn(h)
    assert evidence["lifecycle"]["cleanup_pending"] and not evidence["lifecycle"]["cleanup_custody_lost"]
    assert actions["retry_cleanup_available"] and not actions["build_available"]
    assert not actions["application_trial_source_available"]
    current = h.guide.store.get(h.state["id"])
    with pytest.raises(DraftConflict, match="cleanup is not confirmed"):
        await h.implementation.submit(current["id"], current["version"], expected_selected_build_id=h.directory.name)
    stopped = []

    async def stop_fixture(observed):
        assert observed is child
        stopped.append(observed)

    monkeypatch.setattr(h.implementation, "_stop_process", stop_fixture)
    await h.implementation.request_stop(current["id"], current["version"], h.directory.name)
    task = h.implementation.tasks.get(h.directory.name)
    if task:
        await task
    assert stopped == [child] and h.directory.name not in h.implementation.processes
    evidence, actions = await operational_turn(h)
    assert evidence["lifecycle"]["worker_stop_confirmed"] and not evidence["lifecycle"]["cleanup_pending"]
    assert evidence["lifecycle"]["stop_requested"] and evidence["lifecycle"]["content_current"]
    assert actions["build_available"] and actions["resume_available"]
    assert actions["build_action"] == "Resume saved build"
    assert not actions["stop_available"] and not actions["retry_cleanup_available"]
    assert not actions["application_trial_source_available"]


@pytest.mark.asyncio
async def test_provider_error_cannot_become_a_confirmed_missing_build(lifecycle_guide):
    h = lifecycle_guide
    h.provider_error = True
    evidence, actions = await operational_turn(h)
    assert h.status_snapshots  # The real temporary service was observed first.
    assert evidence["lifecycle"]["availability"] == "unavailable"
    assert not any(actions[key] for key in ("build_available", "stop_available", "retry_cleanup_available",
                                          "resume_available", "application_trial_source_available"))
    assert "unavailable" in actions["unavailable_reason"]


@pytest.mark.asyncio
@pytest.mark.parametrize("change", [
    {"status": "reviewing"}, {"phase": {"private": PRIVATE_VALUES[0]}},
    {"stop_requested": "false"}, {"worker_stop_confirmed": None},
    {"build_id": PRIVATE_VALUES[4]}, {"guide_version": PRIVATE_VALUES[0]},
])
async def test_invalid_real_receipt_cannot_authorize_actions_or_leak_private_metadata(lifecycle_guide, change):
    h = lifecycle_guide
    update_receipt(h.directory, **change)
    evidence, actions = await operational_turn(h)
    assert evidence["lifecycle"]["availability"] == "invalid"
    assert not any(actions[key] for key in ("build_available", "stop_available", "retry_cleanup_available",
                                          "resume_available", "application_trial_source_available"))
    assert "incomplete" in actions["unavailable_reason"]


@pytest.mark.asyncio
async def test_confirmed_no_build_offers_first_build_on_capable_host(lifecycle_guide):
    h = lifecycle_guide
    h.implementation.root = h.implementation.root / "confirmed-no-build"
    evidence, actions = await operational_turn(h)
    assert evidence["lifecycle"]["availability"] == "missing"
    assert actions["host_build_capable"] and actions["saved_proposal_available"] and actions["build_available"]
    assert actions["build_action"] == "Build application" and actions["unavailable_reason"] is None
    assert not actions["resume_available"] and not actions["application_trial_source_available"]


@pytest.mark.asyncio
async def test_no_saved_proposal_keeps_first_build_disabled_even_when_host_is_capable(lifecycle_guide):
    h = lifecycle_guide
    state = deepcopy(h.state)
    state.update(proposal=None, blueprint=None)
    state["messages"][-1]["text"] = (
        "I would like help with our team's work, but have not chosen its first job. "
        "Keep the admin Console, staff Slack and a custom staff UI.")
    state["brief"]["facts"]["outcome"].update(
        summary="The team wants help, but its first useful job is still undecided.", status="unresolved")
    state["readiness"] = review_readiness(state["brief"])
    h.guide.store.save(state, state["version"])
    h.implementation.root = h.implementation.root / "confirmed-no-build"
    _, actions = await operational_turn(h)
    assert actions["host_build_capable"] and not actions["saved_proposal_available"]
    assert not actions["build_available"] and not actions["resume_available"]
    assert not actions["application_trial_source_available"] and "concrete proposed agent system" in actions["unavailable_reason"]


@pytest.mark.asyncio
async def test_missing_isolated_checker_keeps_terminal_build_disabled(lifecycle_guide, monkeypatch):
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: None)
    evidence, actions = await operational_turn(lifecycle_guide)
    assert evidence["lifecycle"]["build_gate_clear"] and not actions["host_build_capable"]
    assert actions["saved_proposal_available"] and not actions["build_available"] and not actions["resume_available"]
    assert "isolated executable checker" in actions["unavailable_reason"]


@pytest.mark.asyncio
async def test_current_signed_source_enables_only_trial_source_gate_and_edits_remove_it(lifecycle_guide):
    h = lifecycle_guide
    record_source_check(h)
    update_receipt(h.directory, status="completed", phase="complete")
    evidence, actions = await operational_turn(h)
    assert evidence["source_verified"] and not evidence["acceptance_current"]
    assert actions["application_trial_source_available"] and actions["build_available"]
    assert actions["build_action"] == "Review and improve application"
    assert "separately checks its provider" in actions["application_trial"]
    assert "independent acceptance" in actions["local_install"]
    h.workspace.write_file("application/__init__.py", "# Changed after the synthetic source check.\n")
    evidence, actions = await operational_turn(h)
    assert not evidence["source_verified"] and not actions["application_trial_source_available"]
    assert actions["build_available"] and not actions["build_requires_feedback"]


@pytest.mark.asyncio
@pytest.mark.parametrize("kind,availability", [("unconfigured", "unavailable"), ("none", "unavailable"),
    ("generic_lookup", "unavailable"), ("empty", "invalid"), ("list", "invalid")])
async def test_unknown_provider_is_distinct_from_typed_confirmed_absence(lifecycle_guide, kind, availability):
    h = lifecycle_guide
    original = h.guide.implementation_evidence_provider

    async def unknown_provider(detached):
        await original(detached)  # Observe the real temporary status service first.
        if kind == "generic_lookup":
            raise LookupError(PRIVATE_VALUES[-2])
        return {"none": None, "empty": {}, "list": [PRIVATE_VALUES[0]]}[kind]

    h.guide.implementation_evidence_provider = None if kind == "unconfigured" else unknown_provider
    evidence, actions = await operational_turn(h)
    assert evidence["lifecycle"]["availability"] == availability
    assert not evidence["lifecycle"]["build_gate_clear"]
    assert not any(actions[key] for key in ("build_available", "stop_available", "retry_cleanup_available",
                                          "resume_available", "application_trial_source_available"))
    assert actions["unavailable_reason"]


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["malformed_json", "non_object", "invalid_id", "non_string_id",
                                  "unselected_history", "unreadable_selector"])
async def test_corrupt_selection_or_unselected_saved_work_requires_recovery_without_first_build_advice(lifecycle_guide, kind):
    h = lifecycle_guide
    selector = h.directory.parent / "latest.json"
    if kind == "malformed_json":
        selector.write_text(PRIVATE_VALUES[4], encoding="utf-8")
    elif kind == "non_object":
        write_private(selector, [PRIVATE_VALUES[0]])
    elif kind == "invalid_id":
        write_private(selector, {"build_id": PRIVATE_VALUES[4]})
    elif kind == "non_string_id":
        write_private(selector, {"build_id": PRIVATE_PID})
    else:
        selector.unlink()
        if kind == "unreadable_selector":
            selector.mkdir()
    evidence, actions = await operational_turn(h)
    assert h.directory.exists() and (h.directory / "input.json").exists()
    assert evidence["lifecycle"]["availability"] == "unavailable"
    assert evidence["lifecycle"]["reason_code"] == "selection_recovery_required"
    assert not any(actions[key] for key in ("build_available", "stop_available", "retry_cleanup_available",
                                          "resume_available", "application_trial_source_available"))
    assert "saved build selection needs recovery" in actions["unavailable_reason"]
    current = h.guide.store.get(h.state["id"])
    with pytest.raises(DraftConflict, match="Recover"):
        await h.implementation.submit(current["id"], current["version"], expected_selected_build_id=None)
    assert not h.implementation.tasks


@pytest.mark.asyncio
@pytest.mark.parametrize("during_preparation", ["ordinary", "other_history", "malformed_selector", "different_selection"])
async def test_first_build_admission_reserves_only_its_own_directory_and_rechecks_explicit_empty_selection(lifecycle_guide, monkeypatch, during_preparation):
    h = lifecycle_guide
    h.implementation.root = h.implementation.root / "new-first-build"
    seed = ImplementationWorkspace.seed
    reservations, native_replacements = [], []

    def seed_and_change_selection(workspace, *args, **kwargs):
        result = seed(workspace, *args, **kwargs)
        reserved = workspace.root.parent
        reservations.append(reserved)
        # Ordinary absence is valid only for this exact admission's directory.
        with pytest.raises(NoApplicationBuild):
            h.implementation._directory(h.state["id"], _preparing_build_id=reserved.name)
        if during_preparation == "other_history":
            (reserved.parent / str(uuid.uuid4())).mkdir()
        elif during_preparation == "malformed_selector":
            (reserved.parent / "latest.json").write_text(PRIVATE_VALUES[4], encoding="utf-8")
        elif during_preparation == "different_selection":
            write_private(reserved.parent / "latest.json", {"build_id": str(uuid.uuid4())})
        return result

    async def offline_worker(directory):
        # This replaces the entire native runner, not its worker internals.
        native_replacements.append(directory)
        await asyncio.Event().wait()

    monkeypatch.setattr(ImplementationWorkspace, "seed", seed_and_change_selection)
    monkeypatch.setattr(h.implementation, "_run", offline_worker)
    current = h.guide.store.get(h.state["id"])
    if during_preparation == "ordinary":
        admitted = await h.implementation.submit(current["id"], current["version"], expected_selected_build_id=None)
        await asyncio.sleep(0)
        assert admitted["status"] == "queued" and admitted["build_id"] == reservations[0].name
        assert native_replacements == reservations
        assert json.loads((reservations[0].parent / "latest.json").read_text())["build_id"] == admitted["build_id"]
        evidence, actions = await operational_turn(h)
        assert evidence["lifecycle"]["reason_code"] == "queued"
        assert actions["stop_available"] and not actions["build_available"]
    else:
        with pytest.raises(DraftConflict):
            await h.implementation.submit(current["id"], current["version"], expected_selected_build_id=None)
        await asyncio.sleep(0)
        assert not native_replacements and not h.implementation.tasks
        evidence, actions = await operational_turn(h)
        assert evidence["lifecycle"]["availability"] == "unavailable"
        assert not actions["build_available"] and not actions["application_trial_source_available"]
