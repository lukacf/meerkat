"""Offline configuration tests; these do not claim native enforcement."""
import json
from pathlib import Path
import tomllib

import pytest
from pydantic import ValidationError

import launchkit.blueprint as blueprint_module
from launchkit.blueprint import Blueprint, ToolGrants, starter_blueprint
from launchkit.compiler import application_profiles, generate_files, mob_toml
from launchkit.design_knowledge import design_findings
from launchkit.rehearsal_worker import rehearsal_mob_toml

from test_meta_guide_runtime import selected as selected_guide_runtime


@pytest.fixture(autouse=True)
def candidate_origin():
    assert Path(blueprint_module.__file__).resolve().parents[1] == Path(__file__).resolve().parents[1]


def configured():
    data = starter_blueprint().model_dump(mode="json")
    data["agent_classes"][1]["tools"].update(
        read_only=True, workspace=True, notes="write", builtins=True,
        shell=True, memory=True, workgraph=True, schedule=True, knowledge=True,
        mcp=[{"connection_id": "evidence", "tool_names": ["read", "update"]}],
    )
    data["knowledge"].update(mode="remote")
    data["connections"] = [{"id": "evidence", "name": "Evidence", "transport": "http",
                            "url": "https://example.test/mcp"}]
    return Blueprint.model_validate(data)


def test_older_blueprint_defaults_to_unrestricted_policy():
    data = starter_blueprint().model_dump(mode="json")
    for item in data["agent_classes"]:
        item["tools"].pop("read_only", None)
    spec = Blueprint.model_validate(data)
    assert all(item.tools.read_only is False for item in spec.agent_classes)
    assert all("read_only" not in profile["tools"]
               for profile in tomllib.loads(mob_toml(spec))["profiles"].values())


@pytest.mark.parametrize("value", ["true", 1, None])
def test_native_policy_does_not_coerce_non_boolean_values(value):
    with pytest.raises(ValidationError):
        ToolGrants.model_validate({"read_only": value})


def test_round_trip_retains_policy_and_all_explicit_grants():
    original = configured()
    restored = Blueprint.model_validate_json(original.model_dump_json())
    assert restored == original
    grants = restored.agent_classes[1].tools.model_dump()
    assert grants["read_only"] is True
    assert grants["notes"] == "write" and grants["shell"] is True
    assert grants["schedule"] is True and grants["memory"] is True
    assert grants["mcp"] == [{"connection_id": "evidence", "tool_names": ["read", "update"]}]


def test_native_fixed_and_reusable_profiles_receive_same_policy_without_rewiring():
    spec = configured()
    native = tomllib.loads(mob_toml(spec))
    reusable = application_profiles(spec)
    for name in ("research__1", "research__2", reusable["research"]):
        tools = native["profiles"][name]["tools"]
        assert tools["read_only"] is True
        assert tools["comms"] is True
        assert all(tools[key] is True for key in ("builtins", "shell", "memory", "workgraph", "schedule"))
    assert "read_only" not in native["profiles"]["lead"]["tools"]
    unrestricted = spec.model_copy(deep=True)
    unrestricted.agent_classes[1].tools.read_only = False
    previous = tomllib.loads(mob_toml(unrestricted))
    assert native["flows"] == previous["flows"]
    assert native["skills"] == previous["skills"]
    assert spec.resolved_edges() == unrestricted.resolved_edges()


def test_package_and_rehearsal_keep_policy_in_every_configuration_surface(selected_guide_runtime):
    spec = configured()
    files = generate_files(spec)
    assert Blueprint.model_validate_json(files["blueprint.json"]) == spec
    runtime = json.loads(files["deployment.json"])
    selected = spec.agent_classes[1].tools.model_dump(mode="json")
    assert next(item for item in runtime["classes"] if item["id"] == "research")["tools"] == selected
    assert all(item["tools"] == selected for item in runtime["agents"] if item["class_id"] == "research")
    native = tomllib.loads(files["config/mob.toml"])
    rehearsal = tomllib.loads(rehearsal_mob_toml(spec))
    assert rehearsal["profiles"].keys() == native["profiles"].keys()
    for role, profile in native["profiles"].items():
        assert rehearsal["profiles"][role]["tools"].get("read_only", False) == profile["tools"].get("read_only", False)
        assert rehearsal["profiles"][role]["tools"]["comms"] == profile["tools"]["comms"]
        assert rehearsal["profiles"][role]["tools"]["memory"] == profile["tools"]["memory"]
        # Rehearsal intentionally masks execution grants; the native policy
        # survives that mask rather than granting them back.
        assert all(rehearsal["profiles"][role]["tools"][key] is False
                   for key in ("builtins", "shell", "workgraph", "schedule"))


def test_profile_only_class_keeps_policy_without_creating_an_identity():
    data = configured().model_dump(mode="json")
    data["agent_classes"][1].update(instances=0, members=[])
    data["flows"] = []
    data["topology"]["edges"] = []
    spec = Blueprint.model_validate(data)
    files = generate_files(spec)
    runtime = json.loads(files["deployment.json"])
    native = tomllib.loads(files["config/mob.toml"])
    assert not any(item["class_id"] == "research" for item in runtime["agents"])
    assert native["profiles"][runtime["applicationProfiles"]["research"]]["tools"]["read_only"] is True
    assert "research__1" not in native["profiles"]


def test_restricted_mutable_grants_are_visible_advice_not_silently_removed():
    spec = configured()
    before = spec.model_dump(mode="json")
    findings = [item for item in design_findings(spec) if item["code"] == "read_only_capability"]
    assert len(findings) == 1
    assert spec.agent_classes[1].name in findings[0]["detail"]
    assert spec.model_dump(mode="json") == before
    assert generate_files(spec)["config/mob.toml"]
    assert not [item for item in design_findings(starter_blueprint()) if item["code"] == "read_only_capability"]


def test_legacy_frozen_artifact_binding_survives_false_policy_normalization():
    from launchkit.artifact_binding import frozen_matches, package_matches, proposal_hash
    from launchkit.projects import fingerprint
    legacy = starter_blueprint().model_dump(mode="json")
    assert all("read_only" not in item["tools"] for item in legacy["agent_classes"])
    brief = {"mission": "Compare supplied evidence"}
    frozen = {"guide_id": "legacy", "guide_version": 3, "blueprint": legacy, "brief": brief}
    receipt = {"guide_id": "legacy", "guide_version": 3,
               "blueprint_hash": fingerprint(legacy), "brief_hash": fingerprint(brief)}
    explicit_false = json.loads(json.dumps(legacy))
    for item in explicit_false["agent_classes"]:
        item["tools"]["read_only"] = False
    restored = Blueprint.model_validate(explicit_false).model_dump(mode="json")
    assert restored == legacy
    assert proposal_hash(explicit_false) == receipt["blueprint_hash"]
    state = {"id": "legacy", "version": 4, "proposal": restored, "brief": brief}
    assert frozen_matches(receipt, frozen, state)
    assert package_matches(receipt, state)
    restored["agent_classes"][0]["tools"]["read_only"] = True
    assert not frozen_matches(receipt, frozen, state)
    assert not package_matches(receipt, state)
