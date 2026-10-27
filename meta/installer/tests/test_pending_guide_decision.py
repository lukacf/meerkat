"""Focused pending-decision regression using the existing actual GuideService fixture."""
import copy
import os
from pathlib import Path
import runpy

import pytest
from launchkit.guide import compact_guidance_context
from launchkit.guide_models import review_readiness

# Reuse fixture/helpers only. None of the historical test functions is collected.
_original = runpy.run_path(str(Path(os.environ.get("FORGE_GUIDE_TESTS", Path(__file__).parent)) / "test_guide.py"))
guide_client = _original["guide_client"]
start, answer, respond = (_original[name] for name in ("start", "answer", "respond"))
complete_brief = _original["complete_brief"]


def test_pending_context_choices_survive_unchanged_chat_then_accept_only_explicit_answer(guide_client):
    client, agent, service = guide_client
    state = answer(client, start(client), text="Prepare a useful weekly project brief from our notes.")
    question = {
        "id": "participating_people", "stage": "purpose", "title": "Who should this help?",
        "description": "Choose whose working context should shape the brief.",
        "multiple": True, "allow_text": True, "options": [
            {"id": "personal", "label": "My own work", "description": "Keep a private context for my commitments."},
            {"id": "team", "label": "Our shared work", "description": "Keep shared team priorities separate from private notes."},
        ],
    }
    state.update(question=question, stage="purpose", status="question")
    state["reopened_topics"] = ["audience"]
    state = service.store.save(state, state["version"])
    before = copy.deepcopy(state)
    note = "We are still discussing this, not deciding who can access private notes."
    help_text = (
        "Briefly recommend a sensible choice or combination and explain the main tradeoff. "
        "Tentative context for this question, not a confirmed answer: Our shared work. "
        "Full unsent note:\n" + note + "\nThis is not confirmation or permission to change tool grants."
    )

    def clarification(context, blueprint):
        assert context["action"] == "chat"
        assert context["current_question"] == question
        assert context["current_user_message"]["text"] == help_text
        assert "selected" not in context["current_user_message"]
        assert blueprint == before["proposal"]
        return {"guide": {"question": None, "message": "Start with shared priorities, keeping private notes separate.",
            "brief": copy.deepcopy(context["brief"]), "design_changed": False}, "proposal": None}

    agent.reply = clarification
    response = respond(client, state, action="chat", text=help_text)
    assert response.status_code == 200, response.text
    saved = response.json()
    assert saved["question"] == question, "Clarification must keep the unanswered multiple-choice decision."
    for key in ("brief", "blueprint", "proposal", "stage", "status", "answers", "manual_edits", "fact_history", "reopened_topics"):
        assert saved[key] == before[key], key
    assert saved["status"] == "question"
    assert saved["messages"][-2]["action"] == "chat"
    assert saved["messages"][-1]["question"] is None, "The clarification itself offers no new question."
    assert service.store.get(saved["id"])["question"] == question
    refused = client.post(f"/api/guide/{saved['id']}/apply", json={"version": saved["version"]})
    assert refused.status_code == 422, refused.text
    assert service.store.get(saved["id"])["version"] == saved["version"]

    next_question = {**copy.deepcopy(question), "id": "privacy_boundary", "title": "What should stay private?"}

    def explicit_answer(context, blueprint):
        assert context["action"] == "answer"
        assert context["current_question"] == question
        assert context["current_user_message"]["selected"] == ["team"]
        assert context["answers"][-1]["selected"] == ["team"]
        brief = copy.deepcopy(context["brief"])
        brief["facts"]["audience"] = {"summary": "The shared team uses the brief; private notes are not included.",
            "status": "confirmed", "source_message_ids": [context["current_user_message"]["id"]]}
        return {"guide": {"question": next_question, "message": "The team brief is selected. Now choose its privacy boundary.",
            "brief": brief, "design_changed": False}, "proposal": None}

    agent.reply = explicit_answer
    response = respond(client, saved, action="answer", selected=["team"], text="Private notes are not part of it.")
    assert response.status_code == 200, response.text
    advanced = response.json()
    assert advanced["question"] == next_question
    assert advanced["version"] == saved["version"] + 1
    assert advanced["proposal"] == before["proposal"]
    assert advanced["answers"][:-1] == before["answers"]
    assert advanced["answers"][-1]["question"] == question
    assert advanced["answers"][-1]["selected"] == ["team"]
    assert advanced["brief"]["facts"]["audience"]["source_message_ids"] == [advanced["messages"][-2]["id"]]
    assert advanced["reopened_topics"] == []
    assert len(agent.calls) == 3, "One initial answer, one clarification, one explicit next answer."
