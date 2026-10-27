import copy
import json

import pytest

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.guide_models import DesignBrief
from launchkit.guided_edits import (
    MAX_GUIDED_EDITS,
    _path,
    apply_guided_edits,
    guided_document,
    guided_edit_schema,
    guided_edit_token,
)
from launchkit.guide_store import MAX_STATE_BYTES


@pytest.fixture
def document():
    return guided_document(starter_blueprint().model_dump(mode="json"), {
        "facts": {"outcome": {"summary": "Retain an observer's evidence and decisions.",
            "status": "confirmed", "source_message_ids": ["user-current"]}},
        "requirements": [{"id": "retained-evidence", "outcome": "Keep the original evidence.",
            "disposition": "open", "source_message_ids": ["user-current"]}],
    })


@pytest.fixture
def context(document):
    return {
        "guide_id": "owned-synthetic-guide", "guide_version": 3, "action": "answer",
        "current_stage": "purpose", "current_status": "question",
        "current_question": {"id": "observing-job", "title": "Which observing job?"},
        "saved_plan_available": True, "manual_plan_submitted": False,
        "current_user_message": {"id": "user-current", "role": "user", "text": "Retain my evidence."},
        "evidence_messages": [{"id": "user-current", "role": "user", "text": "Retain my evidence."}],
        "manual_edits": [], "reopened_topics": [], "brief": copy.deepcopy(document["brief"]),
    }


@pytest.mark.parametrize("pointer, expected", [
    ("/blueprint/a~1b/m~0n/~01", ["blueprint", "a/b", "m~n", "~1"]),
    ("/brief/~0~1/01/-", ["brief", "~/", "01", "-"]),
    ("/blueprint//", ["blueprint", "", ""]),
])
def test_json_pointer_decodes_exactly_once(pointer, expected):
    assert _path(pointer) == expected


def test_escaped_nested_dictionary_paths_apply_before_final_canonical_validation(document):
    edits = [
        {"op": "add", "path": "/blueprint/transient", "value": {"a/b": {"m~n": {"~1": "original"}}}},
        {"op": "replace", "path": "/blueprint/transient/a~1b/m~0n/~01", "value": "changed"},
        {"op": "remove", "path": "/blueprint/transient/a~1b/m~0n/~01"},
        {"op": "remove", "path": "/blueprint/transient/a~1b/m~0n"},
        {"op": "remove", "path": "/blueprint/transient/a~1b"},
        {"op": "remove", "path": "/blueprint/transient"},
    ]
    before = copy.deepcopy((document, edits))
    assert apply_guided_edits(document, edits) == document
    assert (document, edits) == before


def test_dictionary_numeric_keys_are_not_array_indexes(document):
    document["blueprint"]["connections"] = [{
        "id": "synthetic", "name": "Synthetic source", "transport": "http",
        "url": "https://example.invalid/mcp", "headers": {
            "0": "${FIRST_LOCAL}", "01": "${SECOND_LOCAL}", "-": "${THIRD_LOCAL}"},
    }]
    before = copy.deepcopy(document)
    edits = [
        {"op": "replace", "path": "/blueprint/connections/0/headers/01", "value": "${UPDATED_LOCAL}"},
        {"op": "add", "path": "/blueprint/connections/0/headers/2", "value": "${ADDED_LOCAL}"},
        {"op": "remove", "path": "/blueprint/connections/0/headers/0"},
        {"op": "remove", "path": "/blueprint/connections/0/headers/-"},
    ]
    original_edits = copy.deepcopy(edits)
    result = apply_guided_edits(document, edits)
    assert result["blueprint"]["connections"][0]["headers"] == {
        "01": "${UPDATED_LOCAL}", "2": "${ADDED_LOCAL}"}
    assert document == before and edits == original_edits


def test_array_operations_use_the_current_ordered_document(document):
    document["blueprint"]["connections"] = [{
        "id": "synthetic", "name": "Synthetic source", "transport": "stdio",
        "command": "synthetic-mcp", "args": ["first", "third"],
    }]
    before = copy.deepcopy(document)
    edits = [
        {"op": "add", "path": "/blueprint/connections/0/args/1", "value": "second"},
        {"op": "add", "path": "/blueprint/connections/0/args/-", "value": "fourth"},
        {"op": "replace", "path": "/blueprint/connections/0/args/0", "value": "zero"},
        {"op": "remove", "path": "/blueprint/connections/0/args/2"},
        {"op": "add", "path": "/blueprint/connections/0/args/3", "value": "fifth"},
    ]
    original_edits = copy.deepcopy(edits)
    result = apply_guided_edits(document, edits)
    assert result["blueprint"]["connections"][0]["args"] == ["zero", "second", "fourth", "fifth"]
    assert document == before and edits == original_edits


def test_exact_unicode_internal_whitespace_and_inserted_values_are_not_inferred(document):
    literal = "Observe blåbär 星 🌌.\n  Keep\tinterior  whitespace and ${INPUT_CONTEXT} verbatim."
    fact = {"summary": "One  coherent\tconversation\nwith exact evidence.",
        "status": "proposed", "source_message_ids": []}
    edits = [
        {"op": "replace", "path": "/blueprint/agent_classes/0/instructions", "value": literal},
        {"op": "add", "path": "/brief/facts/interfaces", "value": fact},
    ]
    before = copy.deepcopy((document, edits))
    result = apply_guided_edits(document, edits)
    assert result["blueprint"]["agent_classes"][0]["instructions"] == literal
    assert result["brief"]["facts"]["interfaces"] == fact
    result["brief"]["facts"]["interfaces"]["source_message_ids"].append("result-only")
    assert (document, edits) == before


def test_canonical_normalization_preserves_the_existing_blueprint_brief_contract(document):
    raw = copy.deepcopy(document["blueprint"])
    raw.pop("storage")
    raw["project"]["name"] = "  A renamed project  "
    brief = {"facts": {}, "requirements": []}
    before = copy.deepcopy((raw, brief))
    result = guided_document(raw, brief)
    assert result["blueprint"] == Blueprint.model_validate(raw).model_dump(mode="json")
    assert result["brief"] == DesignBrief.model_validate(brief).model_dump(mode="json")
    assert result["blueprint"]["storage"] == "sqlite"
    assert result["blueprint"]["project"]["name"] == "A renamed project"
    assert (raw, brief) == before


@pytest.mark.parametrize("edit", [
    {"op": "move", "path": "/brief/facts"},
    {"op": "copy", "path": "/brief/facts", "from": "/brief/facts"},
    {"op": "test", "path": "/brief/facts", "value": {}},
    {"op": [], "path": "/brief/facts"},
    {"op": {}, "path": "/brief/facts"},
    {"path": "/brief/facts", "value": {}},
    {"op": "add", "path": "/brief/facts"},
    {"op": "replace", "path": "/brief/facts"},
    {"op": "remove", "path": "/brief/facts", "value": {}},
    {"op": "add", "path": "/brief/facts", "value": {}, "extra": True},
    {"op": "remove", "path": "/brief/facts", "from": "/brief/facts"},
    {"op": "remove", "path": ""},
    {"op": "remove", "path": "brief/facts"},
    {"op": "remove", "path": "/not-a-root/facts"},
    {"op": "remove", "path": "/brief/facts/outcome~2"},
    {"op": "remove", "path": "/brief/facts/outcome~"},
    {"op": "remove", "path": "/brief/facts/missing"},
    {"op": "add", "path": "/brief/missing/child", "value": {}},
    {"op": "replace", "path": "/brief/facts/outcome/summary/child", "value": "bad"},
    {"op": "remove", "path": "/brief"},
    {"op": "remove", "path": "/blueprint"},
    None,
    "replace",
])
def test_invalid_operation_rejects_without_mutating_a_preceding_valid_change(document, edit):
    edits = [{"op": "replace", "path": "/blueprint/project/name", "value": "Discard this update"}, edit]
    before = copy.deepcopy((document, edits))
    with pytest.raises(ValueError):
        apply_guided_edits(document, edits)
    assert (document, edits) == before


@pytest.mark.parametrize("index", ["-", "-1", "01", "+1", "1.0", " 1", "1 ", "2", "9" * 5000])
@pytest.mark.parametrize("operation", ["replace", "remove"])
def test_invalid_array_target_index_rejects(document, index, operation):
    edits = [{"op": operation, "path": "/brief/requirements/" + index}]
    if operation == "replace":
        edits[0]["value"] = copy.deepcopy(document["brief"]["requirements"][0])
    before = copy.deepcopy((document, edits))
    with pytest.raises(ValueError):
        apply_guided_edits(document, edits)
    assert (document, edits) == before


@pytest.mark.parametrize("index", ["-1", "01", "+1", "1.0", "2", "9" * 5000])
def test_invalid_array_insertion_index_rejects(document, index):
    before = copy.deepcopy(document)
    with pytest.raises(ValueError):
        apply_guided_edits(document, [{"op": "add", "path": "/brief/requirements/" + index,
            "value": {"id": "new-feature", "outcome": "Retain another useful outcome.",
                "disposition": "open", "source_message_ids": ["user-current"]}}])
    assert document == before


@pytest.mark.parametrize("value", [False, [], "", 1])
def test_edited_brief_root_must_be_an_object(document, value):
    before = copy.deepcopy(document)
    with pytest.raises(ValueError):
        apply_guided_edits(document, [{"op": "replace", "path": "/brief", "value": value}])
    assert document == before


@pytest.mark.parametrize("path, value", [
    ("/blueprint/topology/entrypoint", "missing:main"),
    ("/blueprint/agent_classes/0/instances", True),
    ("/blueprint/agent_classes", []),
    ("/brief/facts/outcome/source_message_ids", []),
    ("/brief/requirements/0/disposition", "not-a-disposition"),
])
def test_complete_canonical_blueprint_and_brief_are_validated(document, path, value):
    before = copy.deepcopy(document)
    with pytest.raises(ValueError):
        apply_guided_edits(document, [{"op": "replace", "path": path, "value": value}])
    assert document == before


@pytest.mark.parametrize("value", [float("nan"), float("inf"), float("-inf"), {"nested": float("nan")}])
def test_nonfinite_json_is_rejected_before_editing(document, value):
    before = copy.deepcopy(document)
    with pytest.raises(ValueError, match="finite JSON"):
        apply_guided_edits(document, [{"op": "replace", "path": "/blueprint/project/name", "value": value}])
    assert document == before


def test_operation_count_and_encoded_payload_bounds_are_work_limits(document):
    before = copy.deepcopy(document)
    harmless = {"op": "replace", "path": "/blueprint/project/name", "value": "Bounded update"}
    assert apply_guided_edits(document, [harmless] * MAX_GUIDED_EDITS)["blueprint"]["project"]["name"] == "Bounded update"
    with pytest.raises(ValueError, match="at most"):
        apply_guided_edits(document, [harmless] * (MAX_GUIDED_EDITS + 1))
    with pytest.raises(ValueError, match="too large"):
        apply_guided_edits(document, [{**harmless, "value": "x" * MAX_STATE_BYTES}])
    oversized = copy.deepcopy(document)
    oversized["blueprint"]["oversized_transient_value"] = "x" * MAX_STATE_BYTES
    with pytest.raises(ValueError, match="too large"):
        apply_guided_edits(oversized, [])
    assert document == before


@pytest.mark.parametrize("edits", [None, {}, "[]", ()])
def test_edit_collection_requires_a_json_array(document, edits):
    with pytest.raises(ValueError):
        apply_guided_edits(document, edits)


@pytest.mark.parametrize("field, value", [
    ("guide_id", "another-guide"), ("guide_version", 4), ("action", "manual"),
    ("current_stage", "autonomy"), ("current_status", "refining"),
    ("current_question", {"id": "another-question"}),
    ("saved_plan_available", False), ("manual_plan_submitted", True),
    ("current_user_message", {"id": "another-message", "role": "user", "text": "Retain my evidence."}),
    ("evidence_messages", [{"id": "earlier-evidence", "role": "user", "text": "Different evidence."}]),
    ("manual_edits", [{"changed_paths": ["/project/name"], "rationale": "An explicit correction."}]),
    ("reopened_topics", ["ownership"]),
])
def test_token_changes_with_host_turn_manual_and_evidence_bindings(document, context, field, value):
    original = copy.deepcopy((document, context))
    initial = guided_edit_token(context, document["blueprint"])
    changed = copy.deepcopy(context)
    changed[field] = value
    assert guided_edit_token(changed, document["blueprint"]) != initial
    assert (document, context) == original


def test_token_binds_the_canonical_full_blueprint_and_brief(document, context):
    initial = guided_edit_token(context, document["blueprint"])
    changed_blueprint = copy.deepcopy(document["blueprint"])
    changed_blueprint["agent_classes"][0]["instructions"] += " Preserve an additional correction."
    assert guided_edit_token(context, changed_blueprint) != initial
    changed_context = copy.deepcopy(context)
    changed_context["brief"]["facts"]["outcome"]["summary"] = "Retain corrected observer evidence."
    assert guided_edit_token(changed_context, document["blueprint"]) != initial
    reordered = json.loads(json.dumps(document["blueprint"], sort_keys=True))
    reordered.pop("storage")
    assert guided_edit_token(context, reordered) == initial


@pytest.mark.parametrize("field, value", [
    ("guide_id", None), ("guide_id", ""), ("guide_id", 3),
    ("guide_version", None), ("guide_version", 0), ("guide_version", -1),
    ("guide_version", True), ("guide_version", "3"),
    ("current_user_message", {}), ("current_user_message", {"id": ""}),
])
def test_token_requires_host_saved_guide_version_and_current_message(document, context, field, value):
    changed = copy.deepcopy(context)
    changed[field] = value
    with pytest.raises(ValueError):
        guided_edit_token(changed, document["blueprint"])
    with pytest.raises(ValueError):
        guided_edit_token({}, document["blueprint"])


def test_schema_references_resolve_from_the_tool_root_and_exclude_model_authority_fields():
    schema = guided_edit_schema()
    assert schema == guided_edit_schema()
    assert schema["additionalProperties"] is False
    assert set(schema["required"]) == {"base_token", "edits", "question", "message"}
    assert not {"brief", "blueprint", "design_changed", "guide_version"} & schema["properties"].keys()
    references = []

    def visit(value):
        if isinstance(value, dict):
            if "$ref" in value:
                references.append(value["$ref"])
            for child in value.values():
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)

    visit(schema)
    assert references
    for reference in references:
        assert reference.startswith("#/")
        target = schema
        for part in reference[2:].split("/"):
            target = target[part.replace("~1", "/").replace("~0", "~")]
        assert isinstance(target, dict)
    alternatives = schema["properties"]["edits"]["items"]["oneOf"]
    assert all(item["additionalProperties"] is False for item in alternatives)
    assert schema["properties"]["edits"]["maxItems"] == MAX_GUIDED_EDITS
