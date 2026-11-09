"""A concrete first job gets a provisional proposal during continued discovery.

These offline contract tests use a recorded architect fixture. They verify saved
guide behavior and tool feedback, not model design quality or native execution.
"""
import copy

import pytest

from launchkit.architect import ArchitectService
from launchkit.blueprint import Blueprint
from launchkit.guide_models import validate_guide_publication
from test_guide import answer, fact, guide_client, respond, start


JOB = "Help our volunteer river-monitoring coordinator plan collections and flag unsafe conditions with original evidence."


def context_question():
    return {"id": "collection_audience", "stage": "purpose", "multiple": False,
        "title": "Who will rely on the collection plan?",
        "description": "This determines whether continuing planning context belongs to one person or a shared team.",
        "allow_text": True, "options": [
            {"id": "one", "label": "One coordinator", "description": "One person keeps the planning conversation and preferences."},
            {"id": "team", "label": "A shared volunteer team", "description": "Several people share collection plans and continuing team context."},
        ]}


def initial_context(*, action="answer", saved=False):
    message = {"id": "collection-job", "role": "user", "text": JOB, "intent_only": False}
    return {"action": action, "saved_plan_available": saved, "manual_plan_submitted": False,
        "current_stage": "purpose", "brief": {"facts": {}, "requirements": []},
        "evidence_messages": [message], "current_user_message": message}


def first_publication(context):
    current = context["current_user_message"]
    return {"question": context_question(), "message": "I can start a provisional design while we clarify who needs the plan.",
        "brief": {"facts": {"outcome": fact(JOB, current["id"])}, "requirements": []},
        "design_changed": False}


@pytest.mark.parametrize("action", ["answer", "chat"])
def test_confirmed_first_job_requires_an_initial_proposal_without_waiting_for_final_context(action):
    context = initial_context(action=action)
    publication = first_publication(context)
    before = copy.deepcopy((publication, context))
    with pytest.raises(ValueError, match="purpose-specific first draft"):
        validate_guide_publication(publication, context, None)
    assert (publication, context) == before
    # Missing audience, hosting and access do not prevent an early draft. The
    # native tool/service owns checking the arbitrary proposal, not this guard.
    candidate = {"arbitrary_checked_proposal": True}
    assert validate_guide_publication(publication, context, candidate)["question"] == context_question()


@pytest.mark.parametrize("status", ["unresolved", "proposed"])
def test_unclear_job_keeps_context_discovery_open_without_inventing_a_team(status):
    context = initial_context()
    publication = first_publication(context)
    publication["brief"]["facts"]["outcome"] = fact("The first useful collection job remains unclear.", None, status)
    publication["question"].update(id="first_collection_job", title="Which part of collection work should improve first?")
    assert validate_guide_publication(publication, context, None)["brief"]["facts"]["outcome"]["status"] == status


@pytest.mark.parametrize("saved", ["absent", None])
def test_legacy_unknown_saved_plan_flag_is_not_treated_as_confirmed_absence(saved):
    context = initial_context(saved=saved)
    if saved == "absent":
        context.pop("saved_plan_available")
    assert validate_guide_publication(first_publication(context), context, None)["design_changed"] is False


def test_first_draft_cannot_be_skipped_by_a_status_shaped_publication():
    context = initial_context(action="chat")
    publication = first_publication(context)
    context["brief"] = copy.deepcopy(publication["brief"])
    publication["question"] = None
    with pytest.raises(ValueError, match="brief is not ready for review"):
        validate_guide_publication(publication, context, None)


def test_first_draft_guard_preserves_existing_manual_and_changed_plan_checks():
    context = initial_context()
    publication = first_publication(context)
    publication["design_changed"] = True
    with pytest.raises(ValueError, match="changed plan"):
        validate_guide_publication(publication, context, None)


@pytest.mark.asyncio
async def test_actual_publication_tool_rejects_missing_initial_draft_with_actionable_feedback(tmp_path):
    architect = ArchitectService(tmp_path)
    architect.guide_context = initial_context()
    publication = first_publication(architect.guide_context)
    result = await architect.publish_guide_step(publication)
    assert result["valid"] is False and "propose_blueprint" in result["issue"]
    assert "provisional" in result["issue"] and architect.guide_result is None
    # This fixture represents an already checked staged proposal. No runtime,
    # compiler, source generation or model is started to exercise publication.
    architect.proposal = {"blueprint": {"arbitrary_checked_proposal": True}}
    result = await architect.publish_guide_step(publication)
    assert result["valid"] is True and result["stage"] == "purpose"
    assert architect.guide_result["question"] == context_question()


def test_actual_service_does_not_save_a_known_first_job_with_an_empty_proposal(guide_client):
    client, agent, service = guide_client
    state = start(client)
    before = copy.deepcopy(service.store.get(state["id"]))
    agent.reply = lambda context, _blueprint: {"guide": first_publication(context), "proposal": None}
    response = respond(client, state, action="answer", text=JOB)
    assert response.status_code == 422 and "purpose-specific first draft" in response.text
    assert service.store.get(state["id"]) == before
    assert agent.calls[-1]["guide_context"]["saved_plan_available"] is False
    assert before["proposal"] is None and before["blueprint"] is None


def save_initial_draft(client, agent):
    def reply(context, blueprint):
        publication = first_publication(context)
        publication["design_changed"] = True
        publication["brief"]["facts"].update({
            "audience": fact("The people who will use the plan are still to be decided.", None, "unresolved"),
            "destination": fact("Hosting and operator responsibilities are still to be decided.", None, "unresolved"),
        })
        blueprint["project"] = {"name": "Collection planning", "slug": "collection-planning", "description": JOB}
        # The roster is a host contract fixture, not an expected model answer.
        candidate = Blueprint.model_validate(blueprint).model_dump(mode="json")
        return {"guide": publication, "proposal": candidate}
    agent.reply = reply
    return answer(client, start(client), text=JOB)


def test_actual_service_saves_first_draft_and_context_choices_before_installation_readiness(guide_client):
    client, agent, service = guide_client
    state = save_initial_draft(client, agent)
    assert state["proposal"]["project"]["description"] == JOB
    assert state["question"] == context_question() and state["status"] == "question"
    assert state["brief"]["facts"]["outcome"]["status"] == "confirmed"
    assert state["brief"]["facts"]["audience"]["status"] == "unresolved"
    assert state["brief"]["facts"]["destination"]["status"] == "unresolved"
    assert state["readiness"]["context_ready"] is False and state["readiness"]["ready"] is False
    assert state["changes"] == ["Initial agent team proposed"]
    assert service.store.get(state["id"])["proposal"] == state["proposal"]


@pytest.mark.parametrize("publication_kind", ["question", "operational"])
def test_actual_service_preserves_later_no_change_question_or_operational_turn(guide_client, publication_kind):
    client, agent, service = guide_client
    state = save_initial_draft(client, agent)
    before = copy.deepcopy(service.store.get(state["id"]))
    def reply(context, _blueprint):
        assert context["saved_plan_available"] is True
        return {"guide": {"question": context_question() if publication_kind == "question" else None,
            "message": "The provisional plan remains open while we clarify the people and operating context.",
            "brief": copy.deepcopy(context["brief"]), "design_changed": False}, "proposal": None}
    agent.reply = reply
    state = answer(client, state, action="chat", text="What remains uncertain in this draft?")
    assert state["proposal"] == before["proposal"] and state["brief"] == before["brief"]
    assert state["changes"] == [] and state["readiness"] == before["readiness"]
    assert state["question"] == context_question(), "An operational update does not answer or discard the retained decision"
    assert state["status"] == before["status"] and state["version"] == before["version"] + 1
