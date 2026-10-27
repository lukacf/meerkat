"""Local declarations and recovery byte custody, using inert inventory/owner boundaries.

No native process, installed host, SDK, credentials or authority is exercised.
"""
from copy import deepcopy
from dataclasses import replace
import json
from types import SimpleNamespace

import pytest

from launchkit.blueprint import Blueprint
from launchkit.connect_installer_caller import CustodyError, NativeApplicationInventory
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner, _encode
from launchkit.local_application_source import _initial_policy
from test_forge_application_host import put, observation, selected, select_member_classes
from test_meta_toolkit_runtime_selection import product, no_product_execution


def local_owner(root, inventory, monkeypatch, *, host=None, model=None, policy=None):
    """An explicitly inert installation-owner fixture around real declaration code."""
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    original = deepcopy(inventory)
    pin = put(root / "original-inventory.json", original)
    actual = {"realm_id": "original-realm", "members": [{"identity": row["identity"],
        "member": {"mob_id": original["mob_id"], "role": row["profile"],
            "member": "opaque-native-" + str(index)}} for index, row in enumerate(original["roster"])]}
    principal = {"kind": "human", "id": "original-console-user",
        "qualification": {"kind": "qualified", "trust_domain_id": "original-domain"}}
    if host is None:
        host = SimpleNamespace(_inventory=original, _inventory_pin=pin,
            _blueprint={"advanced": {"port": 8765}}, _policy_bytes=_encode(policy or {"members": []}),
            console_operator_declaration=lambda: deepcopy(principal))
    factory = SimpleNamespace(_selected_model_configuration=lambda: model,
        _installed_admission=None, console_setup_selection=None, _console_choice=lambda value: None)
    owner = LocalApplicationPreparationOwner(factory)
    owner.host = host
    owner._guide_id, owner._deployment_id = "original-guide", "original-installation"
    owner._inputs = SimpleNamespace(supervisor=SimpleNamespace(destination=root),
        startup_selection_path=root / ".mobkit/forge-local-setup/startup-selection.json")
    console = {"origin": "http://127.0.0.1:8765", "access_management": "requester_issued_leaves"}
    choices = owner._initial_choices(owner._guide_id, owner._deployment_id, console, None)
    owner._choices_bytes = _encode(choices)
    checks = []
    def require(guide, deployment):
        assert (guide, deployment) == (owner._guide_id, owner._deployment_id)
        checks.append("original-owner")
    monkeypatch.setattr(owner, "_require", require)
    native = NativeApplicationInventory(None, None, host._inventory_pin, (), pin, pin)
    monkeypatch.setattr(NativeApplicationInventory, "authenticate", lambda self: (deepcopy(original), deepcopy(actual)))
    return SimpleNamespace(owner=owner, original=original, actual=actual, native=native,
        checks=checks, choices=choices, root=root)


def original_inventory(*, empty=False, helper=False):
    value = {"mob_id": "original-app", "roster": [] if empty else [
        {"identity": "reader:main", "profile": "reader"}]}
    if helper:
        value["roster"].append({"identity": "setup:main", "profile": "setup"})
        value["helper_identity"] = "setup:main"
    if empty:
        value["member_classes"] = [{"id": "reader", "profile": "reusable-reader",
            "factory": "application:reader", "callbacks": [], "grants": []}]
    return value


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_local_defaults_request_only_actual_original_route_controllers(tmp_path, monkeypatch, empty):
    f = local_owner(tmp_path, original_inventory(empty=empty), monkeypatch)
    before = deepcopy(f.original)
    result = f.owner.declarations(f.native)
    assert f.original == before
    assert not {"helper", "acquisition"} & result.keys()
    assert set(result["authorities"]) == {"controllers"}
    native = result["owner"]["native_host"]
    assert len(native["members"]) == len(native["routes"]) == len(f.original["roster"])
    assert len(result["authorities"]["controllers"]) == len(native["routes"])
    for request, route in zip(result["authorities"]["controllers"], native["routes"]):
        assert request["target"] == {"kind": "original_route", "route": route["id"]}
        assert route["application_parent"] is route["controller_parent"] is None
        assert route["origin"]["requester"] == f.choices["console_operator"]
    assert f.checks == ["original-owner", "original-owner"]
    assert not {"limits", "initial_setup_identities", "backend_generation", "output_audience"} & f.choices.keys()


@pytest.mark.parametrize("helperless", [False, True])
def test_local_class_defaults_join_the_actual_forge_plan_reader(selected, monkeypatch, helperless):
    selected = select_member_classes(selected, helperless=helperless)
    inventory, actual = observation(selected, monkeypatch)
    f = local_owner(selected.root / "installation", json.loads(selected.inventory.read()), monkeypatch,
        host=selected.host)
    monkeypatch.setattr(NativeApplicationInventory, "authenticate", lambda self: (deepcopy(f.original), deepcopy(actual)))
    declarations = f.owner.declarations(inventory)
    plan = json.loads(selected.host.preparation_plan(inventory, **declarations))
    assert plan["application"]["roster"] == f.original["roster"]
    assert plan["application"]["member_classes"] == f.original["member_classes"]
    assert plan["owner"]["native_host"]["member_classes"] == f.original["member_classes"]
    if helperless:
        assert not {"helper", "helper_tools", "helper_profile"} & plan["application"].keys()
        assert "acquisition" not in plan
    else:
        assert plan["application"]["helper_tools"] == "checked_installation"
        assert plan["application"]["helper_profile"] == "dedicated"
        assert f.choices["initial_setup_identities"] == ["assistant:main"]
        assert len(plan["authorities"]["agent_setup"]) == 1
        assert "helper" not in plan["authorities"]["agent_setup"][0]
        assert all(row.get("initial_message") is None for row in plan["application"]["roster"])


@pytest.mark.parametrize("selected_caller", [False, True])
def test_helper_with_classes_requests_setup_only_for_explicit_seed_caller(tmp_path, monkeypatch, selected_caller):
    original = original_inventory(helper=True)
    original["roster"].append({"identity": "discovery:main", "profile": "discovery"})
    grant = {"tool_name": "find_and_install", "action": "invoke", "consequence": "r1"}
    original["member_classes"] = [{"id": "project", "profile": "application__project",
        "factory": "application:project", "callbacks": [], "grants": [deepcopy(grant)]}]
    policy = {"members": [{"member_identity": row["identity"],
        "grants": [deepcopy(grant)] if selected_caller and row["identity"] == "reader:main" else []}
        for row in original["roster"]]}
    f = local_owner(tmp_path, original, monkeypatch, policy=policy)
    assert f.choices["initial_setup_identities"] == (["reader:main"] if selected_caller else [])
    if not selected_caller:
        with pytest.raises(CustodyError, match="dedicated helper and explicit Console application route"):
            f.owner.declarations(f.native)
        return
    result = f.owner.declarations(f.native)
    assert len(result["owner"]["native_host"]["routes"]) == 2
    requests = result["authorities"]["agent_setup"]
    assert len(requests) == 1 and requests[0]["member"] == f.actual["members"][0]["member"]
    assert requests[0]["route"] == "forge-console-0" and "helper" not in requests[0]
    assert result["helper"]["member"] == f.actual["members"][1]["member"]
    assert f.original == original


def test_local_helperless_declarations_refuse_another_inventory_source(tmp_path, monkeypatch):
    f = local_owner(tmp_path, original_inventory(), monkeypatch)
    other = put(tmp_path / "other-inventory.json", f.original)
    with pytest.raises(CustodyError, match="substitute another"):
        f.owner.declarations(replace(f.native, compiler_input=other))


def test_existing_fixed_helper_keeps_original_setup_defaults(tmp_path, monkeypatch):
    original = original_inventory(helper=True)
    original["roster"].append({"identity": "discovery:main", "profile": "discovery"})
    f = local_owner(tmp_path, original, monkeypatch)
    assert f.choices["initial_setup_identities"] == ["reader:main", "discovery:main"]
    result = f.owner.declarations(f.native)
    assert [row["member"] for row in result["authorities"]["agent_setup"]] == [
        f.actual["members"][0]["member"], f.actual["members"][2]["member"]]


def recovery(f, monkeypatch, *, choices=None):
    root = f.root
    for name in (".mobkit", ".mobkit/forge-local-setup", ".mobkit/forge-local-setup/finalization"):
        (root / name).mkdir(mode=0o700)
    home = root / ".mobkit/forge-local-setup"
    put(home / "local-application-policy.json", _encode(f.choices if choices is None else choices))
    put(home / "activation-template.json", _encode({"activation_id": f.owner._deployment_id}))
    put(home / "startup-selection.json", {"inert": "existing-selection-presence"})
    admission = SimpleNamespace(guide_id=f.owner._guide_id, deployment_id=f.owner._deployment_id,
        installation="original-installation-binding")
    f.owner.factory._installed_admission = admission
    monkeypatch.setattr(f.owner, "_binding", lambda value: admission.installation)
    monkeypatch.setattr(f.owner, "_recovery_admission", lambda value: value)
    from launchkit import deployment_ports
    monkeypatch.setattr(deployment_ports, "installed_console_port", lambda root, port: port)
    receipt = {"guide_id": admission.guide_id, "destination": str(root)}
    return admission, receipt, home


@pytest.mark.parametrize("helper,timeout", [(False, None), (True, 180_000), (True, 3_600_000)])
def test_recovery_preserves_exact_helperless_or_original_helper_policy_bytes(tmp_path, monkeypatch, helper, timeout):
    f = local_owner(tmp_path, original_inventory(helper=helper), monkeypatch)
    if helper:
        f.choices["limits"]["setup_timeout_ms"] = timeout
    admission, receipt, home = recovery(f, monkeypatch)
    before = {p.name: p.read_bytes() for p in home.iterdir() if p.is_file()}
    choices_pin, _, _, _ = f.owner._recovery_inputs(admission, receipt)
    assert choices_pin.read() == _encode(f.choices)
    assert {p.name: p.read_bytes() for p in home.iterdir() if p.is_file()} == before


@pytest.mark.parametrize("field,value", [("limits", None), ("limits", {"setup_timeout_ms": 3_600_000}),
    ("initial_setup_identities", []), ("backend_generation", "foreign"), ("output_audience", "foreign")])
def test_helperless_recovery_refuses_injected_helper_choices_without_rewriting(tmp_path, monkeypatch, field, value):
    f = local_owner(tmp_path, original_inventory(), monkeypatch)
    changed = {**f.choices, field: value}
    admission, receipt, home = recovery(f, monkeypatch, choices=changed)
    before = (home / "local-application-policy.json").read_bytes()
    with pytest.raises(CustodyError, match="original local setup policy"):
        f.owner._recovery_inputs(admission, receipt)
    assert (home / "local-application-policy.json").read_bytes() == before


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_initial_policy_contains_no_setup_helper_grants(selected, monkeypatch, empty):
    selected = select_member_classes(selected, helperless=True)
    original = json.loads(selected.inventory.read())
    if empty:
        original["roster"] = []
    policy = json.loads(_initial_policy(Blueprint.model_validate(selected.spec), original, "original-guide"))
    assert policy["members"] == ([] if empty else [{"member_identity": "assistant:main", "grants": []}])
