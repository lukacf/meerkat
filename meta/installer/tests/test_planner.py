import hashlib
import json

import pytest
from pydantic import ValidationError

from launchkit import bootstrap, guide_sdk, planner
from launchkit.planner import parse_design, planner_capability, starter_design
from launchkit.schema import DesignRequest, DeploymentSpec


def request(**overrides):
    return DesignRequest(description="Help organize household notes and make practical project plans.", **overrides)


@pytest.mark.parametrize("template", ["personal", "operations", "research", "custom"])
def test_starters_are_valid_for_all_templates(template):
    design = starter_design(request(template=template))
    assert design.source == "template"
    assert len(design.agents) == 3
    DeploymentSpec(project={"name": "My team", "slug": "my-team", "description": request().description}, agents=design.agents)


def test_no_configuration_is_truthfully_reported(monkeypatch):
    for key in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"]:
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    capability = planner_capability()
    assert capability["available"] is False
    assert capability["providers"] == []
    assert "provider" in capability["reason"]


def test_model_output_cannot_smuggle_deployment_fields():
    data = starter_design(request()).model_dump()
    data.pop("source")
    data.pop("warnings")
    assert parse_design(json.dumps(data)).source == "agent"
    assert parse_design("```json\n" + json.dumps(data) + "\n```").source == "agent"
    data["target"] = "gcp"
    with pytest.raises(ValueError):
        parse_design(json.dumps(data))


def test_model_output_must_have_unique_safe_ids():
    data = starter_design(request()).model_dump()
    data = {"agents": data["agents"], "summary": data["summary"]}
    data["agents"][1]["id"] = data["agents"][0]["id"]
    with pytest.raises(ValidationError):
        parse_design(json.dumps(data))
    data["agents"][1]["id"] = "$(touch /tmp/pwned)"
    with pytest.raises(ValidationError):
        parse_design(json.dumps(data))


def test_package_selection_does_not_fall_back_to_retained_legacy_cache(tmp_path, monkeypatch):
    from launchkit.connect_installer_caller import ExactFile

    package = tmp_path / "same-checkout"
    legacy = package / ".tools/rpc_gateway"
    legacy.parent.mkdir(parents=True)
    legacy.write_bytes(b"Retained legacy runtime, never executed.")
    legacy.chmod(0o700)
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", None)
    monkeypatch.setattr(planner, "__file__", str(package / "launchkit/planner.py"))
    monkeypatch.setattr(planner.shutil, "which", lambda name: None)
    monkeypatch.delenv("MOBKIT_GATEWAY_BIN", raising=False)
    assert planner.gateway_path() is None
    current = bootstrap.package_tool_path(package, "rpc_gateway")
    current.parent.mkdir(parents=True)
    current.write_bytes(b"Current runtime, never executed.")
    current.chmod(0o700)
    assert planner.gateway_path() is None, "A cached executable is not a selected Meta runtime."
    selected = bootstrap.SelectedGuideRuntime(
        ExactFile(current, hashlib.sha256(current.read_bytes()).hexdigest()),
        guide_sdk.SELECTED_VERSION, guide_sdk.SELECTED_VERSION, "a" * 40, "b" * 40)
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", selected)
    assert planner.gateway_path() == current
    assert legacy.read_bytes() == b"Retained legacy runtime, never executed."
