"""Explicit native bundle intent, without registering or granting its tools."""
from copy import deepcopy
import tomllib

import pytest
from pydantic import ValidationError

from launchkit.blueprint import Blueprint, ToolGrants, starter_blueprint
from launchkit.compiler import application_profiles, generate_package, mob_toml, package_availability, package_preview
from launchkit.rehearsal import masked_capabilities
from launchkit.rehearsal_worker import rehearsal_mob_toml
from test_meta_guide_runtime import selected as selected_guide_runtime


def test_explicit_bundle_survives_selected_profiles_but_not_ordinary_install_or_trial(selected_guide_runtime):
    original = starter_blueprint().model_dump(mode="json")
    data = deepcopy(original)
    selected = data["agent_classes"][1]
    bundles = ["meerkat_toolkit.setup", "application.custom-bundle"]
    selected["tools"]["rust_bundles"] = bundles
    spec = Blueprint.model_validate(data)
    assert spec.model_dump(mode="json") == data
    assert data["agent_classes"][1]["tools"]["rust_bundles"] == bundles
    native = tomllib.loads(mob_toml(spec))
    selected_profiles = {"research__1", "research__2", application_profiles(spec)["research"]}
    for name, profile in native["profiles"].items():
        if name in selected_profiles:
            assert profile["tools"]["rust_bundles"] == bundles
        else:
            assert "rust_bundles" not in profile["tools"]

    # A declarative name cannot make the ordinary host register a dispatcher.
    readiness = package_availability(spec)
    assert readiness["available"] is False and readiness["status"] == "pending-integration"
    preview = package_preview(spec)
    assert preview["valid"] is True and preview["files"] == []
    assert preview["package_availability"] == readiness
    with pytest.raises(ValueError) as refused:
        generate_package(spec)
    assert str(refused.value) == readiness["reason"]

    trial = tomllib.loads(rehearsal_mob_toml(spec))
    assert trial["skills"] == native["skills"] and trial["flows"] == native["flows"]
    assert all(not profile["tools"].get("rust_bundles") for profile in trial["profiles"].values())
    for row in masked_capabilities(spec):
        expected = ["rust_bundle:" + name for name in bundles] if row["class_id"] == "research" else []
        assert [name for name in row["masked"] if name.startswith("rust_bundle:")] == expected

    # Clearing the explicit selection restores the original source and readiness.
    selected["tools"]["rust_bundles"] = []
    cleared = Blueprint.model_validate(data)
    assert cleared.model_dump(mode="json") == original
    assert mob_toml(cleared) == mob_toml(original)
    assert package_availability(cleared)["available"] is True
    assert "rust_bundles" not in ToolGrants().model_dump(mode="json")


@pytest.mark.parametrize("bundles", [[""], ["meerkat_toolkit.setup", "meerkat_toolkit.setup"], [1]])
def test_bundle_names_are_explicit_nonempty_unique_strings(bundles):
    with pytest.raises(ValidationError):
        ToolGrants(rust_bundles=bundles)
