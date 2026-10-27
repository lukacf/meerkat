"""Reusable class intent must never manufacture an initial durable owner."""
from __future__ import annotations

import copy
import json
import tomllib

import pytest
from pydantic import ValidationError

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.blueprint_authoring import materialize_blueprint
from launchkit.compiler import generate_files, package_preview, resolved_blueprint
from launchkit.visualizer import checked_inputs, visualization_context
from meerkat_mobkit.agent_builder import CallbackDispatcher
from test_application_extension import load_host, write_application


def profile_plan():
    raw = starter_blueprint().model_dump(mode="json")
    lead, reusable = copy.deepcopy(raw["agent_classes"][:2])
    reusable.update(id="resident", name="Resident context", instances=0, members=[],
                    description="Own one actual resident's preferences and requests.",
                    instructions="Retain only the actual adopted resident's context and authorized evidence.")
    reusable["tools"].update(notes="read", mcp=[])
    raw.update(agent_classes=[lead, reusable],
               topology={"mode": "manual", "entrypoint": "lead:main", "edges": []},
               flows=[{"id": "intake", "name": "Receive a request", "steps": [
                   {"id": "receive", "agent": "lead:main", "prompt": "Review the actual request.",
                    "depends_on": []}]}])
    return raw


def graph(raw):
    spec = Blueprint.model_validate(raw).model_dump(mode="json")
    return {"nodes": [{"id": "lead:main", "kind": "agent", "name": "Actual intake owner"}],
            "edges": [], "phases": [{"id": "intake", "activeNodeIds": ["lead:main"],
                                    "activeEdgeIds": []}],
            "profile_only_classes": [copy.deepcopy(spec["agent_classes"][1])]}


def test_zero_class_has_a_published_profile_but_no_durable_identity():
    raw = profile_plan()
    before = copy.deepcopy(raw)
    spec = Blueprint.model_validate(raw)
    assert spec.agent_classes[1].identities() == []
    assert spec.identities() == ["lead:main"]
    assert spec.model_json_schema()["$defs"]["AgentClass"]["properties"]["instances"]["minimum"] == 0
    assert resolved_blueprint(spec)["identities"][0]["identity"] == "lead:main"
    assert raw == before


@pytest.mark.parametrize("case", ["negative", "boolean", "fraction", "members", "entrypoint",
                                  "edge", "flow", "empty-population"])
def test_profile_intent_does_not_relax_identity_or_tool_authority(case):
    raw = profile_plan()
    resident = raw["agent_classes"][1]
    if case == "negative":
        resident["instances"] = -1
    elif case == "boolean":
        resident["instances"] = False
    elif case == "fraction":
        resident["instances"] = 0.0
    elif case == "members":
        resident["members"] = [{"key": "unassigned", "name": "Unassigned",
                                "instructions": "Do not create a substitute for a real owner."}]
    elif case == "entrypoint":
        raw["topology"]["entrypoint"] = "resident:main"
    elif case == "edge":
        raw["topology"]["edges"] = [{"a": "lead:main", "b": "resident:main"}]
    elif case == "flow":
        raw["flows"][0]["steps"][0]["agent"] = "resident:main"
    else:
        raw["agent_classes"][0]["instances"] = 0
    with pytest.raises(ValidationError):
        Blueprint.model_validate(raw)


def test_authoring_keeps_a_profile_only_class_without_fabricating_members():
    raw = profile_plan()
    before = copy.deepcopy(raw)
    expanded = materialize_blueprint(raw, {})
    assert expanded == raw == before
    assert Blueprint.model_validate(expanded).agent_classes[1].identities() == []
    directive = {"member_sequences": [{"id": "residents", "class_id": "resident", "start": 1,
                 "count": 1, "key_template": "person{n}", "name_template": "Person {n}",
                 "instructions_template": "Own only actual person {n}."}]}
    with pytest.raises(ValueError, match="declared instance count"):
        materialize_blueprint(raw, directive)
    assert raw == before


def test_compiler_preserves_reusable_grants_without_template_profile_or_seed():
    spec = Blueprint.model_validate(profile_plan())
    files = generate_files(spec)
    deployment = json.loads(files["deployment.json"])
    native = tomllib.loads(files["config/mob.toml"])
    assert [row["identity"] for row in deployment["agents"]] == ["lead:main"]
    assert deployment["connections"] == []
    profile = deployment["applicationProfiles"]["resident"]
    assert profile in native["profiles"]
    assert set(native["profiles"]) == {"lead", *deployment["applicationProfiles"].values()}
    assert native["profiles"][profile]["skills"] == ["resident_role", "launch_comms"]
    assert native["flows"]["intake"]["steps"]["receive"]["role"] == "lead"
    assert deployment["classes"][1]["tools"] == spec.agent_classes[1].tools.model_dump(mode="json")
    assert package_preview(spec)["summary"]["agents"] == 1
    readme = files["README.md"]
    assert "instances: 0" in readme
    assert "its application callback catalog is empty" in readme
    assert "No lazy catalog expansion or new registration hook" in readme
    assert "resident:unassigned" not in files["config/mob.toml"]


def test_optional_visual_metadata_is_exact_presentation_without_nodes_or_edges():
    raw, model = profile_plan(), graph(profile_plan())
    before = copy.deepcopy((raw, model))
    spec, checked = checked_inputs(raw, model)
    projected, overview = visualization_context(spec, checked)
    assert overview["profile_only_classes"] == [spec["agent_classes"][1]]
    assert projected["agent_classes"][1]["instances"] == 0
    assert {node["id"] for node in checked["nodes"]} == {"lead:main"}
    assert checked["edges"] == []
    assert (raw, model) == before
    legacy = copy.deepcopy(model)
    legacy.pop("profile_only_classes")
    assert "profile_only_classes" not in checked_inputs(raw, legacy)[1]


@pytest.mark.parametrize("case", ["missing-class", "wrong-count", "wrong-grant", "wrong-class",
                                  "phantom-agent", "phantom-edge"])
def test_visual_metadata_cannot_claim_another_class_population_or_grant(case):
    raw, model = profile_plan(), graph(profile_plan())
    if case == "missing-class":
        model["profile_only_classes"] = []
    elif case == "wrong-count":
        model["profile_only_classes"][0]["instances"] = 1
    elif case == "wrong-grant":
        model["profile_only_classes"][0]["tools"]["shell"] = True
    elif case == "wrong-class":
        model["profile_only_classes"][0]["id"] = "lead"
    elif case == "phantom-agent":
        model["nodes"].append({"id": "resident:unassigned", "kind": "agent"})
    else:
        model["edges"] = [{"id": "phantom", "source": "lead:main", "target": "resident:main"}]
    with pytest.raises(ValueError):
        checked_inputs(raw, model)


APPLICATION = '''
import copy

class Integration:
    def __init__(self, context):
        self.rows = [{"identity": row["identity"], "classId": row["classId"]}
                     for row in context.deployment["agents"]]
        self.rows.extend(INITIAL_ROWS)
        self.callbacks = INITIAL_CALLBACKS
    def desired_roster(self):
        return self.rows
    def desired_edges(self, identities):
        return []
    def tools_for_agent(self, agent):
        if agent["classId"] != "resident" or not self.callbacks:
            return []
        adopted = copy.deepcopy(agent)
        async def read(arguments):
            return {"identity": adopted["identity"], "context": adopted.get("context"),
                    "tools": adopted["tools"]}
        return [{"name": "read_resident", "handler": read, "description": "Read the actual owner's record.",
                 "schema": {"type": "object", "properties": {}}}]
'''


async def application(tmp_path, monkeypatch, *, initial_rows=None, callbacks=False):
    host = load_host(monkeypatch, tmp_path)
    deployment = json.loads(generate_files(Blueprint.model_validate(profile_plan()))["deployment.json"])
    source = APPLICATION.replace("INITIAL_ROWS", repr(initial_rows or []))
    source = source.replace("INITIAL_CALLBACKS", repr(callbacks))
    write_application(tmp_path, source)
    adapter = host.Application(host.ApplicationContext(tmp_path, deployment, None))
    await adapter.load()
    return host, adapter, deployment


async def test_actual_generated_roster_accepts_future_real_owner_with_class_authority(tmp_path, monkeypatch):
    _, adapter, deployment = await application(tmp_path, monkeypatch)
    assert {row.identity for row in await adapter.roster(None)} == {"lead:main"}
    assert adapter._tool_catalog["resident"] == {}
    adapter.integration.rows.append({"identity": "resident:alice", "classId": "resident",
                                     "context": {"resident_record": "alice"}})
    await adapter._prepare_snapshot()
    rows = await adapter.roster(None)
    adopted = next(row for row in rows if row.identity == "resident:alice")
    assert adopted.profile == deployment["applicationProfiles"]["resident"]
    assert {row.identity for row in rows} == {"lead:main", "resident:alice"}
    assert adapter._snapshot[0]["resident:alice"]["tools"] == deployment["classes"][1]["tools"]
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(adapter)
    result = await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
        "scope_id": "actual-owner", "context": {"identity": adopted.identity},
        "spec": adopted.to_dict(), "draft": {}})
    assert result.get("external_tools", []) == []
    assert adapter.context.runtime is None


async def test_empty_startup_catalog_rejects_later_callback_without_publishing_snapshot(tmp_path, monkeypatch):
    _, adapter, _ = await application(tmp_path, monkeypatch)
    before = adapter._snapshot
    adapter.integration.rows.append({"identity": "resident:alice", "classId": "resident"})
    adapter.integration.callbacks = True
    with pytest.raises(ValueError, match="initial class catalog"):
        await adapter._prepare_snapshot()
    assert adapter._snapshot is before
    assert "resident:alice" not in adapter._identity_classes


async def test_actual_initial_desired_owner_establishes_catalog_for_future_owner_and_resume(tmp_path, monkeypatch):
    _, adapter, deployment = await application(tmp_path, monkeypatch, callbacks=True,
        initial_rows=[{"identity": "resident:alice", "classId": "resident", "context": {"owner": "alice"}}])
    assert [row["identity"] for row in deployment["agents"]] == ["lead:main"]
    assert set(adapter._tool_catalog["resident"]) == {"read_resident"}
    adapter.integration.rows.append({"identity": "resident:bob", "classId": "resident",
                                     "context": {"owner": "bob"}})
    await adapter._prepare_snapshot()
    dispatcher = CallbackDispatcher()
    dispatcher.register_agent_customizer(adapter)
    for identity, scope in [("resident:alice", "alice-create"), ("resident:bob", "bob-create"),
                            ("resident:alice", "alice-resume")]:
        adopted = next(row for row in await adapter.roster(None) if row.identity == identity)
        assert adopted.profile == deployment["applicationProfiles"]["resident"]
        await dispatcher.handle_callback("callback/agent_customizer/customize_build", {
            "scope_id": scope, "context": {"identity": identity}, "spec": adopted.to_dict(),
            "draft": {"resume_session_id": "existing" if "resume" in scope else None}})
        result = await dispatcher.handle_callback("callback/call_tool", {
            "scope_id": scope, "tool": "read_resident", "arguments": {}})
        assert result["content"]["identity"] == identity
        assert result["content"]["context"] == {"owner": identity.split(":")[1]}
        assert result["content"]["tools"] == deployment["classes"][1]["tools"]


@pytest.mark.parametrize("override", [{"classId": "unknown"}, {"profile": "lead"}, {"tools": {"shell": True}}])
async def test_desired_rows_cannot_change_compiled_profile_class_or_grants(tmp_path, monkeypatch, override):
    _, adapter, _ = await application(tmp_path, monkeypatch)
    before = adapter._snapshot
    adapter.integration.rows.append({"identity": "resident:alice", "classId": "resident", **override})
    with pytest.raises(ValueError):
        await adapter._prepare_snapshot()
    assert adapter._snapshot is before


def minimal_profile_plan():
    raw = profile_plan()
    raw["agent_classes"][1] = {key: raw["agent_classes"][1][key]
                              for key in ("id", "name", "description", "instructions", "instances")}
    return raw


def test_raw_valid_profile_metadata_accepts_omitted_defaults_without_rewriting_source():
    raw = minimal_profile_plan()
    model = graph(raw)
    model["profile_only_classes"] = [copy.deepcopy(raw["agent_classes"][1])]
    before = copy.deepcopy((raw, model))
    spec, checked = checked_inputs(raw, model)
    assert spec["agent_classes"][1]["provider"] == "openai"
    assert spec["agent_classes"][1]["tools"]["memory"] is True
    assert checked["profile_only_classes"] == model["profile_only_classes"]
    assert "provider" not in checked["profile_only_classes"][0]
    assert "tools" not in checked["profile_only_classes"][0]
    assert (raw, model) == before
    checked["profile_only_classes"][0]["name"] = "Presentation copy"
    assert (raw, model) == before
    canonical = graph(raw)
    assert checked_inputs(raw, canonical)[1] == canonical


@pytest.mark.parametrize("change", ["extra-field", "changed-grant", "changed-model", "missing-id",
                                     "duplicate", "not-list", "not-class"])
def test_normalizing_optional_defaults_never_relaxes_profile_authority(change):
    raw = minimal_profile_plan()
    model = graph(raw)
    model["profile_only_classes"] = [copy.deepcopy(raw["agent_classes"][1])]
    entry = model["profile_only_classes"][0]
    if change == "extra-field":
        entry["authority"] = "invented"
    elif change == "changed-grant":
        entry["tools"] = {"memory": False}
    elif change == "changed-model":
        entry["model"] = "gpt-6-astra"
        assert entry["model"] != Blueprint.model_validate(raw).model
    elif change == "missing-id":
        entry.pop("id")
    elif change == "duplicate":
        model["profile_only_classes"].append(copy.deepcopy(entry))
    elif change == "not-list":
        model["profile_only_classes"] = {"resident": entry}
    else:
        model["profile_only_classes"] = ["resident"]
    before = copy.deepcopy((raw, model))
    with pytest.raises(ValueError):
        checked_inputs(raw, model)
    assert (raw, model) == before
