"""Actual local factory/Install/preparation joins with inert external boundaries.

The reused complete package fixture retains synthetic successful worker exit
and inventory stdout. Production readers authenticate those original bytes;
no SDK, gateway, provider or native setup is executed or qualified here.
The real GuideStore is disposable and belongs only to this fixture account.
"""
from copy import deepcopy
import hashlib
import inspect
import json
from pathlib import Path
import shutil
from uuid import UUID

import pytest

from launchkit import deployment, deployment_worker, guide_store
from launchkit import local_application_source
from launchkit.blueprint import Blueprint
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.deployment_ports import installed_console_port
from launchkit.forge_application_host import ApplicationPreparationInputs, ForgeApplicationHost
from launchkit.guide_store import GuideStore
from launchkit.local_application_factory import LocalApplicationFactory
from test_forge_application_host import put
import test_forge_application_host as source_fixture
from test_product_install_retention import (retaining, finished, prepared,
    implementation_service, workspace, selected, product, no_product_execution,
    no_processes)


@pytest.fixture
def local(retaining, tmp_path, monkeypatch):
    f = retaining
    f.inert_inventory_command = f.supervisor.command
    f.store = GuideStore(tmp_path / "actual-local-guide-store")
    # The normal store owns creation/version/operation semantics. Fix only its
    # disposable UUID to the installation fixture's original admitted guide.
    with monkeypatch.context() as patch:
        patch.setattr(guide_store, "uuid4", lambda: UUID(f.guide_id))
        f.saved = f.store.create({"proposal": deepcopy(f.spec), "status": "refining",
            "stage": "purpose", "messages": []})
    assert f.saved["id"] == f.guide_id
    f.setup_root = f.destination / ".mobkit/forge-local-setup"
    f.installed_port = installed_console_port(f.destination, f.spec["advanced"]["port"])
    f.console_choice = {"origin": "http://127.0.0.1:" + str(f.installed_port),
        "access_management": "requester_issued_leaves"}

    def select(console_setup_selection=None):
        f.factory = LocalApplicationFactory(guide_store=f.store, guide_id=f.guide_id,
            runtime_target=f.target, acquisition_selection=f.acquisition,
            viewer_selection=f.viewer, console_setup_selection=console_setup_selection)
        assert inspect.signature(f.factory).bind().arguments == {}
        f.host = f.factory()
        assert type(f.host) is ForgeApplicationHost
        f.owner = f.factory.preparation_owner
        f.service.application_setup_retention = f.host.retain_installation_inputs
        f.service.application_setup_selection = f.host._selected_startup
        f.service.application_setup_preparation = f.host.prepare_application
        return f.host

    f.select = select
    return f


def source_bytes(root):
    return {path.relative_to(root).as_posix(): path.read_bytes()
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


async def retain(f):
    await finished(f)
    f.inputs = f.owner.preparation_inputs(f.guide_id, f.directory.name)
    assert type(f.inputs) is ApplicationPreparationInputs
    assert f.inputs is f.host._retained_preparation_inputs
    assert f.inputs.supervisor is f.host._installation_supervisor
    assert type(f.inputs.supervisor) is deployment_worker.Supervisor
    assert f.inputs.supervisor.child is None and not f.inputs.supervisor.stop.is_set()
    assert f.service.tasks == {} and f.service.processes == {}
    return f.inputs


async def prepare(f, monkeypatch):
    monkeypatch.setattr(f.inputs.supervisor, "command", f.inert_inventory_command)
    async with f.service.lock:
        return await f.host.prepare_application(f.service, f.guide_id, f.directory.name)


@pytest.mark.parametrize("explicit", [False, True])
async def test_local_factory_retains_install_owner_and_read_only_setup(local, monkeypatch, explicit):
    f = local
    console = put(f.root / "console-setup.json", f.console_choice) if explicit else None
    f.select(console)
    assert f.factory() is f.host and not f.setup_root.exists()
    inputs = await retain(f)
    choices = json.loads(f.owner._choices_pin.read())
    assert choices["guide_id"] == f.guide_id and choices["deployment_id"] == f.directory.name
    assert choices["console_setup"] == f.console_choice
    assert choices["console_setup_source"] == (console.pin() if explicit else None)
    # Discovery, source inspection and protected human review share this budget.
    # The native installed-setup default is one hour, not a request timeout.
    assert choices["limits"]["setup_timeout_ms"] == 3_600_000
    assert inputs.supervisor.directory == f.directory and inputs.supervisor.destination == f.destination
    before = source_bytes(f.setup_root), (f.destination / ".env").read_bytes(), console.read() if console else None

    def no_constructor(*args, **kwargs):
        pytest.fail("Setup GET cannot construct another Supervisor.")
    async def no_inventory(*args, **kwargs):
        pytest.fail("Setup GET cannot enter native inventory.")
    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", no_constructor)
    monkeypatch.setattr(inputs.supervisor, "command", no_inventory)
    for _ in range(2):
        status = await f.host.setup_status(f.service, f.guide_id, f.directory.name)
        assert status["available"] and status["action"] == "prepare-setup"
        assert f.owner.preparation_inputs(f.guide_id, f.directory.name) is inputs
        with pytest.raises(FileNotFoundError):
            f.owner.startup_selection(f.guide_id, f.directory.name)
    assert before == (source_bytes(f.setup_root), (f.destination / ".env").read_bytes(), console.read() if console else None)
    assert not f.calls and not inputs.inventory_attempt_path.exists()
    assert not inputs.preparation_plan_path.exists() and not inputs.startup_selection_path.exists()
    with pytest.raises(CustodyError, match="only once"):
        async with f.service.lock:
            f.host.retain_installation_inputs(f.service, f.guide_id, f.directory.name)
    assert f.host._installation_supervisor is inputs.supervisor


@pytest.fixture(params=["public-only", "new-default", "wrong-member"])
def policy_local(request, monkeypatch):
    mode = request.param
    if mode != "public-only":
        original_put = source_fixture.put

        def selected_policy(path, value):
            if path.name == "tool-policy.json":
                spec = Blueprint.model_validate(json.loads((path.parent / "blueprint.json").read_bytes()))
                inventory = json.loads((path.parent / "application-inventory-input.json").read_bytes())
                value = json.loads(local_application_source._initial_policy(spec, inventory, "fixture-guide"))
                if mode == "wrong-member":
                    for row in value["members"]:
                        if row["member_identity"] == inventory["helper_identity"]:
                            row["grants"] = [grant for grant in row["grants"]
                                if grant["tool_name"] != "toolkit_request_service_account"]
                        else:
                            row["grants"].append({"tool_name": "toolkit_request_service_account",
                                "action": "invoke", "consequence": "r1"})
            return original_put(path, value)

        # Author the exact policy before the original fixture selects files or
        # constructs its host. Never rewrite an already admitted source pin.
        monkeypatch.setattr(source_fixture, "put", selected_policy)
    f = request.getfixturevalue("local")
    f.account_mode = mode
    return f


async def test_actual_inventory_becomes_finite_source_choices_and_same_fresh_pin(policy_local, monkeypatch):
    f = policy_local
    f.select()
    human = f.host.console_operator_declaration()
    selected_humans = []
    original_human = f.host.console_operator_declaration
    def chosen_human():
        value = original_human()
        selected_humans.append(deepcopy(value))
        return value
    monkeypatch.setattr(f.host, "console_operator_declaration", chosen_human)
    await retain(f)
    pin = await prepare(f, monkeypatch)
    plan = json.loads(f.inputs.preparation_plan_path.read_bytes())
    native = plan["owner"]["native_host"]
    assert selected_humans and all(value == human for value in selected_humans)
    assert native["realm"] == f.actual["realm_id"] and native["remote_operator"] == human
    assert native["grant_administrator"]["kind"] == "runtime_host"
    assert native["grant_administrator"] != human
    assert [row["member"] for row in native["members"]] == [row["member"] for row in f.actual["members"]]
    assert [row["logical_owner"]["id"] for row in native["members"]] == [row["identity"] for row in f.actual["members"]]
    assert all(row["logical_owner"]["kind"] == "shared_agent" for row in native["members"])
    assert plan["application"]["roster"] == json.loads(f.inventory.read())["roster"]
    assert plan["application"]["definition"]["text"] == f.definition.read().decode()
    helper = plan["application"]["helper"]
    assert helper["operation_basis"] == {"kind": "targeted_grant_lineages", "base": None, "targets": []}
    assert helper["controller_parent"] == [] and helper["represented_subject"] == human
    assert all(route["application_parent"] is None and route["controller_parent"] is None for route in native["routes"])
    assert all(route["origin"]["requester"] == human and route["represented_subject"] == human for route in native["routes"])
    assert "controller_route_key" not in json.dumps(plan)
    report = plan["authorities"]["helper_report"]
    actual_helper = next(row["member"] for row in f.actual["members"]
        if row["identity"] == f.host._inventory["helper_identity"])
    assert report["member"] == actual_helper == helper["member"]
    if f.account_mode == "new-default":
        assert report["account_issuance_id"] == "forge:" + f.directory.name + ":helper-service-account"
        assert len({report[name] for name in ("issuance_id", "public_issuance_id", "account_issuance_id")}) == 3
    else:
        assert "account_issuance_id" not in report
    targets = [None, {"kind": "mcp_server", "server": "toolkit-setup"},
        {"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
            "operation": "toolkit_request_public_service"}]
    if f.account_mode == "new-default":
        targets.append({"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
            "operation": "toolkit_request_service_account"})
    targets.append({"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_results",
        "operation": "setup_report"})
    requests = plan["authorities"]["agent_setup"]
    assert requests and {json.dumps(row["member"], sort_keys=True) for row in requests} == {
        json.dumps(route["origin"]["member"], sort_keys=True) for route in native["routes"]}
    for request in requests:
        assert [row["target"] for row in request.get("helper_targets", [])] == targets
    def issuance_ids(value):
        if isinstance(value, dict):
            for key, child in value.items():
                if key.endswith("issuance_id"):
                    yield child
                else:
                    yield from issuance_ids(child)
        elif isinstance(value, list):
            for child in value:
                yield from issuance_ids(child)
    issued = list(issuance_ids(plan["authorities"]))
    assert len(issued) == len(set(issued))
    assert "standing_policy" not in plan["acquisition"] and "service_parent_administration" not in native
    assert plan["owner"]["components"] == [] and plan["owner"]["channels"] == []
    assert plan["startup"] == {"kind": "initialize", "namespace": f.directory.name, "generation": 1}
    assert plan["acquisition"]["console_setup"] == f.console_choice
    assert plan["acquisition"]["limits"]["setup_timeout_ms"] == 3_600_000
    assert plan["acquisition"]["proposal_ceiling"]["services"] == [] and plan["acquisition"]["proposal_ceiling"]["frontends"] == []
    assert plan["application"]["tool_policy"]["members"][0]["grants"] == json.loads(f.policy.read())["members"][0]["grants"]
    assert len(f.calls) == 1
    assert f.owner.startup_selection(f.guide_id, f.directory.name) == pin
    assert f.owner.startup_selection(f.guide_id, f.directory.name) is f.owner._fresh_pin
    value = json.loads(pin.read())
    assert value["kind"] == "fresh" and value["activation_template"] == f.inputs.activation_template.pin()
    assert value["preparation_plan"] == ExactFile(f.inputs.preparation_plan_path,
        hashlib.sha256(f.inputs.preparation_plan_path.read_bytes()).hexdigest()).pin()
    assert deployment.read_receipt(f.directory)["status"] == "stopped"
    for name in ("provision_attempt_path", "bundle_path", "activation_path"):
        assert not Path(f.inputs.fresh_paths[name]).exists()
    assert not (f.directory / "application-bootstrap-entry.json").exists()
    pin.path.write_bytes(pin.read() + b" ")
    with pytest.raises(CustodyError):
        f.owner.startup_selection(f.guide_id, f.directory.name)
    assert len(f.calls) == 1


@pytest.mark.parametrize("choice", ["unknown-field", "wrong-mode", "non-loopback", "pin-drift",
    "wrong-recovery", "null-recovery", "recovery-actor", "recovery-wrong-access"])
async def test_invalid_explicit_console_source_refuses_before_install_retention(local, choice):
    f = local
    value = deepcopy(f.console_choice)
    if choice == "unknown-field":
        value["unselected"] = True
    elif choice == "wrong-mode":
        value["access_management"] = "host_issued"
    elif choice == "non-loopback":
        value["origin"] = "http://example.invalid:" + str(f.installed_port)
    elif choice == "wrong-recovery":
        value["recovery_management"] = "requester_issued_leaves"
    elif choice == "null-recovery":
        value["recovery_management"] = None
    elif choice == "recovery-actor":
        value["recovery_management"] = {"operator": "another-user"}
    elif choice == "recovery-wrong-access":
        value["recovery_management"] = "operator_unpublished_service_approvals"
        value["access_management"] = "host_issued"
    pin = put(f.root / "console-setup.json", value)
    if choice == "pin-drift":
        pin.path.write_bytes(pin.read() + b" ")
    original = (f.destination / ".env").read_bytes(), (f.directory / "receipt.json").read_bytes()
    with pytest.raises(CustodyError):
        f.select(pin)
    assert not f.setup_root.exists() and f.service.tasks == {} and f.service.processes == {}
    assert original == ((f.destination / ".env").read_bytes(), (f.directory / "receipt.json").read_bytes())
    assert not f.calls


async def test_explicit_console_port_mismatch_refuses_before_private_source_publication(local):
    f = local
    value = {**f.console_choice, "origin": "http://127.0.0.1:" + str(1024 if f.installed_port != 1024 else 1025)}
    pin = put(f.root / "console-setup.json", value)
    f.select(pin)
    private_before = (f.destination / ".env").read_bytes()
    with pytest.raises(CustodyError, match="installed Console port"):
        await finished(f)
    assert not f.setup_root.exists() and not f.calls
    assert f.host._installation_supervisor is not None and f.host._retained_preparation_inputs is None
    assert private_before == (f.destination / ".env").read_bytes() and pin.read() == put_bytes(value)
    assert deployment.read_receipt(f.directory)["status"] == "stopped"


def put_bytes(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


async def test_same_normalized_plan_acceptance_advances_without_replacing_host(local):
    f = local
    f.select()
    original = f.host
    saved = f.store.get(f.guide_id)
    saved["status"], saved["stage"] = "accepted", "review"
    newer = f.store.save(saved, saved["version"])
    assert newer["version"] > saved["version"]
    assert f.factory() is original
    await retain(f)
    assert f.owner.preparation_inputs(f.guide_id, f.directory.name) is f.inputs


@pytest.mark.parametrize("change", ["proposal", "running-operation"])
async def test_installed_inputs_survive_live_guide_changes_without_preparation(local, change):
    f = local
    f.select()
    await retain(f)
    original_host, original_inputs = f.host, f.inputs
    before = source_bytes(f.setup_root), source_bytes(f.root)
    if change == "proposal":
        state = f.store.get(f.guide_id)
        state["proposal"]["project"]["name"] += " changed"
        f.store.save(state, state["version"])
    else:
        f.store.save_operation(f.guide_id, {"status": "running"})
    assert f.factory() is original_host
    assert f.owner.preparation_inputs(f.guide_id, f.directory.name) is original_inputs
    assert not f.calls and (source_bytes(f.setup_root), source_bytes(f.root)) == before


@pytest.mark.parametrize("changed", ["choices", "template", "policy"])
async def test_retained_source_change_refuses_before_inventory(local, changed):
    f = local
    f.select()
    await retain(f)
    selected = {"choices": f.owner._choices_pin, "template": f.inputs.activation_template,
        "policy": f.policy}[changed]
    selected.path.write_bytes(selected.read() + b" ")
    with pytest.raises(CustodyError):
        f.owner.preparation_inputs(f.guide_id, f.directory.name)
    assert not f.calls and not f.inputs.startup_selection_path.exists()


@pytest.mark.parametrize("changed", ["missing-db", "replaced-db", "missing-root", "replaced-root"])
async def test_original_guide_store_loss_never_recreates_or_reselects_owner(local, changed):
    f = local
    f.select()
    await retain(f)
    root, database = f.store.root, f.store.root / "guided-setups.sqlite3"
    if changed.endswith("db"):
        original = database.read_bytes()
        database.rename(root / "original-database.sqlite3")
        if changed == "replaced-db":
            database.write_bytes(original)
            database.chmod(0o600)
    else:
        old = root.with_name(root.name + "-original")
        root.rename(old)
        if changed == "replaced-root":
            shutil.copytree(old, root)
    before = source_bytes(root) if root.exists() else None
    for read in (f.factory, lambda: f.owner.preparation_inputs(f.guide_id, f.directory.name),
            lambda: f.owner.startup_selection(f.guide_id, f.directory.name)):
        with pytest.raises(CustodyError):
            read()
        assert (source_bytes(root) if root.exists() else None) == before
        if changed == "missing-db":
            assert not database.exists()
        if changed == "missing-root":
            assert not root.exists()
    assert not f.calls and not f.inputs.startup_selection_path.exists()


async def test_explicit_operator_recovery_choice_is_retained_by_normal_prepare(local, monkeypatch):
    f = local
    # Select a new protected source before constructing the actual factory.
    # Never edit generated native output or an already accepted source pin.
    selected = {**f.console_choice,
        "recovery_management": "operator_unpublished_service_approvals"}
    console = put(f.root / "console-setup-recovery.json", selected)
    original = console.read(), f.viewer.read(), f.acquisition.read()
    f.select(console)
    assert not f.setup_root.exists() and not f.calls
    await retain(f)
    choices = json.loads(f.owner._choices_pin.read())
    assert choices["console_setup"] == selected
    assert choices["console_setup_source"] == console.pin()
    assert choices["console_operator"] == f.host.console_operator_declaration()
    pin = await prepare(f, monkeypatch)
    plan_bytes = f.inputs.preparation_plan_path.read_bytes()
    plan = json.loads(plan_bytes)
    assert plan["acquisition"]["console_setup"] == selected
    assert plan["owner"]["native_host"]["remote_operator"] == f.host.console_operator_declaration()
    assert json.loads(pin.read())["preparation_plan"] == ExactFile(
        f.inputs.preparation_plan_path, hashlib.sha256(plan_bytes).hexdigest()).pin()
    assert original == (console.read(), f.viewer.read(), f.acquisition.read())
    assert len(f.calls) == 1 and f.service.tasks == {} and f.service.processes == {}
    # The original source pin remains authoritative even after preparation.
    console.path.write_bytes(console.read() + b" ")
    with pytest.raises(CustodyError):
        f.factory.require_current()
    assert len(f.calls) == 1
