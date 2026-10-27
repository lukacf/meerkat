"""A desired helper choice cannot silently become an unconfigured installation."""
import pytest
from copy import deepcopy

from launchkit.blueprint import AgentClass, Blueprint, ToolGrants, starter_blueprint
from launchkit.compiler import generate_package, package_preview


@pytest.mark.parametrize("mode", ["preparation", "inert_acquisition", "checked_installation"])
def test_selected_helper_requires_qualified_installation_support(mode):
    plan = starter_blueprint().model_dump(mode="json")
    plan["agent_classes"].append(AgentClass(
        id="connection_researcher", name="Connection researcher",
        description="Research and prepare requested tool connections.",
        instructions="Research requested connections and return findings to the lead. "
                     "Do not activate tools or claim a successful installation.",
        tools=ToolGrants(memory=False),
    ).model_dump(mode="json"))
    plan["acquisition"] = {
        "helper_identity": "connection_researcher:main",
        "helper_profile": "exclusive_application_role",
        "helper_tools": mode,
    }
    original = deepcopy(plan)
    selected = Blueprint.model_validate(plan)
    assert selected.acquisition.helper_tools == mode
    assert selected.model_dump(mode="json") == original
    assert plan == original
    preview = package_preview(selected)
    assert preview["valid"] is True
    assert preview["resolved"]["identities"][-1]["identity"] == "connection_researcher:main"
    assert preview["files"] == [] and preview["file_count"] == 0
    assert preview["checks"][0]["status"] == "pending-integration"
    with pytest.raises(ValueError, match="connection setup helper.*compatible selected runtime"):
        generate_package(selected)


def test_absent_selection_and_legacy_capability_do_not_infer_checked_installation():
    original = starter_blueprint().model_dump(mode="json")
    assert "acquisition" not in original
    assert Blueprint.model_validate(original).model_dump(mode="json") == original
    selected = deepcopy(original)
    selected["acquisition"] = {
        "helper_identity": selected["topology"]["entrypoint"],
        "helper_profile": "exclusive_application_role",
        "helper_tools": "preparation",
    }
    assert Blueprint.model_validate(selected).model_dump(mode="json") == selected
