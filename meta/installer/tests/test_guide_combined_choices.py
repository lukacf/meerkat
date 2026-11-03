"""Explicit combined choice intent preserves the question and durable recovery."""
import asyncio
import copy
import json
from types import SimpleNamespace

import pytest
from pydantic import ValidationError

from launchkit.guide import compact_guidance_context
from launchkit.guide_models import default_question, starter_state
from launchkit.guide_operations import GuideOperations
from launchkit.guide_store import GuideStore
from launchkit.guided_edits import guided_edit_token
from launchkit.projects import fingerprint
from launchkit.server import GuideResponse
from test_guide import answer, guide_client, inspected_guide_turn, respond, start


def history_question():
    return {
        "id": "history_contexts", "stage": "purpose", "multiple": False,
        "title": "Which context should keep the work history?",
        "description": "These offered histories may overlap; tell the guide what fits your work.",
        "allow_text": True, "options": [
            {"id": "personal", "label": "Personal history",
             "description": "Each person keeps private preferences and conversations."},
            {"id": "crew", "label": "Crew history",
             "description": "A working crew keeps the shared decisions for its own jobs."},
            {"id": "organization", "label": "Organization history",
             "description": "Shared operating knowledge can be reused across crews."},
        ],
    }


@pytest.mark.parametrize("value", ["true", "false", 1, 0, None, [], {}])
def test_combine_options_is_a_strict_bool(value):
    with pytest.raises(ValidationError):
        GuideResponse(version=1, selected=["personal", "crew"], combine_options=value)


@pytest.mark.parametrize("action", ["chat", "back", "manual"])
def test_combine_intent_cannot_be_a_different_action(action):
    with pytest.raises(ValidationError):
        GuideResponse(version=1, action=action, selected=["personal", "crew"], combine_options=True)


@pytest.mark.parametrize("selected", [[], ["personal"]])
def test_combine_intent_requires_more_than_one_selection(selected):
    with pytest.raises(ValidationError):
        GuideResponse(version=1, selected=selected, combine_options=True)


def legacy_body():
    return {"version": 1, "action": "answer", "selected": ["personal"],
            "text": "Keep my notes private.", "stage": None, "blueprint": None}


def test_false_and_omitted_override_keep_the_exact_legacy_request_body():
    body = legacy_body()
    omitted = GuideResponse.model_validate(body)
    explicit = GuideResponse.model_validate({**body, "combine_options": False})
    assert omitted.combine_options is False and explicit.combine_options is False
    assert omitted.model_dump(mode="json") == explicit.model_dump(mode="json") == body
    assert json.loads(omitted.model_dump_json()) == body
    combined = GuideResponse(version=1, selected=["personal", "crew"], combine_options=True)
    assert combined.model_dump(mode="json")["combine_options"] is True
    ordinary = GuideResponse(version=1, selected=["personal", "crew"])
    assert fingerprint(combined.model_dump(mode="json")) != fingerprint(ordinary.model_dump(mode="json"))


def install_question(client, service):
    state = start(client)
    state["question"] = history_question()
    state["messages"].append({"id": "crew-job", "role": "user",
        "text": "Help our crews coordinate event work while people retain private preferences."})
    return service.store.save(state, state["version"])


def test_combined_answer_preserves_original_question_and_exact_context_evidence(guide_client, tmp_path):
    client, agent, service = guide_client
    state = install_question(client, service)
    original = copy.deepcopy(state)

    def follow_up(context, blueprint):
        return {"guide": {"question": default_question("autonomy"),
            "message": "Let us check how those requested contexts can cooperate.",
            "brief": copy.deepcopy(context["brief"])}, "proposal": None}

    agent.reply = follow_up
    state = answer(client, state, text="Keep crew sharing separate from private notes.",
                   selected=["crew", "personal"], combine_options=True)
    call = agent.calls[-1]
    context = call["guide_context"]
    current = context["current_user_message"]
    assert current["text"] == "More than one fits: Personal history; Crew history; Keep crew sharing separate from private notes."
    assert current["combine_options"] is True and current["selected"] == ["crew", "personal"]
    assert current["question"] == original["question"] and current["question"]["multiple"] is False
    assert context["current_question"] == original["question"]
    assert state["answers"][-1] == {"stage": "purpose", "question": original["question"],
        "selected": ["crew", "personal"], "text": "Keep crew sharing separate from private notes.",
        "message_id": current["id"], "combine_options": True}
    saved_user = next(row for row in state["messages"] if row["id"] == current["id"])
    assert saved_user == current
    assert state["brief"] == original["brief"] and state["readiness"] == original["readiness"]
    assert state["status"] == "question" and state["proposal"] is None and state["blueprint"] is None
    projected = compact_guidance_context(context)
    choice = projected["answered_choices"][-1]
    assert choice["combine_options"] is True
    assert choice["selected_options"] == [original["question"]["options"][1], original["question"]["options"][0]]
    assert projected["current_user_message"]["combine_options"] is True
    assert projected["evidence_messages"][-1]["combine_options"] is True
    assert "Organization history" not in str(choice)
    sent = json.loads(call["prompt"].split("\n\nCurrent setup turn:\n", 1)[1])
    assert sent["current_user_message"] == projected["current_user_message"]
    assert "answered_choices" not in sent
    assert inspected_guide_turn(call, tmp_path)["guided_setup"]["answered_choices"] == projected["answered_choices"]
    assert "does not establish that incompatible choices can work" in call["prompt"]
    without_override = copy.deepcopy(context)
    without_override["current_user_message"].pop("combine_options")
    assert guided_edit_token(without_override, call["blueprint"]) != context["edit_base_token"]
    state = answer(client, state, action="chat", text="How should these histories cooperate?")
    later = compact_guidance_context(agent.calls[-1]["guide_context"])
    assert later["answered_choices"][-1] == choice
    assert state["brief"] == original["brief"] and state["readiness"] == original["readiness"]


@pytest.mark.parametrize("damage", ["multiple", "text_only", "no_question", "unknown", "duplicate", "missing_override"])
def test_combined_answer_never_bypasses_current_question_validation(guide_client, damage):
    client, agent, service = guide_client
    state = install_question(client, service)
    if damage == "multiple":
        state["question"]["multiple"] = True
    elif damage == "text_only":
        state["question"]["options"] = []
    elif damage == "no_question":
        state["question"] = None
    if damage in {"multiple", "text_only", "no_question"}:
        state = service.store.save(state, state["version"])
    selected = ["personal", "crew"]
    if damage == "unknown":
        selected = ["personal", "old_question_option"]
    elif damage == "duplicate":
        selected = ["personal", "personal"]
    before = service.store.get(state["id"])
    response = respond(client, state, selected=selected, combine_options=damage != "missing_override", text="These fit.")
    assert response.status_code == 422
    assert not agent.calls
    assert service.store.get(state["id"]) == before


def test_combined_answer_cannot_target_an_old_question_version(guide_client):
    client, agent, service = guide_client
    state = install_question(client, service)
    newer = copy.deepcopy(state)
    newer["question"]["options"][1]["description"] = "A new crew context with different visibility."
    newer = service.store.save(newer, state["version"])
    response = respond(client, state, selected=["personal", "crew"], combine_options=True)
    assert response.status_code == 409 and not agent.calls
    assert service.store.get(state["id"]) == newer


def combined_projection_context():
    question = history_question()
    current = {"id": "choice", "role": "user", "action": "answer", "text": "More than one fits: Personal history; Crew history",
        "question": question, "selected": ["personal", "crew"], "combine_options": True}
    return {"brief": {}, "current_user_message": current, "evidence_messages": [current],
        "answers": [{"message_id": "choice", "question": question, "selected": current["selected"], "combine_options": True}]}


def test_current_override_keeps_full_choice_meaning_without_an_answer_index():
    context = combined_projection_context()
    context.pop("answers")
    before = copy.deepcopy(context)
    projected = compact_guidance_context(context)
    assert projected["answered_choices"][0]["combine_options"] is True
    assert projected["answered_choices"][0]["selected_options"] == history_question()["options"][:2]
    assert context == before


@pytest.mark.parametrize("damage", ["no_user_flag", "no_answer_flag", "wrong_action", "wrong_question", "wrong_selection"])
def test_rich_combined_projection_requires_the_matching_explicit_user_evidence(damage):
    context = combined_projection_context()
    # Break shared object references before damaging one side of the evidence join.
    context = json.loads(json.dumps(context))
    current = context["current_user_message"]
    if damage == "no_user_flag":
        current.pop("combine_options")
    elif damage == "no_answer_flag":
        context["answers"][0].pop("combine_options")
    elif damage == "wrong_action":
        current["action"] = "chat"
    elif damage == "wrong_question":
        current["question"]["title"] = "A different question"
    else:
        current["selected"] = ["personal", "organization"]
    projected = compact_guidance_context(context)
    assert "selected_options" not in projected["answered_choices"][0]
    assert "combine_options" not in projected["answered_choices"][0]


@pytest.mark.asyncio
@pytest.mark.parametrize("explicit_false", [False, True])
async def test_completed_legacy_operation_recovers_without_another_turn(tmp_path, explicit_false):
    store = GuideStore(tmp_path)
    initial = store.create(starter_state({}))
    body = legacy_body()
    store.save_operation(initial["id"], {"status": "completed", "version": 1,
        "request": body, "request_id": fingerprint({"guide_id": initial["id"], **body})})
    saved = store.save({**initial, "summary": "An existing completed response."}, 1)
    async def get(identity):
        return store.get(identity)
    async def forbidden_respond(*_args):
        pytest.fail("An existing completed request must not start another turn.")
    service = SimpleNamespace(store=store, get=get, respond=forbidden_respond)
    value = GuideResponse.model_validate({**body, **({"combine_options": False} if explicit_false else {})})
    recovered, pending = await GuideOperations().submit(service, initial["id"], value)
    assert pending is False and recovered == saved


@pytest.mark.asyncio
async def test_running_legacy_body_and_false_override_share_one_owned_turn(tmp_path):
    store = GuideStore(tmp_path)
    initial = store.create(starter_state({}))
    body = legacy_body()
    release = asyncio.Event()
    calls = []
    async def get(identity):
        return store.get(identity)
    async def respond_once(identity, value):
        calls.append(value.model_dump(mode="json"))
        await release.wait()
        return store.save({**store.get(identity), "summary": "The original turn completed."}, value.version)
    service = SimpleNamespace(store=store, get=get, respond=respond_once)
    operations = GuideOperations(wait_seconds=0.01)
    first, pending = await operations.submit(service, initial["id"], GuideResponse.model_validate(body))
    assert pending and first["operation"]["request"] == body
    request_id = fingerprint({"guide_id": initial["id"], **body})
    assert first["operation"]["request_id"] == request_id
    recovered, pending = await operations.submit(service, initial["id"], GuideResponse.model_validate({**body, "combine_options": False}))
    assert pending and recovered["operation"]["request_id"] == request_id and calls == [body]
    release.set()
    await asyncio.gather(*list(operations.tasks.values()))
    recovered, pending = await operations.submit(service, initial["id"], GuideResponse.model_validate(body))
    assert not pending and recovered["version"] == 2 and calls == [body]
