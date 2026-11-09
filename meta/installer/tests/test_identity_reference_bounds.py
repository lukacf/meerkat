"""Every valid named Blueprint identity remains usable as an exact reference."""
import copy
import json
import tomllib

import pytest
from pydantic import ValidationError

from launchkit.blueprint import AcquisitionSetup, Blueprint, Edge, FlowStep, Topology, starter_blueprint
from launchkit.blueprint_authoring import authoring_schema, materialize_blueprint
from launchkit.compiler import generate_files
from launchkit.guide_models import IncomingDataRoute, default_question, validate_guide_publication


def named_plan(member_length=20, mode="manual"):
    plan = starter_blueprint().model_dump(mode="json")
    classes = [copy.deepcopy(plan["agent_classes"][0]), copy.deepcopy(plan["agent_classes"][-1])]
    for role, letter in zip(classes, ("a", "b")):
        role.update(id=letter * 40, instances=1, members=[{
            "key": "m" * member_length, "name": role["name"],
            "instructions": "Keep the exact project member context and evidence."}])
    identities = [role["id"] + ":" + role["members"][0]["key"] for role in classes]
    plan["agent_classes"] = classes
    plan["topology"] = {"mode": mode, "entrypoint": identities[0],
        "edges": [{"a": identities[0], "b": identities[1]}] if mode == "manual" else []}
    plan["flows"] = [{"id": "review", "name": "Review project evidence", "steps": [
        {"id": "prepare", "agent": identities[0], "prompt": "Prepare the project evidence."},
        {"id": "check", "agent": identities[1], "prompt": "Check the prepared evidence.", "depends_on": ["prepare"]}]}]
    return plan, identities


@pytest.mark.parametrize("member_length", [19, 20])
@pytest.mark.parametrize("mode", ["manual", "hub", "pipeline", "mesh"])
def test_full_length_named_identities_survive_compilation(member_length, mode):
    plan, identities = named_plan(member_length, mode)
    before = copy.deepcopy(plan)
    spec = Blueprint.model_validate(plan)
    assert spec.identities() == identities
    assert all(len(identity) == 41 + member_length for identity in identities)
    files = generate_files(spec)
    deployment = json.loads(files["deployment.json"])
    assert deployment["entrypoint"] == identities[0]
    assert [agent["identity"] for agent in deployment["agents"]] == identities
    assert deployment["connections"] == [identities]
    native = tomllib.loads(files["config/mob.toml"])
    for step, agent in zip(("prepare", "check"), deployment["agents"]):
        assert native["flows"]["review"]["steps"][step]["role"] == agent["profile"]
        assert any(agent["identity"] in native["skills"][skill]["content"]
                   for skill in native["profiles"][agent["profile"]]["skills"])
    assert plan == before


def test_full_length_identity_selectors_materialize_and_compile():
    plan, identities = named_plan()
    plan["topology"]["edges"] = []
    authoring = {"peer_edge_groups": [
        {"a": {"identities": [identities[0]]}, "b": {"identities": [identities[1]]}}]}
    before = copy.deepcopy((plan, authoring))
    expanded = materialize_blueprint(plan, authoring)
    assert expanded["topology"]["edges"] == [{"a": identities[0], "b": identities[1]}]
    compiled = generate_files(Blueprint.model_validate(expanded))
    assert json.loads(compiled["deployment.json"])["connections"] == [identities]
    selector = authoring_schema()["$defs"]["IdentitySelector"]["properties"]["identities"]
    array = next(item for item in selector["anyOf"] if item.get("type") == "array")
    assert array["items"]["maxLength"] == 61
    assert (plan, authoring) == before


@pytest.mark.parametrize("side", ["a", "b"])
def test_identity_selectors_reject_62_character_references(side):
    plan, identities = named_plan()
    plan["topology"]["edges"] = []
    group = {"a": {"identities": [identities[0]]}, "b": {"identities": [identities[1]]}}
    group[side]["identities"][0] += "x"
    with pytest.raises(ValidationError) as error:
        materialize_blueprint(plan, {"peer_edge_groups": [group]})
    assert any(row["loc"] == ("peer_edge_groups", 0, side, "identities", 0)
               and row["type"] == "string_too_long" for row in error.value.errors())


@pytest.mark.parametrize("delivery", ["direct", "triage", "conversation"])
def test_maximum_named_identity_can_receive_incoming_information(delivery):
    plan, identities = named_plan()
    source = {"id": "changes", "requirement_id": "updates", "name": "Project updates",
        "scope": "Selected project records", "purpose": "Notice relevant changes.",
        "routes": [{"recipient": identities[1], "delivery": delivery}],
        "source_message_ids": ["request"]}
    brief = {"facts": {"outcome": {"summary": "Review changes to project records.",
        "status": "confirmed", "source_message_ids": ["request"]}},
        "requirements": [{"id": "updates", "outcome": "Deliver relevant project changes.",
            "disposition": "open", "source_message_ids": ["request"]}], "incoming_data": [source]}
    result = validate_guide_publication({"message": "Choose the information scope.", "brief": brief,
        "question": default_question("connections"), "design_changed": True},
        {"brief": {}, "evidence_messages": [{"id": "request", "role": "user"}]}, plan)
    assert result["brief"]["incoming_data"][0]["routes"] == source["routes"]


@pytest.mark.parametrize("model,args,field", [
    (Edge, {"b": "lead:main"}, "a"),
    (Edge, {"a": "lead:main"}, "b"),
    (Topology, {}, "entrypoint"),
    (FlowStep, {"id": "review", "prompt": "Review evidence."}, "agent"),
    (IncomingDataRoute, {"delivery": "direct"}, "recipient"),
    (AcquisitionSetup, {"helper_profile": "dedicated", "helper_tools": "preparation"}, "helper_identity"),
])
def test_identity_references_reject_more_than_blueprint_can_declare(model, args, field):
    with pytest.raises(ValidationError) as error:
        model.model_validate({**args, field: "a" * 40 + ":" + "m" * 21})
    assert any(row["loc"] == (field,) and row["type"] == "string_too_long" for row in error.value.errors())


@pytest.mark.parametrize("part", ["class", "member"])
def test_identity_reference_fix_does_not_expand_declaration_bounds(part):
    plan, _ = named_plan()
    if part == "class":
        plan["agent_classes"][0]["id"] += "a"
        location = ("agent_classes", 0, "id")
    else:
        plan["agent_classes"][0]["members"][0]["key"] += "m"
        location = ("agent_classes", 0, "members", 0, "key")
    with pytest.raises(ValidationError) as error:
        Blueprint.model_validate(plan)
    assert any(row["loc"] == location and row["type"] == "string_pattern_mismatch" for row in error.value.errors())
