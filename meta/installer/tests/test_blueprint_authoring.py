import copy
import json
import tomllib

import pytest

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.blueprint_authoring import blueprint_tool_schema, materialize_blueprint
from launchkit.compiler import generate_files
from launchkit.planning_limits import MAX_IDENTITIES, MAX_INSTANCES_PER_CLASS, MAX_PEER_EDGES


def population(count=501):
    value = starter_blueprint().model_dump(mode="json")
    base = value["agent_classes"][0]
    value["agent_classes"] = [{**copy.deepcopy(base), "id": "owner", "instances": count,
                               "members": []}] + [
        {**copy.deepcopy(base), "id": identity, "instances": 1, "members": []}
        for identity in ("domain_a", "domain_b", "domain_c", "portfolio")]
    value["topology"] = {"mode": "manual", "entrypoint": "portfolio:main", "edges": []}
    value["flows"] = []
    return value


def sequence(count=501, **changes):
    return {"id": "owners", "class_id": "owner", "start": 1, "count": count,
            "key_template": "p{n:03d}", "name_template": "Project {n:03d}",
            "instructions_template": "Own only {name} and retain its decisions and evidence.", **changes}


def group(first, last, target):
    return {"a": {"sequence": "owners", "start": first, "stop": last},
            "b": {"identities": [target]}}


def authored_population():
    return {"member_sequences": [sequence()], "peer_edge_groups": [
        group(1, 167, "domain_a:main"), group(168, 334, "domain_b:main"),
        group(335, 501, "domain_c:main")]}


def test_exact_known_501_population_and_sparse_boundaries_become_the_normal_blueprint():
    raw, authoring = population(), authored_population()
    original = copy.deepcopy((raw, authoring))
    expanded = materialize_blueprint(raw, authoring)
    spec = Blueprint.model_validate(expanded)
    assert len(spec.identities()) == 505
    owners = spec.agent_classes[0]
    assert owners.instances == len(owners.members) == 501
    assert owners.members[0].key == "p001" and owners.members[-1].name == "Project 501"
    assert owners.members[167].instructions == "Own only Project 168 and retain its decisions and evidence."
    actual = {(edge.a, edge.b) for edge in spec.resolved_edges()}
    assert actual == {(f"owner:p{n:03d}", f"domain_{'a' if n <= 167 else 'b' if n <= 334 else 'c'}:main")
                      for n in range(1, 502)}
    for n, domain in ((1, "a"), (167, "a"), (168, "b"), (334, "b"), (335, "c"), (501, "c")):
        assert (f"owner:p{n:03d}", f"domain_{domain}:main") in actual
    assert not any("portfolio:main" in pair for pair in actual)
    assert [(row["id"], row["tools"], row["model"], row["provider"], row["instructions"])
            for row in expanded["agent_classes"]] == [
        (row["id"], row["tools"], row["model"], row["provider"], row["instructions"])
        for row in raw["agent_classes"]]
    assert "authoring" not in expanded and (raw, authoring) == original
    files = generate_files(spec)
    runtime = json.loads(files["deployment.json"])
    native = tomllib.loads(files["config/mob.toml"])
    compiled = {row["identity"]: row for row in runtime["agents"]}
    assert set(compiled) == set(spec.identities())
    assert {tuple(pair) for pair in runtime["connections"]} == actual
    selected = compiled["owner:p168"]
    assert selected["name"] == "Project 168" and selected["profile"] == "owner__p168"
    assert selected["classId"] == "owner" and selected["tools"] == owners.tools.model_dump(mode="json")
    profile = native["profiles"][selected["profile"]]
    member_skill = native["skills"]["owner__p168_member"]["content"]
    assert "Your durable identity is owner:p168." in member_skill
    assert "Own only Project 168 and retain its decisions and evidence." in member_skill
    assert profile["model"] == owners.model and profile["provider"] == owners.provider
    assert "owner__p168_member" in profile["skills"]


def test_raw_blueprint_unchanged_and_sequences_do_not_infer_any_relationships():
    raw = starter_blueprint().model_dump(mode="json")
    assert materialize_blueprint(raw) == raw
    expanded = materialize_blueprint(population(2), {"member_sequences": [sequence(2)]})
    assert Blueprint.model_validate(expanded).resolved_edges() == []


def test_multiple_explicit_sequences_can_fill_one_class_and_preserve_raw_members():
    raw = population(3)
    raw["agent_classes"][0]["members"] = [{"key": "existing", "name": "Existing item",
                                         "instructions": "Retain this independently named context."}]
    authoring = {"member_sequences": [sequence(1, id="first", start=10),
                                      sequence(1, id="second", start=20)]}
    expanded = materialize_blueprint(raw, authoring)
    spec = Blueprint.model_validate(expanded)
    assert spec.agent_classes[0].identities() == ["owner:existing", "owner:p010", "owner:p020"]
    assert expanded["agent_classes"][0]["members"][0] == raw["agent_classes"][0]["members"][0]


@pytest.mark.parametrize("count", [True, 1.0, "2", 0, MAX_INSTANCES_PER_CLASS + 1])
def test_noninteger_or_out_of_bound_sequence_counts_reject_without_mutation(count):
    raw = population(2)
    before = copy.deepcopy(raw)
    with pytest.raises(ValueError):
        materialize_blueprint(raw, {"member_sequences": [sequence(count)]})
    assert raw == before


@pytest.mark.parametrize("mutation", [
    lambda a: a["member_sequences"].append(copy.deepcopy(a["member_sequences"][0])),
    lambda a: a["member_sequences"][0].update(class_id="missing"),
    lambda a: a["member_sequences"][0].update(count=500),
    lambda a: a["member_sequences"][0].update(start=True),
    lambda a: a["peer_edge_groups"][0]["a"].update(sequence="missing"),
    lambda a: a["peer_edge_groups"][0]["a"].update(start=0),
    lambda a: a["peer_edge_groups"][0]["a"].update(stop=502),
    lambda a: a["peer_edge_groups"][0]["a"].update(start=168, stop=167),
    lambda a: a["peer_edge_groups"][0]["a"].update(start=True),
    lambda a: a["peer_edge_groups"][0]["a"].update(stop=False),
    lambda a: a["peer_edge_groups"][0]["b"].update(identities=["missing:main"]),
    lambda a: a["peer_edge_groups"][0]["b"].update(identities=[]),
    lambda a: a["peer_edge_groups"][0]["b"].update(identities=["domain_a:main", "domain_a:main"]),
    lambda a: a["peer_edge_groups"][0]["b"].update(sequence="owners"),
    lambda a: a["peer_edge_groups"][0].pop("b"),
    lambda a: a.update(grants={"shell": True}),
])
def test_invalid_or_ambiguous_authoring_is_not_a_partial_graph(mutation):
    raw, authoring = population(), authored_population()
    mutation(authoring)
    before = copy.deepcopy((raw, authoring))
    with pytest.raises(ValueError):
        materialize_blueprint(raw, authoring)
    assert (raw, authoring) == before


def test_duplicate_generated_keys_self_links_and_unordered_edge_duplicates_reject():
    raw = population(2)
    with pytest.raises(ValueError, match="keys must be unique"):
        materialize_blueprint(raw, {"member_sequences": [sequence(1), sequence(1, id="other")]})
    for pairs in ([{"a": {"identities": ["owner:p001"]}, "b": {"identities": ["owner:p001"]}}],
                  [group(1, 1, "domain_a:main"),
                   {"a": {"identities": ["domain_a:main"]}, "b": {"sequence": "owners", "stop": 1}}]):
        with pytest.raises(ValueError, match="unique unordered pairs"):
            materialize_blueprint(raw, {"member_sequences": [sequence(2)], "peer_edge_groups": pairs})
    raw["topology"]["edges"] = [{"a": "domain_a:main", "b": "owner:p001"}]
    with pytest.raises(ValueError, match="unique unordered pairs"):
        materialize_blueprint(raw, {"member_sequences": [sequence(2)],
                                  "peer_edge_groups": [group(1, 1, "domain_a:main")]})


def test_explicit_peer_groups_cannot_silently_change_hub_or_mesh_semantics():
    for mode in ("hub", "mesh", "pipeline"):
        raw = population()
        raw["topology"]["mode"] = mode
        with pytest.raises(ValueError, match="explicitly manual"):
            materialize_blueprint(raw, authored_population())


@pytest.mark.parametrize("template", ["same", "{n.__class__}", "{n[0]}", "{n!r}",
                                     "{n:999999999999d}", "{n:{name}}", "{unknown}", "{n"])
def test_templates_are_literal_bounded_and_do_not_evaluate_expressions(template):
    with pytest.raises(ValueError):
        materialize_blueprint(population(1), {"member_sequences": [sequence(1, key_template=template)]})


def test_expanded_member_string_bounds_and_utf8_are_checked():
    with pytest.raises(ValueError):
        materialize_blueprint(population(1), {"member_sequences": [sequence(1,
            instructions_template="{name}" * 570)]})
    with pytest.raises(ValueError):
        materialize_blueprint(population(1), {"member_sequences": [sequence(1, name_template="\ud800 {n}")]})
    expanded = materialize_blueprint(population(1), {"member_sequences": [sequence(1,
        name_template="Projekt åäö {n:03d}", instructions_template="Bevara {name} och dess egna beslut.")]})
    assert expanded["agent_classes"][0]["members"][0]["instructions"] == "Bevara Projekt åäö 001 och dess egna beslut."
    expanded = materialize_blueprint(population(1), {"member_sequences": [sequence(1,
        instructions_template="{name}")]})
    assert expanded["agent_classes"][0]["members"][0]["instructions"] == "Project 001"


def test_aggregate_identity_limit_rejects_before_rendering_members(monkeypatch):
    import launchkit.blueprint_authoring as module
    raw = population(1)
    base = raw["agent_classes"][0]
    raw["agent_classes"] = [{**copy.deepcopy(base), "id": f"class{n}", "instances": MAX_INSTANCES_PER_CLASS}
                           for n in range(MAX_IDENTITIES // MAX_INSTANCES_PER_CLASS + 1)]
    monkeypatch.setattr(module, "_render", lambda *args: pytest.fail("No members may be rendered above the total limit"))
    with pytest.raises(ValueError, match="total identity limit"):
        materialize_blueprint(raw, {"member_sequences": [sequence(1, class_id="class0")]})


def test_aggregate_peer_limit_rejects_before_allocating_cartesian_pairs(monkeypatch):
    import launchkit.blueprint_authoring as module
    raw = population(1)
    class Selected:
        def __len__(self):
            return MAX_INSTANCES_PER_CLASS
        def __iter__(self):
            pytest.fail("No Cartesian pair allocation is permitted above the limit")
    monkeypatch.setattr(module, "_select", lambda *args: Selected())
    with pytest.raises(ValueError, match="peer-edge limit"):
        materialize_blueprint(raw, {"peer_edge_groups": [
            {"a": {"identities": ["domain_a:main"]}, "b": {"identities": ["domain_b:main"]}}]})
    assert MAX_INSTANCES_PER_CLASS**2 > MAX_PEER_EDGES


def test_peer_budget_is_aggregate_across_groups_and_existing_edges(monkeypatch):
    import launchkit.blueprint_authoring as module
    monkeypatch.setattr(module, "MAX_PEER_EDGES", 2)
    raw = population(2)
    raw["topology"]["edges"] = [{"a": "domain_a:main", "b": "domain_b:main"}]
    with pytest.raises(ValueError, match="peer-edge limit"):
        materialize_blueprint(raw, {"member_sequences": [sequence(2)],
            "peer_edge_groups": [group(1, 1, "domain_a:main"), group(2, 2, "domain_b:main")]})


def test_registered_tool_schema_keeps_refs_resolvable_and_raw_calls_optional():
    for summary in (False, True):
        schema = blueprint_tool_schema(summary=summary)
        assert "authoring" not in schema["required"]
        assert ("summary" in schema["required"]) == summary
        def refs(node):
            if isinstance(node, dict):
                if "$ref" in node:
                    reference = node["$ref"]
                    assert reference.startswith("#/$defs/")
                    assert reference.split("/")[-1] in schema["$defs"]
                for value in node.values():
                    refs(value)
            elif isinstance(node, list):
                for value in node:
                    refs(value)
        refs(schema)
