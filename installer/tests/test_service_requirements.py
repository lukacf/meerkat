"""Service intent preserves user scope without manufacturing native readiness."""
import copy

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.guide_models import DesignBrief, default_question, review_readiness, validate_guide_publication
from launchkit.guide_store import GuideStore


def brief():
    return {
        "facts": {"outcome": {"summary": "Answer the team's project questions in Slack.",
            "status": "confirmed", "source_message_ids": ["request"]}},
        "requirements": [{"id": "team-help", "outcome": "Read our private repository and converse in Slack.",
            "disposition": "open", "extension": "Resolve native service checks and incoming frontend.",
            "source_message_ids": ["request"]}],
        "service_requirements": [
            {"id": "repository-read", "requirement_id": "team-help", "service": "GitHub Enterprise",
                "capability": "read", "operation": "Read the selected repository's source", "resource": "",
                "source_message_ids": ["request"]},
            {"id": "slack-conversation", "requirement_id": "team-help", "service": "Slack",
                "capability": "incoming_interaction", "operation": "Receive and reply to the team's questions",
                "resource": "The team's chosen channel", "source_message_ids": ["request"]},
        ],
    }


def publish(value, old=None):
    messages = [{"id": "request", "role": "user", "text": "Read our private repository and converse in Slack."},
        {"id": "assistant", "role": "assistant", "text": "Outbound Slack tools are available."},
        {"id": "decision", "role": "user", "text": "Change that Slack step to reading history only."}]
    return validate_guide_publication({"message": "The service requirements are retained.", "brief": value,
        "question": default_question("connections"), "design_changed": True},
        {"brief": old or {}, "evidence_messages": messages, "current_user_message": messages[-1]},
        starter_blueprint().model_dump(mode="json"))


def test_legacy_briefs_do_not_acquire_an_empty_service_attestation():
    old = {"facts": {}, "requirements": []}
    assert DesignBrief.model_validate(old).model_dump(mode="json") == old
    assert DesignBrief.model_validate({**old, "service_requirements": []}).model_dump(mode="json") == old


def test_distinct_service_capabilities_survive_actual_store_reopen_as_unverified_intent(tmp_path):
    result = publish(brief())
    saved = GuideStore(tmp_path).create({"brief": result["brief"]})
    reopened = GuideStore(tmp_path).get(saved["id"])
    rows = reopened["brief"]["service_requirements"]
    assert [row["capability"] for row in rows] == ["read", "incoming_interaction"]
    assert rows[0]["resource"] == ""
    assert rows == brief()["service_requirements"]
    assert reopened["brief"]["requirements"][0]["disposition"] == "open"
    assert review_readiness(reopened["brief"])["ready"] is False
    assert all(not {"ready", "authenticated", "granted", "result"}.intersection(row) for row in rows)


@pytest.mark.parametrize("change", [
    lambda value: value.pop("service_requirements"),
    lambda value: value["service_requirements"].pop(),
    lambda value: value["service_requirements"][1].update(id="outbound-slack"),
])
def test_publication_cannot_silently_drop_or_replace_required_service_identity(change):
    old = brief(); value = copy.deepcopy(old); change(value)
    with pytest.raises(ValueError, match="Do not drop a required service"):
        publish(value, old)


def test_outbound_access_cannot_silently_replace_the_incoming_requirement():
    old = brief(); value = copy.deepcopy(old)
    value["service_requirements"][1].update(capability="read", operation="Read channel history")
    with pytest.raises(ValueError, match="later user evidence"):
        publish(value, old)
    value["service_requirements"][1]["source_message_ids"].append("decision")
    assert publish(value, old)["brief"]["service_requirements"][1]["capability"] == "read"


@pytest.mark.parametrize("source", ["assistant", "missing"])
def test_model_or_missing_evidence_cannot_establish_service_scope(source):
    value = brief(); value["service_requirements"][0]["source_message_ids"] = [source]
    with pytest.raises(ValueError, match="missing or non-user"):
        publish(value)


@pytest.mark.parametrize("field,value", [("authenticated", True), ("ready", True),
    ("grant_id", "invented"), ("credential", "not-a-real-secret"), ("native_tool", "tools/list")])
def test_guide_cannot_publish_credentials_native_choices_or_results_as_intent(field, value):
    proposed = brief(); proposed["service_requirements"][0][field] = value
    with pytest.raises(ValueError):
        publish(proposed)


@pytest.mark.parametrize("change", [
    lambda value: value["service_requirements"].append(copy.deepcopy(value["service_requirements"][0])),
    lambda value: value["service_requirements"][0].update(requirement_id="unknown"),
    lambda value: value["service_requirements"][0].update(source_message_ids=["request", "request"]),
])
def test_service_intent_cannot_lose_its_requirement_or_evidence_binding(change):
    value = brief(); change(value)
    with pytest.raises(ValueError):
        publish(value)
