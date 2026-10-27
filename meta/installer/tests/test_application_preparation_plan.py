"""Inert plan-assembly controls, without SDK, driver, grants or native execution.

Opaque native principal/lineage values below exercise preservation, not native
qualification. Real native types and roster lowering remain Toolkit's checks.
"""
from copy import deepcopy
import hashlib
import json

import pytest

from launchkit.connect_application_plan import MAX_PLAN_BYTES, application_preparation_plan
from launchkit.connect_installer_caller import CustodyError, ExactFile


def encoded(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def member(alias, role):
    return {"member": {"mob_id": "fixture-app", "role": role, "member": alias},
        "logical_owner": {"opaque_native_principal": alias}, "context_generation": 7,
        "audience": {"opaque_native_audience": alias}}


def route(route_id, binding):
    return {"id": route_id, "origin": {"kind": "local_console", "member": deepcopy(binding),
        "requester": {"opaque_native_principal": "operator"},
        "ingress_actor": {"opaque_native_principal": "console"}},
        "application_parent": [{"opaque_native_parent": "application"}],
        "controller_parent": [{"opaque_native_parent": "controller"}],
        "logical_executor": {"opaque_native_principal": "caller"}, "represented_subject": None,
        "source_authority": {"opaque_native_principal": "source"},
        "authentication_domain": {"opaque_native_domain": "authentication"},
        "event_domain": {"opaque_native_domain": "console-event"}}


class Fixture:
    def __init__(self, root):
        self.root = root.resolve()
        self.root.chmod(0o700)
        self.helper = member("native-helper-7", "research")
        self.caller = member("native-caller-9", "writer")
        self.other = member("native-unselected-3", "reviewer")
        members = [self.helper, self.caller, self.other]
        self.plan = {"schema": 1, "deployment_root": str(self.root / "installation"),
            "owner": {"deployment": "deployment-actual", "components": ["actual-console"], "channels": [],
                "native_host": {"realm": "mob:fixture-app",
                    "grant_administrator": {"opaque_native_principal": "application"},
                    "remote_operator": {"opaque_native_principal": "operator"},
                    "ingress_domain": {"opaque_native_domain": "selected-ingress"},
                    "members": deepcopy(members),
                    "routes": [route("caller-console", self.caller["member"]),
                               route("unselected-console", self.other["member"])]}},
            "startup": {"kind": "restore"},
            "authorities": {"controllers": [], "agent_setup": [{"member": deepcopy(self.caller["member"]),
                "route": "caller-console", "issuance_id": "retained-setup-issuance"}], "helper_report": None},
            "application": {"schema": 4,
                "realm": {"realm_id": "mob:fixture-app", "context_root": str(self.root / "contexts"),
                    "state_root": str(self.root / "state"), "user_config_root": None},
                "definition": {"format": "toml", "text": '# Exact source comments remain.\n[mob]\nid = "fixture-app"\n'},
                "roster": [{"identity": "desired:helper", "profile": "research"},
                           {"identity": "desired:caller", "profile": "writer"},
                           {"identity": "desired:other", "profile": "reviewer"}],
                "members": deepcopy(members),
                "helper": {"member": deepcopy(self.helper["member"]),
                    "ingress_actor": {"opaque_native_principal": "helper-source"},
                    "logical_executor": {"opaque_native_principal": "helper"}, "represented_subject": None,
                    "controller_parent": [{"opaque_native_parent": "helper-controller"}],
                    "operation_basis": {"opaque_native_operation_basis": ["retained"]}},
                "helper_tools": "preparation", "helper_profile": "dedicated",
                "tool_policy": {"provider_id": "application", "policy_id": "exact-tool-selection", "revision": 8,
                    "source": {"opaque_native_policy_provenance": "unchanged"},
                    "members": [{"member_identity": item["member"]["member"], "grants": []} for item in members]}},
            "acquisition": {"console_setup": {"opaque_native_console_setup": "selected"},
                "backend_generation": "retained-workgraph-generation",
                "proposal_ceiling": {"actions": [{"action": "deliver", "resource": "existing-delivery"}],
                    "services": [{"opaque_native_service_bound": ["connection", "operation"]}],
                    "frontends": [{"opaque_native_frontend_bound": "participant"}],
                    "output_audiences": ["original-result", "other-approved-audience"]},
                "output_audience": ["original-result"],
                "limits": {"setup_timeout_ms": 25000, "max_workers": 2,
                    "max_effect_attempts": 3, "admission_timeout_ms": 5000}},
            "output": str(self.root / "fresh-output"), "hold_timeout_ms": 50000}
        self.source = self.root / "selected-draft.json"
        self.checks = 0
        self.after_selection = lambda: None

    def pin(self, raw=None):
        self.raw = encoded(self.plan) if raw is None else raw
        self.source.write_bytes(self.raw)
        self.source.chmod(0o600)
        return ExactFile(self.source, hashlib.sha256(self.raw).hexdigest())

    def current(self):
        self.checks += 1
        if self.checks == 2:
            self.after_selection()

    def assemble(self, raw=None):
        self.pin_value = self.pin(raw)
        return json.loads(application_preparation_plan(self.pin_value, require_current=self.current))


def pairs(plan):
    return [(row["action"], row["resource"]) for row in plan["acquisition"]["proposal_ceiling"]["actions"]]


def helperless_fixture(root, *, empty=False):
    f = Fixture(root)
    application = f.plan["application"]
    for name in ("helper", "helper_tools", "helper_profile"):
        del application[name]
    del f.plan["acquisition"]
    f.plan["authorities"] = {}
    native = f.plan["owner"]["native_host"]
    for rows in (native["members"], application["members"], application["roster"],
                 application["tool_policy"]["members"]):
        rows.pop(0)
        if empty:
            rows.clear()
    if empty:
        native["routes"] = []
    application["member_classes"] = [{"id": "project", "profile": "application__project",
        "factory": "application:project", "callbacks": [], "grants": []}]
    native["member_classes"] = deepcopy(application["member_classes"])
    return f


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_plan_preserves_selected_classes_without_acquisition_or_effects(tmp_path, empty):
    f = helperless_fixture(tmp_path, empty=empty)
    original = deepcopy(f.plan)
    assert f.assemble() == original and f.plan == original
    assert f.source.read_bytes() == encoded(original) and f.checks == 2
    assert set(f.root.iterdir()) == {f.source}


def test_helperless_plan_retains_actual_original_controller_selection(tmp_path):
    f = helperless_fixture(tmp_path)
    f.plan["authorities"] = {"controllers": [{"target": {
        "kind": "original_route", "route": "caller-console"},
        "issuance_id": "retained-controller-id", "controller_route": "native-controller-key"}]}
    assert f.assemble() == f.plan
    f.plan["authorities"]["controllers"][0]["target"]["route"] = "unselected-route"
    with pytest.raises(CustodyError, match="unknown original route"):
        f.assemble()


@pytest.mark.parametrize("field", ["helper", "helper_tools", "helper_profile", "acquisition"])
def test_helperless_plan_refuses_partial_or_null_helper_selection(tmp_path, field):
    f = helperless_fixture(tmp_path)
    target = f.plan if field == "acquisition" else f.plan["application"]
    target[field] = None
    with pytest.raises(CustodyError):
        f.assemble()
    assert set(f.root.iterdir()) == {f.source}


@pytest.mark.parametrize("field", ["agent_setup", "helper_report", "helper_mcp", "helper_catalog"])
def test_helperless_plan_refuses_helper_authority_fields_even_when_empty(tmp_path, field):
    f = helperless_fixture(tmp_path)
    f.plan["authorities"][field] = [] if field == "agent_setup" else None
    with pytest.raises(CustodyError, match="PreparationAuthorityIntent"):
        f.assemble()
    assert set(f.root.iterdir()) == {f.source}


@pytest.mark.parametrize("change", ["controller", "setup_capability"])
def test_helperless_plan_refuses_helper_controller_and_retained_setup_capability(tmp_path, change):
    f = helperless_fixture(tmp_path)
    if change == "controller":
        f.plan["authorities"]["controllers"] = [{"target": {"kind": "helper"},
            "issuance_id": "retained-helper-controller"}]
    else:
        for members in (f.plan["application"]["members"], f.plan["owner"]["native_host"]["members"]):
            members[0]["setup_capability"] = {"route": "caller-console",
                "parent": {"opaque_native_lineage": "retained"}}
    with pytest.raises(CustodyError, match="actual helper"):
        f.assemble()


@pytest.mark.parametrize("classes", [[], None, ["project"]])
def test_empty_helperless_plan_requires_native_class_declarations(tmp_path, classes):
    f = helperless_fixture(tmp_path, empty=True)
    f.plan["application"]["member_classes"] = classes
    with pytest.raises(CustodyError):
        f.assemble()


@pytest.mark.parametrize("change", ["roster", "members", "policy"])
def test_empty_helperless_plan_refuses_invented_initial_inventory(tmp_path, change):
    f = helperless_fixture(tmp_path, empty=True)
    if change == "roster":
        f.plan["application"]["roster"].append({"identity": "project:placeholder",
            "profile": "application__project"})
    elif change == "members":
        f.plan["application"]["members"].append(f.caller)
    else:
        f.plan["application"]["tool_policy"]["members"].append({
            "member_identity": "invented", "grants": []})
    with pytest.raises(CustodyError):
        f.assemble()
    assert set(f.root.iterdir()) == {f.source}


def test_helperful_plan_also_preserves_selected_member_classes(tmp_path):
    f = Fixture(tmp_path)
    f.plan["authorities"] = {}
    f.plan["application"]["member_classes"] = [{"id": "project", "profile": "application__project",
        "factory": "application:project", "callbacks": [], "grants": []}]
    f.plan["owner"]["native_host"]["member_classes"] = deepcopy(f.plan["application"]["member_classes"])
    assert f.assemble() == f.plan


@pytest.mark.parametrize("mode", ["preparation", "inert_acquisition", "checked_installation"])
def test_explicit_helper_composition_preserves_presenter_policy_and_other_members(tmp_path, mode):
    f = Fixture(tmp_path)
    f.plan["application"]["helper_tools"] = mode
    # The application selects its origin and requester-issued access policy.
    # Plan assembly preserves these choices without granting native access.
    f.plan["acquisition"]["console_setup"] = {
        "origin": "http://127.0.0.1:4110",
        "access_management": "requester_issued_leaves",
    }
    # This plan already has its exact selected source action pairs. Assembling
    # it must not grant new helper tools, invent review or change any owner.
    f.plan["acquisition"]["proposal_ceiling"]["actions"] += [
        {"action": "find_and_install", "resource": "native-caller-9"},
        {"action": "delegate", "resource": "native-caller-9"}]
    original = deepcopy(f.plan)
    assert f.assemble() == original
    assert f.plan == original


@pytest.mark.parametrize("mode", ["CheckedInstallation", "install", "checked_installation ", True, None])
def test_helper_composition_aliases_do_not_silently_enable_installation(tmp_path, mode):
    f = Fixture(tmp_path)
    f.plan["application"]["helper_tools"] = mode
    with pytest.raises(CustodyError, match="selected native strategy"):
        f.assemble()


def test_exact_native_console_target_pair_preserves_all_other_selected_facts(tmp_path):
    f = Fixture(tmp_path)
    original = deepcopy(f.plan)
    result = f.assemble()
    assert pairs(result) == [("deliver", "existing-delivery"),
                             ("find_and_install", "native-caller-9"), ("delegate", "native-caller-9")]
    assert all(resource not in {"native-helper-7", "native-unselected-3", "desired:caller"}
               for action, resource in pairs(result) if action in {"find_and_install", "delegate"})
    result["acquisition"]["proposal_ceiling"]["actions"] = original["acquisition"]["proposal_ceiling"]["actions"]
    assert result == original
    assert f.source.read_bytes() == f.raw and f.checks == 2
    assert not (tmp_path / "fresh-output").exists()


def test_provider_event_uses_exact_unique_compiled_channel_binding(tmp_path):
    f = Fixture(tmp_path)
    origin = {"kind": "provider_event", "channel_id": "actual-channel", "sender_id": "actual-sender",
              "target_identity": "desired:caller"}
    f.plan["owner"]["native_host"]["routes"][0]["origin"] = origin
    channel = {"component": "provider-component", **{key: value for key, value in origin.items() if key != "kind"},
        "service_subject": "actual-subject", "provider_account": "actual-account",
        "requester": {"opaque_native_principal": "provider-user"},
        "ingress_actor": {"opaque_native_principal": "provider-source"},
        "member": deepcopy(f.caller["member"]), "realm": "mob:fixture-app", "required_surfaces": []}
    other_channel = {**deepcopy(channel), "sender_id": "different-sender", "member": deepcopy(f.other["member"])}
    f.plan["owner"]["channels"] = [other_channel, channel]
    result = f.assemble()
    assert pairs(result)[-2:] == [("find_and_install", "native-caller-9"), ("delegate", "native-caller-9")]
    assert result["owner"] == f.plan["owner"]
    f.plan["owner"]["channels"].append(deepcopy(channel))
    with pytest.raises(CustodyError, match="one exact selected channel"):
        f.assemble()
    f.plan["owner"]["channels"] = [other_channel]
    with pytest.raises(CustodyError, match="one exact selected channel"):
        f.assemble()


def test_restore_retains_named_existing_capability_bounds_without_new_request(tmp_path):
    f = Fixture(tmp_path)
    capability = {"route": "caller-console", "parent": {"opaque_native_lineage": ["existing-root", "existing-id"]}}
    f.plan["owner"]["native_host"]["members"][1]["setup_capability"] = deepcopy(capability)
    f.plan["application"]["members"][1]["setup_capability"] = deepcopy(capability)
    f.plan["authorities"]["agent_setup"] = []
    f.plan["acquisition"]["proposal_ceiling"]["actions"] += [
        {"action": "find_and_install", "resource": "native-caller-9"},
        {"action": "delegate", "resource": "native-caller-9"}]
    original = deepcopy(f.plan)
    assert f.assemble() == original


def test_no_explicit_capability_or_request_adds_no_acquisition_actions(tmp_path):
    f = Fixture(tmp_path)
    f.plan["authorities"]["agent_setup"] = []
    assert f.assemble() == f.plan
    f.plan["acquisition"]["proposal_ceiling"]["actions"].append(
        {"action": "find_and_install", "resource": "native-unselected-3"})
    with pytest.raises(CustodyError, match="explicitly configured original"):
        f.assemble()


def test_selected_existing_find_bound_gets_only_its_missing_pair(tmp_path):
    f = Fixture(tmp_path)
    actions = f.plan["acquisition"]["proposal_ceiling"]["actions"]
    actions += [{"action": "delegate", "resource": "other-approved-delegation"},
                {"action": "find_and_install", "resource": "native-caller-9"}]
    original_actions = deepcopy(actions)
    result = f.assemble()
    assert result["acquisition"]["proposal_ceiling"]["actions"] == [
        *original_actions, {"action": "delegate", "resource": "native-caller-9"}]
    f.plan = result
    assert f.assemble() == result


@pytest.mark.parametrize("change", ["request_member", "request_route", "route_role", "ambiguous_route", "owner_member", "tool_policy"])
def test_mismatched_native_request_route_member_or_inventory_refuses(tmp_path, change):
    f = Fixture(tmp_path)
    if change == "request_member":
        f.plan["authorities"]["agent_setup"][0]["member"] = deepcopy(f.helper["member"])
    elif change == "request_route":
        f.plan["authorities"]["agent_setup"][0]["route"] = "unselected-console"
    elif change == "route_role":
        f.plan["owner"]["native_host"]["routes"][0]["origin"]["member"]["role"] = "wrong-role"
    elif change == "ambiguous_route":
        f.plan["owner"]["native_host"]["routes"].append(deepcopy(f.plan["owner"]["native_host"]["routes"][0]))
    elif change == "owner_member":
        f.plan["application"]["members"][1]["context_generation"] = 8
    else:
        f.plan["application"]["tool_policy"]["members"][1]["member_identity"] = "desired:caller"
    with pytest.raises(CustodyError):
        f.assemble()


def test_existing_configured_route_cannot_be_retargeted_by_new_request(tmp_path):
    f = Fixture(tmp_path)
    f.plan["owner"]["native_host"]["routes"].append(route("other-caller-route", f.caller["member"]))
    capability = {"route": "caller-console", "parent": {"opaque_native_lineage": "retained"}}
    for inventory in (f.plan["owner"]["native_host"]["members"], f.plan["application"]["members"]):
        inventory[1]["setup_capability"] = deepcopy(capability)
    f.plan["authorities"]["agent_setup"][0]["route"] = "other-caller-route"
    with pytest.raises(CustodyError, match="changed the retained configured"):
        f.assemble()


def test_empty_helper_controller_needs_explicit_independent_request(tmp_path):
    f = Fixture(tmp_path)
    f.plan["application"]["helper"]["controller_parent"] = []
    with pytest.raises(CustodyError, match="empty helper controller"):
        f.assemble()
    f.plan["authorities"]["controllers"] = [{"target": {"kind": "helper"},
        "issuance_id": "retained-controller-id", "controller_route": "actual-native-controller-key"}]
    f.plan["authorities"]["helper_report"] = {"member": deepcopy(f.helper["member"]),
        "issuance_id": "retained-independent-report-id"}
    result = f.assemble()
    assert result["authorities"] == f.plan["authorities"]
    assert result["application"]["helper"]["controller_parent"] == []


@pytest.mark.parametrize("selected", [
    {}, {"account_issuance_id": None, "public_issuance_id": None},
    {"account_issuance_id": "retained-account-id"},
    {"public_issuance_id": "retained-public-id"},
    {"account_issuance_id": "retained-account-id", "public_issuance_id": "retained-public-id"},
])
def test_helper_optional_requests_preserve_exact_choices_without_grants_or_defaults(tmp_path, selected):
    f = Fixture(tmp_path)
    f.plan["authorities"]["agent_setup"] = []
    f.plan["authorities"]["helper_report"] = {"member": deepcopy(f.helper["member"]),
        "issuance_id": "retained-report-id", **selected}
    original = deepcopy(f.plan)
    assert f.assemble() == original
    assert f.plan == original and f.source.read_bytes() == f.raw
    assert not (tmp_path / "fresh-output").exists()


@pytest.mark.parametrize("field", ["account_issuance_id", "public_issuance_id"])
@pytest.mark.parametrize("value", [True, 1, [], {}, "", " ", " padded "])
def test_optional_helper_request_ids_require_exact_text_or_null(tmp_path, field, value):
    f = Fixture(tmp_path)
    f.plan["authorities"]["helper_report"] = {"member": deepcopy(f.helper["member"]),
        "issuance_id": "retained-report-id", field: value}
    with pytest.raises(CustodyError, match="exact nonempty text"):
        f.assemble()
    assert not (tmp_path / "fresh-output").exists()


@pytest.mark.parametrize("field,other", [
    ("account_issuance_id", "issuance_id"), ("public_issuance_id", "issuance_id"),
    ("public_issuance_id", "account_issuance_id"),
    ("account_issuance_id", "controller"), ("public_issuance_id", "controller"),
    ("account_issuance_id", "agent"), ("public_issuance_id", "agent"),
])
def test_optional_helper_ids_cannot_alias_any_selected_authority_request(tmp_path, field, other):
    f = Fixture(tmp_path)
    f.plan["authorities"]["controllers"] = [{"target": {"kind": "helper"},
        "issuance_id": "retained-controller-id", "controller_route": "actual-controller-key"}]
    request = {"member": deepcopy(f.helper["member"]), "issuance_id": "retained-report-id",
        "account_issuance_id": "retained-account-id", "public_issuance_id": "retained-public-id"}
    request[field] = ("retained-controller-id" if other == "controller" else
        f.plan["authorities"]["agent_setup"][0]["issuance_id"] if other == "agent" else request[other])
    f.plan["authorities"]["helper_report"] = request
    with pytest.raises(CustodyError, match="repeat an issuance ID"):
        f.assemble()


@pytest.mark.parametrize("field", ["mob_id", "role", "member"])
def test_optional_public_request_still_requires_the_exact_current_helper(tmp_path, field):
    f = Fixture(tmp_path)
    binding = deepcopy(f.helper["member"])
    binding[field] = "another-selection"
    f.plan["authorities"]["helper_report"] = {"member": binding,
        "issuance_id": "retained-report-id", "public_issuance_id": "retained-public-id"}
    with pytest.raises(CustodyError, match="actual helper"):
        f.assemble()


@pytest.mark.parametrize("field", ["account_issuance", "public_issuance", "account_parent", "public_parent"])
def test_optional_requests_do_not_open_helper_request_fields(tmp_path, field):
    f = Fixture(tmp_path)
    f.plan["authorities"]["helper_report"] = {"member": deepcopy(f.helper["member"]),
        "issuance_id": "retained-report-id", "public_issuance_id": "retained-public-id", field: "unexpected"}
    with pytest.raises(CustodyError, match="exact native input fields"):
        f.assemble()


@pytest.mark.parametrize("change", ["duplicate", "non_json", "schema", "timeout", "surface_placeholder", "origin_tag"])
def test_invalid_closed_wire_shape_is_rejected_before_driver(tmp_path, change):
    f = Fixture(tmp_path)
    raw = None
    if change == "duplicate":
        raw = encoded(f.plan).replace(b'"backend_generation":', b'"backend_generation":"duplicate","backend_generation":', 1)
    elif change == "non_json":
        raw = encoded(f.plan).replace(b'"hold_timeout_ms":50000', b'"hold_timeout_ms":NaN', 1)
    elif change == "schema":
        f.plan["application"]["schema"] = True
    elif change == "timeout":
        f.plan["hold_timeout_ms"] = 900001
    elif change == "surface_placeholder":
        f.plan["owner"]["native_host"]["members"][1]["initial_surface"] = {"name": "fake", "sha256": "0" * 64}
    else:
        f.plan["owner"]["native_host"]["routes"][0]["origin"]["kind"] = "LocalConsole"
    with pytest.raises(CustodyError):
        f.assemble(raw)


def test_source_byte_pin_and_existing_owner_currentness_are_rechecked(tmp_path):
    f = Fixture(tmp_path)
    pin = f.pin()
    f.source.write_bytes(f.raw + b" ")
    with pytest.raises(CustodyError, match="selected bytes"):
        application_preparation_plan(pin, require_current=f.current)
    f.checks = 0
    f.after_selection = lambda: f.source.write_bytes(f.raw + b" ")
    with pytest.raises(CustodyError, match="selected bytes"):
        f.assemble()
    f.checks = 0

    def owner_changed():
        raise CustodyError("The existing installation selection changed.")

    f.after_selection = owner_changed
    with pytest.raises(CustodyError, match="installation selection changed"):
        f.assemble()


def test_bounded_input_and_expanded_output_are_rejected_without_writes(tmp_path):
    f = Fixture(tmp_path)
    with pytest.raises(CustodyError, match="draft exceeds"):
        f.assemble(encoded(f.plan) + b" " * MAX_PLAN_BYTES)
    base = encoded(f.plan)
    f.plan["application"]["definition"]["text"] += "x" * (MAX_PLAN_BYTES - len(base) - 1)
    assert len(encoded(f.plan)) == MAX_PLAN_BYTES - 1
    with pytest.raises(CustodyError, match="assembled preparation plan exceeds"):
        f.assemble()
    assert not (tmp_path / "fresh-output").exists()
