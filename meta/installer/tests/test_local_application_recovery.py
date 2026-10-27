"""Deliberate recovery of original local Install ownership after Forge restarts.

Real stores, services, Build acceptance/source readers, Install completion and
action journals are used. Builder results and worker exit remain the explicit
inert boundaries of the original registry fixture. No native setup, application,
SDK, provider, OS service or useful job is executed or qualified here.
"""
import asyncio
from copy import deepcopy
import hashlib
import io
import json
from pathlib import Path
import threading
from types import SimpleNamespace
from uuid import uuid4
import zipfile

import pytest

from launchkit import deployment, deployment_worker, runtime_dependencies
from launchkit.connect_installer_caller import CustodyError
from launchkit.deployment import DeploymentService
from launchkit.guide import GuideService
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from launchkit.local_application_preparation_inputs import LocalApplicationPreparationOwner
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.projects import DraftConflict
from launchkit.runtime_selection import selected_runtime_artifacts
from launchkit.templates import install_candidate as adapter
from launchkit.templates.storage_setup import project_lock
from test_local_application_registry import (accept_source_fixture, build,
    install_fixture, new_guide, no_processes, no_product_execution, product,
    registry, source_bytes)


CALLBACKS = (("application_host_selection", "selected_host"),
    ("application_setup_retention", "retain_installation_inputs"),
    ("application_setup_selection", "_selected_startup"),
    ("application_setup_preparation", "prepare_application"),
    ("application_setup_continuation", "continue_application_startup"))


def disk_bytes(root):
    return {path.relative_to(root).as_posix(): path.read_bytes()
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


@pytest.fixture
async def restarted_installation(registry, monkeypatch, request, tmp_path):
    f = registry
    native_classes = getattr(request, "param", None) == "native-classes"
    if native_classes:
        # The helperless suite's ordinary plan, on the selected catalog's exact model.
        from test_ordinary_application_registry import ordinary_guide
        original = ordinary_guide(f)
        plan = original["proposal"]
        plan["agent_classes"].append({**deepcopy(plan["agent_classes"][0]),
            "id": "later", "instances": 0, "members": []})
        original = f.store.save(original, original["version"])
    else:
        original = new_guide(f)
    console_pin = None
    with monkeypatch.context() as source_choice:
        if getattr(request, "param", None) in {"explicit-console", "explicit-recovery"}:
            from launchkit import local_application_source
            from launchkit.connect_installer_caller import ExactFile
            choice_root = tmp_path / "original-explicit-console"
            choice_root.mkdir(mode=0o700)
            path = choice_root / "selection.json"
            choice = {"origin": "http://127.0.0.1:" +
                str(original["proposal"]["advanced"]["port"]),
                "access_management": "requester_issued_leaves"}
            if request.param == "explicit-recovery":
                choice["recovery_management"] = "operator_unpublished_service_approvals"
            path.write_text(json.dumps(choice) + "\n")
            path.chmod(0o600)
            console_pin = ExactFile(path, hashlib.sha256(path.read_bytes()).hexdigest())
            factory_type = local_application_source.LocalApplicationFactory
            def selected_factory(**kwargs):
                return factory_type(**kwargs, console_setup_selection=console_pin)
            source_choice.setattr(local_application_source, "LocalApplicationFactory", selected_factory)
        installed = await build(f, original)
    if native_classes:
        from test_helperless_application_source import authored_inventory
        authored_inventory(f, installed)
    await accept_source_fixture(f, installed)
    old_host = f.registry.host_for_build(original["id"], installed["build_id"])
    old_plan = deepcopy(old_host._blueprint)
    directory, destination = await install_fixture(f, original, installed, monkeypatch)
    assert f.registry.selected_host(original["id"], directory.name) is old_host
    if console_pin is not None:
        policy = json.loads(old_host._preparation_inputs.__self__._choices_pin.read())
        assert policy["console_setup_source"] == console_pin.pin()
        assert old_host._preparation_inputs.__self__.factory.console_setup_selection == console_pin
    old_supervisor_id = id(old_host._installation_supervisor)

    changed = f.store.get(original["id"])
    changed["proposal"]["project"]["name"] += " revised draft B"
    changed = f.store.save(changed, changed["version"])
    next_build = await build(f, changed, "Implement draft B", previous=installed)
    if native_classes:
        authored_inventory(f, next_build)
    await accept_source_fixture(f, next_build)
    assert next_build["build_id"] != installed["build_id"]
    assert f.implementation.selected_build_id(original["id"]) == next_build["build_id"]
    await f.implementation.shutdown()

    # All active owner instances are new. The original process's maps and
    # source/preparation objects are unavailable to these real consumers.
    store = GuideStore(f.store.root)
    guide = GuideService(store, architect=None)
    implementation = ImplementationService(f.implementation.root.parent, guide)
    service = DeploymentService(f.service.root.parent, guide, implementation)
    owner = LocalApplicationRegistry(guide_store=store, implementation=implementation,
        deployment=service, runtime_target=f.target)
    for name, method in CALLBACKS:
        setattr(service, name, getattr(owner, method))
    assert not owner._builds and not owner._installations
    assert not service.tasks and not service.processes
    assert not implementation.tasks and not implementation.processes

    supervisors, accepted_reads, proof_reads = [], [], []
    original_init = deployment_worker.Supervisor.__init__
    original_accept = implementation._accepted_framework

    def observe_supervisor(supervisor, *args, **kwargs):
        supervisors.append(supervisor)
        original_init(supervisor, *args, **kwargs)

    def observe_acceptance(*args, **kwargs):
        accepted_reads.append(args)
        return original_accept(*args, **kwargs)

    def no_retention(*args, **kwargs):
        pytest.fail("Recovery cannot replay the successful Install input publisher.")

    original_proof = deepcopy(deployment.read_receipt(directory)["installed_candidate_runtime_proof"])

    def inert_sdk_proof(selected_destination, version, **kwargs):
        # This fixture never installs/imports an SDK. The original Install's
        # synthetic proof remains the explicit external SDK boundary; exact
        # selected native prefix bytes still use the real production reader.
        assert Path(selected_destination) == destination and version == f.target.meta.version
        assert kwargs == {"runtime_target": f.target}
        adapter.verify_meta_toolkit_prefix(selected_runtime_artifacts(f.target),
            deployment_root=destination)
        proof_reads.append((selected_destination, version, kwargs))
        return deepcopy(original_proof)

    monkeypatch.setattr(deployment_worker.Supervisor, "__init__", observe_supervisor)
    monkeypatch.setattr(implementation, "_accepted_framework", observe_acceptance)
    monkeypatch.setattr(LocalApplicationPreparationOwner, "retain", no_retention)
    monkeypatch.setattr(runtime_dependencies, "verify_runtime", inert_sdk_proof)
    f = SimpleNamespace(store=store, guide=guide, implementation=implementation,
        service=service, registry=owner, target=f.target, directory=directory,
        destination=destination, guide_id=original["id"], installed=installed,
        installed_version=deployment.read_receipt(directory)["guide_version"],
        old_plan=old_plan, old_supervisor_id=old_supervisor_id, changed=changed,
        original_build=implementation._directory(original["id"], installed["build_id"]),
        supervisors=supervisors, accepted_reads=accepted_reads, proof_reads=proof_reads,
        console_pin=console_pin)
    yield f
    assert not service.tasks and not service.processes
    assert not implementation.tasks and not implementation.processes


def original_bytes(f):
    return ((f.directory / "package.zip").read_bytes(),
        (f.directory / "receipt.json").read_bytes(),
        (f.destination / ".env").read_bytes(),
        source_bytes(f.destination / ".mobkit/forge-local-setup"),
        disk_bytes(f.original_build))


async def stop(f, request_id=None):
    return await f.service.action(f.guide_id, f.directory.name, "stop",
        action_request_id=request_id or str(uuid4()))


async def test_stop_reconnects_original_install_after_restart_while_draft_b_evolves(
        restarted_installation):
    f = restarted_installation
    before = original_bytes(f)
    status = await f.registry.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["available"] is False and status["reason"] == "owner_reconnect"
    assert status["action"] == "prepare-setup" and "kind" not in status
    assert not f.supervisors and not f.accepted_reads and not f.proof_reads
    assert not f.registry._builds and not f.registry._installations
    assert original_bytes(f) == before

    request_id = str(uuid4())
    result = await stop(f, request_id)
    assert result["action_request"]["phase"] == "settled"
    assert result["action_request"]["outcome"] == "returned"
    host = f.registry.selected_host(f.guide_id, f.directory.name)
    assert host._blueprint == f.old_plan and host._blueprint != f.changed["proposal"]
    inputs = host._retained_preparation_inputs
    assert inputs.supervisor is host._installation_supervisor
    assert inputs.supervisor is f.supervisors[0] and len(f.supervisors) == 1
    assert id(inputs.supervisor) != f.old_supervisor_id
    assert inputs.supervisor.directory == f.directory and inputs.supervisor.destination == f.destination
    assert f.accepted_reads and all(row[1] == f.installed["build_id"] for row in f.accepted_reads)
    assert len(f.proof_reads) == 1
    assert not inputs.startup_selection_path.exists()
    assert not inputs.inventory_attempt_path.exists() and not inputs.preparation_plan_path.exists()
    assert original_bytes(f) == before

    after = disk_bytes(f.service.root), disk_bytes(f.store.root)
    status = await f.registry.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["available"] is True and status["action"] == "prepare-setup"
    assert status["build_id"] == f.installed["build_id"]
    assert status["guide_version"] == f.installed_version
    reads = len(f.accepted_reads)
    repeated = await stop(f, request_id)
    assert repeated["action_request"] == result["action_request"]
    assert len(f.supervisors) == 1 and len(f.accepted_reads) == reads and len(f.proof_reads) == 1
    assert f.registry.selected_host(f.guide_id, f.directory.name) is host
    assert (disk_bytes(f.service.root), disk_bytes(f.store.root)) == after


@pytest.mark.parametrize("restarted_installation", ["native-classes"], indirect=True)
async def test_cold_controller_recovers_original_classes_and_explicit_seed_policy(restarted_installation):
    from launchkit.application_inventory_source import POLICY_PATH
    f = restarted_installation
    before = original_bytes(f)
    inventory_path = f.original_build / "accepted-application-inventory.json"
    policy_path = f.original_build / "accepted-application-tool-policy.json"
    inventory, policy = inventory_path.read_bytes(), policy_path.read_bytes()
    result = await stop(f)
    assert result["action_request"]["outcome"] == "returned"
    host = f.registry.selected_host(f.guide_id, f.directory.name)
    assert host._inventory_pin.path == inventory_path and host._inventory_pin.read() == inventory
    assert host._accepted_policy_pin.path == policy_path and host._policy_bytes == policy
    assert host._policy_source_path == POLICY_PATH
    assert host._inventory["member_classes"][0]["id"] == "later"
    assert all(not row["identity"].startswith("later:") for row in host._inventory["roster"])
    assert host._policy["members"][0]["grants"][0]["tool_name"] == "read_context"
    assert host._retained_preparation_inputs.supervisor is f.supervisors[0]
    assert original_bytes(f) == before
    assert not f.service.tasks and not f.service.processes


@pytest.mark.parametrize("restarted_installation", ["explicit-console", "explicit-recovery"], indirect=True)
async def test_cold_controller_preserves_original_explicit_console_pin(restarted_installation):
    f = restarted_installation
    before = original_bytes(f), f.console_pin.read()
    result = await stop(f)
    assert result["action_request"]["outcome"] == "returned"
    host = f.registry.selected_host(f.guide_id, f.directory.name)
    owner = host._preparation_inputs.__self__
    assert owner.factory.console_setup_selection == f.console_pin
    assert json.loads(owner._choices_pin.read())["console_setup_source"] == f.console_pin.pin()
    assert (original_bytes(f), f.console_pin.read()) == before
    assert len(f.supervisors) == 1 and len(f.proof_reads) == 1


@pytest.mark.parametrize("restarted_installation", ["explicit-console"], indirect=True)
@pytest.mark.parametrize("change", ["missing-source", "source-bytes", "policy-choice", "source-hash",
    "source-fields", "foreign-guide", "foreign-deployment", "linked-source"])
async def test_cold_explicit_console_refuses_changed_provenance_before_owner_construction(
        restarted_installation, change):
    f = restarted_installation
    policy_path = f.destination / ".mobkit/forge-local-setup/local-application-policy.json"
    policy = json.loads(policy_path.read_bytes())
    if change == "missing-source":
        f.console_pin.path.unlink()
    elif change == "source-bytes":
        f.console_pin.path.write_bytes(f.console_pin.read() + b" ")
    elif change == "policy-choice":
        policy["console_setup"]["origin"] = "http://127.0.0.1:12345"
    elif change == "source-hash":
        policy["console_setup_source"]["sha256"] = "f" * 64
    elif change == "source-fields":
        policy["console_setup_source"]["replacement"] = "not-an-original-pin"
    elif change == "foreign-guide":
        policy["guide_id"] = str(uuid4())
    elif change == "foreign-deployment":
        policy["deployment_id"] = str(uuid4())
    else:
        copied = f.console_pin.path.with_name("linked-copy.json")
        copied.write_bytes(f.console_pin.read())
        copied.chmod(0o600)
        f.console_pin.path.unlink()
        f.console_pin.path.symlink_to(copied)
    if change in {"policy-choice", "source-hash", "source-fields", "foreign-guide", "foreign-deployment"}:
        policy_path.write_text(json.dumps(policy) + "\n")
    before = original_bytes(f)
    with pytest.raises((CustodyError, FileNotFoundError)):
        await stop(f)
    assert not f.supervisors and not f.proof_reads
    assert not f.registry._installations
    # Action journaling may advance; retained source, policy and private inputs
    # cannot be repaired or replaced by unsuccessful reconstruction.
    after = original_bytes(f)
    assert after[0] == before[0] and after[2:] == before[2:]


@pytest.mark.parametrize("restarted_installation", ["explicit-console"], indirect=True)
async def test_recovered_factory_keeps_only_the_admitted_console_pin(restarted_installation):
    from launchkit.local_application_factory import LocalApplicationFactory
    from launchkit.connect_installer_caller import ExactFile
    f = restarted_installation
    await stop(f)
    factory = f.registry.selected_host(f.guide_id, f.directory.name)._preparation_inputs.__self__.factory
    other = f.console_pin.path.with_name("same-bytes-different-source.json")
    other.write_bytes(f.console_pin.read())
    other.chmod(0o600)
    substitute = ExactFile(other, f.console_pin.sha256)
    with pytest.raises(CustodyError, match="original installed source owner"):
        LocalApplicationFactory(guide_store=factory.guide_store, guide_id=factory.guide_id,
            runtime_target=factory.runtime_target, acquisition_selection=factory.acquisition_selection,
            viewer_selection=factory.viewer_selection, console_setup_selection=substitute,
            _installed_admission=factory._installed_admission)
    f.console_pin.path.write_bytes(f.console_pin.read() + b" ")
    with pytest.raises(CustodyError):
        factory.require_current()


@pytest.mark.parametrize("changed,error,detail", [
    ("source", CustodyError, "original complete application plan"),
    ("missing-source", FileNotFoundError, "source-selection.json"),
    ("private-policy", CustodyError, "original local setup policy or activation source changed"),
    ("template", CustodyError, "original local setup policy or activation source changed"),
    ("signed-acceptance", DraftConflict, "no longer independently accepted"),
    ("receipt", CustodyError, "The checked package is not this original admitted application source."),
    ("cleanup", DraftConflict, "original runtime cleanup remains unconfirmed"),
    ("partial-preparation", CustodyError, "closed original Fresh or Restore selection"),
    ("fresh-history", CustodyError, "caller output or attempt must be fresh"),
])
async def test_reconnect_refuses_changed_original_source_or_unsettled_custody(
        restarted_installation, changed, error, detail):
    f = restarted_installation
    source = f.original_build / "local-application-source"
    setup = f.destination / ".mobkit/forge-local-setup"
    if changed == "source":
        path = source / "blueprint.json"
        path.write_bytes(path.read_bytes() + b" ")
    elif changed == "missing-source":
        (source / "source-selection.json").unlink()
    elif changed == "private-policy":
        path = setup / "local-application-policy.json"
        value = json.loads(path.read_bytes())
        value["generation"] += 1
        path.write_text(json.dumps(value) + "\n")
    elif changed == "template":
        path = setup / "activation-template.json"
        path.write_text(json.dumps({"activation_id": str(uuid4())}) + "\n")
    elif changed == "signed-acceptance":
        path = f.original_build / "acceptance/acceptance-key"
        path.write_bytes(b"0" * len(path.read_bytes()))
    elif changed == "receipt":
        deployment.update_receipt(f.directory, source_digest="f" * 64)
    elif changed == "cleanup":
        receipt = deployment.read_receipt(f.directory)
        cleanup = deepcopy(receipt["interactive_cleanup"])
        cleanup["confirmed"] = False
        deployment.update_receipt(f.directory, interactive_cleanup=cleanup)
    elif changed == "partial-preparation":
        path = setup / "startup-selection.json"
        path.write_text("{}\n")
        path.chmod(0o600)
    else:
        from launchkit.deployment_application_bootstrap import FRESH_ENTRY_NAME
        path = f.directory / FRESH_ENTRY_NAME
        path.write_text("{}\n")
        path.chmod(0o600)
    before = original_bytes(f)
    with pytest.raises(error, match=detail):
        await stop(f)
    assert original_bytes(f) == before
    assert not f.registry._builds and not f.registry._installations
    assert not f.service.tasks and not f.service.processes


async def test_pending_stop_uuid_never_reconstructs_the_original_owner(restarted_installation):
    f = restarted_installation
    request_id = str(uuid4())
    binding = {"action_request_id": request_id, "guide_id": f.guide_id,
        "deployment_id": f.directory.name, "action": "stop", "update_id": None}
    _, claimed = f.service._claim_action_request(binding)
    assert claimed
    before = disk_bytes(f.service.root), disk_bytes(f.store.root)
    result = await stop(f, request_id)
    assert result["action_request"]["phase"] == "pending"
    assert not f.supervisors and not f.accepted_reads and not f.proof_reads
    assert not f.registry._builds and not f.registry._installations
    assert (disk_bytes(f.service.root), disk_bytes(f.store.root)) == before


async def test_cancelled_reconnect_joins_original_reader_and_keeps_uuid_pending(
        restarted_installation, monkeypatch):
    f = restarted_installation
    entered, release = threading.Event(), threading.Event()
    original = f.implementation._accepted_framework

    def held_original_reader(*args, **kwargs):
        entered.set()
        assert release.wait(5), "The original source reader was not released by its owner."
        return original(*args, **kwargs)

    monkeypatch.setattr(f.implementation, "_accepted_framework", held_original_reader)
    before = original_bytes(f)
    request_id = str(uuid4())
    task = asyncio.create_task(stop(f, request_id))
    try:
        assert await asyncio.to_thread(entered.wait, 5)
        with pytest.raises(RuntimeError, match="already"):
            with project_lock(f.destination):
                pytest.fail("The original recovery reader must still own its actual project lease.")
        task.cancel()
        await asyncio.sleep(0.01)
        task.cancel()
        await asyncio.sleep(0.01)
        assert not task.done() and not f.supervisors and not f.proof_reads
        with pytest.raises(RuntimeError, match="already"):
            with project_lock(f.destination):
                pytest.fail("Repeated cancellation cannot release the reader's original project lease.")
    finally:
        release.set()
        try:
            await asyncio.wait_for(task, 5)
        except asyncio.CancelledError:
            pass
    assert task.cancelled()
    with project_lock(f.destination) as lease:
        lease.require_current()
    request = await f.service.action_request(f.guide_id, f.directory.name, request_id)
    assert request["action_request"]["phase"] == "pending"
    assert original_bytes(f) == before
    assert not f.registry._builds and not f.registry._installations
    reads = len(f.accepted_reads)
    repeated = await stop(f, request_id)
    assert repeated["action_request"]["phase"] == "pending"
    assert len(f.accepted_reads) == reads and not f.supervisors and not f.proof_reads


@pytest.mark.parametrize("field", ["brief_hash", "blueprint_hash"])
async def test_reconnect_refuses_report_scope_outside_independently_accepted_build(
        restarted_installation, field):
    f = restarted_installation
    accepted = disk_bytes(f.original_build)
    package_path = f.directory / "package.zip"
    original_payload = package_path.read_bytes()
    package = deployment.package_files(original_payload)
    report = json.loads(package["implementation-report.json"][0])
    assert report[field] != "f" * 64
    report[field] = "f" * 64
    package["implementation-report.json"] = (
        (json.dumps(report, indent=2) + "\n").encode(), package["implementation-report.json"][1])
    sums = "".join(f"{hashlib.sha256(content).hexdigest()}  {name}\n"
        for name, (content, _) in sorted(package.items()) if name != "SHA256SUMS")
    package["SHA256SUMS"] = (sums.encode(), package["SHA256SUMS"][1])
    payload = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(original_payload)) as original:
        with zipfile.ZipFile(payload, "w") as replacement:
            for entry in original.infolist():
                replacement.writestr(entry, package[entry.filename][0])
    package_path.write_bytes(payload.getvalue())
    for name in ("implementation-report.json", "SHA256SUMS"):
        (f.destination / name).write_bytes(package[name][0])

    # The canonical unsigned Install closure is internally consistent. Only
    # the generated report's scope disagrees with independently signed Build A.
    # No application Blueprint, signed acceptance or private source is changed.
    receipt = deployment.read_receipt(f.directory)
    receipt.update(package_sha256=hashlib.sha256(payload.getvalue()).hexdigest())
    receipt[field] = report[field]
    deployment.write_private(f.destination / deployment.OWNER_FILE, deployment.ownership(receipt))
    receipt["interactive_cleanup"].update(package_sha256=receipt["package_sha256"],
        blueprint_hash=receipt["blueprint_hash"],
        owner_sha256=hashlib.sha256((f.destination / deployment.OWNER_FILE).read_bytes()).hexdigest())
    deployment.write_private(f.directory / "receipt.json", receipt)
    assert deployment.interactive_cleanup_matches(receipt)
    from launchkit.deployment_application_bootstrap import _read_source
    from launchkit.deployment_sources import read_package_source
    checked, _ = _read_source(f.directory, receipt)
    assert read_package_source(checked, receipt)[field] == report[field]
    assert disk_bytes(f.original_build) == accepted
    before = original_bytes(f), disk_bytes(f.destination)

    with pytest.raises(CustodyError, match="independently accepted complete Build"):
        await stop(f)
    assert f.accepted_reads and all(row[1] == f.installed["build_id"] for row in f.accepted_reads)
    assert not f.supervisors and not f.proof_reads
    assert not f.registry._builds and not f.registry._installations
    assert (original_bytes(f), disk_bytes(f.destination)) == before
    assert disk_bytes(f.original_build) == accepted


async def test_reconnect_refuses_raw_archive_mode_outside_independently_accepted_build(
        restarted_installation):
    f = restarted_installation
    accepted = disk_bytes(f.original_build)
    private = source_bytes(f.destination / ".mobkit/forge-local-setup")
    package_path = f.directory / "package.zip"
    original_payload = package_path.read_bytes()
    original_package = deployment.package_files(original_payload)
    original_receipt = deployment.read_receipt(f.directory)
    from launchkit.deployment_application_bootstrap import _read_source
    from launchkit.deployment_sources import read_package_source
    source_digest = read_package_source(original_package, original_receipt)["source_digest"]
    payload = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(original_payload)) as original:
        assert (original.getinfo("application/inert.txt").external_attr >> 16) & 0o777 == 0o600
        with zipfile.ZipFile(payload, "w") as replacement:
            for entry in original.infolist():
                selected = deepcopy(entry)
                if entry.filename == "application/inert.txt":
                    selected.external_attr = (entry.external_attr & ~(0o777 << 16)) | (0o644 << 16)
                replacement.writestr(selected, original.read(entry.filename))
    with zipfile.ZipFile(io.BytesIO(payload.getvalue())) as changed:
        assert (changed.getinfo("application/inert.txt").external_attr >> 16) & 0o777 == 0o644
    assert deployment.package_files(payload.getvalue()) == original_package
    package_path.write_bytes(payload.getvalue())

    # All file bytes and source digests still match. Only the raw archive mode
    # differs from signed Build A; the canonical reader projects both to 0644.
    receipt = deepcopy(original_receipt)
    receipt["package_sha256"] = hashlib.sha256(payload.getvalue()).hexdigest()
    deployment.write_private(f.destination / deployment.OWNER_FILE, deployment.ownership(receipt))
    receipt["interactive_cleanup"].update(package_sha256=receipt["package_sha256"],
        owner_sha256=hashlib.sha256((f.destination / deployment.OWNER_FILE).read_bytes()).hexdigest())
    deployment.write_private(f.directory / "receipt.json", receipt)
    assert deployment.interactive_cleanup_matches(receipt)
    checked, _ = _read_source(f.directory, receipt)
    assert checked["application/inert.txt"][1] == 0o644
    assert read_package_source(checked, receipt)["source_digest"] == source_digest
    assert receipt.get("source_digest") is original_receipt.get("source_digest") is None
    assert disk_bytes(f.original_build) == accepted
    assert source_bytes(f.destination / ".mobkit/forge-local-setup") == private
    before = original_bytes(f), disk_bytes(f.destination)

    with pytest.raises(CustodyError, match="independently accepted complete Build"):
        await stop(f)
    assert f.accepted_reads and all(row[1] == f.installed["build_id"] for row in f.accepted_reads)
    assert not f.supervisors and not f.proof_reads
    assert not f.registry._builds and not f.registry._installations
    assert (original_bytes(f), disk_bytes(f.destination)) == before
    assert disk_bytes(f.original_build) == accepted
    assert source_bytes(f.destination / ".mobkit/forge-local-setup") == private
