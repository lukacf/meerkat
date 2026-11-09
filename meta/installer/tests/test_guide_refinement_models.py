"""Refinement saves and readiness categories retain the same evidence authority."""
import copy

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.guide_models import (
    REVIEW_TOPICS, GuidePublication, default_question, review_readiness,
    status_only_publication, validate_guide_publication,
)


def fact(summary="A specified part of the proposed system.", *, status="confirmed", source="request"):
    return {"summary": summary, "status": status, "source_message_ids": [source] if source else []}


def requirement(identity="staff-ui", *, disposition="open", extension="Build the requested staff page.", decision=None):
    return {"id": identity, "outcome": "Staff use their requested page.", "supported_subset": "Native agent conversation.",
            "extension": extension, "disposition": disposition, "source_message_ids": ["request"],
            "decision_message_ids": [decision] if decision else []}


def brief(*, complete=False, requirements=None):
    topics = REVIEW_TOPICS if complete else ("outcome",)
    return {"facts": {topic: fact() for topic in topics}, "requirements": requirements or []}


def context(old=None, *, action="chat", saved=False, intent_only=False):
    messages = [
        {"id": "request", "role": "user", "text": "Help staff answer questions and prepare a handoff.", "intent_only": False},
        {"id": "assistant", "role": "assistant", "text": "The proposed system uses shared expertise."},
        {"id": "correction", "role": "user", "text": "Fourteen staff now share the same system; continue refining.",
         "intent_only": intent_only},
    ]
    return {"brief": old or {"facts": {}, "requirements": []}, "evidence_messages": messages,
            "current_user_message": messages[-1], "action": action, "saved_plan_available": saved,
            "manual_plan_submitted": action == "manual"}


def publication(value, *, mode="refinement", changed=True, question=None):
    return {"publication_mode": mode, "question": question, "message": "The updated proposal is saved.",
            "brief": value, "design_changed": changed}


def proposal():
    return starter_blueprint().model_dump(mode="json")


def test_legacy_publications_default_to_advance_and_unsupported_modes_are_rejected():
    value = publication(brief(), mode="advance", question=default_question("purpose"))
    value.pop("publication_mode")
    assert GuidePublication.model_validate(value).publication_mode == "advance"
    assert validate_guide_publication(value, context(), proposal())["publication_mode"] == "advance"
    value["publication_mode"] = "approved"
    with pytest.raises(ValueError):
        GuidePublication.model_validate(value)


@pytest.mark.parametrize("complete", [False, True])
def test_initial_refinement_can_save_understood_outcome_with_unfinished_context_and_application_work(complete):
    value = brief(complete=complete, requirements=[requirement()])
    if complete:
        value["facts"]["knowledge"] = fact("A proposed shared-notes default.", status="proposed", source=None)
    before = copy.deepcopy(value)
    result = validate_guide_publication(publication(value), context(), proposal())
    assert result["publication_mode"] == "refinement" and result["question"] is None
    assert result["brief"]["requirements"][0]["disposition"] == "open"
    assert value == before
    readiness = review_readiness(result["brief"])
    assert readiness["ready"] is False and readiness["context_ready"] is complete
    assert "Implementation still required: Staff use their requested page." in readiness["implementation_blockers"]
    assert not {"ready", "ready_to_package", "connected", "running"}.intersection(result)


@pytest.mark.parametrize("changed", [False, True])
def test_refinement_correction_preserves_cumulative_requirements_and_user_evidence_without_final_readiness(changed):
    old = brief(complete=True, requirements=[requirement()])
    new = copy.deepcopy(old)
    new["facts"]["audience"] = fact("Fourteen staff share the existing system.", source="correction")
    result = validate_guide_publication(publication(new, changed=changed), context(old, saved=True), proposal())
    assert result["brief"]["facts"]["audience"]["source_message_ids"] == ["correction"]
    assert result["brief"]["requirements"][0]["source_message_ids"] == ["request"]
    assert result["question"] is None and review_readiness(result["brief"])["ready"] is False
    assert status_only_publication(result, context(old, saved=True), proposal()) is False


@pytest.mark.parametrize("changed", [False, True])
def test_refinement_requires_a_fresh_validated_proposal_even_with_an_unchanged_saved_brief(changed):
    old = brief(complete=True)
    value = publication(old, changed=changed)
    saved = context(old, saved=True)
    assert status_only_publication(value, saved, None) is False
    with pytest.raises(ValueError, match="Validate and propose"):
        validate_guide_publication(value, saved, None)


@pytest.mark.parametrize("question", [default_question("purpose"), default_question("destination")])
def test_refinement_cannot_publish_an_interview_question(question):
    with pytest.raises(ValueError, match="question:null"):
        validate_guide_publication(publication(brief(), question=question), context(), proposal())


@pytest.mark.parametrize("status", ["proposed", "unresolved"])
def test_refinement_requires_a_confirmed_concrete_outcome(status):
    value = brief()
    value["facts"]["outcome"]["status"] = status
    with pytest.raises(ValueError, match="Understand a concrete outcome"):
        validate_guide_publication(publication(value), context(), proposal())


def test_refinement_rejects_a_category_as_the_only_outcome_evidence():
    value = brief()
    value["facts"]["outcome"]["source_message_ids"] = ["correction"]
    with pytest.raises(ValueError, match="broad category"):
        validate_guide_publication(publication(value), context(intent_only=True), proposal())


def test_refinement_does_not_skip_current_category_or_manual_submission_validation():
    with pytest.raises(ValueError, match="Keep the purpose open"):
        validate_guide_publication(publication({"facts": {}, "requirements": []}), context(intent_only=True), None)
    value = publication(brief(), changed=False)
    with pytest.raises(ValueError, match="Validate and propose the changed plan"):
        validate_guide_publication(value, context(action="manual"), None)
    assert validate_guide_publication(value, context(action="manual"), proposal())["publication_mode"] == "refinement"


@pytest.mark.parametrize("source", ["missing", "assistant"])
@pytest.mark.parametrize("target", ["fact", "requirement", "decision"])
def test_refinement_keeps_fact_and_requirement_evidence_user_owned(source, target):
    value = brief(requirements=[requirement()])
    if target == "fact":
        value["facts"]["outcome"]["source_message_ids"] = [source]
    elif target == "requirement":
        value["requirements"][0]["source_message_ids"] = [source]
    else:
        value["requirements"][0]["decision_message_ids"] = [source]
    with pytest.raises(ValueError, match="missing or non-user"):
        validate_guide_publication(publication(value), context(), proposal())


@pytest.mark.parametrize("drop", ["fact", "requirement"])
def test_refinement_cannot_erase_independent_facts_or_requested_scope(drop):
    old = brief(complete=True, requirements=[requirement()])
    new = copy.deepcopy(old)
    if drop == "fact":
        del new["facts"]["audience"]
    else:
        new["requirements"] = []
    with pytest.raises(ValueError, match="Preserve independent brief facts|Do not drop a requested feature"):
        validate_guide_publication(publication(new), context(old), proposal())


@pytest.mark.parametrize("disposition", ["deferred", "cancelled", "handoff"])
def test_refinement_cannot_treat_the_original_request_as_authorizing_scope_reduction(disposition):
    old = brief(requirements=[requirement()])
    new = copy.deepcopy(old)
    new["requirements"][0].update(disposition=disposition, decision_message_ids=["request"])
    with pytest.raises(ValueError, match="request itself cannot authorize"):
        validate_guide_publication(publication(new), context(old), proposal())


@pytest.mark.parametrize("disposition", ["deferred", "cancelled", "handoff"])
def test_refinement_preserves_an_explicit_later_user_scope_decision(disposition):
    old = brief(complete=True, requirements=[requirement()])
    new = copy.deepcopy(old)
    new["facts"]["first_version"] = fact("The later choice defines the first version.", source="correction")
    new["requirements"][0].update(disposition=disposition, decision_message_ids=["correction"])
    result = validate_guide_publication(publication(new), context(old), proposal())
    assert result["brief"]["requirements"][0]["disposition"] == disposition
    assert result["brief"]["requirements"][0]["decision_message_ids"] == ["correction"]


def test_readiness_groups_decisions_and_application_work_without_reordering_combined_blockers():
    value = brief(complete=True, requirements=[
        requirement("missing-code"), requirement("choice", extension=""),
        requirement("handoff", disposition="handoff", decision="correction"),
        requirement("deferred", disposition="deferred", decision="correction"),
    ])
    value["facts"]["audience"]["status"] = "unresolved"
    value["facts"]["destination"] = fact("An explicit proposed default.", status="proposed", source=None)
    readiness = review_readiness(value)
    assert readiness["context_ready"] is False and readiness["ready"] is False
    assert readiness["decision_blockers"] == [
        "Resolve audience or show an explicit proposed default.",
        "Agree how to handle: Staff use their requested page.",
        "Confirm the useful first version when deferring a requested feature.",
    ]
    assert readiness["implementation_blockers"] == [
        "Implementation still required: Staff use their requested page.",
        "Implementation still required: Staff use their requested page.",
    ]
    assert readiness["blockers"] == [
        readiness["decision_blockers"][0], readiness["implementation_blockers"][0],
        readiness["decision_blockers"][1], readiness["implementation_blockers"][1],
        readiness["decision_blockers"][2],
    ]
    assert readiness["proposed_topics"] == ["destination"]


def test_deferred_first_version_decision_keeps_prior_context_readiness_timing():
    value = brief(complete=True, requirements=[requirement(disposition="deferred", decision="correction")])
    readiness = review_readiness(value)
    assert readiness["context_ready"] is True and readiness["ready"] is False
    assert readiness["decision_blockers"] == ["Confirm the useful first version when deferring a requested feature."]
    assert readiness["implementation_blockers"] == []


def test_supported_prerequisite_cancelled_and_confirmed_deferral_have_no_new_blockers():
    value = brief(complete=True, requirements=[
        requirement("supported", disposition="supported", extension=""),
        requirement("prerequisite", disposition="prerequisite", extension=""),
        requirement("cancelled", disposition="cancelled", decision="correction"),
        requirement("deferred", disposition="deferred", decision="correction"),
    ])
    value["facts"]["first_version"] = fact("The first version follows the later decision.", source="correction")
    readiness = review_readiness(value)
    assert readiness["ready"] is True and readiness["context_ready"] is True
    assert readiness["blockers"] == readiness["decision_blockers"] == readiness["implementation_blockers"] == []


def test_legacy_final_review_still_requires_readiness_and_a_fresh_proposal():
    unfinished = brief(complete=True, requirements=[requirement()])
    with pytest.raises(ValueError, match="brief is not ready for review"):
        validate_guide_publication(publication(unfinished, mode="advance"), context(), proposal())
    complete = brief(complete=True)
    with pytest.raises(ValueError, match="complete blueprint before final review"):
        validate_guide_publication(publication(complete, mode="advance", changed=False), context(action="answer"), None)
    result = validate_guide_publication(publication(complete, mode="advance"), context(), proposal())
    assert result["publication_mode"] == "advance" and result["question"] is None


def test_legacy_exact_status_only_publication_keeps_unfinished_brief_without_final_review():
    old = brief(requirements=[requirement()])
    value = publication(copy.deepcopy(old), mode="advance", changed=False)
    saved = context(old, saved=True)
    assert status_only_publication(value, saved, None) is True
    result = validate_guide_publication(value, saved, None)
    assert result["brief"]["requirements"][0]["disposition"] == "open"
    assert review_readiness(result["brief"])["ready"] is False
    value["publication_mode"] = "refinement"
    assert status_only_publication(value, saved, None) is False
    with pytest.raises(ValueError, match="current blueprint before saving a refinement"):
        validate_guide_publication(value, saved, None)


@pytest.mark.parametrize("change", [
    lambda value, state: value.update(design_changed=True),
    lambda value, state: value["brief"]["facts"]["outcome"].update(summary="Changed scope."),
    lambda value, state: state.update(action="answer"),
    lambda value, state: state.update(manual_plan_submitted=True),
    lambda value, state: state.update(saved_plan_available=False),
])
def test_legacy_status_only_exception_remains_exactly_fenced(change):
    old = brief(requirements=[requirement()])
    value = publication(copy.deepcopy(old), mode="advance", changed=False)
    saved = context(old, saved=True)
    change(value, saved)
    assert status_only_publication(value, saved, None) is False
    with pytest.raises(ValueError):
        validate_guide_publication(value, saved, None)
