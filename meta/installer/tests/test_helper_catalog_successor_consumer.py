"""Inert host-consumer checks, not native issuance or reset qualification."""
from copy import deepcopy
import json

import pytest

from launchkit.connect_installer_caller import CustodyError
from test_application_preparation_plan import Fixture, encoded
from test_application_preparation_custody import PreparationFixture
from test_connect_completed_application_setup import write


SCOPE = "current_helper_successors"


def request(issue="fresh-successor-catalog"):
    return {"issuance_id": issue, "scope": SCOPE}


def basis():
    return {"kind": "targeted_grant_lineages", "base": None, "targets": []}


def reference(issue):
    # Native owns canonical coordinates, bounds, issuance and currentness.
    return {"grant_id": issue, "root_authority": "opaque-operator",
        "opaque_native_coordinate": [False, 7]}


def completed(path, *, scope=SCOPE):
    f = PreparationFixture(path)
    f.plan_value["application"]["helper"]["operation_basis"] = basis()
    f.setup_value["helper"]["operation_basis"] = basis()
    f.plan_value["authorities"]["helper_catalog"] = request()
    if scope is None:
        f.plan_value["authorities"]["helper_catalog"].pop("scope")
    else:
        f.plan_value["authorities"]["helper_catalog"]["scope"] = scope
    parent = [reference("fresh-successor-catalog")]
    if scope == SCOPE:
        f.setup_value["helper"]["catalog_capability"] = {"parent": parent}
    else:
        f.setup_value["helper"]["operation_basis"]["base"] = parent
    f.repin_plan()
    f.publish()
    return f


def test_explicit_successor_scope_reaches_the_existing_plan_unchanged(tmp_path):
    f = Fixture(tmp_path)
    f.plan["application"]["helper"]["operation_basis"] = basis()
    f.plan["authorities"]["helper_catalog"] = request()
    result = f.assemble()
    assert result["authorities"] == f.plan["authorities"]
    assert result["application"] == f.plan["application"]
    assert "catalog_capability" not in result["application"]["helper"]
    assert f.source.read_bytes() == f.raw


def test_omission_and_exact_session_have_identical_legacy_plan_bytes(tmp_path):
    f = Fixture(tmp_path)
    f.plan["authorities"]["helper_catalog"] = {"issuance_id": "legacy-catalog"}
    original = encoded(f.assemble())
    f.plan["authorities"]["helper_catalog"]["scope"] = "exact_session"
    assert encoded(f.assemble()) == original
    assert json.loads(f.source.read_bytes())["authorities"]["helper_catalog"]["scope"] == "exact_session"


@pytest.mark.parametrize("scope", [None, True, 1, {}, [], "", "current_helper_successors ", "all_helpers"])
def test_catalog_scope_is_closed_and_explicit(tmp_path, scope):
    f = Fixture(tmp_path)
    f.plan["authorities"]["helper_catalog"] = {"issuance_id": "fresh", "scope": scope}
    with pytest.raises(CustodyError):
        f.assemble()


def test_scope_does_not_expand_the_helper_mcp_request_shape(tmp_path):
    f = Fixture(tmp_path)
    f.plan["authorities"]["helper_mcp"] = request()
    with pytest.raises(CustodyError):
        f.assemble()


@pytest.mark.parametrize("change", ["base", "id", "root", "two"])
def test_draft_cannot_reinterpret_an_existing_parent_with_a_new_selection(tmp_path, change):
    f = Fixture(tmp_path)
    helper = f.plan["application"]["helper"]
    helper["operation_basis"] = basis()
    parent = reference("fresh-successor-catalog")
    parent["root_authority"] = deepcopy(f.plan["owner"]["native_host"]["remote_operator"])
    helper["catalog_capability"] = {"parent": [parent]}
    f.plan["authorities"]["helper_catalog"] = request()
    if change == "base":
        helper["operation_basis"]["base"] = [deepcopy(parent)]
    elif change == "id":
        parent["grant_id"] = "previous-id"
    elif change == "root":
        parent["root_authority"] = {"opaque_native_principal": "different"}
    else:
        helper["catalog_capability"]["parent"] *= 2
    with pytest.raises(CustodyError):
        f.assemble()


def test_successor_parent_is_retained_separately_from_executable_authority(tmp_path):
    f = completed(tmp_path)
    expected = deepcopy(f.setup_value)
    bundle = f.retain()
    assert json.loads(bundle.authenticate()[0]) == expected
    assert expected["helper"]["operation_basis"]["base"] is None
    assert expected["helper"]["catalog_capability"]["parent"] == [reference("fresh-successor-catalog")]
    assert not f.calls and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("scope", [None, "exact_session"])
def test_legacy_completed_catalog_stays_in_its_exact_session_base(tmp_path, scope):
    f = completed(tmp_path, scope=scope)
    assert json.loads(f.retain().authenticate()[0]) == f.setup_value
    assert "catalog_capability" not in f.setup_value["helper"]


@pytest.mark.parametrize("change", ["missing", "null", "empty", "two", "foreign_id", "foreign_root",
    "extra_field", "base", "source_base", "unrequested", "legacy_mode", "mcp_target", "request_field"])
def test_unrequested_or_malformed_successor_parent_cannot_pass_repinned_output(tmp_path, change):
    f = completed(tmp_path)
    helper = f.setup_value["helper"]
    cap = helper["catalog_capability"]
    if change == "missing":
        del helper["catalog_capability"]
    elif change == "null":
        helper["catalog_capability"] = None
    elif change == "empty":
        cap["parent"] = []
    elif change == "two":
        cap["parent"] *= 2
    elif change == "foreign_id":
        cap["parent"][0]["grant_id"] = "old-catalog"
    elif change == "foreign_root":
        cap["parent"][0]["root_authority"] = "another-operator"
    elif change == "extra_field":
        cap["operations"] = ["execute_sql"]
    elif change == "base":
        helper["operation_basis"]["base"] = deepcopy(cap["parent"])
    elif change == "source_base":
        f.plan_value["application"]["helper"]["operation_basis"]["base"] = deepcopy(cap["parent"])
    elif change == "unrequested":
        del f.plan_value["authorities"]["helper_catalog"]
    elif change == "legacy_mode":
        f.plan_value["authorities"]["helper_catalog"].pop("scope")
    elif change == "mcp_target":
        helper["operation_basis"]["targets"] = [{"target": {"kind": "mcp_server", "server": "other"},
            "lineage": deepcopy(cap["parent"])}]
    else:
        f.plan_value["authorities"]["helper_catalog"]["operations"] = ["execute_sql"]
    f.repin_plan()
    f.publish()
    with pytest.raises(CustodyError):
        f.retain()
    assert not f.calls and not f.caller.provision_attempt_path.exists()


def test_retained_parent_does_not_upgrade_without_a_request(tmp_path):
    f = completed(tmp_path)
    f.plan_value["application"]["helper"]["catalog_capability"] = deepcopy(
        f.setup_value["helper"]["catalog_capability"])
    del f.plan_value["authorities"]["helper_catalog"]
    f.repin_plan()
    assert json.loads(f.retain().authenticate()[0]) == f.setup_value
    f.setup_value["helper"]["catalog_capability"]["parent"][0]["grant_id"] = "new-parent"
    f.publish()
    with pytest.raises(CustodyError):
        f.retain()


def explicit_target(f):
    configured = f.plan_value["authorities"]["agent_setup"][0]
    configured["issuance_id"] = "independently-fresh-outer"
    configured["helper_targets"] = [{"target": None, "issuance_id": "independently-fresh-catalog-target"}]
    capability = f.setup_value["native_members"][0]["setup_capability"]
    capability["parent"] = reference(configured["issuance_id"])
    capability["helper_targets"] = [{"target": None,
        "parent": reference("independently-fresh-catalog-target"),
        "restrictions": {"inert_native_bounds": "catalog-read, helper/audience, depth1, unrestricted domains"}}]
    return configured, capability


@pytest.mark.parametrize("change", [None, "outer_old_id", "target_old_id", "missing_target", "retained_bounds"])
def test_independent_original_target_and_outer_request_ids_keep_exact_custody(tmp_path, change):
    f = completed(tmp_path)
    configured, capability = explicit_target(f)
    if change == "outer_old_id":
        capability["parent"]["grant_id"] = "previous-exact-session-outer"
    elif change == "target_old_id":
        capability["helper_targets"][0]["parent"]["grant_id"] = "previous-exact-session-catalog"
    elif change == "missing_target":
        capability["helper_targets"] = []
    elif change == "retained_bounds":
        # An existing compiled Original ceiling cannot be widened by repinning
        # output, even if all member/output projections repeat the change.
        old = deepcopy(capability)
        for owner in (f.plan_value["owner"]["native_host"], f.plan_value["application"]):
            owner["members"][0]["setup_capability"] = deepcopy(old)
        capability["helper_targets"][0]["restrictions"] = {"inert_native_bounds": "different"}
    f.join_members()
    f.repin_plan()
    f.publish()
    if change is None:
        assert json.loads(f.retain().authenticate()[0]) == f.setup_value
    else:
        with pytest.raises(CustodyError):
            f.retain()
    assert not f.calls


def test_successor_catalog_and_mcp_issuance_stay_independent(tmp_path):
    f = completed(tmp_path)
    f.plan_value["authorities"]["helper_mcp"] = {"issuance_id": "fresh-operation-only"}
    f.setup_value["server"] = {"name": "selected-setup-server"}
    row = {"target": {"kind": "mcp_server", "server": "selected-setup-server"},
        "lineage": [reference("fresh-operation-only")]}
    f.setup_value["helper"]["operation_basis"]["targets"] = [row]
    f.repin_plan()
    f.publish()
    helper = json.loads(f.retain().authenticate()[0])["helper"]
    assert helper["operation_basis"] == {**basis(), "targets": [row]}
    assert helper["catalog_capability"]["parent"] == [reference("fresh-successor-catalog")]


@pytest.mark.parametrize("scope", [SCOPE, "exact_session", None])
def test_catalog_addition_does_not_change_the_independent_mcp_target(tmp_path, scope):
    f = completed(tmp_path, scope=scope)
    old = {"target": {"kind": "mcp_server", "server": "retained-server"},
        "lineage": [reference("retained-operation")]}
    for helper in (f.plan_value["application"]["helper"], f.setup_value["helper"]):
        helper["operation_basis"]["targets"] = [deepcopy(old)]
    f.repin_plan()
    f.publish()
    assert json.loads(f.retain().authenticate()[0])["helper"]["operation_basis"]["targets"] == [old]


@pytest.mark.parametrize("change", ["plan", "source", "output", "currentness"])
def test_successor_completion_still_reauthenticates_request_and_source_custody(tmp_path, change):
    f = completed(tmp_path)
    bundle = f.retain()
    if change == "currentness":
        f.current = False
    else:
        path = {"plan": f.plan.path, "source": f.source.path, "output": bundle.setup.path}[change]
        write(path, path.read_bytes() + b" ")
    with pytest.raises(CustodyError):
        bundle.authenticate()
    assert not f.calls
