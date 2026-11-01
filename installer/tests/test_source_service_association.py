"""Source-operation intent survives publication without becoming native coverage."""
import copy

import pytest

from launchkit.guide_models import DesignBrief
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from test_service_requirements import brief, publish


def associated_brief():
    value = brief()
    read = value["service_requirements"][0]
    read["resource"] = "Repository A"
    value["service_requirements"].append({**read, "id": "repository-b-read", "resource": "Repository B"})
    value["incoming_data"] = [
        {"id": "repository-changes", "requirement_id": "team-help", "name": "Repository changes",
         "scope": "Repositories A and B", "purpose": "Poll project changes", "source_message_ids": ["request"],
         "service_requirement_ids": ["repository-read", "repository-b-read"]},
        {"id": "slack-questions", "requirement_id": "team-help", "name": "Slack questions",
         "scope": "The selected channel", "purpose": "Receive questions", "source_message_ids": ["request"]},
    ]
    return value


def test_published_association_survives_reopen_but_does_not_clear_native_source_checks(tmp_path):
    proposed = associated_brief()
    before = copy.deepcopy(proposed)
    published = publish(proposed)
    saved = GuideStore(tmp_path).create({"version": 1, "brief": published["brief"]})
    reopened = GuideStore(tmp_path).get(saved["id"])
    impl = ImplementationService("/unused", None, require_service_checks=True)
    hold = impl.service_preparation(reopened)
    assert hold["status"] == "unavailable"
    rows = {row["id"]: row for row in hold["required_operations"]}
    assert set(rows) == {"repository-read", "repository-b-read", "slack-conversation",
                         "incoming:repository-changes", "incoming:slack-questions"}
    source = rows["incoming:repository-changes"]
    assert source["capability"] == "unresolved"
    assert source["reason_code"] == "source_binding_not_verified"
    assert [(row["id"], row["resource"]) for row in source["planned_operations"]] == [
        ("repository-read", "Repository A"), ("repository-b-read", "Repository B")]
    assert rows["incoming:slack-questions"]["reason_code"] == "source_operation_not_selected"
    assert "service_requirement_ids" not in reopened["brief"]["incoming_data"][1]
    source["planned_operations"][0]["resource"] = "Another repository"
    assert impl.service_preparation(reopened)["required_operations"][0]["resource"] == "Repository A"
    assert proposed == before


@pytest.mark.parametrize("identities", [["unknown"], ["repository-read", "repository-read"], ["repository-read", "unknown"]])
def test_missing_or_duplicate_operation_links_cannot_be_published(identities):
    value = associated_brief()
    value["incoming_data"][0]["service_requirement_ids"] = identities
    with pytest.raises(ValueError):
        publish(value)


def test_source_cannot_borrow_an_operation_from_an_unrelated_feature():
    value = associated_brief()
    value["requirements"].append({**value["requirements"][0], "id": "other-feature"})
    value["service_requirements"][0]["requirement_id"] = "other-feature"
    with pytest.raises(ValueError, match="same requested feature"):
        publish(value)


@pytest.mark.parametrize("extra", [{"native_binding_id": "invented"}, {"ready": True}, {"operation_receipts": []}])
def test_source_links_cannot_carry_model_authored_native_evidence(extra):
    value = associated_brief()
    value["incoming_data"][0].update(extra)
    with pytest.raises(ValueError):
        publish(value)


def test_legacy_source_serialization_is_unchanged_when_links_are_missing_or_empty():
    value = associated_brief()
    value["incoming_data"][0].pop("service_requirement_ids")
    normalized = DesignBrief.model_validate(value).model_dump(mode="json")
    value["incoming_data"][0]["service_requirement_ids"] = []
    assert DesignBrief.model_validate(value).model_dump(mode="json") == normalized
    assert "service_requirement_ids" not in normalized["incoming_data"][0]


@pytest.mark.parametrize("change", [
    lambda value: value["service_requirements"].pop(),
    lambda value: value["service_requirements"][0].update(requirement_id="another-feature"),
    lambda value: value["incoming_data"][0].update(service_requirement_ids=["repository-read", "repository-read"]),
    lambda value: value["incoming_data"][0].update(service_requirement_ids=[{}]),
])
def test_inconsistent_retained_association_remains_unselected_and_blocked(change):
    value = associated_brief()
    change(value)
    hold = ImplementationService("/unused", None, require_service_checks=True).service_preparation(
        {"id": "guide", "version": 2, "brief": value})
    source = next(row for row in hold["required_operations"] if row["id"] == "incoming:repository-changes")
    assert hold["status"] == "unavailable"
    assert source["reason_code"] == "source_operation_not_selected"
    assert source["planned_operations"] == []
