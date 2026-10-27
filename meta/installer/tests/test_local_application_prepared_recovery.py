"""Prepared local ownership survives loss of the original Forge objects.

Canonical Install, independently signed Build readers, inventory receipt
parsing, preparation, finalizer publication and recovery readers are real.
Inventory stdout, native completed outputs and SDK proof are explicit inert
boundaries. No native grants, runtime success or useful job is qualified.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_application_bootstrap as bootstrap
from launchkit import deployment_worker, runtime_dependencies
from launchkit.connect_installer_caller import CustodyError
from launchkit.deployment import DeploymentService
from launchkit.guide import GuideService
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.runtime_selection import selected_runtime_artifacts
from launchkit.templates import install_candidate as adapter
from launchkit.templates.storage_setup import project_lock
from test_forge_application_host import encoded
from test_local_application_recovery import CALLBACKS, disk_bytes
from test_local_application_registry import (accept_source_fixture, build,
    install_fixture, new_guide, no_processes, no_product_execution, product,
    registry, source_bytes)
from test_local_application_restore import finalize_original
import test_local_application_restore as restore_fixtures


async def unavailable_service_query(arguments, *, allow_failure, capture_output, timeout=None):
    """Only the read-only OS capability query is an external inert boundary."""
    assert allow_failure is True and capture_output is False
    assert (len(arguments) == 3 and arguments[1] == "print"
        or arguments[1:] == ["--user", "show", "--property=Version"])
    return 1, ""


@pytest.fixture
async def prepared_recovery(registry, monkeypatch, request):
    original = registry
    legacy_account_policy = request.param == "legacy-account-policy-restore"
    paired_account_policy = request.param == "paired-account-parent-restore"
    successor_catalog = request.param == "successor-catalog-restore"
    legacy_budget = request.param.startswith("legacy-budget-")
    state = (request.param.removeprefix("legacy-budget-") if legacy_budget else
        "restore" if legacy_account_policy or paired_account_policy or successor_catalog else request.param)
    from launchkit import local_application_source
    current_initial_policy = local_application_source._initial_policy

    def legacy_public_policy(spec, inventory, guide_id):
        value = json.loads(current_initial_policy(spec, inventory, guide_id))
        for row in value["members"]:
            if row["member_identity"] == inventory["helper_identity"]:
                row["grants"] = [grant for grant in row["grants"]
                    if grant["tool_name"] != "toolkit_request_service_account"]
        return local_application_source._json_bytes(value)

    # These historical choices are authored before original admission. The
    # custom legacy branch selects the real account-entry tool but deliberately
    # omits its independent parent request in the original declarations.
    production_declarations = LocalApplicationPreparationOwner.declarations
    legacy_declaration_calls = []
    legacy_declarations_patch = None
    if legacy_account_policy:
        def legacy_declarations(owner, inventory):
            choices = production_declarations(owner, inventory)
            report = choices["authorities"]["helper_report"]
            account = report.pop("account_issuance_id")
            assert account not in {report["issuance_id"], report["public_issuance_id"]}
            # Preserve the actual older declaration shape before contributor
            # target requests existed, rather than upgrading its retained plan.
            for configured in choices["authorities"]["agent_setup"]:
                configured.pop("helper_targets", None)
            legacy_declaration_calls.append(account)
            return choices
        legacy_declarations_patch = pytest.MonkeyPatch()
        request.addfinalizer(legacy_declarations_patch.undo)
        legacy_declarations_patch.setattr(LocalApplicationPreparationOwner,
            "declarations", legacy_declarations)
    elif not paired_account_policy:
        # Preserve the existing explicitly public-only source branch.
        monkeypatch.setattr(local_application_source, "_initial_policy", legacy_public_policy)
    if successor_catalog:
        def successor_declarations(owner, inventory):
            choices = production_declarations(owner, inventory)
            choices["authorities"]["helper_catalog"]["scope"] = "current_helper_successors"
            choices["authorities"]["helper_catalog"]["issuance_id"] += ":successors"
            for configured in choices["authorities"]["agent_setup"]:
                configured["issuance_id"] += ":successors"
                for target in configured["helper_targets"]:
                    if target["target"] is None:
                        target["issuance_id"] += ":successors"
            return choices
        legacy_declarations_patch = pytest.MonkeyPatch()
        request.addfinalizer(legacy_declarations_patch.undo)
        legacy_declarations_patch.setattr(LocalApplicationPreparationOwner,
            "declarations", successor_declarations)
    saved = new_guide(original)
    installed = await build(original, saved)
    await accept_source_fixture(original, installed)
    host = original.registry.host_for_build(saved["id"], installed["build_id"])
    with monkeypatch.context() as initial_choices_patch:
        if legacy_budget:
            production_choices = LocalApplicationPreparationOwner._initial_choices
            def original_short_budget(owner, *args, **kwargs):
                choices = production_choices(owner, *args, **kwargs)
                choices["limits"]["setup_timeout_ms"] = 180_000
                return choices
            # Author the historical policy before its original Install admits
            # any files. Recovery runs today's unpatched production reader.
            initial_choices_patch.setattr(LocalApplicationPreparationOwner,
                "_initial_choices", original_short_budget)
        directory, destination = await install_fixture(original, saved, installed, monkeypatch)
    owner = host._preparation_inputs.__self__
    assert type(owner) is LocalApplicationPreparationOwner
    inputs = owner.preparation_inputs(saved["id"], directory.name)
    old_plan = deepcopy(host._blueprint)
    original_inventory = deepcopy(host._inventory)
    actual = {"schema": 1, "mob_id": original_inventory["mob_id"],
        "helper_identity": original_inventory["helper_identity"], "realm_id": "opaque-inert-realm",
        "members": [{"identity": row["identity"], "member": {
            "mob_id": original_inventory["mob_id"], "role": row["profile"],
            "member": "inert-original-native-" + str(index)}}
            for index, row in enumerate(original_inventory["roster"])]}
    inventory_calls = []
    caller = asyncio.current_task()

    async def inventory_command(argv, phase, timeout, *, raw_outcome):
        assert asyncio.current_task() is caller and original.service.lock.locked() and raw_outcome
        assert argv[1:] == ("--lower-application-inventory", "--input", str(host._inventory_pin.path))
        with pytest.raises(RuntimeError, match="already|running"):
            with project_lock(destination):
                pytest.fail("The original preparation must hold its real project lease.")
        inventory_calls.append(tuple(argv))
        return {"returncode": 0, "stdout": encoded(actual), "stderr": b"inert inventory boundary\n"}

    if legacy_account_policy or paired_account_policy:
        selected_helper = next(row for row in json.loads(host._policy_bytes)["members"]
            if row["member_identity"] == original_inventory["helper_identity"])
        assert any(grant["tool_name"] == "toolkit_request_service_account"
            and grant["action"] == "invoke" for grant in selected_helper["grants"])
    monkeypatch.setattr(inputs.supervisor, "command", inventory_command)
    prepared = await original.service.action(saved["id"], directory.name, "prepare-setup",
        action_request_id=str(uuid4()))
    assert prepared["action_request"]["outcome"] == "returned" and len(inventory_calls) == 1
    fresh_pin = owner.startup_selection(saved["id"], directory.name)
    fresh = json.loads(fresh_pin.read())
    setup_root = inputs.startup_selection_path.parent
    policy = json.loads((setup_root / "local-application-policy.json").read_bytes())
    plan = json.loads(inputs.preparation_plan_path.read_bytes())
    assert plan["startup"] == {"kind": "initialize", "namespace": directory.name, "generation": 1}
    assert policy["namespace"] == plan["startup"]["namespace"]
    assert policy["generation"] == plan["startup"]["generation"]
    specimen = SimpleNamespace(host=host, owner=owner, inputs=inputs, directory=directory,
        destination=destination, guide_id=saved["id"], setup_root=setup_root,
        fresh_pin=fresh_pin, fresh=fresh, native_commands=[])
    restore_pin = None
    if state != "fresh":
        if paired_account_policy or successor_catalog:
            original_completed_specimen = restore_fixtures.completed_specimen
            def paired_completed_specimen(selected):
                completed = original_completed_specimen(selected)
                # Explicit inert native output, before its original finalizer
                # admission. No issuer, source plan or reader is replaced.
                if paired_account_policy:
                    completed.setup_value["helper"]["report_capability"]["account_parent"] = [
                        {"inert_native_parent": "original-account-request"}]
                if successor_catalog:
                    helper = completed.setup_value["helper"]
                    helper["catalog_capability"] = {"parent": helper["operation_basis"]["base"]}
                    helper["operation_basis"]["base"] = None
                completed.publish()
                return completed
            with monkeypatch.context() as patch:
                patch.setattr(restore_fixtures, "completed_specimen", paired_completed_specimen)
                await finalize_original(specimen, monkeypatch)
        else:
            await finalize_original(specimen, monkeypatch)
        assert len(specimen.native_commands) == 2
        if state == "restore":
            stopped = await original.service.action(saved["id"], directory.name, "stop",
                action_request_id=str(uuid4()))
            assert stopped["action_request"]["outcome"] == "returned"
            restore_pin = owner.startup_selection(saved["id"], directory.name)
            assert json.loads(restore_pin.read())["kind"] == "restore"
        else:
            assert state == "finalized" and not (setup_root / "restore-selection.json").exists()
    with project_lock(destination):
        bootstrap._observe_inputs(directory, deployment.read_receipt(directory), fresh,
            entered=state != "fresh").require_current()
    fresh_bytes = fresh_pin.read()
    restore_bytes = restore_pin.read() if restore_pin is not None else None
    original_entry = ((directory / bootstrap.FRESH_ENTRY_NAME).read_bytes()
        if state != "fresh" else None)
    if legacy_account_policy:
        assert legacy_declaration_calls
        assert "account_issuance_id" not in plan["authorities"]["helper_report"]
        # Later reconstruction must call production declarations, while the
        # admitted original plan/source/Restore files remain byte-for-byte intact.
        legacy_declarations_patch.undo()
        assert LocalApplicationPreparationOwner.declarations is production_declarations
    if successor_catalog:
        assert plan["authorities"]["helper_catalog"]["scope"] == "current_helper_successors"
        assert all(row["issuance_id"].endswith(":successors") for row in plan["authorities"]["agent_setup"])
        legacy_declarations_patch.undo()
        assert LocalApplicationPreparationOwner.declarations is production_declarations
    changed = original.store.get(saved["id"])
    changed["proposal"]["project"]["name"] += " evolving draft B"
    changed = original.store.save(changed, changed["version"])
    next_build = await build(original, changed, "Implement draft B", previous=installed)
    await accept_source_fixture(original, next_build)
    assert original.implementation.selected_build_id(saved["id"]) == next_build["build_id"]
    await original.implementation.shutdown()

    store = GuideStore(original.store.root)
    guide = GuideService(store, architect=None)
    implementation = ImplementationService(original.implementation.root.parent, guide)
    service = DeploymentService(original.service.root.parent, guide, implementation)
    registry_owner = LocalApplicationRegistry(guide_store=store, implementation=implementation,
        deployment=service, runtime_target=original.target)
    for name, method in CALLBACKS:
        setattr(service, name, getattr(registry_owner, method))
    assert not registry_owner._builds and not registry_owner._installations
    assert not service.tasks and not service.processes
    monkeypatch.setattr(service.service_manager, "command", unavailable_service_query)
    supervisors, accepted_reads, proof_reads = [], [], []
    original_init = deployment_worker.Supervisor.__init__
    original_accept = implementation._accepted_framework
    retained_proof = deepcopy(deployment.read_receipt(directory)["installed_candidate_runtime_proof"])

    def observe_supervisor(supervisor, *args, **kwargs):
        supervisors.append(supervisor)
        original_init(supervisor, *args, **kwargs)

    def observe_acceptance(*args, **kwargs):
        accepted_reads.append(args)
        return original_accept(*args, **kwargs)

    def verify_installed_prefix(selected_destination, expected_version, *, runtime_target, **kwargs):
        assert Path(selected_destination) == destination and runtime_target == original.target
        assert expected_version == original.target.meta.version and not kwargs
        adapter.verify_meta_toolkit_prefix(selected_runtime_artifacts(original.target), deployment_root=destination)
        proof_reads.append(destination)
        return deepcopy(retained_proof)

    async def no_native_command(*args, **kwargs):
        pytest.fail("Recovery must not run inventory, native preparation, Fresh or an Install command.")

    def no_install_retainer(*args, **kwargs):
        pytest.fail("Recovery must reopen original private choices without replaying Install retention.")

    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", observe_supervisor)
    monkeypatch.setattr(deployment_worker.Supervisor, "command", no_native_command)
    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", no_install_retainer)
    monkeypatch.setattr(implementation, "_accepted_framework", observe_acceptance)
    monkeypatch.setattr(runtime_dependencies, "verify_runtime", verify_installed_prefix)
    f = SimpleNamespace(state=state, store=store, guide=guide, implementation=implementation,
        service=service, registry=registry_owner, target=original.target, guide_id=saved["id"],
        directory=directory, destination=destination, setup_root=setup_root, installed=installed,
        old_plan=old_plan, old_host_id=id(host), old_supervisor_id=id(inputs.supervisor),
        original_build=original.implementation._directory(saved["id"], installed["build_id"]),
        fresh_pin=fresh_pin, fresh=fresh, fresh_bytes=fresh_bytes, restore_bytes=restore_bytes,
        original_entry=original_entry, plan=plan, policy=policy, inventory_calls=inventory_calls,
        native_commands=specimen.native_commands, supervisors=supervisors,
        accepted_reads=accepted_reads, proof_reads=proof_reads,
        legacy_account_policy=legacy_account_policy, paired_account_policy=paired_account_policy,
        production_declarations=production_declarations,
        legacy_declaration_calls=legacy_declaration_calls,
        original_tool_policy_bytes=host._policy_bytes)
    yield f
    await implementation.shutdown()
    assert not service.tasks and not service.processes and not implementation.tasks


def retained_bytes(f):
    return (disk_bytes(f.original_build), source_bytes(f.setup_root),
        (f.directory / "package.zip").read_bytes(), (f.destination / ".env").read_bytes(),
        {path.relative_to(f.destination).as_posix(): str(path.readlink())
            for path in f.destination.rglob("*") if path.is_symlink()})


@pytest.mark.parametrize("prepared_recovery", ["fresh", "finalized", "restore", "successor-catalog-restore"], indirect=True)
async def test_claimed_prepare_recovers_original_prepared_selection_without_native_replay(prepared_recovery):
    f = prepared_recovery
    before = retained_bytes(f)
    hint = await f.registry.setup_status(f.service, f.guide_id, f.directory.name)
    assert not hint["available"] and hint["reason"] == "owner_reconnect" and "kind" not in hint
    assert not f.supervisors and not f.accepted_reads and not f.proof_reads
    assert retained_bytes(f) == before
    request_id = str(uuid4())
    result = await f.service.action(f.guide_id, f.directory.name, "prepare-setup", action_request_id=request_id)
    assert result["action_request"]["phase"] == "settled" and result["action_request"]["outcome"] == "returned"
    host = f.registry.selected_host(f.guide_id, f.directory.name)
    assert id(host) != f.old_host_id and host._blueprint == f.old_plan
    assert len(f.supervisors) == 1 and id(f.supervisors[0]) != f.old_supervisor_id
    assert host._installation_supervisor is f.supervisors[0]
    assert f.accepted_reads and all(row[1] == f.installed["build_id"] for row in f.accepted_reads)
    assert f.proof_reads == [f.destination]
    owner = host._preparation_inputs.__self__
    pin = owner.startup_selection(f.guide_id, f.directory.name)
    selected = json.loads(pin.read())
    assert selected["kind"] == ("fresh" if f.state == "fresh" else "restore")
    assert owner._fresh_pin == f.fresh_pin and owner._fresh_pin.read() == f.fresh_bytes
    assert owner._fresh_value == f.fresh
    assert json.loads(owner._choices_pin.read())["namespace"] == f.directory.name
    assert json.loads(owner._choices_pin.read())["generation"] == 1
    assert json.loads(owner._inputs.preparation_plan_path.read_bytes()) == f.plan
    assert "account_issuance_id" not in f.plan["authorities"]["helper_report"]
    helper_policy = next(row for row in json.loads(host._policy_bytes)["members"]
        if row["member_identity"] == host._inventory["helper_identity"])
    assert host._policy_bytes == f.original_tool_policy_bytes
    assert "toolkit_request_service_account" not in {grant["tool_name"] for grant in helper_policy["grants"]}
    if f.state == "fresh":
        assert pin == f.fresh_pin and retained_bytes(f) == before
        assert not (f.directory / bootstrap.FRESH_ENTRY_NAME).exists()
    else:
        bootstrap.require_restore_history_current(f.directory, selected)
        assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.original_entry
        if f.state == "restore":
            assert pin.read() == f.restore_bytes and retained_bytes(f) == before
        else:
            assert "restore-selection.json" not in before[1]
            after = retained_bytes(f)
            assert after[0] == before[0] and after[2:] == before[2:]
            assert {name: data for name, data in after[1].items() if name != "restore-selection.json"} == before[1]
    counts = len(f.supervisors), len(f.accepted_reads), len(f.proof_reads)
    stable = retained_bytes(f), disk_bytes(f.service.root)
    status = await f.registry.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["available"] and status["kind"] == selected["kind"] and "action" not in status
    repeated = await f.service.action(f.guide_id, f.directory.name, "prepare-setup", action_request_id=request_id)
    assert repeated["action_request"] == result["action_request"]
    assert counts == (len(f.supervisors), len(f.accepted_reads), len(f.proof_reads))
    assert (retained_bytes(f), disk_bytes(f.service.root)) == stable
    assert len(f.inventory_calls) == 1 and len(f.native_commands) == (0 if f.state == "fresh" else 2)


@pytest.mark.parametrize("prepared_recovery", ["legacy-budget-fresh", "legacy-budget-restore"], indirect=True)
async def test_recovery_preserves_historical_setup_budget_and_exact_plan(prepared_recovery):
    f = prepared_recovery
    assert f.policy["limits"]["setup_timeout_ms"] == 180_000
    assert f.plan["acquisition"]["limits"]["setup_timeout_ms"] == 180_000
    before = retained_bytes(f)
    result = await f.service.action(f.guide_id, f.directory.name, "prepare-setup",
        action_request_id=str(uuid4()))
    assert result["action_request"]["outcome"] == "returned"
    host = f.registry.selected_host(f.guide_id, f.directory.name)
    owner = host._preparation_inputs.__self__
    assert json.loads(owner._choices_pin.read())["limits"]["setup_timeout_ms"] == 180_000
    assert json.loads(owner._inputs.preparation_plan_path.read_bytes()) == f.plan
    assert retained_bytes(f) == before
    assert len(f.inventory_calls) == 1 and len(f.native_commands) == (0 if f.state == "fresh" else 2)


@pytest.mark.parametrize("prepared_recovery", ["fresh"], indirect=True)
@pytest.mark.parametrize("timeout_ms", [True, 179_999, 3_600_001])
async def test_recovery_refuses_unrecognized_setup_budget_without_rewriting(prepared_recovery, timeout_ms):
    f = prepared_recovery
    path = f.setup_root / "local-application-policy.json"
    policy = json.loads(path.read_bytes())
    policy["limits"]["setup_timeout_ms"] = timeout_ms
    path.write_bytes(encoded(policy))
    before = retained_bytes(f)
    with pytest.raises(CustodyError, match="original local setup policy"):
        await f.service.action(f.guide_id, f.directory.name, "prepare-setup",
            action_request_id=str(uuid4()))
    assert retained_bytes(f) == before
    assert not f.registry._installations
    assert len(f.inventory_calls) == 1 and not f.native_commands


@pytest.mark.parametrize("prepared_recovery", [
    pytest.param("legacy-account-policy-restore", id="legacy-account-policy-restore"),
    pytest.param("paired-account-parent-restore", id="paired-account-parent-restore"),
], indirect=True)
async def test_restore_preserves_original_account_parent_choice_with_explicit_account_tool(prepared_recovery):
    f = prepared_recovery
    assert f.state == "restore" and (f.legacy_account_policy or f.paired_account_policy)
    assert LocalApplicationPreparationOwner.declarations is f.production_declarations
    report = f.plan["authorities"]["helper_report"]
    if f.legacy_account_policy:
        assert f.legacy_declaration_calls and "account_issuance_id" not in report
    else:
        assert not f.legacy_declaration_calls
        assert type(report["account_issuance_id"]) is str and report["account_issuance_id"]
        assert report["account_issuance_id"] not in {report["issuance_id"], report["public_issuance_id"]}
    assert report["member"] == f.plan["application"]["helper"]["member"]
    assert report["issuance_id"] != report["public_issuance_id"]
    before = retained_bytes(f)
    assert not f.supervisors and not f.accepted_reads and not f.proof_reads
    request_id = str(uuid4())
    result = await f.service.action(f.guide_id, f.directory.name, "prepare-setup",
        action_request_id=request_id)
    assert result["action_request"]["phase"] == "settled"
    assert result["action_request"]["outcome"] == "returned"
    host = f.registry.selected_host(f.guide_id, f.directory.name)
    assert id(host) != f.old_host_id and host._blueprint == f.old_plan
    owner = host._preparation_inputs.__self__
    assert owner.declarations.__func__ is f.production_declarations
    assert owner._inputs.declarations.__func__ is f.production_declarations
    helper_policy = next(row for row in json.loads(host._policy_bytes)["members"]
        if row["member_identity"] == host._inventory["helper_identity"])
    assert any(grant["tool_name"] == "toolkit_request_service_account"
        and grant["action"] == "invoke" for grant in helper_policy["grants"])
    recovered_plan = json.loads(owner._inputs.preparation_plan_path.read_bytes())
    assert recovered_plan == f.plan
    assert recovered_plan["authorities"]["helper_report"] == report
    if f.legacy_account_policy:
        assert "account_issuance_id" not in recovered_plan["authorities"]["helper_report"]
    else:
        assert recovered_plan["authorities"]["helper_report"]["account_issuance_id"] == report["account_issuance_id"]
    assert json.loads(owner._choices_pin.read()) == f.policy
    selected = owner.startup_selection(f.guide_id, f.directory.name)
    assert selected.read() == f.restore_bytes
    assert json.loads(selected.read())["kind"] == "restore"
    assert owner._fresh_pin == f.fresh_pin and owner._fresh_pin.read() == f.fresh_bytes
    assert owner._fresh_value == f.fresh
    bootstrap.require_restore_history_current(f.directory, json.loads(selected.read()))
    assert (f.directory / bootstrap.FRESH_ENTRY_NAME).read_bytes() == f.original_entry
    assert retained_bytes(f) == before
    assert len(f.supervisors) == 1 and id(f.supervisors[0]) != f.old_supervisor_id
    assert host._installation_supervisor is f.supervisors[0]
    assert f.accepted_reads and all(row[1] == f.installed["build_id"] for row in f.accepted_reads)
    assert f.proof_reads == [f.destination]
    assert len(f.inventory_calls) == 1 and len(f.native_commands) == 2


@pytest.mark.parametrize("prepared_recovery,changed,detail", [
    pytest.param("fresh", "namespace", "original", id="fresh-namespace"),
    pytest.param("fresh", "inventory-completion", "successful original exited observation",
        id="fresh-inventory-completion"),
    ("restore", "fresh-lineage", "entered original Fresh lineage changed"),
    ("restore", "finalizer-outcome", "did not publish this exact current activation"),
    ("restore", "current", "did not publish this exact current activation"),
    ("restore", "restore-envelope", "retained Restore differs from the actual original native finalization"),
], indirect=["prepared_recovery"])
async def test_claimed_prepare_refuses_changed_retained_preparation_or_history(
        prepared_recovery, changed, detail):
    f = prepared_recovery
    accepted = disk_bytes(f.original_build)
    original_policy = (f.setup_root / "local-application-policy.json").read_bytes()
    inventory_closure = None
    if changed == "inventory-completion":
        attempt = f.setup_root / "inventory.json"
        plan = Path(f.fresh["preparation_plan"]["path"])
        sources = {Path(pin["path"]): Path(pin["path"]).read_bytes()
            for pin in json.loads(attempt.read_bytes())["input_files"][2:]}
        assert len(sources) == 5
        inventory_closure = f.fresh_pin.read(), plan.read_bytes(), attempt.read_bytes(), sources
        path = Path(str(attempt) + ".outcome.json")
        value = json.loads(path.read_bytes())
        assert value["status"] == "receipt_verified" and value["error_type"] is None
        value["status"] = "outcome_unknown"
        path.write_bytes(encoded(value))
        # This substitution leaves the finite bootstrap inputs intact. Only
        # authenticating the original exited inventory observation can refuse.
        with project_lock(f.destination):
            bootstrap._observe_inputs(f.directory, deployment.read_receipt(f.directory), f.fresh).require_current()
    elif changed == "namespace":
        plan_path = Path(f.fresh["preparation_plan"]["path"])
        plan = json.loads(plan_path.read_bytes())
        plan["startup"]["namespace"] = str(uuid4())
        plan_path.write_bytes(encoded(plan))
        fresh = deepcopy(f.fresh)
        fresh["preparation_plan"]["sha256"] = hashlib.sha256(plan_path.read_bytes()).hexdigest()
        f.fresh_pin.path.write_bytes(encoded(fresh))
        # Existing finite source/plan reading accepts this consistent closure.
        # Only the original local declaration/namespace join can refuse it.
        with project_lock(f.destination):
            bootstrap._observe_inputs(f.directory, deployment.read_receipt(f.directory), fresh).require_current()
    elif changed == "fresh-lineage":
        path = f.directory / bootstrap.FRESH_ENTRY_NAME
        value = json.loads(path.read_bytes())
        value["provision_attempt_path"] = str(f.setup_root / "foreign-provision.json")
        path.write_bytes(encoded(value))
    elif changed == "finalizer-outcome":
        path = Path(f.fresh["working_directory"]) / "finalization.outcome.json"
        value = json.loads(path.read_bytes())
        value["status"] = "outcome_unknown"
        path.write_bytes(encoded(value))
    elif changed == "current":
        (f.destination / "current").unlink()
    else:
        path = f.setup_root / "restore-selection.json"
        value = json.loads(path.read_bytes())
        value["gateway"]["sha256"] = "f" * 64
        path.write_bytes(encoded(value))
    before = retained_bytes(f)
    request_id = str(uuid4())
    with pytest.raises(CustodyError, match=detail):
        await f.service.action(f.guide_id, f.directory.name, "prepare-setup", action_request_id=request_id)
    assert f.accepted_reads and all(row[1] == f.installed["build_id"] for row in f.accepted_reads)
    assert not f.registry._builds and not f.registry._installations
    assert retained_bytes(f) == before and disk_bytes(f.original_build) == accepted
    assert (f.setup_root / "local-application-policy.json").read_bytes() == original_policy
    if inventory_closure is not None:
        assert f.fresh_pin.read() == inventory_closure[0]
        assert Path(f.fresh["preparation_plan"]["path"]).read_bytes() == inventory_closure[1]
        assert (f.setup_root / "inventory.json").read_bytes() == inventory_closure[2]
        assert {path: path.read_bytes() for path in inventory_closure[3]} == inventory_closure[3]
    assert len(f.inventory_calls) == 1 and len(f.native_commands) == (0 if f.state == "fresh" else 2)
    request = await f.service.action_request(f.guide_id, f.directory.name, request_id)
    assert request["action_request"]["phase"] == "settled" and request["action_request"]["outcome"] == "error"
    counts = len(f.supervisors), len(f.accepted_reads), len(f.proof_reads)
    repeated = await f.service.action(f.guide_id, f.directory.name, "prepare-setup", action_request_id=request_id)
    assert repeated["action_request"] == request["action_request"]
    assert counts == (len(f.supervisors), len(f.accepted_reads), len(f.proof_reads))
    assert retained_bytes(f) == before
