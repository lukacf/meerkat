import copy
import json

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.guide import GuideService, compact_guidance_context
from launchkit.guide_models import default_question, review_readiness
from launchkit.guide_store import GuideStore
from launchkit.implementation_evidence import project_implementation_evidence
from launchkit.projects import fingerprint
from launchkit.server import GuideResponse


def requested(identity, outcome):
    return {"id": identity, "outcome": outcome, "disposition": "open",
            "supported_subset": "", "extension": "Implement the requested application adapter.",
            "source_message_ids": ["user-request"], "decision_message_ids": []}


def brief():
    return {"facts": {
        "outcome": {"summary": "A weather and astronomy assistant with Telegram and a custom UI.",
                    "status": "confirmed", "source_message_ids": ["user-request"]},
        "interfaces": {"summary": "Telegram and a custom UI; keep both channels.",
                       "status": "confirmed", "source_message_ids": ["user-request"]},
        "ownership": {"summary": "A conversation owner consults weather and astronomy specialists.",
                      "status": "confirmed", "source_message_ids": ["user-request"]}},
        "requirements": [requested("telegram", "Talk to the assistant through Telegram"),
                         requested("custom-ui", "Use an authenticated custom web interface")]}


@pytest.fixture
def state():
    return {"id": "setup-identity", "version": 4, "brief": brief(),
            "proposal": starter_blueprint().model_dump(mode="json"), "blueprint": None}


def checked(state):
    outcomes = [{"id": row["id"], "text": row["outcome"], "status": "tested",
                 "source_paths": [f"application/{row['id'].replace('-', '_')}.py"],
                 "test_ids": [f"test_channels.py::Channels.test_{row['id'].replace('-', '_')}"],
                 "evidence": "The protected test exercised routing and isolated user context with a fake transport.",
                 "remaining_dependencies": ["Configure destination credentials and verify the actual channel."]}
                for row in state["brief"]["requirements"]]
    if not outcomes:
        outcomes = [{"id": "requested-outcome", "text": state["brief"]["facts"]["outcome"]["summary"],
                     "status": "source", "source_paths": ["application/__init__.py"], "test_ids": [],
                     "evidence": "The source implements the requested behavior.", "remaining_dependencies": []}]
    return {"build_id": "build-identity", "guide_id": state["id"], "guide_version": state["version"],
            "blueprint_hash": fingerprint(state["proposal"] or state["blueprint"]), "current": True,
            "status": "completed", "source_verified": True, "source_digest": "a" * 64,
            "phase": "complete", "stop_requested": False, "cleanup_pending": False,
            "cleanup_in_progress": False, "cleanup_custody_lost": False, "worker_stop_confirmed": True,
            "acceptance_trusted": True, "acceptance_current": True, "ready_to_package": True,
            "summary": "Everything is live and ready", "connected": True, "running": True,
            "acceptance": {"issuer": "mobkit-launch-acceptance-host", "status": "pass",
                "source_digest": "a" * 64,
                "blueprint_digest": fingerprint(state["proposal"] or state["blueprint"]),
                "brief_digest": fingerprint(state["brief"]), "scope": "offline",
                "requirements": outcomes, "tests_run": max(1, len(outcomes)), "review_round": 2,
                "test_results": [{"id": row["test_ids"][0], "status": "pass"} for row in outcomes if row["test_ids"]],
                "remaining_dependencies": ["The live device and destination have not been exercised."],
                "counterexamples": [], "signature": "verified-at-the-host-boundary"}}


def checked_repair(state):
    status = checked(state)
    status["trial_replay"] = {"available": True, "guide_id": state["id"],
        "guide_version": state["version"], "build_id": status["build_id"],
        "source_digest": status["source_digest"], "rehearsal_id": "selected-trial",
        "original_build_id": "original-build", "original_source_digest": "b" * 64,
        "payload_sha256": "c" * 64, "detail": "PRIVATE_TRIAL_SENTINEL",
        "fixtures": {"private": "PRIVATE_TRIAL_SENTINEL"}}
    return status


def test_guide_replay_offer_preserves_host_bindings_without_disclosing_trial_payload(state):
    from launchkit.guide import implementation_action_context
    status = checked_repair(state)
    before = copy.deepcopy(status)
    evidence = project_implementation_evidence(state, status)
    replay = evidence["trial_replay"]
    assert replay["available"] and replay["action"] == "Try the same job again"
    assert replay["rehearsal_id"] == "selected-trial" and replay["payload_sha256"] == "c" * 64
    assert replay["original_build_id"] == "original-build" and replay["build_id"] == status["build_id"]
    assert "Model decisions may differ" in replay["scope"]
    assert "PRIVATE_TRIAL_SENTINEL" not in json.dumps(evidence)
    assert not evidence["live_ready"] and not evidence["running"]
    actions = implementation_action_context(state, evidence, {"available": True}, True)
    assert actions["replay_available"] and actions["replay_action"] == replay["action"]
    assert status == before


@pytest.mark.parametrize("field,value", [
    ("current", False), ("source_verified", False), ("source_digest", "d" * 64),
    ("status", "running"), ("cleanup_pending", True), ("cleanup_in_progress", True),
    ("cleanup_custody_lost", True), ("worker_stop_confirmed", False)])
def test_guide_does_not_offer_replay_when_current_source_or_cleanup_is_unverified(state, field, value):
    status = checked_repair(state)
    status[field] = value
    evidence = project_implementation_evidence(state, status)
    assert evidence["trial_replay"] == {"available": False, "action": "Try the same job again"}


@pytest.mark.parametrize("field,value", [
    ("guide_id", "another-guide"), ("guide_version", True), ("guide_version", 3),
    ("build_id", "another-build"), ("source_digest", "d" * 64),
    ("rehearsal_id", "../other-trial"), ("original_source_digest", "invalid"),
    ("payload_sha256", "invalid"), ("available", "true")])
def test_guide_does_not_offer_replay_from_mismatched_or_malformed_host_reference(state, field, value):
    status = checked_repair(state)
    status["trial_replay"][field] = value
    evidence = project_implementation_evidence(state, status)
    assert not evidence["trial_replay"]["available"]


def test_offline_acceptance_is_not_a_connected_or_running_system(state):
    before = copy.deepcopy(state)
    evidence = project_implementation_evidence(state, checked(state))
    assert state == before
    assert evidence["freshness"] == "current" and evidence["status"] == "offline-tested"
    assert evidence["source_verified"] and evidence["acceptance_current"] and evidence["ready_to_package"]
    assert not evidence["connected"] and not evidence["running"] and not evidence["live_ready"]
    assert {row["id"] for row in evidence["requirements"]} == {"telegram", "custom-ui"}
    for row in evidence["requirements"]:
        assert row["implemented"] and row["offline_tested"]
        assert row["source_paths"] and row["test_ids"] and row["remaining_dependencies"]
        assert not row["connected"] and not row["running"] and row["disposition"] == "open"
    assert any("live device" in gap for gap in evidence["remaining_gaps"])


def test_cleanup_observation_preserves_accepted_outcomes_without_offering_another_trial(state):
    status = checked(state)
    before = project_implementation_evidence(state, status)
    status.update(cleanup_pending=True, cleanup_in_progress=True, worker_stop_confirmed=False,
                  ready_to_package=False, detail="private-worker-error-sentinel")
    evidence = project_implementation_evidence(state, status)
    assert evidence["requirements"] == before["requirements"]
    assert evidence["source_verified"] and evidence["acceptance_current"]
    assert evidence["status"] == "offline-tested" and not evidence["ready_to_package"]
    lifecycle = evidence["lifecycle"]
    assert lifecycle["reason_code"] == "cleanup_in_progress" and lifecycle["content_current"]
    assert not lifecycle["build_gate_clear"] and not lifecycle["application_trial_gate_clear"]
    assert not lifecycle["resume_available"] and not lifecycle["retry_cleanup_available"]
    assert "sentinel" not in json.dumps(lifecycle)


def test_source_checked_failed_build_keeps_the_actual_trial_boundary_separate_from_acceptance(state):
    from launchkit.application_snapshot import trial_build_current

    status = checked(state)
    status.update(status="failed", ready_to_package=False)
    status["acceptance"]["status"] = "needs_repair"
    status["acceptance"]["counterexamples"] = ["A requested behavior still needs repair."]
    evidence = project_implementation_evidence(state, status)
    assert evidence["lifecycle"]["resume_available"] and evidence["lifecycle"]["application_trial_gate_clear"]
    assert trial_build_current(status, state, status["build_id"])
    assert evidence["status"] == "needs-repair" and not evidence["ready_to_package"]
    assert not evidence["connected"] and not evidence["running"] and not evidence["live_ready"]
    assert evidence["requirements"] == project_implementation_evidence(state, {**status, "phase": None})["requirements"]


@pytest.mark.parametrize("changed", ["revision", "guide", "proposal", "current"])
def test_stale_build_bindings_cannot_satisfy_current_outcomes(state, changed):
    status = checked(state)
    if changed == "revision":
        state["version"] += 1
    elif changed == "guide":
        status["guide_id"] = "another-setup"
    elif changed == "proposal":
        state["proposal"]["project"]["name"] = "A different proposal"
    else:
        status["current"] = False
    evidence = project_implementation_evidence(state, status)
    assert evidence["freshness"] == "stale" and not evidence["current"]
    assert not evidence["source_verified"] and not evidence["acceptance_current"]
    assert not evidence["ready_to_package"] and evidence["source_digest"] is None
    assert all(not row["implemented"] and not row["test_ids"] for row in evidence["requirements"])


@pytest.mark.parametrize("changed", ["source", "blueprint", "brief", "text", "missing", "extra", "duplicate"])
def test_independent_acceptance_requires_exact_source_brief_and_outcome_bindings(state, changed):
    status = checked(state)
    receipt = status["acceptance"]
    if changed in {"source", "blueprint", "brief"}:
        receipt[f"{changed}_digest"] = "b" * 64
    elif changed == "text":
        receipt["requirements"][0]["text"] = "Use only an admin console instead"
    elif changed == "missing":
        receipt["requirements"].pop()
    elif changed == "extra":
        receipt["requirements"].append({**receipt["requirements"][0], "id": "unrequested-channel"})
    else:
        receipt["requirements"][1] = copy.deepcopy(receipt["requirements"][0])
    evidence = project_implementation_evidence(state, status)
    assert evidence["current"] and evidence["source_verified"]
    assert evidence["status"] == "source-checked"
    assert not evidence["acceptance_current"] and not evidence["ready_to_package"]
    assert all(not row["implemented"] and not row["offline_tested"] for row in evidence["requirements"])


def test_changed_requirement_and_new_channel_invalidate_old_acceptance_even_if_flags_claim_current(state):
    status = checked(state)
    state["brief"]["requirements"][0]["outcome"] += " for multiple isolated users"
    state["brief"]["requirements"].append(requested("slack", "Also support the staff Slack channel"))
    evidence = project_implementation_evidence(state, status)
    assert not evidence["acceptance_current"] and not evidence["ready_to_package"]
    assert {row["id"] for row in evidence["requirements"]} == {"telegram", "custom-ui", "slack"}
    assert all(not row["offline_tested"] for row in evidence["requirements"])


@pytest.mark.parametrize("changed", ["untrusted", "not-current", "issuer", "source-unverified", "no-digest"])
def test_model_claims_and_saved_flags_do_not_promote_readiness(state, changed):
    status = checked(state)
    if changed == "untrusted":
        status["acceptance_trusted"] = False
    elif changed == "not-current":
        status["acceptance_current"] = False
    elif changed == "issuer":
        status["acceptance"]["issuer"] = "implementation-agent"
    elif changed == "source-unverified":
        status["source_verified"] = False
    else:
        status["source_digest"] = None
    evidence = project_implementation_evidence(state, status)
    assert not evidence["ready_to_package"] and not evidence["live_ready"]
    assert not evidence["connected"] and not evidence["running"]


def test_legacy_source_checked_build_is_not_independently_accepted(state):
    status = checked(state)
    status.pop("acceptance")
    status["acceptance_trusted"] = status["acceptance_current"] = False
    evidence = project_implementation_evidence(state, status)
    assert evidence["status"] == "source-checked" and evidence["source_verified"]
    assert not evidence["acceptance_current"] and not evidence["ready_to_package"]
    assert all(not row["implemented"] and not row["offline_tested"] for row in evidence["requirements"])


def test_partial_acceptance_keeps_passing_outcome_evidence_and_actual_repair_gaps(state):
    status = checked(state)
    status["ready_to_package"] = False
    status["acceptance"]["status"] = "needs_repair"
    status["acceptance"]["counterexamples"] = ["The custom UI accepts another user's conversation ID."]
    broken = status["acceptance"]["requirements"][1]
    broken.update(status="unresolved", source_paths=[], test_ids=[],
                  evidence="The isolation counterexample still reproduces.",
                  remaining_dependencies=["Implement and test per-user access checks in the custom UI."])
    evidence = project_implementation_evidence(state, status)
    assert evidence["status"] == "needs-repair" and evidence["acceptance_current"]
    assert evidence["requirements"][0]["offline_tested"]
    assert not evidence["requirements"][1]["implemented"]
    assert evidence["requirements"][1]["remaining_dependencies"] == broken["remaining_dependencies"]
    assert any("another user's" in gap for gap in evidence["remaining_gaps"])
    assert not evidence["ready_to_package"]


def test_source_inspection_and_tested_behavior_are_distinct(state):
    status = checked(state)
    row = status["acceptance"]["requirements"][0]
    row.update(status="source", test_ids=[], remaining_dependencies=[])
    evidence = project_implementation_evidence(state, status)
    assert evidence["requirements"][0]["implemented"] and not evidence["requirements"][0]["offline_tested"]
    assert evidence["requirements"][0]["phase"] == "implemented"
    assert "independent acceptance" in evidence["requirements"][0]["remaining_dependencies"][0]


def test_tested_status_requires_actual_matching_passing_test_ids(state):
    status = checked(state)
    status["acceptance"]["requirements"][0]["test_ids"].append("test_forged.py::Fake.test_everything")
    evidence = project_implementation_evidence(state, status)
    assert not evidence["requirements"][0]["offline_tested"]
    assert evidence["requirements"][0]["test_ids"] == []


def test_explicit_deferral_and_cancellation_remain_user_linked_without_dropping_channels(state):
    state["brief"]["requirements"][1].update(disposition="deferred", decision_message_ids=["later-user-choice"])
    status = checked(state)
    status["acceptance"]["requirements"][1].update(status="unresolved", source_paths=[], test_ids=[],
                                                   remaining_dependencies=[])
    before = copy.deepcopy(state)
    evidence = project_implementation_evidence(state, status)
    assert state == before
    row = evidence["requirements"][1]
    assert row["disposition"] == "deferred" and not row["active"]
    assert row["decision_message_ids"] == ["later-user-choice"]
    assert {item["id"] for item in evidence["requirements"]} == {"telegram", "custom-ui"}


def test_legacy_brief_tracking_is_not_an_application_schema(state):
    state["brief"]["requirements"] = []
    evidence = project_implementation_evidence(state, checked(state))
    assert evidence["acceptance_current"]
    row = evidence["requirements"][0]
    assert row["id"] == "requested-outcome" and row["tracking_only"]
    assert row["implemented"] and not row["offline_tested"]
    assert state["brief"]["requirements"] == []


def test_untrusted_old_report_does_not_invent_an_outcome_in_an_empty_brief(state):
    status = checked(state)
    state["brief"] = {"facts": {}, "requirements": []}
    status["acceptance"]["requirements"] = [{"id": "requested-outcome", "text": "A foreign project"}]
    status["acceptance_trusted"] = False
    evidence = project_implementation_evidence(state, status)
    assert evidence["requirements"] == [] and not evidence["acceptance_current"]


def test_compact_context_includes_bound_findings_once_without_source_or_duplicate_brief(state):
    evidence = project_implementation_evidence(state, checked(state))
    evidence["requirements"][0]["evidence"] = "A unique protected observation"
    context = {"brief": state["brief"], "implementation_evidence": evidence,
               "implementation_status": {"source": "private dump", "checks": ["huge output"]}}
    before = copy.deepcopy(context)
    compact = compact_guidance_context(context)
    encoded = json.dumps(compact)
    assert encoded.count("A unique protected observation") == 1
    assert compact["brief"] == state["brief"]
    assert compact["readiness"] == review_readiness(state["brief"])
    for row, original in zip(compact["implementation_evidence"]["requirements"], evidence["requirements"], strict=True):
        assert {"outcome", "source_message_ids", "decision_message_ids", "disposition"}.isdisjoint(row)
        assert row["id"] == original["id"]
        assert row["evidence"] == original["evidence"]
        assert row["source_paths"] == original["source_paths"]
        assert row["test_ids"] == original["test_ids"]
    assert "private dump" not in encoded and "huge output" not in encoded
    assert compact["implementation_evidence"]["source_digest"] == "a" * 64
    assert compact["implementation_evidence"]["requirements"][0]["test_ids"]
    assert context == before


class EvidenceAwareAgent:
    def __init__(self):
        self.context = self.prompt = None
        self.blueprint = None
        self.reply = None

    async def message(self, prompt, blueprint, *, guide_context):
        self.prompt, self.context = prompt, copy.deepcopy(guide_context)
        self.blueprint = copy.deepcopy(blueprint)
        if self.reply:
            return self.reply(guide_context, blueprint)
        question = default_question("destination")
        question.update(id="remaining_deployment", title="Where should we verify the two working interfaces?",
                        description="The offline tests passed; deployment connections remain to be verified.")
        return {"guide": {"message": "Both requested interfaces remain in the system; their current evidence is offline.",
                           "brief": copy.deepcopy(guide_context["brief"]), "question": question,
                           "design_changed": False, "summary": "", "warnings": []}, "proposal": None}


@pytest.fixture
async def service_state(tmp_path, monkeypatch):
    import launchkit.guide as module
    monkeypatch.setattr(module, "planner_capability", lambda: {"available": True})
    agent = EvidenceAwareAgent()
    service = GuideService(GuideStore(tmp_path), agent)
    state = await service.create()
    state.update(brief=brief(), proposal=starter_blueprint().model_dump(mode="json"))
    state["messages"].append({"id": "user-request", "role": "user", "text": "Build Telegram and a custom UI with specialists."})
    state = service.store.save(state, state["version"])
    status = checked(state)
    async def provider(detached):
        return copy.deepcopy(status)
    service.implementation_evidence_provider = provider
    return service, state, status, agent


async def test_guide_read_projects_current_evidence_without_persisting_it(service_state):
    service, state, _, _ = service_state
    result = await service.get(state["id"])
    assert result["implementation_evidence"]["acceptance_current"]
    assert result["brief"] == state["brief"] and not result["readiness"]["ready"]
    assert "implementation_evidence" not in service.store.get(state["id"])


async def test_injected_provider_cannot_mutate_the_guide_or_spoof_binding(service_state):
    service, state, status, _ = service_state
    def mutating_provider(detached):
        detached["brief"]["requirements"].clear()
        detached["version"] += 100
        status["guide_version"] = detached["version"]
        return status
    service.implementation_evidence_provider = mutating_provider
    result = await service.get(state["id"])
    assert result["brief"] == state["brief"] and result["version"] == state["version"]
    assert not result["implementation_evidence"]["current"]


@pytest.mark.parametrize("error", ["missing", "unknown", "failure"])
async def test_missing_or_unavailable_provider_preserves_the_guide_without_private_details(service_state, error):
    from launchkit.implementation import NoApplicationBuild
    service, state, _, _ = service_state
    error = {"missing": NoApplicationBuild("No build"), "unknown": LookupError("Private unknown build failure"),
             "failure": RuntimeError("Secret provider failure details")}[error]
    def failing_provider(detached):
        raise error
    service.implementation_evidence_provider = failing_provider
    result = await service.get(state["id"])
    assert result["implementation_evidence"]["freshness"] == ("missing" if isinstance(error, NoApplicationBuild) else "unavailable")
    assert "Secret provider failure" not in json.dumps(result)
    assert "Private unknown build failure" not in json.dumps(result)
    assert result["brief"] == state["brief"]


async def test_guide_turn_receives_actual_gaps_and_preserves_combined_channels_without_promoting_claims(service_state):
    from launchkit.architect import ArchitectService

    service, state, status, agent = service_state
    status.update(source="PRIVATE_SOURCE_DUMP", checks=["PRIVATE_CHECK_DUMP"])
    for row in status["acceptance"]["requirements"]:
        row["evidence"] = "Protected offline observation for " + row["id"]
    value = GuideResponse(version=state["version"], action="chat", text="What remains before this is usable?",
                            selected=[], blueprint=None, stage=None)
    result = await service.respond(state["id"], value)
    assert agent.context["operation_request_id"] == fingerprint({"guide_id": state["id"], **value.model_dump(mode="json")})
    evidence = agent.context["implementation_evidence"]
    assert evidence["acceptance_current"] and evidence["requirements"][0]["offline_tested"]
    assert evidence["remaining_gaps"] and not evidence["live_ready"]
    assert "Read the complete current guided_setup via inspect_blueprint with include_schema:false before substantive guidance or publication." in agent.prompt
    outgoing = json.loads(agent.prompt.split("\n\nCurrent setup turn:\n", 1)[1])
    assert set(outgoing) == {"guide_id", "guide_version", "action", "current_user_message"}
    assert outgoing["guide_id"] == state["id"] and outgoing["guide_version"] == value.version
    assert outgoing["current_user_message"] == compact_guidance_context(agent.context)["current_user_message"]
    assert agent.prompt.count('"implementation_evidence"') == 0

    # Exercise the actual inspect_blueprint callback, not just the fake agent's
    # captured context. The turn envelope points to this single evidence view.
    observer = ArchitectService(service.store.root / "inspection")
    observer.set_context(agent.blueprint)
    observer.guide_context = copy.deepcopy(agent.context)
    observer.guide_edit_turn = {"token": agent.context["edit_base_token"]}
    before_inspection = copy.deepcopy((observer.context, observer.guide_context))
    inspected = await observer.inspect({"include_schema": False})
    guided = inspected["guided_setup"]
    assert guided == compact_guidance_context(agent.context)
    assert inspected["blueprint"] == agent.blueprint
    assert guided["guide_id"] == state["id"] and guided["guide_version"] == value.version
    delivered = guided["implementation_evidence"]
    assert delivered["build_id"] == status["build_id"]
    assert delivered["source_digest"] == status["source_digest"]
    assert delivered["blueprint_digest"] == fingerprint(agent.blueprint)
    assert delivered["brief_digest"] == fingerprint(state["brief"])
    assert delivered["status"] == "offline-tested" and delivered["scope"] == "offline"
    assert delivered["acceptance_current"] and delivered["source_verified"]
    assert not delivered["live_ready"] and not delivered["connected"] and not delivered["running"]
    assert delivered["remaining_gaps"] == evidence["remaining_gaps"]
    assert status["acceptance"]["remaining_dependencies"][0] in delivered["remaining_gaps"]
    assert len(delivered["remaining_gaps"]) == len(set(delivered["remaining_gaps"]))
    assert {row["id"] for row in delivered["requirements"]} == {"telegram", "custom-ui"}
    encoded = json.dumps(inspected)
    assert encoded.count('"implementation_evidence"') == 1
    for row, original in zip(delivered["requirements"], status["acceptance"]["requirements"], strict=True):
        assert row["id"] == original["id"] and row["offline_tested"]
        assert row["source_paths"] == original["source_paths"] and row["test_ids"] == original["test_ids"]
        assert row["remaining_dependencies"] == original["remaining_dependencies"]
        assert not row["connected"] and not row["running"]
        assert encoded.count(original["evidence"]) == 1
        assert {"outcome", "source_message_ids", "decision_message_ids", "disposition"}.isdisjoint(row)
    assert guided["brief"] == state["brief"]
    for raw in (status["summary"], status["source"], status["checks"][0], status["acceptance"]["signature"]):
        assert raw not in encoded and raw not in agent.prompt
    assert (observer.context, observer.guide_context) == before_inspection
    assert observer.runtime is None and not observer.root.exists()
    assert result["brief"] == state["brief"]
    assert {row["id"] for row in result["brief"]["requirements"]} == {"telegram", "custom-ui"}
    assert all(row["disposition"] == "open" for row in result["brief"]["requirements"])
    assert result["question"]["id"] == "remaining_deployment"
    assert result["implementation_evidence"]["freshness"] == "stale"
    assert "implementation_evidence" not in service.store.get(state["id"])


async def test_manual_proposal_change_rebinds_evidence_before_native_guide_turn(service_state):
    service, state, _, agent = service_state
    manual = copy.deepcopy(state["proposal"])
    manual["project"]["name"] = "The revised astronomy system"
    def reply(context, blueprint):
        return {"guide": {"message": "I kept the revised proposal and both channels.",
                          "question": default_question("destination"), "brief": context["brief"],
                          "design_changed": True}, "proposal": blueprint}
    agent.reply = reply
    value = GuideResponse(version=state["version"], action="manual", text="Keep this project name.",
                            selected=[], blueprint=manual, stage=None)
    result = await service.respond(state["id"], value)
    assert agent.context["operation_request_id"] == fingerprint({"guide_id": state["id"], **value.model_dump(mode="json")})
    assert agent.context["implementation_evidence"]["freshness"] == "stale"
    assert not agent.context["implementation_evidence"]["acceptance_current"]
    assert result["proposal"]["project"]["name"] == manual["project"]["name"]
    assert result["brief"]["requirements"] == state["brief"]["requirements"]


async def test_prior_persisted_projection_is_discarded_on_read(service_state):
    service, state, _, _ = service_state
    state["implementation_evidence"] = {"live_ready": True, "running": True, "current": True}
    saved = service.store.save(state, state["version"])
    service.implementation_evidence_provider = None
    result = await service.get(saved["id"])
    assert not result["implementation_evidence"]["live_ready"]
    assert result["implementation_evidence"]["freshness"] == "unavailable"


async def test_receipt_prose_cannot_authorize_cancelling_a_requested_channel(service_state):
    service, state, _, agent = service_state
    def cancel(context, blueprint):
        changed = copy.deepcopy(context["brief"])
        changed["requirements"][0].update(disposition="cancelled", extension="",
                                          decision_message_ids=["user-request"])
        return {"guide": {"message": "The tests passed, so I removed Telegram.",
                          "question": default_question("destination"), "brief": changed}, "proposal": None}
    agent.reply = cancel
    value = GuideResponse(version=state["version"], action="chat", text="Keep working on the remaining gaps.",
                            selected=[], blueprint=None, stage=None)
    with pytest.raises(ValueError, match="cannot authorize"):
        await service.respond(state["id"], value)
    assert agent.context["operation_request_id"] == fingerprint({"guide_id": state["id"], **value.model_dump(mode="json")})
    assert service.store.get(state["id"])["brief"] == state["brief"]


async def test_publication_persists_new_requested_scope_before_old_acceptance_can_count(service_state):
    from launchkit.artifact_binding import inputs, matches
    service, state, original, agent = service_state
    bound = {"guide_id": state["id"], "guide_version": state["version"], **inputs(state)}
    def host_provider(detached):
        status = copy.deepcopy(original)
        status["current"] = matches(bound, detached)
        status["current_guide_version"] = detached["version"] if status["current"] else None
        return status
    service.implementation_evidence_provider = host_provider
    # A progress question changes only the transcript. The real host can prove
    # that the same canonical specification still binds the original receipt.
    harmless = GuideResponse(version=state["version"], action="chat", text="What remains before this is usable?",
                               selected=[], blueprint=None, stage=None)
    retained = await service.respond(state["id"], harmless)
    assert agent.context["operation_request_id"] == fingerprint({"guide_id": state["id"], **harmless.model_dump(mode="json")})
    assert retained["brief"] == state["brief"] and retained["proposal"] == state["proposal"]
    assert retained["implementation_evidence"]["ready_to_package"]
    def requested_scope(context, blueprint):
        changed = copy.deepcopy(context["brief"])
        request_id = context["current_user_message"]["id"]
        changed["requirements"].append({"id": "nightly-summary", "outcome": "Send a nightly summary",
            "disposition": "open", "supported_subset": "", "extension": "Implement the requested recurring summary",
            "source_message_ids": [request_id], "decision_message_ids": []})
        return {"guide": {"message": "I retained both interfaces and added your nightly summary requirement.",
            "brief": changed, "question": default_question("destination"), "design_changed": False}, "proposal": None}
    agent.reply = requested_scope
    request = GuideResponse(version=retained["version"], action="chat", text="Also send a nightly summary.",
                              selected=[], blueprint=None, stage=None)
    revised = await service.respond(state["id"], request)
    assert agent.context["operation_request_id"] == fingerprint({"guide_id": state["id"], **request.model_dump(mode="json")})
    saved = service.store.get(state["id"])
    requirement = saved["brief"]["requirements"][-1]
    assert requirement["id"] == "nightly-summary"
    assert requirement["source_message_ids"] == [revised["messages"][-2]["id"]]
    assert not revised["implementation_evidence"]["current"]
    assert not revised["implementation_evidence"]["ready_to_package"]
    assert saved["proposal"] == state["proposal"]
    assert original["guide_version"] == state["version"]
