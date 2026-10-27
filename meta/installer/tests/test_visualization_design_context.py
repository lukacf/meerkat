"""Saved design understanding reaches freeform animation without changing graph authority."""
from copy import deepcopy

import pytest

from launchkit.projects import fingerprint
from launchkit.visualizer import DESIGN_CONTEXT_GUIDANCE, VISUALIZER_PROMPT, checked_inputs
from test_visualizer import inputs, inspected_job, large_inspection_inputs, service


def fact(summary, status="confirmed"):
    return {"summary": summary, "status": status,
            "source_message_ids": ["human-answer"] if status == "confirmed" else []}


def context():
    return {"facts": {"outcome": fact("  Choose useful observing opportunities.\nPreserve uncertainty. 🐘  "),
                      "ownership": fact("People retain separate equipment histories."),
                      "workflow": fact("Ask weather and astronomy experts before equipment operation.", "proposed"),
                      "authority": fact("Equipment approval details are still open.", "unresolved")},
            "requirements": [{"id": "mobile", "outcome": "Discuss the proposed observing plan on a phone.",
                              "supported_subset": "", "extension": "", "disposition": "deferred",
                              "source_message_ids": ["mobile-request"], "decision_message_ids": ["defer-mobile"]}]}


def with_context(value=None):
    blueprint, model = inputs()
    model["design_context"] = context() if value is None else value
    return blueprint, model


def test_checked_context_preserves_exact_text_status_evidence_and_graph_without_mutating_source():
    blueprint, model = with_context()
    before = deepcopy(model)
    checked_blueprint, checked_model = checked_inputs(blueprint, model)
    assert model == before
    assert checked_blueprint == blueprint
    assert checked_model == before
    assert checked_model["design_context"] is not model["design_context"]
    checked_model["design_context"]["facts"]["ownership"]["source_message_ids"].append("different-evidence")
    assert model == before
    assert all(edge["kind"] == "peer" for edge in checked_model["edges"])


def invalid_contexts():
    yield None
    yield []
    yield {"facts": {}, "requirements": [], "native_status": "installed"}
    yield {"facts": None, "requirements": []}
    yield {"facts": {"unknown_topic": fact("Unknown topic")}, "requirements": []}
    for name, value in [
        ("summary", ""), ("summary", " "), ("summary", "🐘" * 1601),
        ("summary", "bad\x00context"), ("summary", "\ud800"), ("summary", "\udfff"),
        ("status", "implemented"), ("source_message_ids", []),
        ("source_message_ids", ["same", "same"]), ("source_message_ids", [42]),
        ("source_message_ids", [" "]), ("source_message_ids", ["bad\nreference"]),
        ("source_message_ids", ["x" * 257]),
        ("source_message_ids", [f"evidence-{i}" for i in range(41)]),
    ]:
        changed = context()
        changed["facts"]["outcome"][name] = value
        yield changed
    for name, value in [
        ("id", "x" * 81), ("outcome", "🐘" * 1201),
        ("supported_subset", "x" * 1601), ("extension", "\ud800"),
        ("disposition", "complete"), ("source_message_ids", []),
        ("source_message_ids", ["same", "same"]), ("decision_message_ids", []),
        ("decision_message_ids", ["same", "same"]),
    ]:
        changed = context()
        changed["requirements"][0][name] = value
        yield changed
    duplicate = context()
    duplicate["requirements"].append(deepcopy(duplicate["requirements"][0]))
    yield duplicate
    too_many = context()
    too_many["requirements"] = [dict(context()["requirements"][0], id=f"request-{i}") for i in range(33)]
    yield too_many


@pytest.mark.parametrize("value", list(invalid_contexts()))
def test_invalid_optional_context_is_rejected_before_native_work(value):
    blueprint, model = inputs()
    model["design_context"] = value
    with pytest.raises(ValueError):
        checked_inputs(blueprint, model)


def test_context_bounds_count_unicode_codepoints_and_retained_reference_limits():
    value = context()
    value["facts"]["outcome"]["summary"] = "🐘" * 1600
    value["facts"]["outcome"]["source_message_ids"] = [f"{i}-" + "🐘" * 253 for i in range(40)]
    value["requirements"] = [dict(value["requirements"][0], id=f"request-{i}",
                                  outcome="🐘" * 1200, supported_subset="🐘" * 1600,
                                  extension="🐘" * 1600) for i in range(32)]
    assert checked_inputs(*with_context(value))[1]["design_context"] == value


@pytest.mark.asyncio
async def test_overview_and_full_inspection_include_current_understanding_at_scale_and_request_local_guidance(service):
    blueprint, model = large_inspection_inputs()
    model["design_context"] = context()
    before = deepcopy(model)
    inspected_job(service, (blueprint, model))
    overview = await service.inspect({})
    assert overview["model"]["edges_complete"] is False
    assert overview["model"]["design_context"] == context()
    assert overview["design_context_guidance"] == DESIGN_CONTEXT_GUIDANCE
    assert "do not create configured identities" in overview["design_context_guidance"]
    assert "composition schema" in overview["design_context_guidance"]
    assert DESIGN_CONTEXT_GUIDANCE not in VISUALIZER_PROMPT
    exact = await service.inspect({"view": "full", "request_key": overview["request_key"]})
    assert exact["model"]["design_context"] == context()
    assert exact["model"]["edges"] == model["edges"]
    assert model == before


@pytest.mark.parametrize("topic", ["ownership", "audience", "workflow", "outcome", "authority"])
def test_context_only_changes_bind_new_native_source_with_identical_blueprint_and_relationships(service, topic):
    blueprint, model = with_context()
    first = service.request_binding(blueprint, model)
    changed = deepcopy(model)
    changed["design_context"]["facts"][topic] = fact(f"Different {topic} understanding")
    current = service.request_binding(blueprint, changed)
    assert first["blueprint_hash"] == current["blueprint_hash"]
    assert first["model_hash"] != current["model_hash"]
    assert first["native_request_key"] != current["native_request_key"]
    assert {key: value for key, value in changed.items() if key != "design_context"} == {
        key: value for key, value in model.items() if key != "design_context"}
    assert fingerprint(model) != fingerprint(changed)


@pytest.mark.asyncio
async def test_legacy_context_absence_does_not_add_assumptions_or_request_local_guidance(service):
    blueprint, model = inputs()
    checked = checked_inputs(blueprint, model)[1]
    assert "design_context" not in checked
    inspected_job(service, (blueprint, model))
    result = await service.inspect({})
    assert "design_context" not in result["model"]
    assert "design_context_guidance" not in result
