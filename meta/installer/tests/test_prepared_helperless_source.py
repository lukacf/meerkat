"""Driver configuration joins using inert outputs, without native execution.

These cases cover the helperless source/owner join only. Completed file custody,
native class validation, held completion and runtime adoption are separate checks.
"""
from copy import deepcopy
import json

import pytest

from launchkit.connect_installer_caller import CustodyError, _prepared_plan_source
from test_application_preparation_plan import helperless_fixture


def completed_specimen(root, *, empty=False):
    plan = helperless_fixture(root, empty=empty).plan
    source = plan["application"]
    setup = {"schema": 4, "native_owner": deepcopy(plan["owner"]),
        "input_sha256": "a" * 64, "realm": deepcopy(source["realm"]),
        "definition_source": deepcopy(source["definition"]),
        "native_members": [{**deepcopy(row), "initial_surface": {
            "name": f"member-surface-{index:064x}.json", "sha256": "b" * 64}}
            for index, row in enumerate(source["members"])],
        "artifact_directory": plan["output"]}
    return plan, setup


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_join_preserves_actual_inventory_and_classes_without_rewriting(tmp_path, empty):
    plan, setup = completed_specimen(tmp_path, empty=empty)
    before = json.dumps([plan, setup])
    assert _prepared_plan_source(plan, setup) is plan["application"]
    assert json.dumps([plan, setup]) == before
    assert len(setup["native_members"]) == (0 if empty else 2)
    assert not list(tmp_path.iterdir())


def test_fixed_helperless_inventory_does_not_require_a_class_declaration(tmp_path):
    plan, setup = completed_specimen(tmp_path)
    del plan["application"]["member_classes"]
    del plan["owner"]["native_host"]["member_classes"]
    del setup["native_owner"]["native_host"]["member_classes"]
    assert _prepared_plan_source(plan, setup) is plan["application"]


@pytest.mark.parametrize("side", ["selected", "completed"])
def test_helperless_join_refuses_classes_missing_from_either_native_owner(tmp_path, side):
    plan, setup = completed_specimen(tmp_path)
    owner = plan["owner"] if side == "selected" else setup["native_owner"]
    del owner["native_host"]["member_classes"]
    with pytest.raises(CustodyError, match="selected member classes"):
        _prepared_plan_source(plan, setup)


def test_helperless_original_controller_only_changes_its_selected_parent(tmp_path):
    plan, setup = completed_specimen(tmp_path)
    selected = plan["owner"]["native_host"]["routes"][0]
    completed = setup["native_owner"]["native_host"]["routes"][0]
    selected["controller_parent"] = None
    completed["controller_parent"] = [{"opaque_native_parent": "completed-controller"}]
    plan["authorities"] = {"controllers": [{"target": {
        "kind": "original_route", "route": selected["id"]}, "issuance_id": "selected-controller"}]}
    assert _prepared_plan_source(plan, setup) is plan["application"]
    completed["origin"]["requester"] = {"opaque_native_principal": "different-operator"}
    with pytest.raises(CustodyError, match="original route or source fact"):
        _prepared_plan_source(plan, setup)


@pytest.mark.parametrize("field", ["helper", "helper_tools", "helper_profile", "source_helper_member",
    "source_helper_roster", "server", "resources", "helper_source_surface"])
def test_helperless_completion_refuses_helper_output_fields_even_when_null(tmp_path, field):
    plan, setup = completed_specimen(tmp_path)
    setup[field] = None
    with pytest.raises(CustodyError, match="helper output fields"):
        _prepared_plan_source(plan, setup)


@pytest.mark.parametrize("field", ["agent_setup", "helper_report", "helper_mcp", "helper_catalog"])
def test_helperless_completion_refuses_helper_authority_fields_even_when_empty(tmp_path, field):
    plan, setup = completed_specimen(tmp_path)
    plan["authorities"][field] = [] if field == "agent_setup" else None
    with pytest.raises(CustodyError, match="authority request custody"):
        _prepared_plan_source(plan, setup)


@pytest.mark.parametrize("field", ["helper", "helper_tools", "helper_profile", "acquisition"])
def test_helperless_join_refuses_partial_or_null_helper_selection(tmp_path, field):
    plan, setup = completed_specimen(tmp_path)
    (plan if field == "acquisition" else plan["application"])[field] = None
    with pytest.raises(CustodyError, match="helper fields|acquisition"):
        _prepared_plan_source(plan, setup)


@pytest.mark.parametrize("change", ["owner", "member", "channel", "unrequested_parent",
    "helper_controller", "retained_setup", "new_setup"])
def test_helperless_join_refuses_owner_or_authority_changes(tmp_path, change):
    plan, setup = completed_specimen(tmp_path)
    native = setup["native_owner"]["native_host"]
    if change == "owner":
        native["remote_operator"] = {"opaque_native_principal": "different-operator"}
    elif change == "member":
        for row in (native["members"][0], setup["native_members"][0]):
            row["audience"] = {"opaque_native_audience": "different-audience"}
    elif change == "channel":
        setup["native_owner"]["channels"] = [{"member": deepcopy(native["members"][0]["member"])}]
    elif change == "unrequested_parent":
        native["routes"][0]["controller_parent"] = [{"opaque_native_parent": "unrequested"}]
    elif change == "helper_controller":
        plan["authorities"] = {"controllers": [{"target": {"kind": "helper"}, "issuance_id": "unrequested"}]}
    else:
        rows = [native["members"][0], setup["native_members"][0]]
        if change == "retained_setup":
            rows.extend([plan["application"]["members"][0], plan["owner"]["native_host"]["members"][0]])
        for row in rows:
            row["setup_capability"] = {"route": "caller-console", "parent": {"opaque_native_parent": "setup"}}
    with pytest.raises(CustodyError):
        _prepared_plan_source(plan, setup)


@pytest.mark.parametrize("change", ["missing_classes", "empty_classes", "invalid_classes", "roster", "policy"])
def test_empty_helperless_join_requires_classes_without_invented_initial_inventory(tmp_path, change):
    plan, setup = completed_specimen(tmp_path, empty=True)
    source = plan["application"]
    if change == "missing_classes":
        del source["member_classes"]
    elif change == "empty_classes":
        source["member_classes"] = []
    elif change == "invalid_classes":
        source["member_classes"] = [None]
    elif change == "roster":
        source["roster"] = [{"identity": "invented", "profile": "application__project"}]
    else:
        source["tool_policy"]["members"] = [{"member_identity": "invented", "grants": []}]
    with pytest.raises(CustodyError, match="no invented initial inventory|selected member classes"):
        _prepared_plan_source(plan, setup)
