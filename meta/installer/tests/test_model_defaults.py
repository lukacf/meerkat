"""Offline target-model policy and emitted configuration, without native startup."""
from copy import deepcopy
import json
import tomllib

import pytest
from pydantic import ValidationError

from launchkit.architect import ArchitectService, SETUP_MODELS
from launchkit.blueprint import AgentClass, Blueprint, from_v1, starter_blueprint
from launchkit.compiler import generate_files, mob_toml
from launchkit.elephant_setup import generate_elephant_files
from launchkit.generator import generate_files as generate_v1_files
from launchkit.planner import DEFAULT_MODELS as PLANNER_MODELS
from launchkit.rehearsal_worker import rehearsal_mob_toml
from launchkit.schema import DEFAULT_MODELS, DeploymentSpec, DesignRequest
from test_meta_guide_runtime import selected as selected_guide_runtime


def project():
    return {"name": "Planning team", "slug": "planning-team",
            "description": "Interpret supplied evidence and present a coherent planning answer."}


def role(**changes):
    return {"id": "owner", "name": "Conversation owner", "description": "Own the user conversation.",
            "instructions": "Consult relevant expertise and present an evidence-linked answer.", **changes}


def blueprint(**changes):
    return {"project": project(), "agent_classes": [role()],
            "topology": {"entrypoint": "owner:main"}, **changes}


def deployment(**changes):
    return {"project": project(), "agents": [{"id": "owner", "name": "Conversation owner",
            "role": "Own the user conversation", "instructions": "Present an evidence-linked answer."}], **changes}


def test_openai_default_is_shared_by_all_target_contracts_and_starter():
    assert PLANNER_MODELS is DEFAULT_MODELS
    assert DeploymentSpec.model_validate(deployment()).model == "gpt-6.1-sol"
    assert DesignRequest(description=project()["description"]).model == "gpt-6.1-sol"
    assert AgentClass.model_validate(role()).model == "gpt-6.1-sol"
    initial = Blueprint.model_validate(blueprint())
    assert initial.model == initial.agent_classes[0].model == "gpt-6.1-sol"
    starter = starter_blueprint()
    assert starter.model == "gpt-6.1-sol"
    assert {item.model for item in starter.agent_classes} == {"gpt-6.1-sol"}
    assert Blueprint.model_json_schema()["properties"]["model"]["default"] == "gpt-6.1-sol"


@pytest.mark.parametrize("provider,model", [
    ("openai", "gpt-6.1-sol"), ("anthropic", "claude-sonnet-4-6"),
    ("gemini", "gemini-3.1-pro-preview")])
def test_omitted_model_uses_selected_provider_without_changing_other_defaults(provider, model):
    assert DeploymentSpec.model_validate(deployment(provider=provider)).model == model
    assert DesignRequest(description=project()["description"], provider=provider).model == model
    assert AgentClass.model_validate(role(provider=provider)).model == model
    spec = Blueprint.model_validate(blueprint(provider=provider))
    assert spec.provider == spec.agent_classes[0].provider == provider
    assert spec.model == spec.agent_classes[0].model == model


@pytest.mark.parametrize("class_provider", [None, "openai"])
def test_omitted_class_model_inherits_explicit_blueprint_choice(class_provider):
    item = role() if class_provider is None else role(provider=class_provider)
    source = blueprint(model="gpt-6-astra", agent_classes=[item])
    untouched = deepcopy(source)
    spec = Blueprint.model_validate(source)
    assert spec.agent_classes[0].model == spec.model == "gpt-6-astra"
    assert source == untouched


def test_unset_class_model_in_python_constructor_inherits_but_explicit_choice_does_not():
    omitted = AgentClass.model_validate(role())
    omitted.tools.memory = False
    omitted.tools.notes = "write"
    explicit = AgentClass.model_validate(role(id="explicit", model="gpt-6.1-sol"))
    spec = Blueprint.model_validate(blueprint(model="gpt-6-astra", agent_classes=[omitted, explicit]))
    assert [item.model for item in spec.agent_classes] == ["gpt-6-astra", "gpt-6.1-sol"]
    assert spec.agent_classes[0].tools == omitted.tools
    changed_provider = Blueprint.model_validate(blueprint(provider="anthropic", model="claude-opus-4-6",
                                                          agent_classes=[omitted, explicit]))
    assert [(item.provider, item.model) for item in changed_provider.agent_classes] == [
        ("anthropic", "claude-opus-4-6"), ("openai", "gpt-6.1-sol")]
    assert changed_provider.agent_classes[0].tools == omitted.tools
    assert omitted.model == "gpt-6.1-sol"  # The caller-owned object is not rewritten.


def test_distinct_explicit_class_provider_uses_its_own_default():
    spec = Blueprint.model_validate(blueprint(provider="anthropic", model="claude-opus-4-6",
                                            agent_classes=[role(provider="openai")]))
    assert spec.model == "claude-opus-4-6"
    assert spec.agent_classes[0].model == "gpt-6.1-sol"


@pytest.mark.parametrize("provider,model", [
    ("openai", "gpt-6-astra"), ("anthropic", "claude-opus-4-6"),
    ("gemini", "gemini-custom-user-model")])
def test_explicit_saved_choices_survive_roundtrip_import_and_native_config(provider, model):
    source = blueprint(provider=provider, model=model, agent_classes=[role(provider=provider, model=model)])
    spec = Blueprint.model_validate(source)
    saved = spec.model_dump(mode="json")
    assert Blueprint.model_validate(saved).model_dump(mode="json") == saved
    native = tomllib.loads(mob_toml(spec))
    assert {(row["provider"], row["model"]) for row in native["profiles"].values()} == {(provider, model)}
    old = deployment(provider=provider, model=model)
    imported = from_v1(old)
    assert imported.model == imported.agent_classes[0].model == model
    assert DesignRequest(description=project()["description"], provider=provider, model=model).model == model
    assert all(row["model"] == model for row in tomllib.loads(generate_v1_files(old)["config/mob.toml"])["profiles"].values())


@pytest.mark.parametrize("model", [None, "", "claude-sonnet-4-6"])
def test_explicit_invalid_openai_model_is_rejected_not_replaced(model):
    with pytest.raises(ValidationError):
        Blueprint.model_validate(blueprint(model=model))
    with pytest.raises(ValidationError):
        AgentClass.model_validate(role(model=model))


def test_new_default_package_and_trial_use_builtin_model_without_custom_collision(selected_guide_runtime):
    spec = Blueprint.model_validate(blueprint())
    files = generate_files(spec)
    native = tomllib.loads(files["config/mob.toml"])
    trial = tomllib.loads(rehearsal_mob_toml(spec))
    assert "models" not in native and "models" not in trial
    assert all(row["model"] == "gpt-6.1-sol" for row in native["profiles"].values())
    manifest = json.loads(files["deployment.json"])
    assert {row["model"] for row in manifest["classes"]} == {"gpt-6.1-sol"}
    legacy = tomllib.loads(generate_v1_files(deployment())["config/mob.toml"])
    assert "models" not in legacy
    # A native model selection is not proof of destination account access.


@pytest.mark.parametrize("provider,model", [
    ("openai", "gpt-6.1-sol"), ("anthropic", "claude-sonnet-4-6"),
    ("gemini", "gemini-3.1-pro-preview")])
def test_omitted_knowledge_extraction_model_uses_provider_default(provider, model):
    files = generate_elephant_files({"mode": "local"}, "planning-team", provider=provider)
    assert json.loads(files["knowledge/settings.json"])["model"] == model
    compose = json.loads(files["knowledge/compose.json"])
    assert compose["services"]["elephant"]["environment"]["PIPELINE_AGENTIC_MODEL"] == model


def test_explicit_knowledge_extraction_model_remains_exact():
    files = generate_elephant_files({"mode": "local"}, "planning-team", model="gpt-6-astra")
    assert json.loads(files["knowledge/settings.json"])["model"] == "gpt-6-astra"


def test_target_defaults_do_not_replace_setup_configuration_or_active_model(tmp_path, monkeypatch):
    monkeypatch.setattr("launchkit.architect.planner_capability", lambda: {
        "available": False, "provider": "openai", "reason": "Offline model-policy fixture"})
    service = ArchitectService(tmp_path / "unused-runtime")
    assert SETUP_MODELS["openai"] == "gpt-6.1-sol"
    monkeypatch.setenv("MOBKIT_ARCHITECT_MODEL", "gpt-6-astra")
    assert service.status()["model"] == "gpt-6-astra"
    service.active_model = "gpt-6.1-sol"
    service.set_context(Blueprint.model_validate(blueprint(model="gpt-6-astra")).model_dump(mode="json"))
    assert service.status()["model"] == "gpt-6.1-sol"
    assert service.runtime is None and not service.root.exists()
