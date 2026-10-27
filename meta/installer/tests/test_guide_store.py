from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
from uuid import uuid4

import pytest
from pydantic import ValidationError

from launchkit.guide_models import GuideQuestion, STAGES, default_question, starter_state
from launchkit.guide_store import GuideStore, MAX_STATE_BYTES
from launchkit.projects import DraftConflict


def test_interview_survives_store_restart_without_mutating_caller(tmp_path):
    initial = starter_state({"available": False})
    store = GuideStore(tmp_path / "studio")
    created = store.create(initial)
    assert initial["id"] is None and initial["version"] == 0
    assert created["version"] == 1
    created["messages"].append({"role": "user", "text": "Help my team."})
    saved = store.save(created, 1)
    assert saved["version"] == 2 and created["version"] == 1
    assert GuideStore(tmp_path / "studio").get(created["id"]) == saved
    saved["messages"][-1]["text"] = "Unsaved edit"
    assert store.get(created["id"])["messages"][-1]["text"] == "Help my team."


def test_concurrent_tabs_cannot_overwrite_each_other(tmp_path):
    created = GuideStore(tmp_path).create(starter_state({}))

    def edit(text):
        state = deepcopy(created)
        state["summary"] = text
        try:
            return GuideStore(tmp_path).save(state, 1)
        except DraftConflict:
            return None

    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(edit, ["First choice", "Second choice"]))
    saved = [result for result in results if result is not None]
    assert len(saved) == 1
    assert GuideStore(tmp_path).get(created["id"]) == saved[0]


def test_creation_request_replays_after_restart_without_resetting_conversation(tmp_path):
    store = GuideStore(tmp_path)
    request_id = str(uuid4())
    created = store.create(starter_state({}), request_id=request_id)
    changed = store.save({**created, "summary": "Retain the user's first idea"}, 1)
    replay = GuideStore(tmp_path).create(starter_state({}), request_id=request_id)
    assert replay == changed
    assert len(store.list()) == 1
    other = store.create(starter_state({}), request_id=str(uuid4()))
    assert other["id"] != created["id"] and len(store.list()) == 2


def test_concurrent_creation_retries_have_one_durable_owner(tmp_path):
    request_id = str(uuid4())
    def create(_):
        return GuideStore(tmp_path).create(starter_state({}), request_id=request_id)
    with ThreadPoolExecutor(max_workers=2) as pool:
        results = list(pool.map(create, range(2)))
    assert results[0] == results[1]
    assert len(GuideStore(tmp_path).list()) == 1


def test_missing_and_malformed_interview_ids(tmp_path):
    store = GuideStore(tmp_path)
    assert store.get(str(uuid4())) is None
    for identity in ("../another", "not-a-uuid", "", None, 1):
        with pytest.raises(ValueError, match="Invalid guided setup ID"):
            store.get(identity)
    missing = {**starter_state({}), "id": str(uuid4()), "version": 1}
    with pytest.raises(DraftConflict):
        store.save(missing, 1)


def test_private_database_and_size_failure_preserves_current_state(tmp_path):
    store = GuideStore(tmp_path / "studio")
    saved = store.create(starter_state({}))
    root = tmp_path / "studio"
    assert root.stat().st_mode & 0o077 == 0
    assert (root / "guided-setups.sqlite3").stat().st_mode & 0o077 == 0
    oversized = {**saved, "summary": "x" * MAX_STATE_BYTES}
    with pytest.raises(ValueError, match="too large"):
        store.save(oversized, 1)
    assert store.get(saved["id"]) == saved
    with pytest.raises(ValueError):
        store.save(saved, True)
    with pytest.raises(DraftConflict):
        store.save({**saved, "version": 2}, 1)


def test_default_questions_are_complete_and_do_not_assume_a_project():
    for stage in STAGES[:-1]:
        question = GuideQuestion.model_validate(default_question(stage))
        assert question.stage == stage
        assert question.multiple is (stage == "connections")
        assert question.allow_text is True
    assert default_question("review") is None
    with pytest.raises(ValueError):
        default_question("unknown")
    initial = starter_state({"available": True})
    assert initial["blueprint"] is None and initial["proposal"] is None
    assert initial["answers"] == [] and initial["completed_stages"] == []


def test_agent_questions_reject_ambiguous_and_invalid_contracts():
    question = default_question("connections")
    duplicate = deepcopy(question)
    duplicate["options"][1]["id"] = duplicate["options"][0]["id"]
    invalid = [duplicate, {**question, "multiple": "false"}, {**question, "allow_text": False},
               {**question, "stage": "review"}, {**question, "options": question["options"][:1]},
               {**question, "unexpected": "field"}]
    for value in invalid:
        with pytest.raises(ValidationError):
            GuideQuestion.model_validate(value)


def test_text_questions_need_no_artificial_choices():
    question = default_question('purpose')
    text_question = {**question, 'id': 'first_job', 'options': []}
    assert GuideQuestion.model_validate(text_question).options == []
    with pytest.raises(ValidationError):
        GuideQuestion.model_validate({**text_question, 'multiple': True})


def test_brief_rejects_unattributed_claims_and_disguised_extension():
    from launchkit.guide_models import DesignBrief
    with pytest.raises(ValidationError, match='supporting user message'):
        DesignBrief.model_validate({'facts': {'outcome': {'summary': 'An invented outcome', 'status': 'confirmed'}}})
    with pytest.raises(ValidationError, match='not an installation prerequisite'):
        DesignBrief.model_validate({'requirements': [{
            'id': 'web_app', 'outcome': 'Staff web app', 'disposition': 'prerequisite',
            'extension': 'Build an authenticated web app', 'source_message_ids': ['real-user-message'],
        }]})
