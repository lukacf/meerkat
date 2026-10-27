"""Navigation retains evidence; an actual later answer can revise it."""
import copy

import pytest

from launchkit import guide as guide_module
from launchkit.blueprint import starter_blueprint
from launchkit.guide_models import REVIEW_TOPICS, conversation_message, starter_state
from launchkit.guide_store import GuideStore
from launchkit.server import GuideResponse


@pytest.mark.asyncio
async def test_audience_scale_revisit_preserves_the_job_and_blocks_acceptance_until_actual_revision(tmp_path, monkeypatch):
    capability = {"available": True, "provider": "openai", "reason": "Inert test fixture"}
    monkeypatch.setattr(guide_module, "planner_capability", lambda: capability)
    original = conversation_message("user", "Help one volunteer coordinator plan river sampling and identify unsafe conditions.",
                                    stage="purpose", action="answer")
    summaries = {
        "outcome": "Plan river sampling and identify unsafe conditions with evidence.",
        "audience": "One volunteer coordinator uses the plan.",
        "interfaces": "The existing console.",
        "ownership": "The coordinator's continuing planning context.",
        "inputs": "Supplied river-sampling evidence.",
        "workflow": "The coordinator requests each plan.",
        "knowledge": "Retain planning history and working notes.",
        "authority": "Read evidence; a human carries out outside actions.",
        "destination": "The coordinator's Mac.",
    }
    brief = {"facts": {topic: {"summary": text, "status": "confirmed", "source_message_ids": [original["id"]]}
                       for topic, text in summaries.items()}, "requirements": []}
    prior_history = [{"topic": "inputs", "fact": {"summary": "An earlier source set.", "status": "confirmed",
                     "source_message_ids": [original["id"]]}, "obsolete": True, "reason": "revised"}]

    class RecordedGuide:
        def __init__(self):
            self.calls = []

        async def message(self, prompt, blueprint, *, guide_context):
            self.calls.append(copy.deepcopy(guide_context))
            # The same concrete job remains established while audience changes.
            assert guide_context["brief"] == brief
            revised = copy.deepcopy(guide_context["brief"])
            revised["facts"]["audience"] = {
                "summary": "Twenty volunteer coordinators will use the same planning service.",
                "status": "confirmed", "source_message_ids": [guide_context["current_user_message"]["id"]],
            }
            return {"guide": {"question": None, "message": "The audience change is saved for review.",
                    "brief": revised, "design_changed": False}, "proposal": copy.deepcopy(blueprint)}

    from launchkit.implementation import NoApplicationBuild

    def no_saved_build(_state):
        raise NoApplicationBuild("No application build in this inert fixture")

    architect = RecordedGuide()
    service = guide_module.GuideService(GuideStore(tmp_path), architect, implementation_evidence_provider=no_saved_build)
    seed = starter_state(capability)
    seed.update(stage="review", status="review", question=None, brief=copy.deepcopy(brief),
                proposal=starter_blueprint().model_dump(mode="json"),
                messages=[original], fact_history=copy.deepcopy(prior_history), completed_stages=list(guide_module.TOPIC_GROUPS),
                answers=[{"stage": "purpose", "text": original["text"], "selected": [],
                          "question": {"id": "audience", "stage": "purpose", "title": "Who uses the plan?",
                                       "description": "Who should retain planning context?", "multiple": False,
                                       "options": [], "allow_text": True}, "message_id": original["id"]}])
    before = service.store.create(seed)
    established = copy.deepcopy(before["brief"])

    reopened = await service.respond(before["id"], GuideResponse(version=before["version"], action="back",
                                      stage="purpose", attachments=[], combine_options=False))
    assert reopened["brief"] == established, "Navigation must not retract the established job or source IDs."
    assert reopened["fact_history"] == prior_history
    assert reopened["proposal"] == before["proposal"]
    assert reopened["status"] == "question" and reopened["question"]["id"] == "revisit_purpose"
    assert reopened["reopened_topics"] == ["outcome"]
    assert reopened["answers"][0]["reopened_topics"] == ["outcome"]
    assert "purpose" not in reopened["completed_stages"]
    assert reopened["readiness"]["ready"] is True
    assert set(reopened["readiness"]["confirmed_topics"]) == set(REVIEW_TOPICS)
    assert architect.calls == []
    stored_revisit = service.store.get(reopened["id"])
    with pytest.raises(ValueError, match="Finish the guided setup"):
        await service.apply(reopened["id"], reopened["version"])
    assert service.store.get(reopened["id"]) == stored_revisit

    # An actual answer still updates the canonical fact and records its old evidence.
    revised = await service.respond(reopened["id"], GuideResponse(version=reopened["version"], action="chat",
                                    text="The first job stays the same, but twenty volunteer coordinators will use it.",
                                    selected=[], attachments=[], combine_options=False, blueprint=None))
    assert len(architect.calls) == 1
    assert revised["brief"]["facts"]["outcome"] == established["facts"]["outcome"]
    assert revised["brief"]["facts"]["audience"]["summary"].startswith("Twenty volunteer coordinators")
    assert revised["fact_history"][:-1] == prior_history
    revision = revised["fact_history"][-1]
    assert revision["topic"] == "audience" and revision["fact"] == established["facts"]["audience"]
    assert revision["obsolete"] is True and revision["reason"] == "revised"
    assert revision["message_id"] == revised["messages"][-2]["id"]
    assert revised["brief"]["facts"]["audience"]["source_message_ids"] == [revision["message_id"]]
    assert revised["status"] == "review" and revised["question"] is None
    assert revised["reopened_topics"] == []
    accepted = await service.apply(revised["id"], revised["version"])
    assert accepted["status"] == "ready" and accepted["proposal"] is None
    assert accepted["blueprint"] == revised["proposal"]
    assert accepted["brief"]["facts"] == revised["brief"]["facts"]
    assert accepted["fact_history"] == revised["fact_history"]
