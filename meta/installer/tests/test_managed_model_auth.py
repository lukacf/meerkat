"""Managed-host model intent in ordinary source compilation, without runtime.

These controls exercise strict schema and generated package fields only. They
neither resolve a native binding nor inspect stored credentials or prove model
access. Trusted startup configuration remains the original host owner's door.
"""
from __future__ import annotations

from copy import deepcopy
import hashlib
import json
import tomllib

import pytest
from pydantic import ValidationError

from launchkit.blueprint import AgentClass, Blueprint, starter_blueprint
from launchkit.compiler import environment_fields, generate_files, mob_toml

MODEL = "claude-sonnet-5-5"
PROVIDER_KEYS = {"ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"}


def source(auth="managed_host"):
    data = starter_blueprint().model_dump(mode="json")
    data.update(provider="anthropic", model=MODEL)
    for item in data["agent_classes"]:
        item.update(provider="anthropic", model=MODEL, model_auth=auth)
    return data


def fields(spec):
    return {item["key"]: item for item in environment_fields(spec)}


def test_default_model_auth_preserves_observed_ordinary_package_bytes():
    # Captured before the scoped schema/compiler edit. Only spec-derived files
    # are pinned, so concurrent trusted app/startup changes are not mirrored.
    expected = {
        "blueprint.json": "c748254af4d01d679a1a3ff033899770075807cce654cb58c6eef135ff31ed3b",
        "deployment.json": "a6675ab13c0b2e6ca3cfd0563a609c0b480b4a89bb65657f9f8c4e3cb3d51005",
        "config/mob.toml": "e186782095f2584c4f19237408efc28a360e02de11ebca2bfd07f6f733e01a4d",
        ".env.example": "1867682e51b6ee83bd7a37499d015837293d19644d825be8861865255e4da356",
    }
    spec = starter_blueprint()
    assert all(item.model_auth == "api_key" for item in spec.agent_classes)
    assert all("model_auth" not in item for item in spec.model_dump()["agent_classes"])
    generated = generate_files(spec)
    assert {name: hashlib.sha256(generated[name].encode()).hexdigest() for name in expected} == expected


@pytest.mark.parametrize("invalid", [True, None, "global:default_anthropic", {}])
def test_class_auth_intent_refuses_untyped_mode_or_binding_reference(invalid):
    data = source()
    data["agent_classes"][0]["model_auth"] = invalid
    with pytest.raises(ValidationError):
        Blueprint.model_validate(data)


def test_managed_host_intent_roundtrips_only_as_nonsecret_class_selection():
    spec = Blueprint.model_validate(source())
    restored = Blueprint.model_validate_json(spec.model_dump_json())
    assert restored.model_dump() == spec.model_dump()
    assert all(item.model_auth == "managed_host" for item in restored.agent_classes)
    assert all(item["model_auth"] == "managed_host" for item in restored.model_dump()["agent_classes"])
    ordinary = source("api_key")
    assert all("model_auth" not in item for item in Blueprint.model_validate(ordinary).model_dump()["agent_classes"])
    first = deepcopy(spec.model_dump()["agent_classes"][0])
    first["auth_binding"] = {"realm": "global", "binding": "default_anthropic"}
    with pytest.raises(ValidationError):
        AgentClass.model_validate(first)


def test_managed_only_package_omits_provider_key_interview_and_keeps_startup_intent():
    spec = Blueprint.model_validate(source())
    generated = generate_files(spec)
    deployment = json.loads(generated["deployment.json"])
    assert not PROVIDER_KEYS & {item["key"] for item in deployment["environment"]}
    assert all(item["model_auth"] == "managed_host" for item in deployment["classes"])
    assert all(item["provider"] == "anthropic" and item["model"] == MODEL for item in deployment["classes"])
    assert all(key + "=" not in generated[".env.example"] for key in PROVIDER_KEYS)
    assert "global:default_anthropic" not in generated["config/mob.toml"]
    assert "auth_binding" not in generated["config/mob.toml"]


@pytest.mark.parametrize("api_provider,api_model,required_key", [
    ("anthropic", MODEL, "ANTHROPIC_API_KEY"),
    ("openai", "gpt-6.1-sol", "OPENAI_API_KEY"),
])
def test_mixed_auth_classes_keep_only_the_actual_api_key_class_requirement(api_provider, api_model, required_key):
    data = source()
    data["agent_classes"][0].update(model_auth="api_key", provider=api_provider, model=api_model)
    spec = Blueprint.model_validate(data)
    selected = fields(spec)
    assert PROVIDER_KEYS & selected.keys() == {required_key}
    assert selected[required_key]["required"] is True and selected[required_key]["secret"] is True
    assert "default" not in selected[required_key]
    runtime = json.loads(generate_files(spec)["deployment.json"])
    assert "model_auth" not in runtime["classes"][0]
    assert all(item["model_auth"] == "managed_host" for item in runtime["classes"][1:])


@pytest.mark.parametrize("provider,model,required", [
    ("anthropic", MODEL, {"ANTHROPIC_API_KEY", "OPENAI_API_KEY"}),
    ("openai", "gpt-6.1-sol", {"OPENAI_API_KEY"}),
    ("gemini", "gemini-3.1-pro-preview", {"GEMINI_API_KEY", "OPENAI_API_KEY"}),
])
def test_managed_agent_intent_does_not_bypass_local_knowledge_credentials(provider, model, required):
    data = source()
    data.update(provider=provider, model=model)
    data["knowledge"]["mode"] = "local"
    spec = Blueprint.model_validate(data)
    selected = fields(spec)
    assert PROVIDER_KEYS & selected.keys() == required
    assert all(selected[key]["required"] is True and selected[key]["secret"] is True
               and "default" not in selected[key] for key in required)


def test_managed_auth_keeps_connector_service_credentials_private_and_required():
    data = source()
    data["connections"] = [{"id": "service", "name": "Synthetic service", "transport": "stdio",
        "command": "synthetic-service-mcp", "env": {"PROVIDER_KEY": "${ANTHROPIC_API_KEY}",
        "SERVICE_TOKEN": "${SERVICE_TOKEN}"}, "required_environment": ["ANTHROPIC_API_KEY", "SERVICE_TOKEN"],
        "setup_inputs": [{"name": "ANTHROPIC_API_KEY", "secret": False, "default": "SYNTHETIC_PUBLIC_DEFAULT_CANARY"}]}]
    data["agent_classes"][0]["tools"]["mcp"] = [{"connection_id": "service", "tool_names": ["read_case"]}]
    spec = Blueprint.model_validate(data)
    generated = generate_files(spec)
    selected = {item["key"]: item for item in json.loads(generated["deployment.json"])["environment"]}
    assert PROVIDER_KEYS & selected.keys() == {"ANTHROPIC_API_KEY"}
    for key in ("ANTHROPIC_API_KEY", "SERVICE_TOKEN"):
        assert selected[key]["required"] is True and selected[key]["secret"] is True
        assert "default" not in selected[key] and key + "=\n" in generated[".env.example"]
    assert "SYNTHETIC_PUBLIC_DEFAULT_CANARY" not in generated[".env.example"]


def test_dedicated_discovery_source_keeps_original_model_provider_and_grants():
    data = source("api_key")
    data["acquisition"] = {"helper_identity": "research:1", "helper_profile": "dedicated", "helper_tools": "checked_installation"}
    data["agent_classes"][0]["tools"]["rust_bundles"] = ["meerkat_toolkit.setup"]
    data["agent_classes"][1]["tools"].update(builtins=True, shell=True, workgraph=True, schedule=True)
    ordinary = Blueprint.model_validate(data)
    managed_data = deepcopy(data)
    for item in managed_data["agent_classes"]:
        item["model_auth"] = "managed_host"
    managed = Blueprint.model_validate(managed_data)
    assert managed.acquisition.helper_profile == "dedicated"
    assert managed.acquisition.helper_tools == "checked_installation"
    assert mob_toml(managed) == mob_toml(ordinary)
    profiles = tomllib.loads(mob_toml(managed))["profiles"]
    assert all(item["provider"] == "anthropic" and item["model"] == MODEL for item in profiles.values())
    assert all("model_auth" not in item and "auth_binding" not in item for item in profiles.values())
    assert profiles["research__1"]["tools"]["builtins"] is True
    assert profiles["research__1"]["tools"]["shell"] is True
