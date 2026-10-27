"""Inert forwarding controls for existing optional native owner selections.

Opaque native values exercise byte custody only, never grants or qualification.
"""
from copy import deepcopy
import json

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.connect_application_policy import completed_application_policy_inputs
from test_application_preparation_plan import Fixture
from test_application_preparation_custody import PreparationFixture
from test_connect_completed_application_setup import write
from test_meta_toolkit_runtime_selection import no_product_execution


def standing(installation="checked_software", access="provision_task_parent"):
    return {"installation": installation, "access": access}


def administration(*routes):
    return {"kind": "application_policy_tasks", "routes": list(routes)}


def complete_fixture(path, *, selections=True):
    f = PreparationFixture(path)
    f.plan_value["acquisition"] = {
        "backend_generation": "retained-workgraph-generation",
        "proposal_ceiling": {"actions": [], "services": [], "frontends": [],
            "output_audiences": ["retained-audience"]},
        "output_audience": ["retained-audience"],
        "limits": {"setup_timeout_ms": 30000, "max_workers": 4,
            "max_effect_attempts": 2, "admission_timeout_ms": 10000}}
    if selections:
        f.plan_value["application"]["helper_tools"] = "checked_installation"
        f.setup_value["helper_tools"] = "checked_installation"
        f.plan_value["acquisition"]["standing_policy"] = standing()
        # Rust's selected route set serializes in sorted order. The pinned
        # source can have another order; neither side adds a selected route.
        f.plan_value["owner"]["native_host"]["service_parent_administration"] = administration(
            "untouched-route", "selected-route")
        f.setup_value["native_owner"]["native_host"]["service_parent_administration"] = administration(
            "selected-route", "untouched-route")
    f.repin_plan(); f.publish()
    f.source_policy = write(f.root / "policy.json", b'{"opaque_source_policy":"retained"}\n')
    return f


def final_inputs(f):
    bundle = f.caller.retain_completed_application_preparation(f.manifest,
        preparation_plan=f.plan, source_files=(f.source, f.source_policy))
    return bundle, completed_application_policy_inputs(bundle, f.source_policy)


@pytest.mark.parametrize("installation", ["human_review", "checked_software"])
@pytest.mark.parametrize("access", ["human_review", "configured_parent", "provision_task_parent"])
def test_exact_existing_choices_survive_trusted_plan_without_grant_or_default_changes(tmp_path, installation, access):
    f = Fixture(tmp_path)
    f.plan["application"]["helper_tools"] = "checked_installation"
    f.plan["owner"]["native_host"]["service_parent_administration"] = administration("caller-console")
    f.plan["acquisition"]["standing_policy"] = standing(installation, access)
    original = deepcopy(f.plan)
    result = f.assemble()
    assert result["owner"] == original["owner"]
    assert result["acquisition"]["standing_policy"] == original["acquisition"]["standing_policy"]
    assert result["authorities"] == original["authorities"]
    assert f.source.read_bytes() == f.raw
    assert not (tmp_path / "fresh-output").exists()


def test_selected_owner_scope_and_standing_policy_reach_protected_finalizer_exactly(tmp_path):
    f = complete_fixture(tmp_path)
    before = {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    bundle, staged = final_inputs(f)
    acquisition = json.loads(staged["acquisition"].as_bytes())
    policy = json.loads(staged["local_policy_input"])
    assert acquisition["standing_policy"] == f.plan_value["acquisition"]["standing_policy"]
    assert policy["native_host"]["service_parent_administration"] == f.setup_value["native_owner"]["native_host"]["service_parent_administration"]
    assert acquisition == {"schema": 1, **f.plan_value["acquisition"],
        "prepared_application_setup": {"name": bundle.setup.path.name, "sha256": bundle.setup.sha256}}
    assert before == {p: p.read_bytes() for p in f.root.rglob("*") if p.is_file()}
    assert not f.calls and not f.caller.provision_attempt_path.exists()


def test_absent_choices_remain_absent_in_plan_custody_and_finalized_policy(tmp_path):
    draft = tmp_path / "draft"; draft.mkdir()
    f = Fixture(draft)
    result = f.assemble()
    assert "service_parent_administration" not in result["owner"]["native_host"]
    assert "standing_policy" not in result["acquisition"]
    complete = tmp_path / "complete"; complete.mkdir()
    g = complete_fixture(complete, selections=False)
    _, staged = final_inputs(g)
    assert "standing_policy" not in json.loads(staged["acquisition"].as_bytes())
    assert "service_parent_administration" not in json.loads(staged["local_policy_input"])["native_host"]
    assert not g.calls


def test_native_optional_null_is_no_selection_and_is_not_replaced_with_a_default(tmp_path):
    f = Fixture(tmp_path)
    f.plan["owner"]["native_host"]["service_parent_administration"] = None
    f.plan["acquisition"]["standing_policy"] = None
    result = f.assemble()
    assert result["owner"]["native_host"]["service_parent_administration"] is None
    assert result["acquisition"]["standing_policy"] is None
    assert result["application"]["helper_tools"] == "preparation"


@pytest.mark.parametrize("value", [True, "human_review", {},
    {"installation": "checked_software", "access": "automatic_admin"},
    {"installation": "agent_allow", "access": "human_review"},
    {"installation": True, "access": "human_review"},
    {"installation": "human_review", "access": "human_review", "actor": "untrusted"}])
def test_malformed_optional_standing_policy_refuses_plan_and_finalizer(tmp_path, value):
    draft = tmp_path / "draft"; draft.mkdir()
    f = Fixture(draft)
    f.plan["application"]["helper_tools"] = "checked_installation"
    f.plan["acquisition"]["standing_policy"] = value
    with pytest.raises(CustodyError, match="Standing acquisition policy"):
        f.assemble()
    complete = tmp_path / "complete"; complete.mkdir()
    g = complete_fixture(complete)
    g.plan_value["acquisition"]["standing_policy"] = value
    g.repin_plan(); g.publish()
    with pytest.raises(CustodyError, match="Standing acquisition policy"):
        final_inputs(g)
    assert not g.calls


@pytest.mark.parametrize("defect", ["kind", "empty", "foreign-route", "id", "routes-type", "extra", "operator"])
def test_malformed_admin_scope_refuses_without_selecting_an_operator_or_route(tmp_path, defect):
    f = Fixture(tmp_path)
    selected = administration("caller-console")
    if defect == "kind": selected["kind"] = "automatic_admin"
    elif defect == "empty": selected["routes"] = []
    elif defect == "foreign-route": selected["routes"] = ["unknown-route"]
    elif defect == "id": selected["routes"] = [True]
    elif defect == "routes-type": selected["routes"] = "caller-console"
    elif defect == "extra": selected["actor"] = "untrusted"
    else: f.plan["owner"]["native_host"]["remote_operator"] = None
    f.plan["owner"]["native_host"]["service_parent_administration"] = selected
    with pytest.raises(CustodyError, match="Service-parent administration"):
        f.assemble()


@pytest.mark.parametrize("mode", ["preparation", "inert_acquisition"])
def test_standing_policy_requires_explicit_existing_checked_surface(tmp_path, mode):
    f = Fixture(tmp_path)
    f.plan["application"]["helper_tools"] = mode
    f.plan["acquisition"]["standing_policy"] = standing()
    with pytest.raises(CustodyError, match="selected checked setup surface"):
        f.assemble()


@pytest.mark.parametrize("defect", ["widen", "drop", "unselected", "malformed"])
def test_completed_owner_cannot_change_selected_optional_scope_even_with_repinned_output(tmp_path, defect):
    f = complete_fixture(tmp_path, selections=defect != "unselected")
    after = f.setup_value["native_owner"]["native_host"]
    if defect == "widen": after["service_parent_administration"]["routes"].append("helper-route")
    elif defect == "drop": del after["service_parent_administration"]
    elif defect == "unselected": after["service_parent_administration"] = administration("selected-route")
    else: after["service_parent_administration"] = administration("unknown-route")
    f.publish()
    with pytest.raises(CustodyError, match="service-parent|Service-parent"):
        final_inputs(f)
    assert not f.calls
