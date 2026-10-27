"""Conversational visual preferences reach the author without changing the mob."""
from copy import deepcopy

import pytest

from launchkit.visualizer import VISUALIZATION_FEEDBACK_GUIDANCE, checked_inputs
from test_visualizer import inputs, inspected_job, large_inspection_inputs, service


def preference():
    return {"text": "  Keep the circular population; enlarge specialist labels.\nMove explanations into the story caption. 🐘  ",
            "source_message_id": "current-human-turn"}


def test_feedback_only_changes_animation_binding_and_preserves_all_graph_facts(service):
    blueprint, legacy = inputs()
    model = {**legacy, "visualization_feedback": preference()}
    before = deepcopy(model)
    checked_blueprint, checked_model = checked_inputs(blueprint, model)
    assert checked_blueprint == blueprint
    assert checked_model == model == before
    checked_model["visualization_feedback"]["text"] = "Another composition"
    assert model == before
    first = service.request_binding(blueprint, legacy)
    current = service.request_binding(blueprint, model)
    assert first["blueprint_hash"] == current["blueprint_hash"]
    assert first["model_hash"] != current["model_hash"]
    assert first["native_request_key"] != current["native_request_key"]
    assert service.request_binding(blueprint, deepcopy(model)) == current
    assert {key: value for key, value in model.items() if key != "visualization_feedback"} == legacy
    cleared = deepcopy(model)
    del cleared["visualization_feedback"]
    assert service.request_binding(blueprint, cleared) == first


@pytest.mark.asyncio
async def test_large_author_overview_and_exact_inspection_retain_current_preference(service):
    blueprint, model = large_inspection_inputs()
    model["visualization_feedback"] = preference()
    before = deepcopy(model)
    inspected_job(service, (blueprint, model))
    overview = await service.inspect({})
    assert overview["model"]["edges_complete"] is False
    assert overview["model"]["visualization_feedback"] == preference()
    assert overview["visualization_feedback_guidance"] == VISUALIZATION_FEEDBACK_GUIDANCE
    assert "does not impose a composition schema" in overview["visualization_feedback_guidance"]
    exact = await service.inspect({"view": "full", "request_key": overview["request_key"]})
    assert exact["model"] == model == before


@pytest.mark.asyncio
async def test_direct_refinement_keeps_its_own_explicit_feedback_and_cleared_constraints(service):
    blueprint, model = inputs()
    model["visualization_feedback"] = preference()
    direct = "For this revision use a wide arc so these labels remain readable."
    guided = service.request_binding(blueprint, model)
    refined = service.request_binding(blueprint, model, direct)
    assert guided["model_hash"] == refined["model_hash"]
    assert guided["native_request_key"] != refined["native_request_key"]
    inspected_job(service, (blueprint, model))
    service.pending["revision_feedback"] = direct
    overview = await service.inspect({})
    assert overview["revision_feedback"] == direct
    assert overview["model"]["visualization_feedback"] == preference()
    assert "specific correction precedence" in overview["visualization_feedback_guidance"]
    del model["visualization_feedback"]
    inspected_job(service, (blueprint, model))
    cleared = await service.inspect({})
    assert "visualization_feedback" not in cleared["model"]
    assert "no conversational composition constraint is active" in cleared["visualization_feedback_guidance"]


@pytest.mark.parametrize("value", [None, [], {}, {"text": "Visual changes"},
    {**preference(), "native_authority": "installed"},
    {**preference(), "text": ""}, {**preference(), "text": "  "},
    {**preference(), "text": "🐘" * 1001}, {**preference(), "text": 42},
    {**preference(), "text": "bad\x00feedback"}, {**preference(), "text": "\ud800"},
    {**preference(), "source_message_id": ""},
    {**preference(), "source_message_id": "bad\nreference"},
    {**preference(), "source_message_id": "x" * 257}])
def test_malformed_preference_is_refused_before_native_request_binding(service, value):
    blueprint, model = inputs()
    model["visualization_feedback"] = value
    with pytest.raises(ValueError):
        service.request_binding(blueprint, model)
    assert service.architect.starts == 0
    assert service.pending is None


def test_visual_preference_limits_count_unicode_codepoints_without_truncation():
    blueprint, model = inputs()
    model["visualization_feedback"] = {"text": "🐘" * 1000, "source_message_id": "🐘" * 256}
    assert checked_inputs(blueprint, model)[1]["visualization_feedback"] == model["visualization_feedback"]
