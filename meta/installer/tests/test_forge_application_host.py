"""Inert source composition controls, without SDK/native execution or acceptance.

Inventory authentication is replaced only inside these tests. Opaque values
exercise preservation and cannot establish qualified principals or native facts.
"""
import asyncio
from copy import deepcopy
from dataclasses import replace
import hashlib
import inspect
import json
from types import SimpleNamespace

import pytest

from launchkit import compiler
from launchkit.blueprint import Blueprint
from launchkit.connect_installer_caller import (CustodyError, ExactFile,
    NativeApplicationInventory, ProductGatewayFile)
from launchkit.forge_application_host import ForgeApplicationHost
from launchkit.projects import DraftConflict
from launchkit.runtime_selection import candidate_selection
from test_meta_toolkit_initial_package import setup_spec
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_meta_toolkit_runtime_update import catalog_target


def encoded(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


def put(path, value):
    content = value if isinstance(value, bytes) else encoded(value)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.write_bytes(content)
    path.chmod(0o600)
    return ExactFile(path, hashlib.sha256(content).hexdigest())


@pytest.fixture
def selected(tmp_path, product):
    target = catalog_target(product)
    root = tmp_path / "selected-original"
    root.mkdir(mode=0o700)
    spec = setup_spec()
    blueprint = put(root / "blueprint.json", spec)
    inventory = put(root / "application-inventory-input.json", compiler.application_inventory_input(spec))
    definition = put(root / "mob.toml", compiler.mob_toml(spec, runtime_target=target).encode())
    policy = put(root / "tool-policy.json", {"provider_id": "application", "policy_id": "original",
        "revision": 2, "members": [
            {"member_identity": "assistant:main", "grants": [{"tool_name": "find_and_install", "action": "invoke", "consequence": "r1"}]},
            {"member_identity": "setup:main", "grants": [{"tool_name": "setup_report", "action": "invoke", "consequence": "r1"}]}]})
    acquisition = put(root / "source-selection.json", {"blueprint": blueprint.pin(),
        "definition": definition.pin(), "original_inventory": inventory.pin(), "source_policy": policy.pin()})
    fields = {"application/configuration.json": {"fields": []},
        "application/console.json": {"authentication": "local-password"},
        "config/access.toml": b'[access]\nenabled = true\n'}
    protected = {name: put(root / name, value) for name, value in fields.items()}
    viewer = put(root / "APPLICATION-SELECTION.json", {"base_source_selection": acquisition.pin(),
        "operator": {"subject": "inert-selected-operator", "private_configuration_key": "CONSOLE_USER_ID",
            "authenticated_evidence": None},
        "source_inputs": {name: pin.pin() for name, pin in protected.items()}})
    host = ForgeApplicationHost(guide_id="original-guide", runtime_target=target,
        acquisition_selection=acquisition, viewer_selection=viewer, require_current=lambda: None)
    return SimpleNamespace(host=host, target=target, spec=spec, inventory=inventory,
        definition=definition, policy=policy, protected=protected, acquisition=acquisition,
        viewer=viewer, root=root)


def observation(selected, monkeypatch):
    original = json.loads(selected.inventory.read())
    actual = {"schema": 1, "mob_id": original["mob_id"],
        "realm_id": "opaque-inert-realm", "members": [{"identity": row["identity"],
            "member": {"mob_id": original["mob_id"], "role": row["profile"], "member": "inert-native-" + str(index)}}
            for index, row in enumerate(original["roster"])]}
    if "helper_identity" in original:
        actual["helper_identity"] = original["helper_identity"]
    gateway = ProductGatewayFile(selected.root / "inert-never-execute", "0" * 64,
        SimpleNamespace(selection=selected.target), (), 0o700, ())
    retained = NativeApplicationInventory(None, gateway, selected.inventory, (), selected.inventory, selected.inventory)
    monkeypatch.setattr(NativeApplicationInventory, "authenticate", lambda self: (deepcopy(original), deepcopy(actual)))
    return retained, actual


def declarations(selected, actual):
    human = selected.host.console_operator_declaration()
    members = [{"member": deepcopy(row["member"]), "logical_owner": {"opaque_principal": row["identity"]},
        "context_generation": 1, "audience": {"opaque_audience": row["identity"]}} for row in actual["members"]]
    origin = {"kind": "local_console", "member": deepcopy(members[0]["member"]),
        "requester": deepcopy(human), "ingress_actor": {"opaque_principal": "host"}}
    route = {"id": "original-console", "origin": origin, "application_parent": None, "controller_parent": None,
        "logical_executor": {"opaque_principal": "assistant"}, "represented_subject": deepcopy(human),
        "source_authority": {"opaque_principal": "console"},
        "authentication_domain": {"opaque_domain": "authentication"}, "event_domain": {"opaque_domain": "console"}}
    result = {"deployment_root": selected.root / "installation", "output": selected.root / "native-output",
        "owner": {"deployment": "original-installation", "components": ["stock-console"], "channels": [],
            "native_host": {"realm": actual["realm_id"], "grant_administrator": {"opaque_principal": "admin"},
                "remote_operator": deepcopy(human), "ingress_domain": {"opaque_domain": "ingress"},
                "members": members, "routes": [route]}},
        "startup": {"kind": "initialize", "namespace": "original-installation", "generation": 1},
        "authorities": {"controllers": [{"target": {"kind": "helper"}, "issuance_id": "selected-helper-controller"}],
            "agent_setup": [{"member": deepcopy(members[0]["member"]), "route": "original-console", "issuance_id": "selected-setup"}],
            "helper_mcp": {"issuance_id": "selected-mcp"}, "helper_catalog": {"issuance_id": "selected-catalog"}},
        "realm_storage": {"context_root": str(selected.root / "installation"), "state_root": str(selected.root / "installation/.mobkit/state")},
        "acquisition": {"backend_generation": "selected-workgraph", "proposal_ceiling": {
            "actions": [], "services": [], "frontends": [], "output_audiences": ["selected-output"]},
            "output_audience": ["selected-output"], "limits": {"setup_timeout_ms": 20000,
                "max_workers": 2, "max_effect_attempts": 2, "admission_timeout_ms": 5000}},
        "hold_timeout_ms": 40000, "source_id": "retained-original-policy"}
    if "helper_identity" in actual:
        result["helper"] = {"member": deepcopy(members[1]["member"]), "ingress_actor": {"opaque_principal": "host"},
            "logical_executor": {"opaque_principal": "setup"}, "represented_subject": None,
            "controller_parent": [], "operation_basis": []}
    else:
        del result["acquisition"]
        result["authorities"] = {}
    return result


def select_member_classes(selected, *, helperless):
    """Repin inert source fixtures before constructing a new source owner."""
    spec = deepcopy(selected.spec)
    if helperless:
        del spec["acquisition"]
        spec["agent_classes"] = spec["agent_classes"][:1]
        spec["agent_classes"][0]["tools"]["rust_bundles"] = []
    project = deepcopy(spec["agent_classes"][0])
    project.update(id="project", instances=0)
    project["tools"]["rust_bundles"] = []
    spec["agent_classes"].append(project)
    spec = Blueprint.model_validate(spec).model_dump(mode="json")
    classes = [{"id": "project", "profile": "application__project", "factory": "application:project",
        "callbacks": [{"name": "project_status", "description": "Read this project.",
            "input_schema": {"type": "object", "properties": {}, "additionalProperties": False}}],
        "grants": [{"tool_name": "project_status", "action": "invoke", "consequence": "r0"}]}]
    blueprint = put(selected.root / "blueprint.json", spec)
    inventory = put(selected.inventory.path, compiler.application_inventory_input(spec, member_classes=classes))
    definition = put(selected.definition.path, compiler.mob_toml(spec, runtime_target=selected.target).encode())
    policy = json.loads(selected.policy.read())
    if helperless:
        policy["members"] = [{"member_identity": "assistant:main", "grants": []}]
    policy = put(selected.policy.path, policy)
    acquisition = put(selected.acquisition.path, {"blueprint": blueprint.pin(),
        "definition": definition.pin(), "original_inventory": inventory.pin(), "source_policy": policy.pin()})
    viewer = json.loads(selected.viewer.read())
    viewer["base_source_selection"] = acquisition.pin()
    viewer = put(selected.viewer.path, viewer)
    host = ForgeApplicationHost(guide_id="original-guide", runtime_target=selected.target,
        acquisition_selection=acquisition, viewer_selection=viewer, require_current=lambda: None)
    return SimpleNamespace(**{**vars(selected), "host": host, "spec": spec, "inventory": inventory,
        "definition": definition, "policy": policy, "acquisition": acquisition, "viewer": viewer})


@pytest.mark.parametrize("helperless", [False, True])
def test_host_passes_exact_selected_class_declarations_into_native_plan(selected, monkeypatch, helperless):
    selected = select_member_classes(selected, helperless=helperless)
    retained, actual = observation(selected, monkeypatch)
    chosen = declarations(selected, actual)
    original = deepcopy(chosen)
    result = json.loads(selected.host.preparation_plan(retained, **chosen))
    source = json.loads(selected.inventory.read())
    assert chosen == original
    assert result["application"]["member_classes"] == source["member_classes"]
    assert result["owner"]["native_host"]["member_classes"] == source["member_classes"]
    assert result["application"]["roster"] == source["roster"]
    assert all(row["identity"] != "project:main" for row in result["application"]["roster"])
    assert result["application"]["definition"]["text"] == selected.definition.read().decode()
    assert result["owner"]["native_host"]["remote_operator"] == selected.host.console_operator_declaration()
    assert not (selected.root / "native-output").exists()
    if helperless:
        assert "acquisition" not in result and result["authorities"] == {}
        assert not {"helper", "helper_tools", "helper_profile"} & result["application"].keys()
        assert [row["identity"] for row in source["roster"]] == ["assistant:main"]


@pytest.mark.parametrize("helperless", [False, True])
def test_host_refuses_a_different_owner_class_selection(selected, monkeypatch, helperless):
    selected = select_member_classes(selected, helperless=helperless)
    retained, actual = observation(selected, monkeypatch)
    chosen = declarations(selected, actual)
    chosen["owner"]["native_host"]["member_classes"] = []
    with pytest.raises(CustodyError, match="selected native member classes"):
        selected.host.preparation_plan(retained, **chosen)


@pytest.mark.parametrize("field", ["helper", "acquisition"])
def test_host_helperless_plan_refuses_explicit_null_helper_declarations(selected, monkeypatch, field):
    selected = select_member_classes(selected, helperless=True)
    retained, actual = observation(selected, monkeypatch)
    chosen = declarations(selected, actual)
    chosen[field] = None
    with pytest.raises(CustodyError, match="must omit"):
        selected.host.preparation_plan(retained, **chosen)


def test_host_refuses_inventory_class_declarations_changed_after_selection(selected, monkeypatch):
    selected = select_member_classes(selected, helperless=True)
    retained, actual = observation(selected, monkeypatch)
    chosen = declarations(selected, actual)
    source = json.loads(selected.inventory.read())
    source["member_classes"][0]["callbacks"][0]["name"] = "unselected_callback"
    # A forged observation cannot replace the class declaration carried by
    # the original input pin, even if the fixed member receipt is identical.
    monkeypatch.setattr(NativeApplicationInventory, "authenticate", lambda self: (source, actual))
    with pytest.raises(CustodyError, match="original inventory input"):
        selected.host.preparation_plan(retained, **chosen)


def test_native_inventory_join_preserves_original_roster_policy_and_explicit_absence(selected, monkeypatch):
    retained, actual = observation(selected, monkeypatch)
    chosen = declarations(selected, actual)
    before = deepcopy(chosen)
    result = json.loads(selected.host.preparation_plan(retained, **chosen))
    assert chosen == before
    assert result["owner"] == chosen["owner"] and result["authorities"] == chosen["authorities"]
    assert result["application"]["roster"] == json.loads(selected.inventory.read())["roster"]
    assert result["application"]["definition"]["text"] == selected.definition.read().decode()
    assert "standing_policy" not in result["acquisition"]
    assert "service_parent_administration" not in result["owner"]["native_host"]
    native_policy = result["application"]["tool_policy"]
    assert native_policy["members"][0]["member_identity"] == actual["members"][0]["member"]["member"]
    assert native_policy["members"][0]["grants"] == json.loads(selected.policy.read())["members"][0]["grants"]
    assert native_policy["source"] == {"source_id": chosen["source_id"], "source_digest": "sha256:" + selected.policy.sha256}


@pytest.mark.parametrize("changed", ["realm", "order", "helper", "source", "product", "operator", "requester", "represented"])
def test_preparation_refuses_foreign_inventory_or_selected_binding(selected, monkeypatch, changed):
    retained, actual = observation(selected, monkeypatch)
    chosen = declarations(selected, actual)
    if changed == "realm":
        chosen["owner"]["native_host"]["realm"] = "foreign-realm"
    elif changed == "order":
        chosen["owner"]["native_host"]["members"].reverse()
    elif changed == "helper":
        chosen["helper"]["member"] = actual["members"][0]["member"]
    elif changed == "source":
        other = put(selected.root / "identical-but-unselected-inventory.json", selected.inventory.read())
        retained = replace(retained, compiler_input=other)
    elif changed == "product":
        retained = replace(retained, gateway=replace(retained.gateway, artifacts=SimpleNamespace(selection=object())))
    elif changed == "operator":
        chosen["owner"]["native_host"]["remote_operator"]["id"] = "another-subject"
    elif changed == "requester":
        chosen["owner"]["native_host"]["routes"][0]["origin"]["requester"]["qualification"]["trust_domain_id"] = "oidc-issuer:another-issuer"
    else:
        chosen["owner"]["native_host"]["routes"][0]["represented_subject"] = None
    with pytest.raises(CustodyError):
        selected.host.preparation_plan(retained, **chosen)


def test_initial_hook_refuses_changed_definition_before_adopting_source(selected):
    adopted = []
    project = SimpleNamespace(blueprint=Blueprint.model_validate(selected.spec),
        framework_provenance=lambda: {"runtime_candidate": candidate_selection(selected.target)},
        read_file=lambda name: selected.definition.read().decode().replace('id = "', 'id = "changed-', 1),
        adopt_application_inputs=lambda *a, **k: adopted.append("inputs"),
        adopt_wheel=lambda *a, **k: adopted.append("wheel"))
    with pytest.raises(CustodyError, match="definition"):
        selected.host.prepare_workspace("original-guide", "build-original", project)
    assert adopted == []


def test_initial_hook_joins_original_protected_inputs_and_exact_wheels(selected, tmp_path):
    from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError
    project = ImplementationWorkspace(tmp_path / "host-prepared-workspace", selected.spec)
    project.seed(runtime_target=selected.target)
    original_definition = project.read_file("config/mob.toml")
    selected.host.prepare_workspace("original-guide", "build-original", project)
    assert project.read_file("config/mob.toml") == original_definition
    original_inputs = {**selected.protected, "application/tool-policy.json": selected.policy}
    for name, pin in original_inputs.items():
        assert project.read_file(name).encode() == pin.read()
        with pytest.raises(WorkspaceError, match="read-only"):
            project.write_file(name, "unselected builder override\n")
    for wheel in (selected.target.meta_wheel, selected.target.toolkit_wheel, *selected.target.dependency_wheels):
        copied = project.project / "application/vendor" / wheel.path.name
        assert copied.read_bytes() == wheel.path.read_bytes()
        assert hashlib.sha256(copied.read_bytes()).hexdigest() == wheel.sha256


def test_stale_original_policy_refuses_build_options_and_other_guide_stays_unselected(selected):
    assert selected.host.initial_build_options("another-guide") == {}
    selected.policy.path.write_bytes(selected.policy.read() + b" ")
    with pytest.raises(CustodyError):
        selected.host.initial_build_options("original-guide")


def replacement_host(selected, supplier):
    return ForgeApplicationHost(guide_id="original-guide", runtime_target=selected.target,
        acquisition_selection=selected.acquisition, viewer_selection=selected.viewer,
        require_current=lambda: None, replacement_selection=supplier)


def replacement_envelope(selected):
    pin = selected.policy.pin()
    return {"kind": "replacement", "selection_file": pin, "prior_tool_policy": pin,
        "preparation_plan": pin, "gateway": pin,
        "association_directory": str(selected.root / "associations"), "expected_revision": 7}


@pytest.mark.parametrize("configured", [False, True])
def test_helperless_replacement_refuses_before_reading_a_supplier(selected, configured):
    from launchkit.connect_installer_caller import HELPERLESS_UPDATE_UNAVAILABLE
    selected = select_member_classes(selected, helperless=True)
    before = selected.inventory.read()

    def supplier(*args):
        pytest.fail("Helperless replacement cannot enter the retained supplier.")

    host = replacement_host(selected, supplier if configured else None)
    assert host._selected_replacement("another-guide", "installation", "update") is None
    with pytest.raises(CustodyError) as refused:
        host._selected_replacement("original-guide", "installation", "update")
    assert str(refused.value) == HELPERLESS_UPDATE_UNAVAILABLE
    assert selected.inventory.read() == before


def test_helperless_registry_update_refuses_before_opening_the_update_transaction(selected):
    from launchkit.connect_installer_caller import HELPERLESS_UPDATE_UNAVAILABLE
    from launchkit.local_application_registry import LocalApplicationRegistry
    selected = select_member_classes(selected, helperless=True)

    def forbidden(*args):
        pytest.fail("Helperless Apply cannot open a transaction or launch an update worker.")

    registry = SimpleNamespace(deployment=SimpleNamespace(_directory=forbidden),
        _reconnect_status=lambda *args: None, selected_host=lambda *args: selected.host)
    with pytest.raises(CustodyError) as refused:
        LocalApplicationRegistry._no_replacement(registry, "original-guide", "installation", "update")
    assert str(refused.value) == HELPERLESS_UPDATE_UNAVAILABLE


async def async_replacement(*args):
    pytest.fail("An asynchronous supplier must never be invoked.")


async def async_generator_replacement(*args):
    pytest.fail("An asynchronous generator supplier must never be invoked.")
    yield None


class AsyncReplacementCallable:
    async def __call__(self, *args):
        pytest.fail("An asynchronous callable must never be invoked.")


@pytest.mark.parametrize("supplier", [object(), async_replacement, async_generator_replacement,
    AsyncReplacementCallable(), lambda guide, deployment: None])
def test_replacement_constructor_refuses_malformed_or_async_supplier(selected, supplier):
    with pytest.raises(CustodyError, match="supplier|exact guide"):
        replacement_host(selected, supplier)


def test_installed_replacement_selection_does_not_read_initial_acquisition_currentness(selected, monkeypatch):
    expected = replacement_envelope(selected)
    pin = put(selected.root / "replacement-envelope.json", expected)
    calls = []
    def supplier(*ids):
        calls.append(ids)
        return pin
    host = replacement_host(selected, supplier)
    monkeypatch.setattr(host, "require_current", lambda: pytest.fail("Initial admission cannot authorize replacement."))
    assert host._selected_replacement("another-guide", "original-installation", "original-update") is None
    assert calls == []
    result = host._selected_replacement("original-guide", "original-installation", "original-update")
    assert result == expected
    assert calls == [("original-guide", "original-installation", "original-update")]
    result["expected_revision"] = 99
    assert json.loads(pin.read()) == expected


@pytest.mark.parametrize("change", ["fresh", "extra", "revision", "public-parent", "public-file", "bytes"])
def test_installed_replacement_refuses_wrong_envelope_or_changed_private_bytes(selected, change):
    value = replacement_envelope(selected)
    if change == "fresh":
        value["kind"] = "fresh"
    elif change == "extra":
        value["inferred_authority"] = {}
    elif change == "revision":
        value["expected_revision"] = True
    parent = selected.root / "replacement-inputs"
    pin = put(parent / "selection.json", value)
    if change == "public-parent":
        parent.chmod(0o755)
    elif change == "public-file":
        pin.path.chmod(0o644)
    elif change == "bytes":
        pin.path.write_bytes(pin.read() + b" ")
    host = replacement_host(selected, lambda *args: pin)
    with pytest.raises(CustodyError):
        host._selected_replacement("original-guide", "original-installation", "original-update")


def test_installed_replacement_rechecks_exact_envelope_after_decoding(selected, monkeypatch):
    pin = put(selected.root / "replacement-envelope.json", replacement_envelope(selected))
    host = replacement_host(selected, lambda *args: pin)
    original = ExactFile.read
    reads = []
    def changed(self, **options):
        result = original(self, **options)
        if self == pin and not reads:
            reads.append(True)
            pin.path.write_bytes(result + b" ")
        return result
    monkeypatch.setattr(ExactFile, "read", changed)
    with pytest.raises(CustodyError, match="selected bytes"):
        host._selected_replacement("original-guide", "original-installation", "original-update")
    assert reads == [True]


def test_sync_replacement_supplier_returning_coroutine_is_closed_without_execution(selected):
    returned = []
    async def never_run():
        pytest.fail("A returned coroutine cannot execute during replacement selection.")
    def supplier(*args):
        value = never_run()
        returned.append(value)
        return value
    host = replacement_host(selected, supplier)
    with pytest.raises(CustodyError, match="exact private replacement envelope"):
        host._selected_replacement("original-guide", "original-installation", "original-update")
    assert len(returned) == 1 and inspect.getcoroutinestate(returned[0]) == inspect.CORO_CLOSED


@pytest.mark.parametrize("returned", [None, {"kind": "replacement"}, ExactFile("/not-a-path-object", "a" * 64)])
def test_installed_replacement_supplier_cannot_return_untyped_or_malformed_pin(selected, returned):
    host = replacement_host(selected, lambda *args: returned)
    with pytest.raises(CustodyError, match="exact private replacement envelope"):
        host._selected_replacement("original-guide", "original-installation", "original-update")


def test_logical_console_declaration_is_exact_and_never_claims_authentication(selected):
    from launchkit.templates_v2.console_auth import ISSUER
    human = selected.host.console_operator_declaration()
    assert human == {"kind": "human", "id": "inert-selected-operator", "qualification": {
        "kind": "qualified", "trust_domain_id": "oidc-issuer:" + ISSUER}}
    human["id"] = "mutated-copy"
    assert selected.host.console_operator_declaration()["id"] == "inert-selected-operator"
    selected.host.validate_settings("original-guide", {"CONSOLE_USER_ID": "inert-selected-operator"})
    selected.host.validate_settings("original-guide", {})
    selected.host.validate_settings("original-guide", {"CONSOLE_USER_ID": ""})
    selected.host.validate_settings("another-guide", {"CONSOLE_USER_ID": "other"})
    with pytest.raises(DraftConflict, match="Console user"):
        selected.host.validate_settings("original-guide", {"CONSOLE_USER_ID": "other"})


def startup(selected, kind):
    policy = selected.policy.pin()
    common = {"kind": kind, "gateway": policy, "preparation_plan": policy}
    if kind == "fresh":
        common.update(source_policy=policy, activation_template=policy,
            provision_attempt_path=str(selected.root / "attempt"), working_directory=str(selected.root / "work"),
            bundle_path=str(selected.root / "bundle"), activation_path=str(selected.root / "activation"))
    else:
        common.update(selection_file=policy, completed_setup=policy)
    pin = put(selected.root / (kind + "-startup.json"), common)
    selected.host._startup_selection = lambda guide, deployment: pin
    return common


def test_finish_action_uses_existing_start_owner_and_never_save_settings(selected):
    expected = startup(selected, "restore")
    calls = []
    async def start(guide, deployment, **options):
        calls.append((guide, deployment, options))
        return {"status": "queued"}
    result = asyncio.run(selected.host.start_application(SimpleNamespace(start_application=start), "original-guide", "original-installation"))
    assert result == {"status": "queued"}
    assert calls == [("original-guide", "original-installation", {"application_startup": expected})]


def test_same_bytes_at_other_policy_locator_cannot_substitute_original_fresh_source(selected):
    value = startup(selected, "fresh")
    other = put(selected.root / "other-source-policy.json", selected.policy.read())
    value["source_policy"] = other.pin()
    pin = put(selected.root / "changed-startup.json", value)
    selected.host._startup_selection = lambda *args: pin
    with pytest.raises(CustodyError, match="policy"):
        asyncio.run(selected.host.start_application(None, "original-guide", "original-installation"))


def test_readback_reuses_fresh_entry_refusal_and_requires_saved_private_settings(selected):
    startup(selected, "fresh")
    directory = selected.root / "retained-installation"
    directory.mkdir(mode=0o700)
    async def settings(*args):
        return {"deployment_id": "original-installation", "guide_version": 7, "version": 9,
            "build_id": "installed-original-build", "package_sha256": "a" * 64, "source_digest": "b" * 64,
            "fields": [{"key": "PASSWORD", "label": "Password", "required": True, "configured": False}]}
    deployment = SimpleNamespace(settings_options=settings, _directory=lambda *args: directory)
    pending = asyncio.run(selected.host.setup_status(deployment, "original-guide", "original-installation"))
    assert pending["available"] is False and pending["missing_fields"] == ["Password"]
    assert pending["scope"] == "selected-stopped-application-inputs"
    assert pending["reason"] == "private_settings_missing"
    assert pending["guide_version"] == 7 and pending["build_id"] == "installed-original-build"
    put(directory / "application-bootstrap-entry.json", {"opaque_original_attempt": "unsettled"})
    uncertain = asyncio.run(selected.host.setup_status(deployment, "original-guide", "original-installation"))
    assert uncertain["available"] is False and "Restore" in uncertain["detail"]
    assert uncertain["reason"] == "previous_attempt"
    assert uncertain["scope"] == "selected-stopped-application-inputs"


def test_clean_stopped_restore_readback_is_available_without_replaying_fresh(selected):
    startup(selected, "restore")
    directory = selected.root / "retained-installation"
    directory.mkdir(mode=0o700)
    put(directory / "application-bootstrap-entry.json", {"opaque_original_attempt": "retained"})
    async def settings(*args):
        return {"deployment_id": "original-installation", "guide_version": 7, "version": 9,
            "build_id": "installed-original-build", "package_sha256": "a" * 64, "source_digest": "b" * 64,
            "fields": []}
    deployment = SimpleNamespace(settings_options=settings, _directory=lambda *args: directory)
    result = asyncio.run(selected.host.setup_status(deployment, "original-guide", "original-installation"))
    assert result["available"] is True and result["kind"] == "restore"
    assert result["label"] == "Start installed application"
    assert result["scope"] == "selected-stopped-application-inputs"
    assert result["guide_id"] == "original-guide" and result["guide_version"] == 7
    assert not {"principal", "policy", "gateway", "source_policy", "application_name"} & result.keys()


def test_readback_preserves_installed_binding_when_host_has_no_startup_selection(selected):
    async def settings(*args):
        return {"deployment_id": "original-installation", "guide_version": 7, "version": 9,
            "build_id": "installed-original-build", "package_sha256": "a" * 64, "fields": []}
    result = asyncio.run(selected.host.setup_status(SimpleNamespace(settings_options=settings),
        "original-guide", "original-installation"))
    assert result["reason"] == "host_unavailable" and result["available"] is False
    assert result["build_id"] == "installed-original-build" and result["package_sha256"] == "a" * 64


@pytest.mark.parametrize("carrier", ["no-attribute", "ad-hoc-matching-attribute"])
def test_host_factory_refuses_configured_custody_for_a_real_application_host(selected, tmp_path, monkeypatch,
        carrier):
    """A real application host has no application model owner that the factory entry could check.

    Its owner comes from its preparation inputs, and the server's host selection
    does not read any model attribute. So configured custody refuses before the
    server is selected, even when an ad hoc attribute matches its selection.
    """
    from launchkit import application_host_launch as launch, packaged_runtime
    from test_application_host_launch import model_owner
    owner = model_owner(tmp_path, monkeypatch)
    for name in ("_selected_target", "_application_model_custody"):
        monkeypatch.setattr(packaged_runtime, name, None)
    packaged_runtime.configure_application_model_custody(owner)
    host = selected.host
    assert type(host) is ForgeApplicationHost
    if carrier == "ad-hoc-matching-attribute":
        host.model_configuration = owner.selection()
    selections = []
    server = SimpleNamespace(app=object(), select_application_host=selections.append)
    load = launch.importlib.import_module
    monkeypatch.setenv(launch.FACTORY_ENV, "trusted_operator.selection:make_host")
    monkeypatch.setattr(launch.importlib, "import_module", lambda name: (
        SimpleNamespace(make_host=lambda: host) if name == "trusted_operator.selection"
        else server if name == "launchkit.server" else load(name)))
    with pytest.raises(CustodyError, match="cannot reach the selection"):
        launch.create_app()
    assert selections == []
